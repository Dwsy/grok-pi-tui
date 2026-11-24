//! Scrollback candidates and viewport navigation for the Timeline sidebar.

use super::*;
use agent_client_protocol::{ToolCall, ToolCallStatus, ToolCallUpdate};
use serde_json::Value;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use xai_grok_shell::util::config::TimelineContents;

pub(super) fn next_branch_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

const PREVIEW_MAX_CHARS: usize = 120;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineEntry {
    pub turn_idx: usize,
    pub prompt_entry_id: EntryId,
    /// Prompt creation time for Jump/timeline pickers.
    pub created_at: Option<chrono::DateTime<chrono::Local>>,
    pub preview: String,
}

/// Semantic kind of one marker in the narrow timeline rail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimelineMarkerKind {
    Prompt,
    Compaction,
    Tool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineTool {
    pub call_id: String,
    pub tool_name: String,
    pub status: &'static str,
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interaction: Option<TimelineInteraction>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize)]
pub struct TimelineInteraction {
    pub target: Option<String>,
    pub mode: Option<String>,
}

impl TimelineTool {
    fn update_interaction(&mut self, input: Option<&Value>, output: Option<&Value>) -> bool {
        let Some(mut incoming) = interaction(&self.tool_name, input, output) else {
            return false;
        };
        if let Some(old) = self.interaction.as_ref() {
            incoming.target = incoming.target.or_else(|| old.target.clone());
            incoming.mode = incoming.mode.or_else(|| old.mode.clone());
        }
        if self.interaction.as_ref() == Some(&incoming) {
            return false;
        }
        self.interaction = Some(incoming);
        true
    }
}

fn interaction(
    name: &str,
    input: Option<&Value>,
    output: Option<&Value>,
) -> Option<TimelineInteraction> {
    let details = output.and_then(|result| result.get("details")).or(output);
    let arg = |field| {
        input
            .and_then(|args| args.get(field))
            .and_then(Value::as_str)
    };
    let result = |field| {
        details
            .and_then(|data| data.get(field))
            .and_then(Value::as_str)
    };
    let list = |field| {
        let items = input?.get(field)?.as_array()?;
        let mut names = items.iter().filter_map(Value::as_str);
        let shown: Vec<_> = names.by_ref().take(3).map(short_label).collect();
        if shown.is_empty() {
            return None;
        }
        let mut text = shown.join(", ");
        let remaining = names.count();
        if remaining > 0 {
            text.push_str(&format!(" +{remaining}"));
        }
        Some(text)
    };
    let subagent = || {
        result("subagentId")
            .or_else(|| arg("task_id"))
            .or_else(|| arg("subagent_id"))
    };
    let target = || result("target").or_else(|| arg("target"));
    let (target, mode) = match name {
        "spawn_subagent" => (arg("description").or_else(subagent).map(short_label), None),
        "send_message_to_subagent" => (
            subagent().map(short_label),
            result("delivery")
                .or_else(|| arg("delivery"))
                .map(short_label),
        ),
        "kill_command_or_subagent" => (subagent().map(short_label), None),
        "get_command_or_subagent_output" => (
            list("task_ids").or_else(|| subagent().map(short_label)),
            None,
        ),
        "spawn_team_agent" => (
            result("agentPath")
                .or_else(|| arg("task_name"))
                .map(short_label),
            None,
        ),
        "spawn_team" => (
            result("team").or_else(|| arg("team")).map(short_label),
            None,
        ),
        "team_send_message" | "team_followup_task" | "team_interrupt" => {
            (target().map(short_label), None)
        }
        "team_wait" => (list("teams"), None),
        "team_report" => (
            result("agentPath").map(short_label),
            result("state").or_else(|| arg("state")).map(short_label),
        ),
        _ => return None,
    };
    (target.is_some() || mode.is_some()).then_some(TimelineInteraction { target, mode })
}

fn short_label(text: &str) -> String {
    text.trim()
        .chars()
        .take(160)
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect()
}

pub(crate) fn timeline_excerpt(text: &str) -> String {
    const MAX_CHARS: usize = 768;
    let mut chars = text.trim_start().chars();
    let mut excerpt: String = chars
        .by_ref()
        .take(MAX_CHARS)
        .map(|ch| {
            if ch.is_control() && ch != '\n' && ch != '\t' {
                ' '
            } else {
                ch
            }
        })
        .collect();
    if chars.next().is_some() {
        excerpt.push('…');
    }
    excerpt
}

#[derive(Debug)]
pub(super) struct HiddenTimelineTool {
    pub anchor: EntryId,
    tool: TimelineTool,
    created_at: Option<chrono::DateTime<chrono::Local>>,
}

