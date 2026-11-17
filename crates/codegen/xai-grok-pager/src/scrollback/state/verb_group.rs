//! Builds the "Read 10 files, Ran 2 subagents" header label for a folded run of consecutive non-destructive tool calls and subagent lifecycle rows.
//! A run also claims any finished collapsed thoughts inside it.
//! The run classification ([`run_step`]) also lives here, shared by the layout fold, range resolution, and the label walk.
//!
//! The layout pass in `state/layout.rs` detects the runs and marks the header via `EntryLayoutInfo::verb_group_header`.
//! The render loop calls [`verb_group_header_label`] to build the live label each frame.
//! Running entries repaint every tick, so tense and counts update in place; no per-call detail churns beside the label while the run executes.
//!
//! The same bucket vocabulary labels the truncation ("N more") headers: the render loop calls [`truncation_header_label`] with the fold's span.
//! Both walks feed the shared `BucketAccumulator` so the two label families can't drift.

use ratatui::style::Modifier;
use ratatui::text::{Line, Span};

use crate::scrollback::block::RenderBlock;
use crate::scrollback::blocks::SubagentBlockKind;
use crate::scrollback::blocks::tool::{ToolCallBlock, VerbGroupKind};
use crate::scrollback::entry::ScrollbackEntry;
use crate::scrollback::types::DisplayMode;
use crate::theme::Theme;

/// Which runs a tool-call density folds.
///
/// `VerbOnly` is the pre-density shape: read/search/list/web/subagent runs fold and
/// nothing else does. `ByFamily` additionally folds runs of edits, runs of commands,
/// and runs of other tools, each family separately. `AllRuns` folds every run of
/// consecutive tool calls into one row, whichever families it mixes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GroupingMode {
    VerbOnly,
    ByFamily,
    AllRuns,
}

impl GroupingMode {
    /// The mode selected by the live `[ui] tool_call_density` cache.
    pub(crate) fn current() -> Self {
        let density = crate::appearance::cache::load_tool_call_density();
        if density.groups_all_tool_runs() {
            Self::AllRuns
        } else if density.groups_action_runs() {
            Self::ByFamily
        } else {
            Self::VerbOnly
        }
    }

    /// Whether this mode is at least as permissive as `other`.
    pub(crate) fn folds_actions(self) -> bool {
        !matches!(self, Self::VerbOnly)
    }

    /// The fold family of a member kind. Members only join a run of their own family;
    /// `AllRuns` puts every tool call in one family so mixed runs fold together.
    pub(crate) fn family(self, kind: VerbGroupKind) -> u8 {
        if matches!(self, Self::AllRuns) {
            return 0;
        }
        match kind {
            // Explore/tool-verb family: reads, searches, listings, web, subagents.
            VerbGroupKind::File
            | VerbGroupKind::Skill
            | VerbGroupKind::Search
            | VerbGroupKind::Dir
            | VerbGroupKind::WebFetch
            | VerbGroupKind::WebSearch
            | VerbGroupKind::MemorySearch
            | VerbGroupKind::IntegrationSearch
            | VerbGroupKind::Subagent => 0,
            VerbGroupKind::EditFile => 1,
            VerbGroupKind::Command => 2,
            VerbGroupKind::OtherTool => 3,
            VerbGroupKind::McpCall => 4,
            VerbGroupKind::Message => 5,
        }
    }
}

/// One step of a verb-group run walk.
pub(crate) enum RunStep {
    /// A collapsed verb-groupable tool or subagent entry: joins the run and counts toward the fold threshold ([`RunScan::folds`]).
    Member(VerbGroupKind),
    /// A finished, collapsed, shown thinking entry: claimed into the run (folds to height 0).
    /// It never counts toward the threshold and never appears in the header label.
    ThoughtMember,
    /// An entry that renders its own rows (or none) without joining or breaking the run.
    /// That covers hidden, streaming, user-opened thinking, thinking that carries prompt chrome, and a manually-opened verb-groupable tool.
    Transparent,
    /// Anything else: ends the run.
    Break,
}

/// A manually-opened member is [`RunStep::Transparent`] and keeps its own rows without splitting the run. Thinking
/// never breaks a run: a finished collapsed thought without prompt chrome folds in as
/// [`RunStep::ThoughtMember`].
pub(crate) fn run_step(entry: &ScrollbackEntry, show_thinking: bool) -> RunStep {
    run_step_with(entry, show_thinking, GroupingMode::current())
}

