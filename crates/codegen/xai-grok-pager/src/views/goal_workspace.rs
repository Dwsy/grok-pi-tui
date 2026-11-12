//! Spacious native Goal workspace. Only consumes GoalUpdated state and the Pager TodoPane;
//! it neither drives the agent nor invents progress. The live timeline is bounded and
//! retained when the workspace closes/reopens within the same AgentView.

use std::collections::VecDeque;

use chrono::{DateTime, Local, Utc};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Widget},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
use xai_grok_shell::tools::{TodoItem, TodoStatus};

use crate::{
    app::agent::{GoalDisplayState, GoalDisplayStatus},
    render::SafeBuf,
    theme::Theme,
    views::agent_status::{active_phase_label, format_tokens_compact},
};

const MAX_GOAL_EVENTS: usize = 256;
const MAX_OVERVIEW_EVENTS: usize = 6;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum GoalWorkspaceTab {
    #[default]
    Overview,
    Timeline,
    Diagnostics,
}

impl GoalWorkspaceTab {
    pub fn next(self) -> Self {
        match self {
            Self::Overview => Self::Timeline,
            Self::Timeline => Self::Diagnostics,
            Self::Diagnostics => Self::Overview,
        }
    }

    pub fn previous(self) -> Self {
        match self {
            Self::Overview => Self::Diagnostics,
            Self::Timeline => Self::Overview,
            Self::Diagnostics => Self::Timeline,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalTimelineEvent {
    pub kind: String,
    pub detail: Option<String>,
    /// Origin timestamp if supplied; otherwise the time the Pager received the update.
    pub timestamp: String,
    source_timestamp: Option<String>,
}

/// UI-only state: the GoalHost/extension remains the source of truth for goal status.
#[derive(Debug, Clone, Default)]
pub struct GoalWorkspaceState {
    goal_id: Option<String>,
    pub tab: GoalWorkspaceTab,
    pub scroll: usize,
    pub max_scroll: usize,
    pub tab_hits: [Rect; 3],
    events: VecDeque<GoalTimelineEvent>,
}

impl GoalWorkspaceState {
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn events(&self) -> &VecDeque<GoalTimelineEvent> {
        &self.events
    }

    pub fn select(&mut self, tab: GoalWorkspaceTab) {
        if self.tab != tab {
            self.tab = tab;
            self.scroll = 0;
            self.max_scroll = 0;
        }
    }

    pub fn scroll_down(&mut self, lines: usize) {
        self.scroll = self.scroll.saturating_add(lines).min(self.max_scroll);
    }

    pub fn scroll_up(&mut self, lines: usize) {
        self.scroll = self.scroll.saturating_sub(lines);
    }

    pub fn record_update(&mut self, goal: &GoalDisplayState) {
        if self.goal_id.as_deref() != Some(goal.goal_id.as_str()) {
            self.goal_id = Some(goal.goal_id.clone());
            self.events.clear();
            self.scroll = 0;
            self.max_scroll = 0;
        }
        let Some(kind) = goal.last_event.as_deref() else {
            return;
        };
        let detail = goal.last_event_detail.as_deref();
        let source_timestamp = goal.last_event_timestamp.as_deref();
        // A GoalUpdated refresh is not necessarily a new event. Do not turn
        // periodic token/elapsed ticks into duplicate timeline records.
        if self.events.front().is_some_and(|previous| {
            previous.kind == kind
                && previous.detail.as_deref() == detail
                && previous.source_timestamp.as_deref() == source_timestamp
        }) {
            return;
        }
        self.events.push_front(GoalTimelineEvent {
            kind: kind.to_owned(),
            detail: detail.map(str::to_owned),
            timestamp: source_timestamp
                .map(str::to_owned)
                .unwrap_or_else(|| Utc::now().to_rfc3339()),
            source_timestamp: source_timestamp.map(str::to_owned),
        });
        self.events.truncate(MAX_GOAL_EVENTS);
    }
}

fn safe(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .trim()
        .to_owned()
}

/// Break by terminal display columns, not UTF-8 bytes. Long paths and Chinese
/// objectives wrap rather than disappearing into a single-line border title.
fn wrap_columns(value: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut result = Vec::new();
    let mut row = String::new();
    let mut used = 0usize;
    for ch in safe(value).chars() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + cw > width && !row.is_empty() {
            result.push(row.trim_end().to_owned());
            row.clear();
            used = 0;
        }
        row.push(ch);
        used += cw;
    }
    if !row.is_empty() {
        result.push(row.trim_end().to_owned());
    }
    if result.is_empty() {
        result.push(String::new());
    }
    result
}

fn title_or_default(goal: &GoalDisplayState) -> String {
    let title = safe(&goal.objective);
    if title.is_empty() {
        "Untitled goal".into()
    } else {
        title
    }
}

fn line(text: impl Into<String>, style: Style) -> Line<'static> {
    Line::from(Span::styled(text.into(), style))
}

fn heading(lines: &mut Vec<Line<'static>>, name: &str, theme: &Theme) {
    if !lines.is_empty() {
        lines.push(Line::default());
    }
    lines.push(line(
        format!("  {name}"),
        Style::default().fg(theme.accent_plan).add_modifier(Modifier::BOLD),
    ));
}

fn wrapped(lines: &mut Vec<Line<'static>>, text: &str, width: usize, style: Style) {
    for segment in wrap_columns(text, width.saturating_sub(2)) {
        lines.push(line(format!("  {segment}"), style));
    }
}

fn event_label(kind: &str) -> &'static str {
    match kind {
        "goal_created" => "Goal created",
        "goal_replaced" => "Goal replaced",
        "goal_resumed" => "Goal resumed",
        "goal_paused" => "Goal paused",
        "goal_completed" => "Goal completed",
        "goal_cleared" => "Goal cleared",
        "planning_started" => "Planning started",
        "planning_completed" => "Planning completed",
        "planning_failed" => "Planning failed",
        "worker_started" => "Worker started",
        "worker_completed" => "Worker completed",
        "worker_failed" => "Worker failed",
        "verify_started" => "Verification started",
        "verify_completed" => "Verification completed",
        "context_rotated" => "Context rotated",
        "budget_exceeded" => "Budget exceeded",
        "premature_stop_detected" => "Early stop detected",
        "progress" => "Progress update",
        _ => "Goal event",
    }
}

