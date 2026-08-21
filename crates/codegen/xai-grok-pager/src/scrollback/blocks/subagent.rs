//! SubagentBlock — scrollback entries for subagent lifecycle.
//!
//! Similar to BgTaskBlock: always collapsed, animated bullet while running,
//! colored bullet when done. Enter / Ctrl-F opens the subagent view.
//!
//! Two modes:
//! - **Blocking** (sync): Single `Started` block. Blinks while running,
//!   turns green/red when done. Text: `Subagent "description"`
//! - **Background** (async): `Started` block stays forever (turns gray).
//!   A separate `Completed`/`Failed` block is added when done.
//!   Started text: `Subagent started: "description"`
//!   Completed text: `Subagent completed in 43s: "description"`

use std::time::Duration;

use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::app::subagent::format_subagent_meta;
use crate::appearance::AppearanceConfig;
use crate::render::color::blend_color;
use crate::render::line_utils::truncate_str;
use crate::scrollback::block::BlockContent;
use crate::scrollback::types::{AccentStyle, BlockContext, BlockOutput, DisplayMode};
use crate::theme::Theme;
use crate::util::format_duration;

/// What kind of subagent lifecycle event this block represents.
#[derive(Debug, Clone)]
pub enum SubagentBlockKind {
    /// Subagent is running (or was running — `finish_running` stops animation).
    Started,
    /// Subagent completed successfully.
    Completed { elapsed: Duration },
    /// Subagent failed.
    Failed {
        elapsed: Duration,
        error: Option<String>,
    },
    /// Subagent was cancelled.
    Cancelled { elapsed: Duration },
}

/// Subagent scrollback block.
///
/// Always collapsed, not foldable, groupable, selectable.
/// Enter / Ctrl-F opens the subagent view.
#[derive(Debug, Clone)]
pub struct SubagentBlock {
    /// Human-readable description of the task.
    pub description: String,
    /// Child session ID (for opening the subagent view).
    pub child_session_id: String,
    /// Subagent type (e.g. "general-purpose", "explore").
    pub subagent_type: String,
    /// Named persona applied to this subagent, if any.
    pub persona: Option<String>,
    /// Role that supplied defaults for this subagent, if any.
    pub role: Option<String>,
    /// Effective model ID used by the subagent, if available.
    pub model: Option<String>,
    /// Whether the subagent was launched in background mode.
    pub is_background: bool,
    /// Lifecycle kind.
    pub kind: SubagentBlockKind,
    /// Live activity label from the child session's turn tracker.
    ///
    /// Updated on each `SubagentProgress` tick while the subagent is running.
    /// Shown inline in the collapsed scrollback line (e.g. "Thinking",
    /// "Running: cargo build") so the user sees interactive progress without
    /// opening the subagent view.
    pub activity_label: Option<String>,
    /// Number of completed child turns, updated on each `SubagentProgress`
    /// tick and stamped at `SubagentFinished`.
    pub turn_count: Option<u32>,
    /// Cumulative tokens used by the child session, updated on each
    /// `SubagentProgress` tick and stamped at `SubagentFinished`.
    pub tokens_used: Option<u64>,
}

impl SubagentBlock {
    /// Create a "Subagent started" block (for both sync and async).
    pub fn started(
        description: impl Into<String>,
        child_session_id: impl Into<String>,
        subagent_type: impl Into<String>,
        persona: Option<String>,
        role: Option<String>,
        model: Option<String>,
        is_background: bool,
    ) -> Self {
        Self {
            description: description.into(),
            child_session_id: child_session_id.into(),
            subagent_type: subagent_type.into(),
            persona,
            role,
            model,
            is_background,
            kind: SubagentBlockKind::Started,
            activity_label: None,
            turn_count: None,
            tokens_used: None,
        }
    }

    /// Create a "Subagent completed" block (background mode only).
    pub fn completed(
        description: impl Into<String>,
        child_session_id: impl Into<String>,
        elapsed: Duration,
    ) -> Self {
        Self {
            description: description.into(),
            child_session_id: child_session_id.into(),
            subagent_type: String::new(),
            persona: None,
            role: None,
            model: None,
            is_background: true,
            kind: SubagentBlockKind::Completed { elapsed },
            activity_label: None,
            turn_count: None,
            tokens_used: None,
        }
    }