/// [`run_step`] under an explicit grouping mode, so tests never depend on the live cache.
pub(crate) fn run_step_with(
    entry: &ScrollbackEntry,
    show_thinking: bool,
    mode: GroupingMode,
) -> RunStep {
    let is_claimable_thinking =
        entry.display_mode == DisplayMode::Collapsed && !entry.is_pending_user_input;
    if let RenderBlock::ToolCall(block) = &entry.block
        && !entry.is_pending_user_input
    {
        if let Some(kind) = block.verb_group_kind() {
            return if entry.display_mode == DisplayMode::Collapsed {
                RunStep::Member(kind)
            } else {
                // A manually-opened member keeps its own rows without splitting the run, the same treatment as opened thinking
                // Toggling a member of an expanded group therefore never dissolves the group
                RunStep::Transparent
            };
        }
        // Action tools (edits, commands, MCP calls, …) join a run only at densities
        // that fold them; a manually-opened row stays transparent, like an opened verb member.
        if mode.folds_actions()
            && let Some(kind) = block.label_kind()
        {
            return if entry.display_mode == DisplayMode::Collapsed {
                RunStep::Member(kind)
            } else {
                RunStep::Transparent
            };
        }
        return RunStep::Break;
    }
    if matches!(entry.block, RenderBlock::Subagent(_)) {
        if entry.display_mode == DisplayMode::Collapsed && !entry.is_pending_user_input {
            RunStep::Member(VerbGroupKind::Subagent)
        } else {
            // Subagent rows are always collapsed single-row entries; prompt chrome keeps them standalone and breaks the run
            RunStep::Break
        }
    } else if entry.block.is_thinking() {
        if show_thinking && !entry.is_running && is_claimable_thinking {
            RunStep::ThoughtMember
        } else {
            RunStep::Transparent
        }
    } else {
        RunStep::Break
    }
}

/// Whether an in-place block swap changes the entry's verb-group kind (e.g. the eager `Other` placeholder refining into a `Read`).
/// Such swaps change fold membership, so the swap site must mark the entry structurally dirty for the layout fold to catch up on the next frame.
pub(crate) fn verb_group_kind_changed(old: &RenderBlock, new: &RenderBlock) -> bool {
    let kind_of = |block: &RenderBlock| match block {
        RenderBlock::ToolCall(tc) => tc.verb_group_kind(),
        _ => None,
    };
    kind_of(old) != kind_of(new)
}

/// Shape of one forward run walk, as reported by [`scan_run_forward`].
pub(crate) struct RunScan {
    /// Member entries counted (tool calls and subagent rows), including a member start entry.
    /// Thought members claim but never count; the fold threshold is members-only.
    pub(crate) members: usize,
    /// Exclusive run end: one past the last claimed entry (member or thought member), so trailing transparent entries stay outside the run.
    pub(crate) end: usize,
    /// Where the walk stopped: the breaking entry's index, or the first index where `entry_at` returned `None`.
    pub(crate) stop: usize,
}

impl RunScan {
    /// Whether the run folds into a verb-group header row. Thought members never count, so a pure-thought run (whose
    /// label would be empty) never folds. A single tool call never folds either: a one-row header hides the call it
    /// summarizes without saving meaningful space. The layout fold and `verb_group_range_of` share this predicate so
    /// the two can't drift.
    pub(crate) fn folds(&self) -> bool {
        self.members >= 2
    }
}

/// Returns `None` when the entry at `start` is missing or cannot anchor a run. Members and thought members can
/// anchor; transparent and breaking entries cannot. Anchor eligibility lives in this function's matches, not in
/// caller pre-checks, so a returned scan always has `end > start` and `stop > start`.
pub(crate) fn scan_run_forward<'e>(
    entry_at: impl Fn(usize) -> Option<&'e ScrollbackEntry>,
    start: usize,
    show_thinking: bool,
) -> Option<RunScan> {
    scan_run_forward_with(entry_at, start, show_thinking, GroupingMode::current())
}