fn display_path(input: Option<&Value>) -> Option<String> {
    input?.get("path")?.as_str().map(|path| {
        path.chars()
            .take(512)
            .map(|ch| if ch.is_control() { ' ' } else { ch })
            .collect()
    })
}

fn result_status(status: ToolCallStatus, output: Option<&Value>) -> &'static str {
    if output
        .and_then(|out| out.get("piToolCancelled"))
        .and_then(Value::as_bool)
        == Some(true)
    {
        return "cancelled";
    }
    match status {
        ToolCallStatus::Completed => "success",
        ToolCallStatus::Failed => "failed",
        _ => "running",
    }
}

fn traced_tool(
    trace: &crate::scrollback::entry::ToolTraceSnapshot,
    name: &str,
    running: bool,
) -> TimelineTool {
    let raw_status = match trace.status.as_str() {
        "Completed" => ToolCallStatus::Completed,
        "Failed" => ToolCallStatus::Failed,
        _ => ToolCallStatus::InProgress,
    };
    let mut status = result_status(raw_status, trace.raw_output.as_ref());
    if status == "running" && !running {
        status = "cancelled";
    }
    TimelineTool {
        call_id: trace.tool_call_id.clone(),
        tool_name: name.to_owned(),
        status,
        path: display_path(trace.raw_input.as_ref()),
        interaction: interaction(name, trace.raw_input.as_ref(), trace.raw_output.as_ref()),
    }
}

fn codemode_timeline_tools<'a>(
    trace: &'a crate::scrollback::entry::ToolTraceSnapshot,
    names: &'a HashSet<&'a str>,
    running: bool,
) -> impl Iterator<Item = TimelineTool> + 'a {
    let calls = trace
        .raw_output
        .as_ref()
        .and_then(|output| output.get("calls"))
        .and_then(Value::as_array);
    calls
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(move |(index, call)| {
            let name = call.get("name")?.as_str()?;
            if !names.contains(name) {
                return None;
            }
            let status = match call.get("status")?.as_str()? {
                "ok" => "success",
                "error" => "failed",
                "running" if running => "running",
                "running" | "cancelled" => "cancelled",
                _ => return None,
            };
            // Pi's partial snapshots use the same /? placeholder for simultaneous
            // calls until the real execution ID is available. Their order is stable.
            let id = call
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.ends_with("/?"));
            let args = call
                .get("args")
                .and_then(Value::as_str)
                .and_then(|args| serde_json::from_str(args).ok());
            Some(TimelineTool {
                call_id: id
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("{}#{}", trace.tool_call_id, index)),
                tool_name: name.to_owned(),
                status,
                path: display_path(args.as_ref()),
                interaction: interaction(name, args.as_ref(), None),
            })
        })
}

fn tool_time(ms: Option<i64>) -> Option<chrono::DateTime<chrono::Local>> {
    ms.and_then(chrono::DateTime::from_timestamp_millis)
        .map(|time| time.with_timezone(&chrono::Local))
}

fn tool_preview(tool: &TimelineTool) -> String {
    if matches!(tool.tool_name.as_str(), "edit" | "write")
        && let Some(path) = tool.path.as_deref().filter(|path| !path.is_empty())
    {
        return format!("{} {path} · {}", tool.tool_name, tool.status);
    }
    if let Some(target) = tool
        .interaction
        .as_ref()
        .and_then(|interaction| interaction.target.as_deref())
    {
        return format!("{} {target} · {}", tool.tool_name, tool.status);
    }
    format!("{} · {}", tool.tool_name, tool.status)
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TimelineFileSummary {
    pub file_count: usize,
    pub added: usize,
    pub deleted: usize,
    pub written: usize,
    pub files: Vec<crate::scrollback::entry::ToolFileChange>,
}

/// One visual marker in the timeline rail, ordered exactly like scrollback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineMarker {
    pub entry_id: EntryId,
    pub entry_index: usize,
    pub turn_idx: Option<usize>,
    pub kind: TimelineMarkerKind,
    pub created_at: Option<chrono::DateTime<chrono::Local>>,
    pub preview: String,
    pub tool: Option<TimelineTool>,
}

#[derive(Debug)]
pub(super) struct MarkerCache {
    rules: TimelineContents,
    markers: Arc<Vec<TimelineMarker>>,
    dirty_from: Option<usize>,
}

fn preview_line(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    let mut preview: String = line.chars().take(PREVIEW_MAX_CHARS).collect();
    if preview.chars().count() == PREVIEW_MAX_CHARS && line.chars().nth(PREVIEW_MAX_CHARS).is_some()
    {
        preview.pop();
        preview.push('…');
    }
    preview
}