    /// Create a "Subagent failed" block (background mode only).
    pub fn failed(
        description: impl Into<String>,
        child_session_id: impl Into<String>,
        elapsed: Duration,
        error: Option<String>,
    ) -> Self {
        Self {
            description: description.into(),
            child_session_id: child_session_id.into(),
            subagent_type: String::new(),
            persona: None,
            role: None,
            model: None,
            is_background: true,
            kind: SubagentBlockKind::Failed { elapsed, error },
            activity_label: None,
            turn_count: None,
            tokens_used: None,
        }
    }

    /// Create a "Subagent cancelled" block (background mode only).
    pub fn cancelled(
        description: impl Into<String>,
        child_session_id: impl Into<String>,
        elapsed: Duration,
    ) -> Self {
        Self {
            description: description.into(),
            child_session_id: child_session_id.into(),
            subagent_type: String::new(),
            persona: None,
            role: None,
            model: None,
            is_background: true,
            kind: SubagentBlockKind::Cancelled { elapsed },
            activity_label: None,
            turn_count: None,
            tokens_used: None,
        }
    }

    pub fn is_running(&self) -> bool {
        matches!(self.kind, SubagentBlockKind::Started)
    }
}

/// Truncate description and wrap in quotes for display.
fn quoted_desc(desc: &str, max_width: usize) -> String {
    // Reserve 2 chars for quotes
    if max_width <= 2 {
        return "\u{201C}\u{2026}\u{201D}".to_string(); // "…"
    }
    let inner = truncate_str(desc, max_width - 2);
    format!("\u{201C}{inner}\u{201D}")
}

/// Compact token count for inline display: `1.2k`, `12k`, `130k`, `1.2M`.
fn compact_tokens(n: u64) -> String {
    if n < 1_000 {
        return n.to_string();
    }
    if n < 1_000_000 {
        let k = n as f64 / 1_000.0;
        if k < 10.0 {
            return format!("{k:.1}k");
        }
        return format!("{:.0}k", k);
    }
    let m = n as f64 / 1_000_000.0;
    if m < 10.0 {
        return format!("{m:.1}M");
    }
    format!("{:.0}M", m)
}

/// Build the `— N turns · M tokens` suffix shown after the description.
///
/// Returns an empty string when neither field is set, so callers can always
/// append it. Uses `\u{2014}` (em dash) and `\u{00b7}` (middle dot) to match
/// the existing meta separator style.
fn format_stats(turn_count: Option<u32>, tokens_used: Option<u64>) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(turns) = turn_count.filter(|&t| t > 0) {
        parts.push(format!("{turns} turn{}", if turns == 1 { "" } else { "s" }));
    }
    if let Some(tokens) = tokens_used.filter(|&t| t > 0) {
        parts.push(format!("{} tokens", compact_tokens(tokens)));
    }
    if parts.is_empty() {
        return String::new();
    }
    format!(" \u{2014} {}", parts.join(" \u{00b7} "))
}

