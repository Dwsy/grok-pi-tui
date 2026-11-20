//! Pi Timeline bridge. The Pager owns the rules, frame anchors, and presentation.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ratatui::text::Line;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use xai_grok_shell::util::config::TimelineContents;

use super::actions::Effect;
use super::agent_view::AgentView;
use super::app_view::{ActiveView, AppView};
use crate::scrollback::state::{
    ScrollbackState, TimelineFileSummary, TimelineMarker, TimelineMarkerKind, TimelineTool,
};
use crate::theme::Theme;
use crate::views::timeline::TimelineRail;

const FORMAT_DEADLINE: Duration = Duration::from_secs(1);
const MAX_BATCH: usize = 64;
const MAX_CACHE: usize = 512;
const MAX_LINE_BYTES: usize = 2048;

pub(super) type TimelineOptions<'a> = (&'a TimelineContents, Option<&'a str>);

impl AgentView {
    pub(super) fn timeline_view_mut(&mut self) -> Option<&mut Self> {
        if let Some(child) = self.active_subagent.as_deref() {
            return self.subagent_views.get_mut(child)?.timeline_view_mut();
        }
        Some(self)
    }
}

pub(super) struct TimelineFrame {
    pub rail: TimelineRail,
    pub branch: u64,
    pub markers: Arc<Vec<TimelineMarker>>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct FormatScope {
    host: String,
    session: String,
    branch: u64,
    theme: u64,
}

#[derive(Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum FormatEntry {
    Prompt {
        prompt: String,
        reply: Option<String>,
        changes: Option<TimelineFileSummary>,
    },
    Compaction {
        summary: String,
    },
    Tool {
        #[serde(flatten)]
        tool: TimelineTool,
    },
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) struct FormatKey {
    marker_id: u64,
    entry: FormatEntry,
    preview_columns: u16,
}

impl FormatKey {
    pub fn new(
        scrollback: &ScrollbackState,
        marker: &TimelineMarker,
        preview_columns: u16,
        rules: Option<&TimelineContents>,
    ) -> Option<Self> {
        let entry = match marker.kind {
            TimelineMarkerKind::Prompt => FormatEntry::Prompt {
                prompt: marker.preview.clone(),
                reply: scrollback.timeline_reply(marker),
                changes: rules.and_then(|rules| scrollback.timeline_file_summary(marker, rules)),
            },
            TimelineMarkerKind::Compaction => FormatEntry::Compaction {
                summary: marker.preview.clone(),
            },
            TimelineMarkerKind::Tool => FormatEntry::Tool {
                tool: marker.tool.clone()?,
            },
        };
        Some(Self {
            marker_id: marker.entry_id.value(),
            entry,
            preview_columns,
        })
    }

    pub fn native_preview<'a>(&'a self, marker: &'a TimelineMarker) -> (&'a str, Option<&'a str>) {
        match &self.entry {
            FormatEntry::Prompt { prompt, reply, .. } => (prompt, reply.as_deref()),
            FormatEntry::Compaction { summary } => ("Compaction summary", Some(summary)),
            FormatEntry::Tool { .. } => (&marker.preview, None),
        }
    }

    pub fn native_file_rows(&self) -> Vec<String> {
        let FormatEntry::Prompt {
            changes: Some(changes),
            ..
        } = &self.entry
        else {
            return Vec::new();
        };
        let mut rows = vec![format!(
            "{} files · +{} -{} · {} written",
            changes.file_count, changes.added, changes.deleted, changes.written
        )];
        rows.extend(changes.files.iter().map(|file| {
            let counts = format!(
                "+{} -{} · {} written",
                file.added, file.deleted, file.written
            );
            let name = crate::render::tool_paths::path_for_tool_header(
                &file.path,
                Some(usize::from(self.preview_columns)),
                counts.len() + 2,
            );
            format!("{counts}  {name}")
        }));
        rows
    }

    fn version(&self) -> String {
        let mut hash = DefaultHasher::new();
        self.hash(&mut hash);
        format!("{:016x}", hash.finish())
    }
}

#[derive(Default)]
pub(super) struct MarkerFormat {
    pub tick: Option<Line<'static>>,
    pub preview: Option<Vec<Line<'static>>>,
}

struct PendingBatch {
    id: u64,
    sent: Instant,
    keys: Vec<FormatKey>,
}

#[derive(Default)]
pub(super) struct TimelineFormatting {
    scope: Option<FormatScope>,
    theme: Option<Theme>,
    theme_version: u64,
    cache: HashMap<FormatKey, MarkerFormat>,
    pending: Option<PendingBatch>,
    next_id: u64,
}