fn prompt_preview(text: &str) -> String {
    preview_line(text)
}

impl ScrollbackState {
    pub fn timeline_entries(&self) -> Vec<TimelineEntry> {
        self.turns
            .iter()
            .enumerate()
            .filter_map(|(turn_idx, turn)| {
                let (prompt_entry_id, entry) = self.entries.get_index(turn.prompt_index)?;
                let RenderBlock::UserPrompt(prompt) = &entry.block else {
                    return None;
                };
                Some(TimelineEntry {
                    turn_idx,
                    prompt_entry_id: *prompt_entry_id,
                    created_at: entry.created_at,
                    preview: prompt_preview(&prompt.text),
                })
            })
            .collect()
    }

    /// Preview the last visible response in this prompt's own turn. A later
    /// tool or reasoning block makes any earlier text intermediate.
    pub(crate) fn timeline_reply(&self, marker: &TimelineMarker) -> Option<String> {
        let turn = self.turns.get(marker.turn_idx?)?;
        for index in ((marker.entry_index + 1)..turn.end_index).rev() {
            let (id, entry) = self.entries.get_index(index)?;
            match &entry.block {
                RenderBlock::AgentMessage(message) if !message.is_empty() => {
                    if entry.is_running
                        || self
                            .timeline_hidden_tools
                            .values()
                            .any(|tool| tool.anchor == *id)
                    {
                        return None;
                    }
                    return message.content().with_source(|source| {
                        let excerpt = timeline_excerpt(source);
                        (!excerpt.trim().is_empty()).then_some(excerpt)
                    });
                }
                RenderBlock::ToolCall(_)
                | RenderBlock::Thinking(_)
                | RenderBlock::Subagent(_)
                | RenderBlock::BgTask(_) => {
                    return None;
                }
                _ => {}
            }
        }
        None
    }

    /// Collect only the hovered user turn's completed file changes. The
    /// tracker records per-call counts before native tool cards can coalesce.
    pub(crate) fn timeline_file_summary(
        &self,
        marker: &TimelineMarker,
        rules: &TimelineContents,
    ) -> Option<TimelineFileSummary> {
        let edits = rules.tools.iter().any(|name| name == "edit");
        let writes = rules.tools.iter().any(|name| name == "write");
        if !edits && !writes {
            return None;
        }
        let turn = self.turns.get(marker.turn_idx?)?;
        let mut files =
            IndexMap::<std::path::PathBuf, crate::scrollback::entry::ToolFileChange>::new();
        for index in (marker.entry_index + 1)..turn.end_index {
            let Some(entry) = self.get(index) else {
                break;
            };
            for trace in &entry.tool_traces {
                if !match trace.pi_tool_name.as_deref() {
                    Some("edit") => edits,
                    Some("write") => writes,
                    _ => false,
                } {
                    continue;
                }
                let Some(change) = trace.file_change.as_ref() else {
                    continue;
                };
                let path = crate::render::tool_paths::resolve_tool_path_target(
                    &change.path,
                    self.cwd.as_deref(),
                )
                .unwrap_or_else(|| std::path::PathBuf::from(&change.path));
                if let Some(previous) = files.get_mut(&path) {
                    previous.added += change.added;
                    previous.deleted += change.deleted;
                    previous.written += change.written;
                } else {
                    files.insert(path, change.clone());
                }
            }
        }
        if files.is_empty() {
            return None;
        }
        Some(TimelineFileSummary {
            file_count: files.len(),
            added: files.values().map(|change| change.added).sum(),
            deleted: files.values().map(|change| change.deleted).sum(),
            written: files.values().map(|change| change.written).sum(),
            files: files
                .into_values()
                .take(6)
                .map(|mut change| {
                    change.path = short_label(&change.path);
                    change
                })
                .collect(),
        })
    }

    pub(super) fn mark_timeline_dirty(&mut self, index: usize) {
        if let Some(cache) = self.timeline_cache.get_mut().as_mut() {
            cache.dirty_from = Some(cache.dirty_from.map_or(index, |old| old.min(index)));
        }
    }

    fn mark_timeline_anchor(&mut self, anchor: EntryId) {
        if let Some(index) = self.index_of_id(anchor) {
            self.mark_timeline_dirty(index);
        }
    }