impl BlockContent for SubagentBlock {
    fn output(&self, ctx: &BlockContext) -> BlockOutput {
        let theme = Theme::current();
        // When selected, lift only the bold "Subagent" label to
        // `text_primary` so it reads as undimmed (mirrors `read.rs` /
        // `search.rs`, which bump only the label and leave the rest at
        // `muted`). The detail text (verb + description + meta) stays
        // muted in every state.
        let bold = if ctx.is_selected {
            theme.primary().add_modifier(Modifier::BOLD)
        } else {
            theme.muted().add_modifier(Modifier::BOLD)
        };
        let muted = theme.muted();
        let w = ctx.width as usize;

        let line = match (&self.kind, self.is_background) {
            (SubagentBlockKind::Started, bg) => {
                let verb = if bg { "started: " } else { "running: " };
                let activity_suffix: String = self
                    .activity_label
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .map(|a| format!(" \u{2014} {a}"))
                    .unwrap_or_default();
                let stats = format_stats(self.turn_count, self.tokens_used);
                let meta = format_subagent_meta(
                    self.persona.as_deref(),
                    self.role.as_deref(),
                    self.model.as_deref(),
                );
                // "Subagent running: " / "Subagent started: " = 18 chars
                let overhead = 18 + meta.width() + activity_suffix.width() + stats.width();
                let desc = quoted_desc(&self.description, w.saturating_sub(overhead));
                let mut spans = vec![
                    Span::styled("Subagent ", bold),
                    Span::styled(verb, muted),
                    Span::styled(desc, muted),
                ];
                if !activity_suffix.is_empty() {
                    spans.push(Span::styled(activity_suffix, muted));
                }
                if !stats.is_empty() {
                    spans.push(Span::styled(stats, muted));
                }
                spans.push(Span::styled(meta, muted));
                Line::from(spans)
            }
            // Completed: Subagent completed in Xs: "description" — N turns · M tokens
            (SubagentBlockKind::Completed { elapsed }, _) => {
                let time_str = format_duration(*elapsed);
                let stats = format_stats(self.turn_count, self.tokens_used);
                // "Subagent completed in Xs: " = 26 + time_str.len()
                let prefix_len = 26 + time_str.len() + stats.len();
                let desc = quoted_desc(&self.description, w.saturating_sub(prefix_len));
                let mut spans = vec![
                    Span::styled("Subagent ", bold),
                    Span::styled(format!("completed in {time_str}: "), muted),
                    Span::styled(desc, muted),
                ];
                if !stats.is_empty() {
                    spans.push(Span::styled(stats, muted));
                }
                Line::from(spans)
            }
            // Failed: Subagent failed in Xs: "description" — N turns · M tokens
            (SubagentBlockKind::Failed { elapsed, error }, _) => {
                let time_str = format_duration(*elapsed);
                let detail = error
                    .as_deref()
                    .map(|e| format!(" ({e})"))
                    .unwrap_or_default();
                let stats = format_stats(self.turn_count, self.tokens_used);
                let prefix_len = 21 + time_str.len() + detail.len() + stats.len();
                let desc = quoted_desc(&self.description, w.saturating_sub(prefix_len));
                let mut spans = vec![
                    Span::styled("Subagent ", bold),
                    Span::styled(format!("failed in {time_str}{detail}: "), muted),
                    Span::styled(desc, muted),
                ];
                if !stats.is_empty() {
                    spans.push(Span::styled(stats, muted));
                }
                Line::from(spans)
            }
            // Cancelled: Subagent cancelled in Xs: "description" — N turns · M tokens
            (SubagentBlockKind::Cancelled { elapsed }, _) => {
                let time_str = format_duration(*elapsed);
                let stats = format_stats(self.turn_count, self.tokens_used);
                // "Subagent cancelled in Xs: " = 26 + time_str.len()
                let prefix_len = 26 + time_str.len() + stats.len();
                let desc = quoted_desc(&self.description, w.saturating_sub(prefix_len));
                let mut spans = vec![
                    Span::styled("Subagent ", bold),
                    Span::styled(format!("cancelled in {time_str}: "), muted),
                    Span::styled(desc, muted),
                ];
                if !stats.is_empty() {
                    spans.push(Span::styled(stats, muted));
                }
                Line::from(spans)
            }
        };

        BlockOutput {
            lines: vec![line.into()],
        }
    }

    fn accent(&self, ctx: &BlockContext) -> Option<AccentStyle> {
        let theme = Theme::current();
        match &self.kind {
            SubagentBlockKind::Started if ctx.is_running => {
                Some(AccentStyle::static_color(theme.accent_running))
            }
            _ => None,
        }
    }