impl TimelineFormatting {
    pub fn request(
        &mut self,
        host: &str,
        session: &str,
        branch: u64,
        inputs: &[Option<FormatKey>],
        theme: &Theme,
    ) -> Option<Value> {
        if self.theme.as_ref() != Some(theme) {
            self.theme = Some(*theme);
            self.theme_version += 1;
        }
        let scope = FormatScope {
            host: host.to_owned(),
            session: session.to_owned(),
            branch,
            theme: self.theme_version,
        };
        if self.scope.as_ref() != Some(&scope) {
            if self.scope.as_ref().is_none_or(|old| {
                old.host != scope.host || old.session != scope.session || old.theme != scope.theme
            }) {
                self.cache.clear();
            }
            // Completed formatter output is a pure function of FormatKey.
            // Requests themselves are scoped to the branch that issued them.
            self.pending = None;
            self.scope = Some(scope);
        }
        if let Some(batch) = self.pending.as_ref() {
            if batch.sent.elapsed() < FORMAT_DEADLINE {
                return None;
            }
            // Cache the native fallback for each timed-out input.
            for key in self.pending.take()?.keys {
                self.cache.insert(key, MarkerFormat::default());
            }
        }
        if self.cache.len() >= MAX_CACHE.max(inputs.len() + MAX_BATCH) {
            self.cache.clear();
        }
        let keys: Vec<_> = inputs
            .iter()
            .flatten()
            .filter(|key| !self.cache.contains_key(*key))
            .take(MAX_BATCH)
            .cloned()
            .collect();
        if keys.is_empty() {
            return None;
        }
        let preview_columns = keys.first()?.preview_columns;
        self.next_id += 1;
        let entries: Vec<_> = keys
            .iter()
            .map(|key| {
                json!({
                    "entry": key.entry,
                    "markerId": key.marker_id,
                    "version": key.version(),
                })
            })
            .collect();
        self.pending = Some(PendingBatch {
            id: self.next_id,
            sent: Instant::now(),
            keys,
        });
        Some(json!({
            "op": "format",
            "scope": self.scope.as_ref(),
            "id": self.next_id,
            "tickColumns": crate::views::timeline::RAIL_WIDTH,
            "previewColumns": preview_columns,
            "entries": entries,
        }))
    }

    pub fn get(&self, key: &FormatKey) -> Option<&MarkerFormat> {
        self.cache.get(key)
    }

    fn accept(&mut self, scope: &FormatScope, params: &Value) -> bool {
        let Some(batch) = self.pending.as_ref() else {
            return false;
        };
        if self.scope.as_ref() != Some(scope)
            || params.get("id").and_then(Value::as_u64) != Some(batch.id)
            || batch.sent.elapsed() >= FORMAT_DEADLINE
        {
            return false;
        }
        let Some(theme) = self.theme else {
            return false;
        };
        let Some(batch) = self.pending.take() else {
            return false;
        };
        let entries = params.get("entries").and_then(Value::as_array);
        for (index, key) in batch.keys.into_iter().enumerate() {
            let formatted = entries
                .and_then(|entries| entries.get(index))
                .filter(|entry| {
                    entry.get("version").and_then(Value::as_str) == Some(key.version().as_str())
                        && entry.get("markerId").and_then(Value::as_u64) == Some(key.marker_id)
                })
                .and_then(|entry| parse_format(entry, &theme))
                .unwrap_or_default();
            self.cache.insert(key, formatted);
        }
        true
    }
}

fn parse_format(entry: &Value, theme: &Theme) -> Option<MarkerFormat> {
    let tick = match entry.get("tickAnsi") {
        None => None,
        Some(value) => {
            let line = sgr_line(value.as_str()?, theme)?;
            (line.width() > 0).then_some(line)
        }
    };
    let preview = match entry.get("previewAnsi") {
        None => None,
        Some(value) => {
            let lines = value.as_array()?;
            if lines.len() > 6 {
                return None;
            }
            let lines = lines
                .iter()
                .map(|value| sgr_line(value.as_str()?, theme))
                .collect::<Option<Vec<_>>>()?;
            lines.iter().any(|line| line.width() > 0).then_some(lines)
        }
    };
    Some(MarkerFormat { tick, preview })
}