    pub(crate) fn set_timeline_spawn_anchor(&mut self, call_id: &str, entry_id: EntryId) {
        let previous = self
            .timeline_spawn_anchors
            .insert(call_id.to_owned(), entry_id)
            .or_else(|| {
                self.timeline_hidden_tools
                    .get(call_id)
                    .map(|tool| tool.anchor)
            });
        if let Some(previous) = previous {
            self.mark_timeline_anchor(previous);
        }
        self.mark_timeline_anchor(entry_id);
    }

    pub fn timeline_branch(&self) -> u64 {
        self.timeline_branch
    }

    pub(crate) fn invalidate_timeline_branch(&mut self) {
        self.timeline_branch = next_branch_id();
    }

    /// Hidden status tools still have a Timeline candidate. Anchor them to the
    /// latest surviving scrollback row, keeping complete tool cards on their
    /// own anchors.
    pub(crate) fn record_hidden_timeline_tool(
        &mut self,
        call: &ToolCall,
        timestamp: Option<i64>,
    ) -> bool {
        let Some(name) = call
            .meta
            .as_ref()
            .and_then(|meta| meta.get("piToolName"))
            .and_then(Value::as_str)
        else {
            return false;
        };
        let Some((&anchor, _)) = self
            .entries
            .iter()
            .rev()
            .find(|(_, entry)| entry.block.is_selectable())
            .or_else(|| self.entries.last())
        else {
            return false;
        };
        let id = call.tool_call_id.0.as_ref();
        let status = result_status(call.status, call.raw_output.as_ref());
        let path = display_path(call.raw_input.as_ref());
        self.mark_timeline_anchor(
            self.timeline_spawn_anchors
                .get(id)
                .copied()
                .unwrap_or(anchor),
        );
        let hidden = self
            .timeline_hidden_tools
            .entry(id.to_owned())
            .or_insert_with(|| HiddenTimelineTool {
                anchor,
                tool: TimelineTool {
                    call_id: id.to_owned(),
                    tool_name: name.to_owned(),
                    status,
                    path: None,
                    interaction: None,
                },
                created_at: tool_time(timestamp),
            });
        hidden.tool.status = status;
        if path.is_some() {
            hidden.tool.path = path;
        }
        hidden
            .tool
            .update_interaction(call.raw_input.as_ref(), call.raw_output.as_ref());
        true
    }

    pub(crate) fn update_hidden_timeline_tool(&mut self, update: &ToolCallUpdate) -> Option<bool> {
        let hidden = self
            .timeline_hidden_tools
            .get_mut(update.tool_call_id.0.as_ref())?;
        let mut changed = false;
        if let Some(status) = update.fields.status {
            let status = result_status(status, update.fields.raw_output.as_ref());
            changed |= hidden.tool.status != status;
            hidden.tool.status = status;
        }
        if let Some(path) = display_path(update.fields.raw_input.as_ref()) {
            changed |= hidden.tool.path.as_deref() != Some(path.as_str());
            hidden.tool.path = Some(path);
        }
        changed |= hidden.tool.update_interaction(
            update.fields.raw_input.as_ref(),
            update.fields.raw_output.as_ref(),
        );
        if changed {
            let anchor = self
                .timeline_spawn_anchors
                .get(update.tool_call_id.0.as_ref())
                .copied()
                .unwrap_or(hidden.anchor);
            self.mark_timeline_anchor(anchor);
        }
        Some(changed)
    }

    pub(crate) fn remove_hidden_timeline_tool(&mut self, id: &str) {
        if let Some(removed) = self.timeline_hidden_tools.shift_remove(id) {
            self.mark_timeline_anchor(
                self.timeline_spawn_anchors
                    .get(id)
                    .copied()
                    .unwrap_or(removed.anchor),
            );
        }
    }

    pub(crate) fn move_hidden_timeline_tools(&mut self, from: EntryId, to: EntryId) {
        self.mark_timeline_anchor(to);
        for hidden in self.timeline_hidden_tools.values_mut() {
            if hidden.anchor == from {
                hidden.anchor = to;
            }
        }
    }

    pub(crate) fn settle_hidden_timeline_tools(&mut self) {
        let mut first = None;
        for (id, hidden) in &mut self.timeline_hidden_tools {
            if hidden.tool.status == "running" {
                hidden.tool.status = "cancelled";
                let anchor = self
                    .timeline_spawn_anchors
                    .get(id)
                    .copied()
                    .unwrap_or(hidden.anchor);
                if let Some(index) = self.entries.get_index_of(&anchor) {
                    first = Some(first.map_or(index, |old: usize| old.min(index)));
                }
            }
        }
        if let Some(index) = first {
            self.mark_timeline_dirty(index);
        }
    }