    fn bullet(&self, ctx: &BlockContext) -> Option<AccentStyle> {
        let theme = Theme::current();
        match &self.kind {
            SubagentBlockKind::Started => {
                if ctx.is_running {
                    let dim = ctx.appearance.scrollback.display.dim_accent;
                    let dimmed = blend_color(theme.bg_base, theme.accent_running, dim)
                        .unwrap_or(theme.accent_running);
                    Some(AccentStyle::animated(dimmed))
                } else {
                    // Finished — gray bullet (same as bg task "started" after completion)
                    None
                }
            }
            SubagentBlockKind::Completed { .. } => {
                Some(AccentStyle::static_color(theme.accent_success))
            }
            SubagentBlockKind::Failed { .. } | SubagentBlockKind::Cancelled { .. } => {
                Some(AccentStyle::static_color(theme.accent_error))
            }
        }
    }

    fn has_vpad_for(&self, _appearance: &AppearanceConfig) -> bool {
        false
    }

    fn has_raw_mode(&self) -> bool {
        false
    }

    fn is_foldable(&self) -> bool {
        false
    }

    fn default_display_mode(&self) -> DisplayMode {
        DisplayMode::Collapsed
    }

    fn is_selectable(&self) -> bool {
        true
    }

    fn has_bullet(&self, _ctx: &BlockContext) -> bool {
        true
    }

    fn is_groupable(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::appearance::AppearanceConfig;

    fn test_ctx() -> BlockContext {
        BlockContext {
            mode: DisplayMode::Collapsed,
            is_running: false,
            width: 120,
            raw: false,
            max_lines: None,
            appearance: AppearanceConfig::default(),
            is_selected: false,
            cwd: None,
        }
    }

    fn line_text(block: &SubagentBlock) -> String {
        block.output(&test_ctx()).lines[0]
            .content
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect()
    }

    #[test]
    fn compact_tokens_scales_units() {
        assert_eq!(compact_tokens(0), "0");
        assert_eq!(compact_tokens(42), "42");
        assert_eq!(compact_tokens(999), "999");
        assert_eq!(compact_tokens(1_234), "1.2k");
        assert_eq!(compact_tokens(12_345), "12k");
        assert_eq!(compact_tokens(129_540), "130k");
        assert_eq!(compact_tokens(1_234_567), "1.2M");
        assert_eq!(compact_tokens(12_345_678), "12M");
    }

    #[test]
    fn format_stats_combines_turns_and_tokens() {
        // Neither field set - no suffix.
        assert_eq!(format_stats(None, None), "");
        // Zero values are treated as absent (matches Finished handler which
        // uses `(turns > 0).then_some(turns)`).
        assert_eq!(format_stats(Some(0), Some(0)), "");
        assert_eq!(format_stats(Some(0), Some(1_500)), " \u{2014} 1.5k tokens");
        assert_eq!(format_stats(Some(1), None), " \u{2014} 1 turn");
        assert_eq!(format_stats(Some(2), None), " \u{2014} 2 turns");
        assert_eq!(
            format_stats(Some(3), Some(4_500)),
            " \u{2014} 3 turns \u{00b7} 4.5k tokens"
        );
    }

    #[test]
    fn started_line_renders_stats_after_activity() {
        let mut block = SubagentBlock::started(
            "research the parser",
            "child-1",
            "general-purpose",
            None,
            None,
            None,
            false,
        );
        block.activity_label = Some("Thinking".into());
        block.turn_count = Some(2);
        block.tokens_used = Some(4_500);

        let text = line_text(&block);
        assert!(
            text.contains("Subagent running: “research the parser”"),
            "missing prefix, got: {text}"
        );
        assert!(
            text.contains("Thinking"),
            "missing activity label, got: {text}"
        );
        assert!(
            text.contains("2 turns \u{00b7} 4.5k tokens"),
            "missing stats suffix, got: {text}"
        );
    }

    #[test]
    fn completed_line_renders_stats_without_activity() {
        let mut block =
            SubagentBlock::completed("do the thing", "child-2", Duration::from_secs(43));
        block.turn_count = Some(7);
        block.tokens_used = Some(129_540);

        let text = line_text(&block);
        assert!(
            text.contains("completed in 43s"),
            "missing elapsed, got: {text}"
        );
        assert!(
            text.contains("7 turns \u{00b7} 130k tokens"),
            "missing stats suffix, got: {text}"
        );
    }
}