// Permit only CSI SGR control sequences. Each row enters the ANSI parser with
// a fresh style, then gets clipped by the native buffer to its column budget.
fn sgr_line(ansi: &str, theme: &Theme) -> Option<Line<'static>> {
    if ansi.len() > MAX_LINE_BYTES {
        return None;
    }
    let mut chars = ansi.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            if chars.next() != Some('[') {
                return None;
            }
            while chars
                .next_if(|ch| ch.is_ascii_digit() || matches!(*ch, ';' | ':'))
                .is_some()
            {}
            if chars.next() != Some('m') {
                return None;
            }
        } else if ch.is_control() {
            return None;
        }
    }
    let line = crate::views::agent_status::ansi_remote_tui_line(ansi, theme);
    line.spans
        .iter()
        .all(|span| !span.content.chars().any(char::is_control))
        .then_some(line)
}

pub(super) fn handle_message(app: &mut AppView, params: Value) -> bool {
    let Some(op) = params.get("op").and_then(Value::as_str) else {
        return false;
    };
    if op == "formatted" {
        let Some(scope) = params
            .get("scope")
            .and_then(|scope| FormatScope::deserialize(scope).ok())
        else {
            return false;
        };
        if app.timeline_host.as_deref() != Some(scope.host.as_str()) {
            return false;
        }
        let ActiveView::Agent(id) = app.active_view else {
            return false;
        };
        return app
            .agents
            .get_mut(&id)
            .and_then(AgentView::timeline_view_mut)
            .is_some_and(|agent| {
                agent.scrollback.timeline_branch() == scope.branch
                    && agent
                        .session
                        .session_id
                        .as_ref()
                        .is_some_and(|id| id.0.as_ref() == scope.session.as_str())
                    && agent.timeline_formatting.accept(&scope, &params)
            });
    }
    let Some(host) = params.get("host").and_then(Value::as_str).filter(|host| {
        host.len() == 36
            && host
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
    }) else {
        return false;
    };
    if op == "ready" {
        app.timeline_host = Some(host.to_owned());
        return true;
    }
    if app.timeline_host.as_deref() != Some(host) {
        return false;
    }
    let Some(id) = params
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| id.len() <= 64)
    else {
        return false;
    };
    match op {
        "get" => reply(app, host, id, None),
        "save" => {
            let mut contents = params
                .get("contents")
                .ok_or_else(|| "Missing Timeline contents".to_owned())
                .and_then(|value| {
                    TimelineContents::deserialize(value).map_err(|error| error.to_string())
                });
            if let Ok(rules) = &mut contents
                && let Err(error) = rules.normalize()
            {
                contents = Err(error.to_owned());
            }
            match contents {
                Ok(contents) if !app.timeline_saving => {
                    app.timeline_saving = true;
                    app.pending_effects.push(Effect::SaveTimeline {
                        host: host.to_owned(),
                        id: id.to_owned(),
                        contents,
                    });
                }
                Ok(_) => reply(
                    app,
                    host,
                    id,
                    Some("A Timeline save is already in progress."),
                ),
                Err(error) => reply(app, host, id, Some(&error)),
            }
        }
        _ => return false,
    }
    true
}

fn reply(app: &mut AppView, host: &str, id: &str, error: Option<&str>) {
    app.pending_effects.push(Effect::TimelineMessage(json!({
        "op": "config",
        "host": host,
        "id": id,
        "contents": app.timeline_contents,
        "error": error,
    })));
}

pub(super) fn finish_save(
    app: &mut AppView,
    host: &str,
    id: &str,
    result: Result<TimelineContents, String>,
) {
    app.timeline_saving = false;
    let error = match result {
        Ok(contents) => {
            if app.timeline_contents != contents {
                app.timeline_contents = contents;
                for agent in app.agents.values_mut() {
                    if let Some(view) = agent.timeline_view_mut() {
                        view.timeline_frame = None;
                    }
                }
            }
            None
        }
        Err(error) => Some(error),
    };
    if app.timeline_host.as_deref() == Some(host) {
        reply(app, host, id, error.as_deref());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_bounded_sgr_tool_output_can_reach_the_timeline() {
        let theme = Theme::current();
        let valid = json!({"tickAnsi": "\x1b[35mE", "previewAnsi": ["src/文件.rs", "success"]});
        let format = parse_format(&valid, &theme).unwrap();
        assert_eq!(format.tick.unwrap().width(), 1);
        assert_eq!(format.preview.unwrap().len(), 2);

        for invalid in [
            json!({"tickAnsi": "\x1b[2J"}),
            json!({"previewAnsi": ["good", "\x1b]8;;https://example.com\x07bad"]}),
            json!({"tickAnsi": "a".repeat(MAX_LINE_BYTES + 1)}),
            json!({"previewAnsi": ["one", "two", "three", "four", "five", "six", "seven"]}),
        ] {
            assert!(parse_format(&invalid, &theme).is_none());
        }
    }
}