    /// Reuse markers before the earliest changed entry, including the prefix
    /// retained by the text tree projection.
    pub fn timeline_markers(&self, rules: &TimelineContents) -> Arc<Vec<TimelineMarker>> {
        let mut cache = self.timeline_cache.borrow_mut();
        let cache = cache.get_or_insert_with(|| MarkerCache {
            rules: rules.clone(),
            markers: Arc::default(),
            dirty_from: Some(0),
        });
        if cache.rules != *rules {
            cache.rules = rules.clone();
            cache.dirty_from = Some(0);
        }
        if let Some(start) = cache.dirty_from.take() {
            let markers = Arc::make_mut(&mut cache.markers);
            let retained = markers.partition_point(|marker| marker.entry_index < start);
            markers.truncate(retained);
            self.append_timeline_markers(rules, start, markers);
        }
        Arc::clone(&cache.markers)
    }

    fn append_timeline_markers(
        &self,
        rules: &TimelineContents,
        start: usize,
        markers: &mut Vec<TimelineMarker>,
    ) {
        if !rules.user_prompts && !rules.compactions && rules.tools.is_empty() {
            return;
        }
        let tools: HashSet<_> = rules
            .tools
            .iter()
            .map(String::as_str)
            .filter(|name| !matches!(*name, "edit" | "write"))
            .collect();
        let mut hidden_by_anchor: std::collections::HashMap<EntryId, Vec<_>> = Default::default();
        for hidden in self.timeline_hidden_tools.values() {
            if tools.contains(hidden.tool.tool_name.as_str()) {
                let anchor = self
                    .timeline_spawn_anchors
                    .get(&hidden.tool.call_id)
                    .copied()
                    .unwrap_or(hidden.anchor);
                hidden_by_anchor.entry(anchor).or_default().push(hidden);
            }
        }
        for (entry_index, (&entry_id, entry)) in self.entries.iter().enumerate().skip(start) {
            let marker = |kind, created_at, preview, tool| TimelineMarker {
                entry_id,
                entry_index,
                turn_idx: self
                    .turns
                    .partition_point(|turn| turn.prompt_index <= entry_index)
                    .checked_sub(1)
                    .filter(|&i| entry_index < self.turns[i].end_index),
                kind,
                created_at,
                preview,
                tool,
            };
            match &entry.block {
                RenderBlock::UserPrompt(prompt) if rules.user_prompts => {
                    markers.push(marker(
                        TimelineMarkerKind::Prompt,
                        entry.created_at,
                        timeline_excerpt(&prompt.text),
                        None,
                    ));
                }
                RenderBlock::SessionEvent(block) if rules.compactions => {
                    use crate::scrollback::blocks::SessionEvent;
                    let preview = match &block.event {
                        SessionEvent::CompactionSummary { summary } => {
                            Some(timeline_excerpt(summary))
                        }
                        _ => None,
                    };
                    if let Some(preview) = preview {
                        markers.push(marker(
                            TimelineMarkerKind::Compaction,
                            entry.created_at,
                            preview,
                            None,
                        ));
                    }
                }
                _ => {}
            }

            if tools.is_empty() {
                continue;
            }
            for trace in &entry.tool_traces {
                let Some(name) = trace.pi_tool_name.as_deref() else {
                    continue;
                };
                let time = tool_time(trace.started_at_ms).or(entry.created_at);
                let tool_marker = |tool| {
                    marker(
                        TimelineMarkerKind::Tool,
                        time,
                        tool_preview(&tool),
                        Some(tool),
                    )
                };
                if tools.contains(name) {
                    markers.push(tool_marker(traced_tool(trace, name, entry.is_running)));
                }
                if name.eq_ignore_ascii_case("codemode") {
                    markers.extend(
                        codemode_timeline_tools(trace, &tools, entry.is_running).map(tool_marker),
                    );
                }
            }
            if let Some(hidden_tools) = hidden_by_anchor.remove(&entry_id) {
                markers.extend(hidden_tools.into_iter().map(|hidden| {
                    marker(
                        TimelineMarkerKind::Tool,
                        hidden.created_at,
                        tool_preview(&hidden.tool),
                        Some(hidden.tool.clone()),
                    )
                }));
            }
        }
    }