fn event_time(timestamp: &str) -> String {
    DateTime::parse_from_rfc3339(timestamp)
        .map(|dt| dt.with_timezone(&Local).format("%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|_| safe(timestamp))
}

fn append_event(lines: &mut Vec<Line<'static>>, event: &GoalTimelineEvent, width: usize, theme: &Theme) {
    let clock = event_time(&event.timestamp);
    let label = event_label(&event.kind);
    let label = if label == "Goal event" {
        safe(&event.kind).replace('_', " ")
    } else {
        label.into()
    };
    lines.push(Line::from(vec![
        Span::styled(format!("  {clock}  "), Style::default().fg(theme.gray)),
        Span::styled(label, Style::default().fg(theme.text_primary).add_modifier(Modifier::BOLD)),
    ]));
    if let Some(detail) = event.detail.as_deref().filter(|s| !s.trim().is_empty()) {
        wrapped(lines, &format!("    {}", detail), width, Style::default().fg(theme.text_secondary));
    }
}

fn content_lines(
    goal: &GoalDisplayState,
    todos: &[TodoItem],
    state: &GoalWorkspaceState,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let normal = Style::default().fg(theme.text_secondary);
    let muted = Style::default().fg(theme.gray);
    match state.tab {
        GoalWorkspaceTab::Overview => {
            heading(&mut lines, "PROGRESS  /  CHECKLIST", theme);
            if todos.is_empty() {
                wrapped(&mut lines, "No progress items yet. Checklist comes from the native TodoPane.", width, muted);
            } else {
                let completed = todos.iter().filter(|item| item.status == TodoStatus::Completed).count();
                wrapped(&mut lines, &format!("{completed} / {} completed", todos.len()), width, normal);
                for item in todos.iter().take(64) {
                    let (icon, color) = match item.status {
                        TodoStatus::Completed => ("✓", theme.accent_success),
                        TodoStatus::InProgress => ("▶", theme.warning),
                        TodoStatus::Cancelled => ("×", theme.accent_error),
                        TodoStatus::Pending => ("○", theme.gray),
                    };
                    let wrapped_item = wrap_columns(&item.content, width.saturating_sub(6));
                    for (idx, segment) in wrapped_item.iter().enumerate() {
                        lines.push(Line::from(vec![
                            Span::styled(if idx == 0 { format!("  {icon} ") } else { "    ".into() }, Style::default().fg(color)),
                            Span::styled(segment.clone(), normal),
                        ]));
                    }
                }
                if todos.len() > 64 {
                    wrapped(&mut lines, &format!("+{} more checklist items", todos.len() - 64), width, muted);
                }
            }
            heading(&mut lines, "RECENT ACTIVITY  /  LIVE", theme);
            if state.events.is_empty() {
                wrapped(&mut lines, "No GoalUpdated events received for this goal yet.", width, muted);
            } else {
                for event in state.events.iter().take(MAX_OVERVIEW_EVENTS) {
                    append_event(&mut lines, event, width, theme);
                }
                if state.events.len() > MAX_OVERVIEW_EVENTS {
                    wrapped(&mut lines, "Press 2 for the full timeline.", width, muted);
                }
            }
            heading(&mut lines, "EXECUTION", theme);
            wrapped(
                &mut lines,
                &format!(
                    "Worker rounds: {}    Verification rounds: {}    Current role: {}",
                    goal.total_worker_rounds,
                    goal.total_verify_rounds,
                    goal.current_subagent_role.as_deref().unwrap_or("none"),
                ),
                width,
                normal,
            );
            if let Some(title) = goal.current_deliverable_title.as_deref() {
                wrapped(&mut lines, &format!("Current deliverable: {title}"), width, normal);
            }
            if goal.status.is_paused() || matches!(goal.status, GoalDisplayStatus::Failed | GoalDisplayStatus::Interrupted) {
                if let Some(reason) = goal.pause_message.as_deref() {
                    heading(&mut lines, "ATTENTION", theme);
                    wrapped(&mut lines, reason, width, Style::default().fg(theme.warning));
                }
            }
        }
        GoalWorkspaceTab::Timeline => {
            heading(&mut lines, &format!("EVENT TIMELINE  /  {} captured", state.events.len()), theme);
            if state.events.is_empty() {
                wrapped(&mut lines, "No events have arrived yet. This view updates as GoalUpdated notifications are received.", width, muted);
            } else {
                wrapped(&mut lines, "Newest first. Timestamps are local time. Repeated state refreshes are suppressed.", width, muted);
                lines.push(Line::default());
                for (i, event) in state.events.iter().enumerate() {
                    append_event(&mut lines, event, width, theme);
                    if i + 1 < state.events.len() {
                        lines.push(Line::default());
                    }
                }
            }
        }
        GoalWorkspaceTab::Diagnostics => {
            heading(&mut lines, "GOAL IDENTITY", theme);
            wrapped(&mut lines, &format!("Goal ID: {}", goal.goal_id), width, normal);
            wrapped(&mut lines, &format!("Status: {:?}  |  Phase: {:?}", goal.status, goal.phase), width, normal);
            heading(&mut lines, "FULL OBJECTIVE", theme);
            wrapped(&mut lines, &goal.objective, width, normal);
            heading(&mut lines, "TOKEN ACCOUNTING", theme);
            wrapped(&mut lines, &format!("Recorded: {} tokens    Baseline: {}", goal.tokens_used, goal.token_baseline), width, normal);
            wrapped(&mut lines, &format!("Budget: {}    Finished subagent: {} tokens", goal.token_budget.map_or("not set".into(), |v| v.to_string()), goal.finished_subagent_tokens), width, normal);
            heading(&mut lines, "SUBAGENT / CONTEXT", theme);
            wrapped(&mut lines, &format!("Role: {}    Tokens: {}    Context: {}",
                goal.current_subagent_role.as_deref().unwrap_or("none"),
                goal.live_subagent_tokens.map_or("-".into(), |v| v.to_string()),
                goal.live_context_pct.map_or("-".into(), |v| format!("{v}%"))
            ), width, normal);
            wrapped(&mut lines, &format!("Turns: {}    Tool calls: {}",
                goal.live_turn_count.map_or("-".into(), |v| v.to_string()),
                goal.live_tool_call_count.map_or("-".into(), |v| v.to_string())
            ), width, normal);
            for (model, tokens) in goal.live_tokens_by_model.iter().take(12) {
                wrapped(&mut lines, &format!("{}: {} tokens", model, tokens), width, normal);
            }
            heading(&mut lines, "COMPLETION REVIEW", theme);
            wrapped(&mut lines, &format!(
                "Verifier attempts: {} / {}    In flight: {}",
                goal.classifier_runs_attempted.map_or("-".into(), |v| v.to_string()),
                goal.classifier_max_runs.map_or("-".into(), |v| v.to_string()),
                goal.verifying_completion
            ), width, normal);
            wrapped(&mut lines, &format!("Last verdict: {:?}", goal.last_classifier_verdict), width, normal);
            if let Some(path) = goal.last_classifier_details_path.as_deref() {
                wrapped(&mut lines, &format!("Evidence: {}{}", path, if goal.last_classifier_details_exists { "" } else { " (unavailable)" }), width, muted);
            }
            heading(&mut lines, "CONTROL", theme);
            wrapped(&mut lines, "/goal status  /goal pause  /goal resume  /goal clear", width, normal);
        }
    }
    lines
}

/// A large, stable workspace instead of a content-height popup. Narrow terminals
/// fall back to the older compact renderer at the call site.
pub fn goal_workspace_area(screen: Rect) -> Rect {
    let mx = if screen.width >= 110 { 3 } else if screen.width >= 64 { 2 } else { 1 };
    let my = if screen.height >= 30 { 2 } else { 1 };
    Rect::new(
        screen.x.saturating_add(mx),
        screen.y.saturating_add(my),
        screen.width.saturating_sub(mx * 2),
        screen.height.saturating_sub(my * 2),
    )
}

fn draw(buf: &mut Buffer, x: u16, y: u16, width: u16, value: Line<'static>) {
    buf.set_line_safe(x, y, &value, width);
}

pub fn render_goal_workspace(
    buf: &mut Buffer,
    area: Rect,
    goal: &GoalDisplayState,
    todos: &[TodoItem],
    state: &mut GoalWorkspaceState,
    context_used: Option<u64>,
    active_subagent_tokens: u64,
    close_hovered: bool,
) -> Option<Rect> {
    if area.width < 44 || area.height < 16 {
        return None;
    }
    let theme = Theme::current();
    let bg = Style::default().bg(theme.bg_base);
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            if let Some(cell) = buf.cell_mut(Position::new(x, y)) {
                cell.reset();
                cell.set_style(bg);
            }
        }
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.gray))
        .style(bg);
    let inner = block.inner(area);
    Widget::render(block, area, buf);
    let x = inner.x + 2;
    let width = inner.width.saturating_sub(4);
    let close = format!("[{}]", crate::glyphs::ballot_x());
    let close_w = UnicodeWidthStr::width(close.as_str()) as u16;
    let close_x = area.x + area.width.saturating_sub(close_w + 2);
    draw(buf, area.x + 2, area.y, close_x.saturating_sub(area.x + 3), line(
        " GOAL  /  WORKSPACE ", Style::default().fg(theme.accent_plan).add_modifier(Modifier::BOLD)
    ));
    draw(buf, close_x, area.y, close_w, line(close, Style::default().fg(
        if close_hovered { theme.text_primary } else { theme.gray }
    )));
    let close_rect = Rect::new(close_x, area.y, close_w, 1);

    let mut y = inner.y;
    let (status, status_color) = match goal.status {
        GoalDisplayStatus::Active => ("ACTIVE", theme.accent_success),
        GoalDisplayStatus::Complete => ("COMPLETE", theme.accent_success),
        GoalDisplayStatus::BudgetLimited => ("BUDGET LIMITED", theme.accent_error),
        GoalDisplayStatus::Failed | GoalDisplayStatus::Interrupted => ("STOPPED", theme.accent_error),
        _ => ("PAUSED", theme.warning),
    };
    draw(buf, x, y, width, Line::from(vec![
        Span::styled(format!(" {status} "), Style::default().fg(status_color).add_modifier(Modifier::BOLD)),
        Span::styled(format!("   {}   ·   elapsed {}", active_phase_label(goal), crate::views::goal_detail::format_elapsed(goal.live_elapsed_ms())), Style::default().fg(theme.text_secondary)),
    ]));
    y += 1;
    draw(buf, x, y, width, line("OBJECTIVE", Style::default().fg(theme.gray).add_modifier(Modifier::BOLD)));
    y += 1;
    // The goal title is real content now, not a one-line border decoration.
    let title_lines = wrap_columns(&title_or_default(goal), width as usize);
    let title_limit = ((inner.height as usize) / 4).clamp(2, 4);
    for title in title_lines.iter().take(title_limit) {
        draw(buf, x, y, width, line(title.clone(), Style::default().fg(theme.text_primary).add_modifier(Modifier::BOLD)));
        y += 1;
    }
    if title_lines.len() > title_limit {
        draw(buf, x, y, width, line("… Full objective in Diagnostics (3)", Style::default().fg(theme.gray)));
        y += 1;
    }

    let tokens = goal.live_tokens_used(context_used, active_subagent_tokens).max(0);
    let tokens_label = format_tokens_compact(tokens);
    let budget = goal.token_budget.filter(|v| *v > 0);
    let progress = budget.map(|b| (tokens as f64 / b as f64).clamp(0.0, 1.0) as f32);
    draw(buf, x, y, width, line(
        match budget {
            Some(b) => format!("TOKENS  {tokens_label} / {}   ·   {:.1}% of budget", format_tokens_compact(b), progress.unwrap_or(0.0) * 100.0),
            None => format!("TOKENS  {tokens_label}   ·   no budget configured"),
        },
        Style::default().fg(theme.text_secondary)
    ));
    y += 1;
    if let Some(pct) = progress {
        let filled = ((width as f32 * pct).round() as usize).min(width as usize);
        draw(buf, x, y, width, Line::from(vec![
            Span::styled("━".repeat(filled), Style::default().fg(if pct > 0.90 { theme.warning } else { theme.accent_plan })),
            Span::styled("─".repeat(width as usize - filled), Style::default().fg(theme.gray_dim)),
        ]));
        y += 1;
    }
    y += 1;
    let tab_titles = if width < 62 {
        [" 1 Info ", " 2 Events ", " 3 Details "]
    } else {
        [" 1 Overview ", " 2 Timeline ", " 3 Diagnostics "]
    };
    let mut tabs = Vec::new();
    let mut tab_x = x;
    for (idx, tab) in [
        GoalWorkspaceTab::Overview,
        GoalWorkspaceTab::Timeline,
        GoalWorkspaceTab::Diagnostics,
    ].into_iter().enumerate() {
        let title = tab_titles[idx];
        let tab_w = UnicodeWidthStr::width(title) as u16;
        state.tab_hits[idx] = Rect::new(tab_x, y, tab_w.min(x.saturating_add(width).saturating_sub(tab_x)), 1);
        let style = if state.tab == tab {
            Style::default().fg(theme.bg_base).bg(theme.accent_plan).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.gray_bright)
        };
        tabs.push(Span::styled(title, style));
        tabs.push(Span::raw("  "));
        tab_x = tab_x.saturating_add(tab_w + 2);
    }
    draw(buf, x, y, width, Line::from(tabs));
    y += 1;
    draw(buf, x, y, width, line("─".repeat(width as usize), Style::default().fg(theme.gray_dim)));
    y += 1;

    let footer_y = inner.y + inner.height.saturating_sub(2);
    let body_height = footer_y.saturating_sub(y) as usize;
    let rows = content_lines(goal, todos, state, width as usize, &theme);
    state.max_scroll = rows.len().saturating_sub(body_height);
    state.scroll = state.scroll.min(state.max_scroll);
    for (i, row) in rows.into_iter().skip(state.scroll).take(body_height).enumerate() {
        draw(buf, x, y + i as u16, width, row);
    }
    draw(buf, x, footer_y, width, line("─".repeat(width as usize), Style::default().fg(theme.gray_dim)));
    let hint = if width < 90 {
        "1/2/3 tabs  ↑↓ scroll  PgUp/PgDn page  Esc close"
    } else {
        "1/2/3 tabs  Tab/Shift+Tab switch  ↑↓/j/k scroll  PgUp/PgDn page  Home/End  Esc/g close"
    };
    draw(buf, x, footer_y + 1, width, line(
        format!("{hint}   [{}/{}]", state.scroll.saturating_add(1).min(state.max_scroll.saturating_add(1)), state.max_scroll.saturating_add(1)),
        Style::default().fg(theme.gray),
    ));
    Some(close_rect)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn goal_title_wraps_by_terminal_columns() {
        assert_eq!(wrap_columns("中文中文中文", 4), vec!["中文", "中文", "中文"]);
        assert_eq!(wrap_columns("abc\u{1b}[31m", 20), vec!["abc [31m"]);
    }

    #[test]
    fn timeline_keeps_distinct_updates_and_ignores_refreshes() {
        let mut state = GoalWorkspaceState::default();
        let mut goal = GoalDisplayState::test_stub();
        goal.last_event = Some("goal_created".into());
        state.record_update(&goal);
        state.record_update(&goal);
        assert_eq!(state.events().len(), 1);
        goal.last_event = Some("progress".into());
        goal.last_event_detail = Some("First check passed".into());
        state.record_update(&goal);
        assert_eq!(state.events().len(), 2);
        assert_eq!(state.events().front().unwrap().detail.as_deref(), Some("First check passed"));
        goal.goal_id = "another".into();
        state.record_update(&goal);
        assert_eq!(state.events().len(), 1);
    }

    #[test]
    fn area_is_spacious_but_clamped() {
        assert_eq!(goal_workspace_area(Rect::new(0, 0, 120, 40)), Rect::new(3, 2, 114, 36));
        assert_eq!(goal_workspace_area(Rect::new(0, 0, 70, 24)), Rect::new(2, 1, 66, 22));
    }

    #[test]
    fn new_workspace_renders_full_multiline_objective_and_timeline() {
        let mut goal = GoalDisplayState::test_stub();
        goal.objective = "完成 Dynocode 严格解释与交付功能，通过全部测试".into();
        goal.last_event = Some("progress".into());
        goal.last_event_detail = Some("verified tests".into());
        let mut state = GoalWorkspaceState::default();
        state.record_update(&goal);
        state.select(GoalWorkspaceTab::Timeline);
        let screen = Rect::new(0, 0, 100, 35);
        let mut buf = Buffer::empty(screen);
        let close = render_goal_workspace(&mut buf, goal_workspace_area(screen), &goal, &[], &mut state, None, 0, false);
        assert!(close.is_some());
        let output = buf.content.iter().map(|cell| cell.symbol()).collect::<String>();
        assert!(output.contains("EVENT TIMELINE"));
        assert!(output.contains("Progress update"));
        assert!(output.contains("verified tests"));
    }
}