/// [`scan_run_forward`] under an explicit grouping mode.
///
/// The run keeps one family: at `ByFamily`, a command after two reads ends the
/// read run, and the command's own run starts there. `AllRuns` treats every
/// tool call as one family, so mixed runs fold together.
pub(crate) fn scan_run_forward_with<'e>(
    entry_at: impl Fn(usize) -> Option<&'e ScrollbackEntry>,
    start: usize,
    show_thinking: bool,
    mode: GroupingMode,
) -> Option<RunScan> {
    // Members and finished thoughts anchor runs; transparent thinking may sit inside one but cannot start one.
    // A leading thought decides no family: the first member it precedes picks one.
    let mut family: Option<u8> = match run_step_with(entry_at(start)?, show_thinking, mode) {
        RunStep::Member(kind) => Some(mode.family(kind)),
        RunStep::ThoughtMember => None,
        RunStep::Transparent | RunStep::Break => return None,
    };
    let mut members = 0usize;
    let mut end = start;
    let mut i = start;
    while let Some(entry) = entry_at(i) {
        match run_step_with(entry, show_thinking, mode) {
            RunStep::Member(kind) => {
                let kind_family = mode.family(kind);
                match family {
                    Some(current) if current != kind_family => break,
                    Some(_) => {}
                    None => family = Some(kind_family),
                }
                members += 1;
                end = i + 1;
            }
            RunStep::ThoughtMember => end = i + 1,
            RunStep::Transparent => {}
            RunStep::Break => break,
        }
        i += 1;
    }
    Some(RunScan {
        members,
        end,
        stop: i,
    })
}

/// Aggregated header state for one verb-group run.
pub struct VerbGroupHeaderLabel {
    /// Styled label line rendered on the header row.
    pub line: Line<'static>,
    /// Plain-text label used as the header row's selection and copy text.
    pub text: String,
    /// Any member still running (animated accent and present-tense verbs).
    pub running: bool,
    /// Any member failed (error accent).
    pub failed: bool,
}

/// The single channel that carries a group-header row's aggregated label, mirroring the fold families of
/// `groups::GroupKind`. A header row belongs to exactly one fold, so a row carries at most one label.
pub enum GroupHeaderLabel {
    /// Verb-group run header ("Read 3 files, Searched 2 patterns").
    VerbRun(VerbGroupHeaderLabel),
    /// Labeled truncation ("N more") header ("Ran 6 commands").
    Truncation(VerbGroupHeaderLabel),
}

impl GroupHeaderLabel {
    /// The aggregated label payload, whichever fold family produced it.
    pub fn label(&self) -> &VerbGroupHeaderLabel {
        match self {
            GroupHeaderLabel::VerbRun(label) | GroupHeaderLabel::Truncation(label) => label,
        }
    }
}

/// One aggregation bucket per verb kind, ordered by first appearance in the run.
/// Borrows citation strings from the walked blocks (per-frame, no allocation).
struct Bucket<'e> {
    kind: VerbGroupKind,
    calls: usize,
    /// Distinct-count override: when non-empty its size replaces `calls` as the displayed count.
    /// Holds WebSearch citation URLs (distinct result websites) and subagent child session ids.
    /// The started and terminal rows of one subagent count once; a burst of terminal rows counts each distinct subagent.
    sources: std::collections::HashSet<&'e str>,
}

/// The label counts members only: folded thoughts contribute nothing here and appear as their own member rows only
/// when the group is expanded. Callers with the fold's span pass its exact end so the label counts precisely the
/// entries the fold claimed.
pub fn verb_group_header_label<'e>(
    entries: &[&'e ScrollbackEntry],
    header_idx: usize,
    end: usize,
    show_thinking: bool,
    theme: &Theme,
) -> VerbGroupHeaderLabel {
    verb_group_header_label_with(
        entries,
        header_idx,
        end,
        show_thinking,
        GroupingMode::current(),
        theme,
    )
}

/// [`verb_group_header_label`] under an explicit grouping mode.
pub(crate) fn verb_group_header_label_with<'e>(
    entries: &[&'e ScrollbackEntry],
    header_idx: usize,
    end: usize,
    show_thinking: bool,
    mode: GroupingMode,
    theme: &Theme,
) -> VerbGroupHeaderLabel {
    let mut acc = BucketAccumulator::default();

    let end = end.min(entries.len());
    for &entry in &entries[header_idx.min(end)..end] {
        let kind = match run_step_with(entry, show_thinking, mode) {
            RunStep::Member(kind) => kind,
            RunStep::Break => break,
            RunStep::ThoughtMember | RunStep::Transparent => continue,
        };
        acc.push(kind, entry);
    }

    acc.into_label(theme)
}