    /// Resolve active/previous/next marker indices from the current viewport.
    ///
    /// Marker indices are stable for the frame that built `markers` and are the
    /// indices stored in [`crate::views::timeline::TimelineHit::Tick`].
    pub fn timeline_marker_viewport(
        &self,
        markers: &[TimelineMarker],
    ) -> (Option<usize>, Option<usize>, Option<usize>) {
        if markers.is_empty() {
            return (None, None, None);
        }
        let Some(cache) = self.layout_cache.as_ref() else {
            return (None, None, None);
        };
        let range = self.visible_entry_range();
        let Some(base) = cache.virtual_y.get(range.start).copied() else {
            return (None, None, None);
        };
        let header = self
            .current_sticky_layout(cache, &range)
            .header_screen_rows();
        let top = base + self.scroll_offset + usize::from(header);
        let marker_y = |marker: &TimelineMarker| cache.virtual_y.get(marker.entry_index).copied();
        let through = markers.partition_point(|marker| marker_y(marker).is_some_and(|y| y <= top));
        let before = markers.partition_point(|marker| marker_y(marker).is_some_and(|y| y < top));
        let active = through.checked_sub(1);
        let up = before.checked_sub(1);
        let down = (through < markers.len()).then_some(through);
        (active, up, down)
    }

    /// Jump by the anchor from the last frame. Selection switches single-turn view to
    /// the target's turn before the viewport is moved.
    pub fn jump_to_timeline_marker(&mut self, entry_id: EntryId) -> bool {
        let Some(entry_idx) = self.index_of_id(entry_id) else {
            return false;
        };
        let previous_turn = self.current_turn;
        self.set_selected(Some(entry_idx));
        if self.view_mode == ViewMode::SingleTurn && previous_turn != self.current_turn {
            self.scroll_offset = 0;
            self.rebuild_layout();
        }
        if self.entry_content_hidden_by_group(entry_idx) {
            self.reveal_entry_line(entry_idx, 0);
        }
        self.scroll_to_entry_top(entry_idx);
        self.bump_generation();
        true
    }

    /// Preview text for one turn, used by the timeline rail hover card.
    pub fn turn_preview(&self, turn_idx: usize) -> Option<String> {
        let turn = self.turns.get(turn_idx)?;
        let entry = self.entries.get_index(turn.prompt_index)?.1;
        let RenderBlock::UserPrompt(prompt) = &entry.block else {
            return None;
        };
        let line = prompt
            .text
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("");
        let mut preview: String = line.chars().take(PREVIEW_MAX_CHARS).collect();
        if line.chars().nth(PREVIEW_MAX_CHARS).is_some() {
            preview.pop();
            preview.push('…');
        }
        Some(preview)
    }

    /// Prompt creation time for one turn, used by the timeline rail hover card.
    pub fn turn_created_at(&self, turn_idx: usize) -> Option<chrono::DateTime<chrono::Local>> {
        let turn = self.turns.get(turn_idx)?;
        self.entries.get_index(turn.prompt_index)?.1.created_at
    }

    /// The turn owning the viewport top, if any.
    pub fn active_turn_for_viewport(&self) -> Option<usize> {
        if self.view_mode == ViewMode::SingleTurn {
            return self.current_turn;
        }
        if self.turns.is_empty() {
            return None;
        }
        Some(self.prompts_above_top(false)?.saturating_sub(1))
    }

    /// Jump to a turn's prompt and anchor it at the viewport top.
    pub fn jump_to_turn(&mut self, turn_idx: usize) -> bool {
        if turn_idx >= self.turns.len() {
            return false;
        }
        self.activate_turn(turn_idx);
        true
    }

    /// The nearest turn strictly above the viewport top.
    pub fn turn_above_viewport_top(&self) -> Option<usize> {
        if self.view_mode == ViewMode::SingleTurn {
            return self.current_turn?.checked_sub(1);
        }
        self.prompts_above_top(true)?.checked_sub(1)
    }

    /// The nearest turn below the viewport top.
    pub fn turn_below_viewport_top(&self) -> Option<usize> {
        if self.view_mode == ViewMode::SingleTurn {
            let next = self.current_turn?.checked_add(1)?;
            return (next < self.turns.len()).then_some(next);
        }
        let next = self.prompts_above_top(false)?;
        (next < self.turns.len()).then_some(next)
    }

    fn prompts_above_top(&self, strict: bool) -> Option<usize> {
        let cache = self.layout_cache.as_ref()?;
        let range = self.visible_entry_range();
        let base = *cache.virtual_y.get(range.start)?;
        let top = base + self.scroll_offset;
        Some(self.turns.partition_point(|turn| {
            cache
                .virtual_y
                .get(turn.prompt_index)
                .is_some_and(|&prompt_y| {
                    if strict {
                        prompt_y < top
                    } else {
                        prompt_y <= top
                    }
                })
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeline_markers_include_compaction_in_scrollback_order() {
        let mut state = ScrollbackState::new();
        state.push_block(RenderBlock::user_prompt("first prompt"));
        state.push_block(RenderBlock::agent_message("first answer"));
        state.push_block(RenderBlock::session_event(
            crate::scrollback::blocks::SessionEvent::CompactionSummary {
                summary: "preserved context and latest tool results".into(),
            },
        ));
        state.push_block(RenderBlock::user_prompt("second prompt"));
        state.push_block(RenderBlock::agent_message("second answer"));

        let markers = state.timeline_markers(&TimelineContents::default());
        assert_eq!(markers.len(), 3);
        assert_eq!(markers[0].kind, TimelineMarkerKind::Prompt);
        assert_eq!(markers[0].entry_index, 0);
        assert_eq!(markers[1].kind, TimelineMarkerKind::Compaction);
        assert_eq!(markers[1].entry_index, 2);
        assert_eq!(markers[1].turn_idx, Some(0));
        assert!(markers[1].preview.contains("preserved context"));
        assert_eq!(markers[2].kind, TimelineMarkerKind::Prompt);
        assert_eq!(markers[2].entry_index, 3);
    }

    #[test]
    fn timeline_cache_reuses_unchanged_frames_and_replaces_the_branch_suffix() {
        let mut state = ScrollbackState::new();
        let first = state.push_block(RenderBlock::user_prompt("common prompt"));
        state.push_block(RenderBlock::agent_message("common answer"));
        let old = state.push_block(RenderBlock::user_prompt("old branch"));
        let rules = TimelineContents::default();
        let before = state.timeline_markers(&rules);
        assert!(Arc::ptr_eq(&before, &state.timeline_markers(&rules)));
        state.remove_from(2);
        let next = state.push_block(RenderBlock::user_prompt("new branch"));
        state.invalidate_timeline_branch();
        let after = state.timeline_markers(&rules);
        assert_eq!(
            before
                .iter()
                .map(|marker| marker.entry_id)
                .collect::<Vec<_>>(),
            [first, old]
        );
        assert_eq!(
            after
                .iter()
                .map(|marker| marker.entry_id)
                .collect::<Vec<_>>(),
            [first, next]
        );
        assert_eq!(after[0], before[0]);
    }

    #[test]
    fn prompt_previews_use_only_the_final_reply_inside_each_turn() {
        let mut state = ScrollbackState::new();
        state.push_block(RenderBlock::user_prompt("first prompt"));
        state.push_block(RenderBlock::agent_message("intermediate text"));
        state.push_block(RenderBlock::tool_call("read", "reading", true));
        state.push_block(RenderBlock::agent_message("final answer, starting here"));
        state.push_block(RenderBlock::session_event(
            crate::scrollback::blocks::SessionEvent::CompactionSummary {
                summary: "summary after the final answer".into(),
            },
        ));
        state.push_block(RenderBlock::user_prompt("second prompt"));
        state.push_block(RenderBlock::agent_message("working on it"));
        state.push_block(RenderBlock::tool_call("edit", "editing", true));
        state.push_block(RenderBlock::user_prompt("third prompt"));
        state.push_block(RenderBlock::agent_message("streaming reply"));
        state.set_last_running(true);

        let markers = state.timeline_markers(&TimelineContents::default());
        assert_eq!(
            state.timeline_reply(&markers[0]).as_deref(),
            Some("final answer, starting here")
        );
        assert_eq!(state.timeline_reply(&markers[2]), None);
        assert_eq!(state.timeline_reply(&markers[3]), None);
    }

    #[test]
    fn edits_and_writes_roll_up_into_the_prompt_preview_only() {
        use crate::scrollback::entry::{ToolFileChange, ToolTraceSnapshot};

        let mut state = ScrollbackState::new();
        state.push_block(RenderBlock::user_prompt("change some files"));
        let first = state.push_block(RenderBlock::tool_call("edit", "first file", true));
        let trace = |name: &str, path: &str, added, deleted, written| ToolTraceSnapshot {
            tool_call_id: format!("{name}:{path}"),
            pi_tool_name: Some(name.to_owned()),
            file_change: Some(ToolFileChange {
                path: path.to_owned(),
                added,
                deleted,
                written,
            }),
            title: name.to_owned(),
            status: "Completed".into(),
            raw_input: None,
            raw_output: None,
            usage: None,
            context_tokens: None,
            stream_start_ms: None,
            started_at_ms: None,
            updated_at_ms: None,
        };
        state.get_by_id_mut(first).unwrap().tool_traces = vec![
            trace("edit", "src/a.rs", 3, 2, 0),
            trace("write", "src/a.rs", 0, 0, 15),
            trace("edit", "src/b.rs", 1, 0, 0),
            trace("write", "src/c.rs", 0, 0, 9),
        ];
        let failed = state.push_block(RenderBlock::tool_call("write", "failed", false));
        let mut failed_trace = trace("write", "failed.rs", 0, 0, 999);
        failed_trace.status = "Failed".into();
        failed_trace.file_change = None;
        state
            .get_by_id_mut(failed)
            .unwrap()
            .tool_traces
            .push(failed_trace);
        state.push_block(RenderBlock::user_prompt("another turn"));
        let other = state.push_block(RenderBlock::tool_call("edit", "other file", true));
        state
            .get_by_id_mut(other)
            .unwrap()
            .tool_traces
            .push(trace("edit", "other.rs", 100, 100, 0));
        let rules = TimelineContents {
            tools: vec!["edit".into(), "write".into()],
            ..Default::default()
        };
        let markers = state.timeline_markers(&rules);
        assert_eq!(markers.len(), 2);
        assert!(
            markers
                .iter()
                .all(|marker| marker.kind == TimelineMarkerKind::Prompt)
        );
        let summary = state.timeline_file_summary(&markers[0], &rules).unwrap();
        assert_eq!(
            (
                summary.file_count,
                summary.added,
                summary.deleted,
                summary.written
            ),
            (3, 4, 2, 24)
        );
        assert_eq!(summary.files.len(), 3);
        assert_eq!(summary.files[0].path, "src/a.rs");
        assert_eq!((summary.files[0].added, summary.files[0].written), (3, 15));
        let edits_only = TimelineContents {
            tools: vec!["edit".into()],
            ..Default::default()
        };
        let summary = state
            .timeline_file_summary(&markers[0], &edits_only)
            .unwrap();
        assert_eq!(
            (
                summary.file_count,
                summary.added,
                summary.deleted,
                summary.written
            ),
            (2, 4, 2, 0)
        );
    }

    #[test]
    fn tool_markers_use_exact_names_and_keep_coalesced_call_anchors() {
        use crate::scrollback::entry::ToolTraceSnapshot;
        let mut state = ScrollbackState::new();
        let prompt = state.push_block(RenderBlock::user_prompt("prompt"));
        let card = state.push_block(RenderBlock::agent_message("coalesced card"));
        state.set_last_running(true);
        let trace = |id: &str, name: &str, status: &str| ToolTraceSnapshot {
            tool_call_id: id.to_owned(),
            pi_tool_name: Some(name.to_owned()),
            file_change: None,
            title: "Display title".into(),
            status: status.to_owned(),
            raw_input: Some(serde_json::json!({"path": "src/main.rs"})),
            raw_output: None,
            usage: None,
            context_tokens: None,
            stream_start_ms: None,
            started_at_ms: None,
            updated_at_ms: None,
        };
        state.get_by_id_mut(card).unwrap().tool_traces = vec![
            trace("a", "mcp__repo__search", "Completed"),
            trace("b", "mcp__repo__search", "InProgress"),
            trace("c", "Mcp__repo__search", "Completed"),
        ];
        let rules = TimelineContents {
            user_prompts: false,
            compactions: false,
            tools: vec!["mcp__repo__search".into()],
        };
        let before = state.timeline_markers(&rules);
        assert_eq!(before.len(), 2);
        assert!(before.iter().all(|marker| marker.entry_id == card));
        assert_eq!(before[1].tool.as_ref().unwrap().status, "running");
        state.get_by_id_mut(card).unwrap().tool_traces[1].status = "Failed".into();
        state.finish_running(card);
        let after = state.timeline_markers(&rules);
        assert_eq!(after.len(), 2);
        assert_eq!(after[1].tool.as_ref().unwrap().call_id, "b");
        assert_eq!(after[1].tool.as_ref().unwrap().status, "failed");
        assert_eq!(state.timeline_entries()[0].prompt_entry_id, prompt);
    }

    #[test]
    fn timeline_marker_jump_targets_compaction_block() {
        let mut state = ScrollbackState::new();
        state.push_block(RenderBlock::user_prompt("first prompt"));
        state.push_block(RenderBlock::agent_message("first answer"));
        state.push_block(RenderBlock::session_event(
            crate::scrollback::blocks::SessionEvent::CompactionSummary {
                summary: "preserved context".into(),
            },
        ));
        state.push_block(RenderBlock::user_prompt("second prompt"));
        state.prepare_layout(80, 10);

        let marker = &state.timeline_markers(&TimelineContents::default())[1];
        assert!(state.jump_to_timeline_marker(marker.entry_id));
        assert_eq!(state.selected(), Some(2));
        assert_eq!(state.current_turn(), Some(0));
    }
}