/// Thoughts occupy participant slots but are never bucketed: like verb-group labels, group labels stay tools-only.
/// Thoughts are the only participants a label may silently omit. anything else would make the label under-describe
/// what the fold conceals.
pub fn truncation_header_label<'e>(
    entries: &[&'e ScrollbackEntry],
    range: std::ops::Range<usize>,
    limit: Option<usize>,
    show_thinking: bool,
    theme: &Theme,
) -> Option<VerbGroupHeaderLabel> {
    let mut acc = BucketAccumulator::default();
    let end = range.end.min(entries.len());
    let mut participants = 0usize;

    for &entry in &entries[range.start.min(end)..end] {
        if limit.is_some_and(|n| participants >= n) {
            break;
        }
        if entry.is_hidden_thinking(show_thinking) {
            continue;
        }
        participants += 1;
        if entry.block.is_thinking() {
            continue;
        }
        match &entry.block {
            RenderBlock::ToolCall(block) => acc.push(block.label_kind()?, entry),
            RenderBlock::Subagent(_) => acc.push(VerbGroupKind::Subagent, entry),
            // A participant the vocabulary can't name would leave the label dishonest about what's hidden
            // Decline so the numerically exact plain count renders instead
            _ => return None,
        }
    }

    if acc.is_empty() {
        return None;
    }
    Some(acc.into_label(theme))
}

/// Shared bucket accumulation and label rendering for the aggregated group headers.
/// Callers own the walk: which entries join and under what classification.
/// This owns per-kind counting, distinct-source overrides, tool outcome counting, and the rendered line.
#[derive(Default)]
struct BucketAccumulator<'e> {
    buckets: Vec<Bucket<'e>>,
    running: bool,
    failed_count: usize,
    /// Total `+N/-M` across folded edits, rendered once at the end of the label.
    insertions: usize,
    deletions: usize,
}

/// Summary families shared by the folded-run header. Kinds in one family share a
/// verb and list one item per kind ("Explored 2 files, 1 search").
#[derive(Clone, Copy, PartialEq, Eq)]
enum SummaryFamily {
    Explore,
    Fetch,
    Search,
    Edit,
    Run,
    Mcp,
    Message,
}

fn summary_family(kind: VerbGroupKind) -> SummaryFamily {
    match kind {
        VerbGroupKind::File | VerbGroupKind::Skill | VerbGroupKind::Dir | VerbGroupKind::Search => {
            SummaryFamily::Explore
        }
        VerbGroupKind::WebFetch => SummaryFamily::Fetch,
        VerbGroupKind::WebSearch
        | VerbGroupKind::MemorySearch
        | VerbGroupKind::IntegrationSearch => SummaryFamily::Search,
        VerbGroupKind::EditFile => SummaryFamily::Edit,
        VerbGroupKind::Command | VerbGroupKind::OtherTool | VerbGroupKind::Subagent => {
            SummaryFamily::Run
        }
        VerbGroupKind::McpCall => SummaryFamily::Mcp,
        VerbGroupKind::Message => SummaryFamily::Message,
    }
}

fn summary_verb(family: SummaryFamily, running: bool) -> &'static str {
    match (family, running) {
        (SummaryFamily::Explore, false) => "Explored",
        (SummaryFamily::Explore, true) => "Exploring",
        (SummaryFamily::Fetch, false) => "Fetched",
        (SummaryFamily::Fetch, true) => "Fetching",
        (SummaryFamily::Search, false) => "Searched",
        (SummaryFamily::Search, true) => "Searching",
        (SummaryFamily::Edit, false) => "Edited",
        (SummaryFamily::Edit, true) => "Editing",
        (SummaryFamily::Run, false) => "Ran",
        (SummaryFamily::Run, true) => "Running",
        (SummaryFamily::Mcp, false) => "Called",
        (SummaryFamily::Mcp, true) => "Calling",
        (SummaryFamily::Message, false) => "Sent",
        (SummaryFamily::Message, true) => "Sending",
    }
}

/// Lowercase a verb's first character so later families read as a sentence
/// ("Edited 2 files, ran 1 command").
fn lowercase_first(verb: &'static str) -> String {
    let mut chars = verb.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

impl<'e> BucketAccumulator<'e> {
    fn is_empty(&self) -> bool {
        self.buckets.is_empty()
    }

    fn push(&mut self, kind: VerbGroupKind, entry: &'e ScrollbackEntry) {
        let pos = match self.buckets.iter().position(|b| b.kind == kind) {
            Some(pos) => pos,
            None => {
                self.buckets.push(Bucket {
                    kind,
                    calls: 0,
                    sources: std::collections::HashSet::new(),
                });
                self.buckets.len() - 1
            }
        };
        let bucket = &mut self.buckets[pos];
        bucket.calls += 1;
        // Both walks only bucket tool-call or subagent rows
        // The block feeds the distinct-count override and failure detection
        match &entry.block {
            RenderBlock::ToolCall(block) => {
                if let ToolCallBlock::WebSearch(b) = block
                    && b.is_success()
                {
                    bucket
                        .sources
                        .extend(b.citations.iter().map(String::as_str));
                }
                if block_failed(block) {
                    self.failed_count += 1;
                }
                if let ToolCallBlock::Edit(edit) = block {
                    let (insertions, deletions) = edit.count_changes();
                    self.insertions += insertions;
                    self.deletions += deletions;
                }
            }
            RenderBlock::Subagent(sb) => {
                bucket.sources.insert(sb.child_session_id.as_str());
                // Cancelled is deliberate, not an error; only Failed feeds the red suffix
                if matches!(sb.kind, SubagentBlockKind::Failed { .. }) {
                    self.failed_count += 1;
                }
            }
            // Unreachable today; release keeps the generic count so the label can't desync from the fold that claimed the entry
            _ => debug_assert!(false, "bucketed entry has a block with no label-extras arm"),
        }

        if entry.is_running {
            self.running = true;
        }
    }

    fn into_label(self, theme: &Theme) -> VerbGroupHeaderLabel {
        let text_style = theme.fg(theme.gray_bright).add_modifier(Modifier::BOLD);
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut text = String::new();

        // Group buckets by summary family in first-appearance order, so kinds that
        // share a verb read as one clause ("Explored 2 files, 1 search").
        let mut groups: Vec<(SummaryFamily, Vec<String>)> = Vec::new();
        for bucket in &self.buckets {
            let count = if bucket.sources.is_empty() {
                bucket.calls
            } else {
                bucket.sources.len()
            };
            let item = format!("{} {}", count, bucket.kind.noun(count));
            let family = summary_family(bucket.kind);
            match groups.iter_mut().find(|(known, _)| *known == family) {
                Some((_, items)) => items.push(item),
                None => groups.push((family, vec![item])),
            }
        }

        for (i, (family, items)) in groups.iter().enumerate() {
            let segment = if i == 0 {
                format!(
                    "{} {}",
                    summary_verb(*family, self.running),
                    items.join(", ")
                )
            } else {
                format!(
                    ", {} {}",
                    lowercase_first(summary_verb(*family, self.running)),
                    items.join(", ")
                )
            };
            text.push_str(&segment);
            spans.push(Span::styled(segment, text_style));
        }
        if self.insertions > 0 || self.deletions > 0 {
            let suffix = format!(" +{} -{}", self.insertions, self.deletions);
            text.push_str(&suffix);
            spans.push(Span::styled(suffix, text_style));
        }
        if self.failed_count > 0 {
            let suffix = format!(" · {} failed", self.failed_count);
            text.push_str(&suffix);
            spans.push(Span::styled(suffix, theme.fg(theme.accent_error)));
        }
        VerbGroupHeaderLabel {
            line: Line::from(spans),
            text,
            running: self.running,
            failed: self.failed_count > 0,
        }
    }
}

/// Whether a bucketed block completed with an error.
/// Variants are listed explicitly so a new `ToolCallBlock` variant must decide here too.
/// The action kinds reach labels only through truncation buckets (verb folds exclude them), where their failures count like any other member's.
fn block_failed(block: &ToolCallBlock) -> bool {
    match block {
        ToolCallBlock::Read(b) => !b.is_success(),
        ToolCallBlock::ListDir(b) => !b.is_success(),
        ToolCallBlock::Search(b) => !b.is_success(),
        ToolCallBlock::WebFetch(b) => !b.is_success(),
        ToolCallBlock::WebSearch(b) => !b.is_success(),
        ToolCallBlock::MemorySearch(b) => !b.is_success(),
        ToolCallBlock::IntegrationSearch(b) => !b.is_success(),
        ToolCallBlock::Skill(b) => !b.is_success(),
        ToolCallBlock::Execute(b) => !b.is_success(),
        ToolCallBlock::Eval(b) => !b.is_success(),
        ToolCallBlock::Codemode(b) => !b.is_success(),
        ToolCallBlock::Edit(b) => !b.is_success(),
        ToolCallBlock::UseTool(b) => !b.is_success(),
        ToolCallBlock::SentMessage(b) => b.is_failure(),
        ToolCallBlock::Other(b) => !b.is_success(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scrollback::blocks::SubagentBlock;
    use crate::scrollback::blocks::tool::{
        ListDirToolCallBlock, ReadToolCallBlock, SearchToolCallBlock, WebSearchToolCallBlock,
    };

    fn entry(block: ToolCallBlock) -> ScrollbackEntry {
        ScrollbackEntry::new(RenderBlock::ToolCall(block)).with_display_mode(DisplayMode::Collapsed)
    }

    fn read(path: &str) -> ScrollbackEntry {
        entry(ToolCallBlock::Read(ReadToolCallBlock::new(path)))
    }

    fn subagent(block: SubagentBlock) -> ScrollbackEntry {
        ScrollbackEntry::new(RenderBlock::Subagent(block))
    }

    fn sub_started(child_sid: &str) -> ScrollbackEntry {
        subagent(SubagentBlock::started(
            "task", child_sid, "explore", None, None, None, /*is_background=*/ true,
        ))
    }

    fn sub_completed(child_sid: &str) -> ScrollbackEntry {
        subagent(SubagentBlock::completed(
            "task",
            child_sid,
            std::time::Duration::from_secs(3),
        ))
    }

    fn label(entries: &[ScrollbackEntry]) -> VerbGroupHeaderLabel {
        let refs: Vec<&ScrollbackEntry> = entries.iter().collect();
        verb_group_header_label(
            &refs,
            0,
            refs.len(),
            /*show_thinking=*/ true,
            &Theme::current(),
        )
    }

    #[test]
    fn buckets_in_first_appearance_order_with_plurality() {
        let entries = vec![
            read("a.rs"),
            entry(ToolCallBlock::Search(SearchToolCallBlock::new("todo"))),
            read("b.rs"),
            entry(ToolCallBlock::ListDir(ListDirToolCallBlock::new("src"))),
        ];
        let l = label(&entries);
        assert_eq!(l.text, "Explored 2 files, 1 search, 1 dir");
        assert!(!l.running);
        assert!(!l.failed);
    }

    #[test]
    fn skill_reads_bucket_separately_from_files() {
        let entries = vec![
            read("a.rs"),
            read("/x/skills/deploy/SKILL.md"),
            read("b.rs"),
        ];
        let l = label(&entries);
        assert_eq!(l.text, "Explored 2 files, 1 skill");
    }

    #[test]
    fn failed_members_append_suffix_and_flag() {
        let entries = vec![
            read("a.rs"),
            entry(ToolCallBlock::Read(
                ReadToolCallBlock::new("gone.rs").with_error("no such file"),
            )),
            entry(ToolCallBlock::Read(
                ReadToolCallBlock::new("also-gone.rs").with_error("no such file"),
            )),
        ];
        let l = label(&entries);
        assert_eq!(l.text, "Explored 3 files · 2 failed");
        assert!(l.failed);
    }

    #[test]
    fn running_flips_tense_only() {
        let mut entries = vec![
            read("a.rs"),
            entry(ToolCallBlock::Search(SearchToolCallBlock::new("todo"))),
        ];
        entries[1].is_running = true;
        let l = label(&entries);
        assert_eq!(l.text, "Exploring 1 file, 1 search");
        assert!(l.running);

        entries[1].is_running = false;
        let l = label(&entries);
        assert_eq!(l.text, "Explored 1 file, 1 search");
        assert!(!l.running);
    }

    #[test]
    fn web_search_counts_distinct_sources_with_call_fallback() {
        let searched = |query: &str, citations: &[&str]| {
            let mut b = WebSearchToolCallBlock::new(query);
            b.citations = citations.iter().map(|s| s.to_string()).collect();
            b.content = Some("results".into());
            entry(ToolCallBlock::WebSearch(b))
        };
        // Three distinct URLs across two searches, one duplicated.
        let entries = vec![
            searched("grok", &["https://a.com", "https://b.com"]),
            searched("pager", &["https://b.com", "https://c.com"]),
        ];
        let l = label(&entries);
        assert_eq!(l.text, "Searched 3 websites");

        // No citations yet (still running or no results): fall back to call count
        let entries = vec![
            entry(ToolCallBlock::WebSearch(WebSearchToolCallBlock::new("a"))),
            entry(ToolCallBlock::WebSearch(WebSearchToolCallBlock::new("b"))),
        ];
        let l = label(&entries);
        assert_eq!(l.text, "Searched 2 websites");
    }

    #[test]
    fn run_ends_at_separator_and_skips_hidden_thinking() {
        let entries = [
            read("a.rs"),
            ScrollbackEntry::new(RenderBlock::thinking("hmm")),
            read("b.rs"),
            ScrollbackEntry::new(RenderBlock::execute("ls")),
            read("c.rs"),
        ];
        let refs: Vec<&ScrollbackEntry> = entries.iter().collect();
        let l = verb_group_header_label(
            &refs,
            0,
            refs.len(),
            /*show_thinking=*/ false,
            &Theme::current(),
        );
        assert_eq!(l.text, "Explored 2 files");
    }

    #[test]
    fn label_stays_tools_only_across_shown_thinking_states() {
        let mut streaming = ScrollbackEntry::new(RenderBlock::thinking("live"));
        streaming.is_running = true;
        let entries = [
            read("a.rs"),
            // Finished and collapsed: folds into the run, never labeled
            ScrollbackEntry::new(RenderBlock::thinking("done"))
                .with_display_mode(DisplayMode::Collapsed),
            read("b.rs"),
            // Streaming: transparent, keeps its own live panel.
            streaming,
            read("c.rs"),
            // User-opened: transparent, keeps its own rows.
            ScrollbackEntry::new(RenderBlock::thinking("opened"))
                .with_display_mode(DisplayMode::Expanded),
            read("d.rs"),
            // Non-thinking separators still end the run.
            ScrollbackEntry::new(RenderBlock::execute("ls")),
            read("e.rs"),
        ];
        let refs: Vec<&ScrollbackEntry> = entries.iter().collect();
        let l = verb_group_header_label(
            &refs,
            0,
            refs.len(),
            /*show_thinking=*/ true,
            &Theme::current(),
        );
        assert_eq!(l.text, "Explored 4 files");
    }

    fn execute() -> ScrollbackEntry {
        ScrollbackEntry::new(RenderBlock::execute("ls")).with_display_mode(DisplayMode::Collapsed)
    }

    fn thought() -> ScrollbackEntry {
        ScrollbackEntry::new(RenderBlock::thinking("hmm")).with_display_mode(DisplayMode::Collapsed)
    }

    fn trunc_label(
        entries: &[ScrollbackEntry],
        limit: Option<usize>,
    ) -> Option<VerbGroupHeaderLabel> {
        let refs: Vec<&ScrollbackEntry> = entries.iter().collect();
        truncation_header_label(
            &refs,
            0..refs.len(),
            limit,
            /*show_thinking=*/ true,
            &Theme::current(),
        )
    }

    #[test]
    fn truncation_label_buckets_commands_and_never_thoughts() {
        // Three commands and two thoughts hidden: thoughts occupy participant slots but the label stays tools-only
        let entries = vec![execute(), thought(), execute(), thought(), execute()];
        let l = trunc_label(&entries, None).expect("commands bucket");
        assert_eq!(l.text, "Ran 3 commands");
    }

    #[test]
    fn truncation_label_limit_counts_participants_not_buckets() {
        // limit=3 covers [execute, thought, execute]: the thought consumes a participant slot without appearing in the label
        let entries = vec![execute(), thought(), execute(), execute(), execute()];
        let l = trunc_label(&entries, Some(3)).expect("prefix buckets");
        assert_eq!(l.text, "Ran 2 commands");
    }

    #[test]
    fn truncation_label_mixes_kinds_in_first_appearance_order() {
        let entries = vec![
            execute(),
            read("a.rs"),
            ScrollbackEntry::new(RenderBlock::edit("src/main.rs", None))
                .with_display_mode(DisplayMode::Collapsed),
            execute(),
        ];
        let l = trunc_label(&entries, None).expect("buckets");
        assert_eq!(l.text, "Ran 2 commands, explored 1 file, edited 1 file");
    }

    #[test]
    fn truncation_label_none_for_pure_thought_prefix() {
        let entries = vec![thought(), thought(), execute()];
        assert!(
            trunc_label(&entries, Some(2)).is_none(),
            "a prefix of only thoughts buckets nothing; caller falls back to 'N more'"
        );
    }

    #[test]
    fn truncation_label_none_for_prefix_with_unbucketable_rows() {
        let system = ScrollbackEntry::new(RenderBlock::system("hook ran"))
            .with_display_mode(DisplayMode::Collapsed);
        let entries = vec![execute(), system, execute()];
        assert!(
            trunc_label(&entries, None).is_none(),
            "a hidden System row has no bucket; the plain count stays numerically honest"
        );
        // The unbucketable row past the limit never walks: the prefix still gets a label
        let entries = vec![
            execute(),
            ScrollbackEntry::new(RenderBlock::system("hook ran"))
                .with_display_mode(DisplayMode::Collapsed),
        ];
        let l = trunc_label(&entries, Some(1)).expect("prefix buckets");
        assert_eq!(l.text, "Ran 1 command");
    }

    #[test]
    fn truncation_label_counts_failed_commands() {
        let failed = RenderBlock::execute_with_output("false", "", Some("exit 1"));
        let entries = vec![
            execute(),
            ScrollbackEntry::new(failed).with_display_mode(DisplayMode::Collapsed),
        ];
        let l = trunc_label(&entries, None).expect("buckets");
        assert_eq!(l.text, "Ran 2 commands · 1 failed");
        assert!(l.failed);
    }

    #[test]
    fn truncation_label_skips_hidden_thinking_without_consuming_limit() {
        let mut hidden_thought = ScrollbackEntry::new(RenderBlock::thinking("hidden"));
        hidden_thought.set_display_mode(DisplayMode::Collapsed);
        let entries = [execute(), hidden_thought, execute()];
        let refs: Vec<&ScrollbackEntry> = entries.iter().collect();
        // show_thinking=false: the thought is hidden chrome, not a participant, so both commands fit in a limit of 2
        let l = truncation_header_label(&refs, 0..refs.len(), Some(2), false, &Theme::current())
            .expect("buckets");
        assert_eq!(l.text, "Ran 2 commands");
    }

    #[test]
    fn subagent_rows_bucket_with_tools_and_count_distinct_subagents() {
        // A background subagent leaves both its started row and a terminal row in the run
        // The child-session-id source override collapses them to one displayed subagent
        let entries = vec![
            read("a.rs"),
            sub_started("child-A"),
            read("b.rs"),
            sub_completed("child-A"),
        ];
        let l = label(&entries);
        assert_eq!(l.text, "Explored 2 files, ran 1 subagent");
        assert!(!l.failed);
    }

    #[test]
    fn subagent_completion_burst_counts_each_subagent() {
        let entries = vec![sub_completed("child-A"), sub_completed("child-B")];
        let l = label(&entries);
        assert_eq!(l.text, "Ran 2 subagents");
    }

    #[test]
    fn subagent_failed_feeds_suffix_cancelled_does_not() {
        let entries = vec![
            subagent(SubagentBlock::failed(
                "task",
                "child-A",
                std::time::Duration::from_secs(3),
                Some("boom".into()),
            )),
            subagent(SubagentBlock::cancelled(
                "task",
                "child-B",
                std::time::Duration::from_secs(3),
            )),
        ];
        let l = label(&entries);
        assert_eq!(l.text, "Ran 2 subagents · 1 failed");
        assert!(l.failed);
    }

    #[test]
    fn running_subagent_flips_group_tense() {
        let mut entries = vec![read("a.rs"), sub_started("child-A")];
        entries[1].is_running = true;
        let l = label(&entries);
        assert_eq!(l.text, "Exploring 1 file, running 1 subagent");
        assert!(l.running);
    }
}
