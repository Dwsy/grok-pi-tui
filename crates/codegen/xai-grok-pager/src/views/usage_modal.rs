//! Tabbed usage / session-info modal, opened by `/usage`, `/session-info`,
//! `/context`, and the context-bar click. Minimal mode keeps the scrollback
//! blocks instead; this modal is never armed there.
//!
//! The modal opens with loading placeholders; the task-result handlers fill
//! the slots in as the fetches land.

use crossterm::event::{KeyCode, KeyEvent, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use crate::scrollback::blocks::ContextInfoBlock;
use crate::theme::Theme;
use crate::views::credit_bar::CreditBalance;
use crate::views::modal_window::{
    self as mw, ModalSizing, ModalWindowConfig, ModalWindowState, Shortcut,
};

/// Footer shortcut ID for "copy session ID".
pub const COPY_SESSION_ID_SHORTCUT: usize = 1;

/// Footer shortcut ID for "copy all session info".
pub const COPY_ALL_SESSION_INFO_SHORTCUT: usize = 2;

/// The three tabs, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageInfoTab {
    ContextUsage,
    UsageLimit,
    SessionInfo,
}

impl UsageInfoTab {
    pub const ALL: [UsageInfoTab; 3] = [
        UsageInfoTab::ContextUsage,
        UsageInfoTab::UsageLimit,
        UsageInfoTab::SessionInfo,
    ];

    pub fn label(self) -> &'static str {
        match self {
            UsageInfoTab::ContextUsage => "Context usage",
            UsageInfoTab::UsageLimit => "Usage limit",
            UsageInfoTab::SessionInfo => "Session info",
        }
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }

    pub fn from_index(i: usize) -> Self {
        *Self::ALL.get(i).unwrap_or(&Self::ALL[0])
    }
}

/// Account/session facts captured when the modal opens.
pub struct UsageInfoContext {
    /// Session ID for the copy shortcut (`None` before the session starts).
    pub session_id: Option<String>,
    /// False for team/enterprise accounts, which have no consumer billing.
    pub usage_visible: bool,
    /// True for gateway chat sessions, which have no Build coding credits.
    pub chat_kind: bool,
    /// Remote-settings kill switch: link out instead of showing billing.
    pub billing_redirect_url: Option<String>,
    /// Plan name for the allowance header (e.g. "SuperGrok").
    pub subscription_tier: Option<String>,
}

/// Modal state. Billing figures are NOT stored here — the render reads the
/// agent's cached `credit_balance` mirror, so a silent billing refresh
/// updates the open modal for free.
pub struct UsageInfoModalState {
    pub window: ModalWindowState,
    pub active_tab: UsageInfoTab,
    pub scroll: u16,
    pub ctx: UsageInfoContext,
    pub context: Option<ContextInfoBlock>,
    pub context_error: Option<String>,
    /// Structured `/session-info` rows, built upstream from typed session data
    /// (never by re-parsing a formatted string).
    pub session_fields: Option<Vec<SessionInfoField>>,
    pub session_error: Option<String>,
    /// Pre-formatted session token/cost summary (`session_usage_block_text`).
    pub session_usage_text: Option<String>,
    pub billing_loading: bool,
    pub billing_error: Option<String>,
    /// Fetch generation stamped at open; results from an earlier open (same
    /// session, modal reopened) are dropped instead of overwriting.
    pub fetch_nonce: u64,
    /// Hit rects for every copyable Session-info value row (click-to-copy),
    /// refreshed every render. Empty on the other tabs.
    pub copy_hits: Vec<SessionCopyHit>,
    /// Content-line index of the value row under the cursor, used to paint the
    /// hover highlight. Cleared on tab switch.
    pub hovered_copy_line: Option<usize>,
}

/// One labeled Session-info row, built upstream from typed session data. The
/// modal renders and copies straight from these — it never parses a formatted
/// string. `compact` selects the dense `Label: value` layout for the
/// model/runtime group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInfoField {
    pub label: &'static str,
    pub value: String,
    pub compact: bool,
}

/// A click-to-copy value row in the Session info tab. `rect` is the on-screen
/// hit area (full content width, one row), refreshed every render; `value` is
/// the text copied on click; `line_idx` indexes the tab's rendered lines so the
/// mouse handler and the renderer agree on which row is hovered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCopyHit {
    pub rect: Rect,
    pub value: String,
    pub line_idx: usize,
}

impl UsageInfoModalState {
    pub fn new(tab: UsageInfoTab, ctx: UsageInfoContext) -> Self {
        Self {
            window: ModalWindowState::with_tabs(UsageInfoTab::ALL.len()),
            active_tab: tab,
            scroll: 0,
            ctx,
            context: None,
            context_error: None,
            session_error: None,
            session_usage_text: None,
            billing_loading: false,
            billing_error: None,
            fetch_nonce: 0,
            session_fields: None,
            copy_hits: Vec::new(),
            hovered_copy_line: None,
        }
    }

    pub fn set_tab(&mut self, tab: UsageInfoTab) {
        if self.active_tab != tab {
            self.active_tab = tab;
            self.scroll = 0;
            self.hovered_copy_line = None;
        }
    }

    /// The full Session-info block as one readable, clipboard-friendly string
    /// (`Label: value` lines). Returns `None` unless the Session info tab is
    /// active and its rows have loaded.
    pub fn session_info_copy_all(&self) -> Option<String> {
        if self.active_tab != UsageInfoTab::SessionInfo {
            return None;
        }
        let fields = self.session_fields.as_ref().filter(|f| !f.is_empty())?;
        Some(
            fields
                .iter()
                .map(|f| format!("{}: {}", f.label, f.value))
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }

    /// Scroll to an absolute offset, dropping any hover highlight. The cursor
    /// stays put while the content shifts under it, so the previously hovered
    /// content line no longer sits under the pointer; the next mouse move
    /// recomputes it.
    fn scroll_to(&mut self, offset: u16) {
        self.scroll = offset;
        self.hovered_copy_line = None;
    }

    fn step_tab(&mut self, forward: bool) {
        let n = UsageInfoTab::ALL.len();
        let i = self.active_tab.index();
        let next = if forward {
            (i + 1) % n
        } else {
            (i + n - 1) % n
        };
        self.set_tab(UsageInfoTab::from_index(next));
    }
}

/// Outcome of a content key/mouse event. Chrome events (Esc, `[✗]`, tab
/// clicks, footer clicks) are handled by the caller via `modal_window`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsageModalOutcome {
    /// Copy the session ID to the clipboard (caller owns clipboard + toast).
    /// Emitted by the `c` shortcut and the footer button.
    CopySessionId,
    /// Copy an arbitrary Session-info value row (caller owns clipboard +
    /// toast). Emitted when a copyable row is clicked.
    CopyText(String),
    Changed,
    Unchanged,
}

pub fn handle_usage_modal_key(
    state: &mut UsageInfoModalState,
    key: &KeyEvent,
) -> UsageModalOutcome {
    use crossterm::event::KeyModifiers;
    // BackTab / `G` legitimately carry SHIFT; reject only real chords.
    if key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
    {
        return UsageModalOutcome::Unchanged;
    }
    match key.code {
        KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') => {
            state.step_tab(true);
            UsageModalOutcome::Changed
        }
        KeyCode::BackTab | KeyCode::Left | KeyCode::Char('h') => {
            state.step_tab(false);
            UsageModalOutcome::Changed
        }
        KeyCode::Char(c @ '1'..='3') => {
            state.set_tab(UsageInfoTab::from_index(c as usize - '1' as usize));
            UsageModalOutcome::Changed
        }
        KeyCode::Up | KeyCode::Char('k') => {
            state.scroll_to(state.scroll.saturating_sub(1));
            UsageModalOutcome::Changed
        }
        KeyCode::Down | KeyCode::Char('j') => {
            state.scroll_to(state.scroll.saturating_add(1));
            UsageModalOutcome::Changed
        }
        KeyCode::PageUp => {
            state.scroll_to(state.scroll.saturating_sub(10));
            UsageModalOutcome::Changed
        }
        KeyCode::PageDown => {
            state.scroll_to(state.scroll.saturating_add(10));
            UsageModalOutcome::Changed
        }
        KeyCode::Home => {
            state.scroll_to(0);
            UsageModalOutcome::Changed
        }
        // Scroll offsets are clamped to the content height at render time.
        KeyCode::End | KeyCode::Char('G') => {
            state.scroll_to(u16::MAX);
            UsageModalOutcome::Changed
        }
        KeyCode::Char('c') if state.ctx.session_id.is_some() => UsageModalOutcome::CopySessionId,
        KeyCode::Char('y') => match state.session_info_copy_all() {
            Some(text) => UsageModalOutcome::CopyText(text),
            None => UsageModalOutcome::Unchanged,
        },
        _ => UsageModalOutcome::Unchanged,
    }
}

pub fn handle_usage_modal_mouse(
    state: &mut UsageInfoModalState,
    kind: MouseEventKind,
    column: u16,
    row: u16,
) -> UsageModalOutcome {
    let hit_at = |state: &UsageInfoModalState| -> Option<SessionCopyHit> {
        state
            .copy_hits
            .iter()
            .find(|h| {
                column >= h.rect.x
                    && column < h.rect.x + h.rect.width
                    && row >= h.rect.y
                    && row < h.rect.y + h.rect.height
            })
            .cloned()
    };
    match kind {
        MouseEventKind::ScrollUp => {
            state.scroll_to(state.scroll.saturating_sub(3));
            UsageModalOutcome::Changed
        }
        MouseEventKind::ScrollDown => {
            state.scroll_to(state.scroll.saturating_add(3));
            UsageModalOutcome::Changed
        }
        MouseEventKind::Down(crossterm::event::MouseButton::Left) => match hit_at(state) {
            Some(hit) => UsageModalOutcome::CopyText(hit.value),
            None => UsageModalOutcome::Unchanged,
        },
        MouseEventKind::Moved => {
            let new_hover = hit_at(state).map(|h| h.line_idx);
            if new_hover != state.hovered_copy_line {
                state.hovered_copy_line = new_hover;
                UsageModalOutcome::Changed
            } else {
                UsageModalOutcome::Unchanged
            }
        }
        _ => UsageModalOutcome::Unchanged,
    }
}

pub fn render_usage_modal(
    buf: &mut Buffer,
    area: Rect,
    state: &mut UsageInfoModalState,
    balance: Option<&CreditBalance>,
    compact: bool,
    theme: &Theme,
) {
    let labels: Vec<&str> = UsageInfoTab::ALL.iter().map(|t| t.label()).collect();
    state.window.active_tab = state.active_tab.index();

    let mut shortcuts: Vec<Shortcut> = vec![
        Shortcut {
            label: "Tab switch",
            clickable: false,
            id: 0,
        },
        Shortcut {
            label: "\u{2191}/\u{2193} scroll",
            clickable: false,
            id: 0,
        },
    ];
    if state.ctx.session_id.is_some() {
        shortcuts.push(Shortcut {
            label: "c copy session ID",
            clickable: true,
            id: COPY_SESSION_ID_SHORTCUT,
        });
    }
    // "Copy all" is meaningful only on the Session info tab once its text has
    // loaded. (The row click-to-copy hint lives next to the tab's title.)
    if state.active_tab == UsageInfoTab::SessionInfo
        && state.session_fields.as_ref().is_some_and(|f| !f.is_empty())
    {
        shortcuts.push(Shortcut {
            label: "y copy all",
            clickable: true,
            id: COPY_ALL_SESSION_INFO_SHORTCUT,
        });
    }
    shortcuts.push(Shortcut {
        label: "Esc close",
        clickable: false,
        id: 0,
    });

    // v_pad / footer_lines pad the body top and bottom (shortcuts render
    // bottom-aligned, so the spare footer row reads as bottom padding).
    let sizing = ModalSizing {
        width_pct: 0.65,
        max_width: 100,
        min_width: 44,
        v_margin: 2,
        h_pad: 2,
        v_pad: 2,
        footer_lines: 3,
    }
    .with_compact(compact);
    // No border title — the tab bar is the header, as in the extensions modal.
    let config = ModalWindowConfig {
        title: "",
        tabs: Some(&labels),
        shortcuts: &shortcuts,
        sizing,
        fold_info: None,
    };

    // The chrome always fills `area` minus `v_margin`, so cap the height
    // ourselves: tall terminals would otherwise get a mostly-empty box.
    // 30 rows ≈ the widest tab's content (context grid + legend) + chrome.
    const MAX_MODAL_HEIGHT: u16 = 30;
    let outer = MAX_MODAL_HEIGHT + sizing.v_margin * 2;
    let area = if area.height > outer {
        Rect {
            x: area.x,
            y: area.y + (area.height - outer) / 2,
            width: area.width,
            height: outer,
        }
    } else {
        area
    };

    let Some(mca) = mw::render_modal_window(buf, area, &mut state.window, &config, theme) else {
        state.copy_hits.clear();
        return;
    };
    let content = mca.content;
    let tab = tab_lines(state, balance, theme, content.width);
    // No wrapping: one row per logical line keeps the scroll clamp exact.
    let max_scroll = tab.lines.len().saturating_sub(content.height as usize);
    state.scroll = (state.scroll as usize).min(max_scroll) as u16;
    // Map each copyable value row to its on-screen hit rect for this frame.
    state.copy_hits = tab
        .copy_targets
        .iter()
        .filter_map(|t| {
            let visible_row = t.line_idx.checked_sub(state.scroll as usize)?;
            (visible_row < content.height as usize).then(|| SessionCopyHit {
                rect: Rect {
                    x: content.x,
                    y: content.y + visible_row as u16,
                    width: content.width,
                    height: 1,
                },
                value: t.value.clone(),
                line_idx: t.line_idx,
            })
        })
        .collect();
    let visible: Vec<Line> = tab
        .lines
        .into_iter()
        .skip(state.scroll as usize)
        .take(content.height as usize)
        .collect();
    Paragraph::new(visible).render(content, buf);
}

/// A copyable value row: `line_idx` indexes `TabContent::lines`; `value` is the
/// text copied when the row is clicked.
struct CopyTarget {
    line_idx: usize,
    value: String,
}

/// Rendered content of one tab.
struct TabContent {
    lines: Vec<Line<'static>>,
    /// Copyable value rows in this tab (empty on non-Session-info tabs).
    copy_targets: Vec<CopyTarget>,
}

impl TabContent {
    fn from_lines(lines: Vec<Line<'static>>) -> Self {
        Self {
            lines,
            copy_targets: Vec::new(),
        }
    }
}

fn tab_lines(
    state: &UsageInfoModalState,
    balance: Option<&CreditBalance>,
    theme: &Theme,
    width: u16,
) -> TabContent {
    match state.active_tab {
        UsageInfoTab::ContextUsage => {
            TabContent::from_lines(context_tab_lines(state, theme, width))
        }
        UsageInfoTab::UsageLimit => {
            TabContent::from_lines(usage_limit_lines(state, balance, theme))
        }
        UsageInfoTab::SessionInfo => session_info_content(state, theme),
    }
}

fn header_style(theme: &Theme) -> Style {
    Style::default()
        .fg(theme.text_primary)
        .add_modifier(Modifier::BOLD)
}

fn plain(theme: &Theme, s: impl Into<String>) -> Line<'static> {
    Line::styled(s.into(), Style::default().fg(theme.text_primary))
}

fn muted_line(theme: &Theme, s: impl Into<String>) -> Line<'static> {
    Line::from(Span::styled(s.into(), theme.muted()))
}

fn context_tab_lines(state: &UsageInfoModalState, theme: &Theme, width: u16) -> Vec<Line<'static>> {
    if let Some(error) = &state.context_error {
        return vec![muted_line(
            theme,
            format!("Couldn't load context usage: {error}"),
        )];
    }
    if let Some(block) = &state.context {
        return block.lines_for_width(theme, width);
    }
    if state.ctx.session_id.is_none() {
        return vec![muted_line(theme, "No active session.")];
    }
    vec![muted_line(theme, "Loading context usage\u{2026}")]
}

/// Account allowance followed by this session's token/cost totals.
fn usage_limit_lines(
    state: &UsageInfoModalState,
    balance: Option<&CreditBalance>,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();

    if state.ctx.chat_kind {
        // Gateway chat sessions have no Build coding credits to show.
    } else if !state.ctx.usage_visible {
        lines.push(muted_line(theme, "Usage limits are managed by your team."));
    } else if let Some(url) = &state.ctx.billing_redirect_url {
        lines.push(plain(theme, format!("Please check your usage on {url}")));
    } else if let Some(bal) = balance {
        lines.extend(allowance_lines(state, bal, theme));
    } else if let Some(error) = &state.billing_error {
        lines.push(muted_line(theme, format!("Couldn't load usage: {error}")));
    } else if state.billing_loading {
        lines.push(muted_line(theme, "Loading usage\u{2026}"));
    } else {
        lines.push(muted_line(theme, "No billing data available."));
    }

    if let Some(usage_text) = &state.session_usage_text {
        if !lines.is_empty() {
            lines.push(Line::default());
        }
        for (i, row) in usage_text.lines().enumerate() {
            if i == 0 {
                lines.push(Line::styled(row.to_string(), header_style(theme)));
            } else {
                lines.push(plain(theme, row));
            }
        }
    } else if state.ctx.session_id.is_some() {
        if !lines.is_empty() {
            lines.push(Line::default());
        }
        lines.push(muted_line(theme, "Loading session usage\u{2026}"));
    }
    lines
}

fn allowance_lines(
    state: &UsageInfoModalState,
    bal: &CreditBalance,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();

    // "Weekly limit" / "Monthly limit" / "Usage", plus the plan name.
    let header = match &state.ctx.subscription_tier {
        Some(tier) => format!("{} ({tier})", bal.usage_label()),
        None => bal.usage_label().to_string(),
    };
    lines.push(Line::styled(header, header_style(theme)));
    lines.push(Line::default());

    const BAR_WIDTH: usize = 30;
    let pct = bal.usage_pct.clamp(0.0, 100.0);
    let filled = (((pct / 100.0) * BAR_WIDTH as f64).round() as usize).min(BAR_WIDTH);
    lines.push(Line::from(vec![
        Span::styled(
            "\u{2588}".repeat(filled),
            Style::default().fg(theme.gray_bright),
        ),
        Span::styled(
            "\u{2591}".repeat(BAR_WIDTH - filled),
            Style::default().fg(theme.gray_dim),
        ),
        // Floored to match the backend's truncation.
        Span::styled(
            format!("  {}%", bal.usage_pct.floor() as i64),
            Style::default().fg(theme.text_primary),
        ),
    ]));

    if let Some(reset) = &bal.period_end_display {
        lines.push(muted_line(theme, format!("Resets: {reset}")));
    }

    // Prepaid credits (stored as negative cents — accounting convention).
    if let Some(prepaid) = bal.prepaid_balance_cents.map(i64::abs).filter(|c| *c > 0) {
        lines.push(Line::default());
        lines.push(plain(
            theme,
            format!("Credits: ${:.2}", prepaid as f64 / 100.0),
        ));
    }

    // Legacy on-demand (pay-as-you-go) billing.
    if bal.pay_as_you_go {
        let used = bal.on_demand_used_cents.unwrap_or(0).abs() as f64 / 100.0;
        let cap = bal.on_demand_cap_cents.unwrap_or(0).abs() as f64 / 100.0;
        lines.push(Line::default());
        lines.push(Line::styled("Pay as you go: Enabled", header_style(theme)));
        lines.push(muted_line(
            theme,
            format!("Usage: ${used:.2} / ${cap:.2} per month"),
        ));
    }
    lines
}

fn session_info_content(state: &UsageInfoModalState, theme: &Theme) -> TabContent {
    if let Some(error) = &state.session_error {
        return TabContent::from_lines(vec![muted_line(
            theme,
            format!("Couldn't load session info: {error}"),
        )]);
    }
    let Some(fields) = state.session_fields.as_ref().filter(|f| !f.is_empty()) else {
        if state.ctx.session_id.is_none() {
            return TabContent::from_lines(vec![muted_line(theme, "No active session.")]);
        }
        return TabContent::from_lines(vec![muted_line(theme, "Loading session info\u{2026}")]);
    };

    // The title carries a dim inline hint for the row click-to-copy affordance.
    let mut lines = vec![Line::from(vec![
        Span::styled("Session info", header_style(theme)),
        Span::styled(
            "   click values to copy",
            Style::default().fg(theme.gray_dim),
        ),
    ])];
    let mut copy_targets: Vec<CopyTarget> = Vec::new();
    let mut prev_compact = false;
    for field in fields {
        let compact = field.compact;
        if !(compact && prev_compact) {
            lines.push(Line::default());
        }
        if compact {
            // Dense `Label: value` on one row: label and value read as a unit,
            // so the whole row highlights on hover and copies as `Label: value`.
            let value_idx = lines.len();
            let hovered = state.hovered_copy_line == Some(value_idx);
            let label_style = if hovered {
                theme.muted().bg(theme.bg_hover)
            } else {
                theme.muted()
            };
            lines.push(Line::from(vec![
                Span::styled(format!("{}: ", field.label), label_style),
                Span::styled(field.value.clone(), copy_value_style(theme, hovered)),
            ]));
            copy_targets.push(CopyTarget {
                line_idx: value_idx,
                value: format!("{}: {}", field.label, field.value),
            });
        } else {
            lines.push(Line::from(Span::styled(
                format!("{}:", field.label),
                theme.muted(),
            )));
            let value_idx = lines.len();
            let hovered = state.hovered_copy_line == Some(value_idx);
            lines.push(Line::from(Span::styled(
                field.value.clone(),
                copy_value_style(theme, hovered),
            )));
            copy_targets.push(CopyTarget {
                line_idx: value_idx,
                value: field.value.clone(),
            });
        }
        prev_compact = compact;
    }
    TabContent {
        lines,
        copy_targets,
    }
}

/// Style for a copyable value: a `bg_hover` background highlight while the
/// cursor is over the row, matching the home screen's hovered rows.
fn copy_value_style(theme: &Theme, hovered: bool) -> Style {
    let mut style = Style::default().fg(theme.text_primary);
    if hovered {
        style = style.bg(theme.bg_hover);
    }
    style
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEventKind, KeyEventState, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn field(label: &'static str, value: &str, compact: bool) -> SessionInfoField {
        SessionInfoField {
            label,
            value: value.to_string(),
            compact,
        }
    }

    fn state_with_session() -> UsageInfoModalState {
        UsageInfoModalState::new(
            UsageInfoTab::UsageLimit,
            UsageInfoContext {
                session_id: Some("sid-123".to_string()),
                usage_visible: true,
                chat_kind: false,
                billing_redirect_url: None,
                subscription_tier: Some("SuperGrok".to_string()),
            },
        )
    }

    #[test]
    fn tab_cycling_wraps_and_resets_scroll() {
        let mut state = state_with_session();
        state.scroll = 7;
        assert_eq!(
            handle_usage_modal_key(&mut state, &key(KeyCode::Tab)),
            UsageModalOutcome::Changed
        );
        assert_eq!(state.active_tab, UsageInfoTab::SessionInfo);
        assert_eq!(state.scroll, 0, "tab switch resets scroll");
        handle_usage_modal_key(&mut state, &key(KeyCode::Tab));
        assert_eq!(state.active_tab, UsageInfoTab::ContextUsage, "wraps");
        handle_usage_modal_key(&mut state, &key(KeyCode::BackTab));
        assert_eq!(state.active_tab, UsageInfoTab::SessionInfo, "wraps back");
    }

    #[test]
    fn copy_shortcut_requires_a_session_id() {
        let mut state = state_with_session();
        assert_eq!(
            handle_usage_modal_key(&mut state, &key(KeyCode::Char('c'))),
            UsageModalOutcome::CopySessionId
        );
        state.ctx.session_id = None;
        assert_eq!(
            handle_usage_modal_key(&mut state, &key(KeyCode::Char('c'))),
            UsageModalOutcome::Unchanged
        );
    }

    #[test]
    fn usage_limit_tab_shows_allowance_and_payg() {
        let state = state_with_session();
        let bal = CreditBalance {
            usage_pct: 50.67,
            effective_usage_pct: 50.67,
            period_end_display: Some("May 29, 00:00".to_string()),
            pay_as_you_go: true,
            on_demand_cap_cents: Some(10_000),
            on_demand_used_cents: Some(0),
            prepaid_balance_cents: None,
            period_type: Some("USAGE_PERIOD_TYPE_WEEKLY".to_string()),
            is_unified_billing_user: None,
        };
        let theme = Theme::current();
        let lines = usage_limit_lines(&state, Some(&bal), &theme);
        let text: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
        assert_eq!(text[0], "Weekly limit (SuperGrok)");
        assert!(text[2].ends_with("50%"), "bar row: {:?}", text[2]);
        assert!(text.iter().any(|l| l.contains("Resets: May 29, 00:00")));
        assert!(text.iter().any(|l| l == "Pay as you go: Enabled"));
        assert!(
            text.iter().any(|l| l == "Usage: $0.00 / $100.00 per month"),
            "{text:?}"
        );
        assert!(
            !text.iter().any(|l| l.to_lowercase().contains("top")),
            "no auto top-up surface: {text:?}"
        );
    }

    #[test]
    fn usage_limit_tab_states() {
        let theme = Theme::current();
        let mut state = state_with_session();
        state.billing_loading = true;
        let lines = usage_limit_lines(&state, None, &theme);
        assert!(lines[0].to_string().contains("Loading usage"));

        state.ctx.billing_redirect_url = Some("https://x.example/usage".to_string());
        let lines = usage_limit_lines(&state, None, &theme);
        assert!(lines[0].to_string().contains("https://x.example/usage"));

        state.ctx.usage_visible = false;
        let lines = usage_limit_lines(&state, None, &theme);
        assert!(lines[0].to_string().contains("managed by your team"));

        // Gateway chat sessions surface no billing at all.
        state.ctx.chat_kind = true;
        let lines = usage_limit_lines(&state, None, &theme);
        assert!(lines[0].to_string().contains("Loading session usage"));
    }

    #[test]
    fn render_smoke_shows_tabs_and_copy_shortcut() {
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        let mut state = state_with_session();
        state.session_usage_text = Some("Session usage: no model calls yet.".to_string());
        let theme = Theme::current();
        render_usage_modal(&mut buf, area, &mut state, None, false, &theme);
        let text: String = (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
                    + "\n"
            })
            .collect();
        for needle in [
            "Context usage",
            "Usage limit",
            "Session info",
            "copy session ID",
            "Session usage: no model calls yet.",
        ] {
            assert!(text.contains(needle), "missing {needle:?} in:\n{text}");
        }
        assert_eq!(state.window.tab_rects.len(), 3);
        assert!(state.window.close_button_rect.is_some());
    }

    #[test]
    fn every_value_row_is_click_to_copy() {
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        let mut state = state_with_session();
        state.set_tab(UsageInfoTab::SessionInfo);
        state.session_fields = Some(vec![
            field("Session ID", "sid-123", false),
            field("Model", "Grok", true),
            field("Model Hash", "fp-abc", true),
            field("Turn", "3", true),
        ]);
        let theme = Theme::current();
        render_usage_modal(&mut buf, area, &mut state, None, false, &theme);

        // Non-compact rows copy just the value; compact model/runtime rows copy
        // the whole `Label: value` line.
        let values: Vec<&str> = state.copy_hits.iter().map(|h| h.value.as_str()).collect();
        assert_eq!(
            values,
            ["sid-123", "Model: Grok", "Model Hash: fp-abc", "Turn: 3"]
        );

        // Clicking the Model Hash row copies the whole line.
        let hash_hit = state
            .copy_hits
            .iter()
            .find(|h| h.value == "Model Hash: fp-abc")
            .expect("model hash row visible")
            .clone();
        assert_eq!(
            handle_usage_modal_mouse(
                &mut state,
                MouseEventKind::Down(crossterm::event::MouseButton::Left),
                hash_hit.rect.x,
                hash_hit.rect.y,
            ),
            UsageModalOutcome::CopyText("Model Hash: fp-abc".to_string())
        );

        // Clicks in empty content don't copy.
        assert_eq!(
            handle_usage_modal_mouse(
                &mut state,
                MouseEventKind::Down(crossterm::event::MouseButton::Left),
                hash_hit.rect.x,
                area.height - 1,
            ),
            UsageModalOutcome::Unchanged
        );

        // Other tabs never expose copy hits.
        state.set_tab(UsageInfoTab::UsageLimit);
        render_usage_modal(&mut buf, area, &mut state, None, false, &theme);
        assert!(state.copy_hits.is_empty());
    }

    #[test]
    fn copy_all_returns_readable_block_and_footer_button() {
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        let mut state = state_with_session();
        // Copy-all is unavailable until the Session info tab has rows.
        assert_eq!(state.session_info_copy_all(), None);
        state.set_tab(UsageInfoTab::SessionInfo);
        assert_eq!(state.session_info_copy_all(), None);
        state.session_fields = Some(vec![
            field("Session ID", "sid-123", false),
            field("Model Hash", "fp-abc", true),
            field("Turn", "3", true),
        ]);
        // Rows are joined as readable `Label: value` lines.
        assert_eq!(
            state.session_info_copy_all().as_deref(),
            Some("Session ID: sid-123\nModel Hash: fp-abc\nTurn: 3")
        );
        // The footer surfaces a clickable "copy all" button on this tab.
        let theme = Theme::current();
        render_usage_modal(&mut buf, area, &mut state, None, false, &theme);
        let text: String = (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
                    + "\n"
            })
            .collect();
        assert!(
            text.contains("copy all"),
            "missing copy-all button:\n{text}"
        );
        assert!(
            text.contains("click values to copy"),
            "missing row-copy hint:\n{text}"
        );
        // The `y` key returns the same readable block.
        assert_eq!(
            handle_usage_modal_key(&mut state, &key(KeyCode::Char('y'))),
            UsageModalOutcome::CopyText(
                "Session ID: sid-123\nModel Hash: fp-abc\nTurn: 3".to_string()
            )
        );
        // On other tabs the button and shortcut are gone.
        state.set_tab(UsageInfoTab::UsageLimit);
        assert_eq!(state.session_info_copy_all(), None);
        assert_eq!(
            handle_usage_modal_key(&mut state, &key(KeyCode::Char('y'))),
            UsageModalOutcome::Unchanged
        );
    }

    #[test]
    fn hovering_a_value_row_updates_hover_state() {
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        let mut state = state_with_session();
        state.set_tab(UsageInfoTab::SessionInfo);
        state.session_fields = Some(vec![field("Turn", "3", true)]);
        let theme = Theme::current();
        render_usage_modal(&mut buf, area, &mut state, None, false, &theme);
        let hit = state.copy_hits.first().expect("turn row visible").clone();
        assert_eq!(
            handle_usage_modal_mouse(&mut state, MouseEventKind::Moved, hit.rect.x, hit.rect.y),
            UsageModalOutcome::Changed
        );
        assert_eq!(state.hovered_copy_line, Some(hit.line_idx));

        // Turn is a compact row: the whole line highlights (label + value) and
        // it copies as "Turn: 3".
        assert_eq!(hit.value, "Turn: 3");
        let tab = session_info_content(&state, &theme);
        let row = &tab.lines[hit.line_idx];
        assert_eq!(
            row.spans[0].style.bg,
            Some(theme.bg_hover),
            "compact label must be highlighted"
        );
        assert_eq!(
            row.spans[1].style.bg,
            Some(theme.bg_hover),
            "value must be highlighted"
        );
        // Moving off the row clears the hover.
        assert_eq!(
            handle_usage_modal_mouse(
                &mut state,
                MouseEventKind::Moved,
                hit.rect.x,
                area.height - 1,
            ),
            UsageModalOutcome::Changed
        );
        assert_eq!(state.hovered_copy_line, None);
    }

    #[test]
    fn scrolling_clears_stale_hover() {
        let mut state = state_with_session();
        state.set_tab(UsageInfoTab::SessionInfo);
        // Wheel scroll: the content shifts under a stationary cursor, so the
        // hover must drop rather than highlight whatever slid under it.
        state.hovered_copy_line = Some(6);
        handle_usage_modal_mouse(&mut state, MouseEventKind::ScrollDown, 0, 0);
        assert_eq!(state.hovered_copy_line, None);
        // Keyboard scroll clears it for the same reason.
        state.hovered_copy_line = Some(6);
        handle_usage_modal_key(&mut state, &key(KeyCode::Down));
        assert_eq!(state.hovered_copy_line, None);
    }

    #[test]
    fn popup_height_is_capped_on_tall_terminals() {
        let area = Rect::new(0, 0, 100, 60);
        let mut buf = Buffer::empty(area);
        let mut state = state_with_session();
        let theme = Theme::current();
        render_usage_modal(&mut buf, area, &mut state, None, false, &theme);
        let popup = state.window.popup_area.expect("popup rendered");
        assert_eq!(popup.height, 30);
        // Still vertically centered.
        assert_eq!(popup.y, (60 - 30) / 2);
    }

    #[test]
    fn session_info_tab_spaces_groups_and_compacts_model_block() {
        let mut state = state_with_session();
        // Auth is never a field (it's prose the string form appends), so it
        // simply isn't present here — no filtering needed at the modal.
        state.session_fields = Some(vec![
            field("Title", "t", false),
            field("Session ID", "sid-123", false),
            field("Working directory", "/tmp", false),
            field("Model", "Grok", true),
            field("Context", "1 / 2", true),
        ]);
        let theme = Theme::current();
        let tab = session_info_content(&state, &theme);
        let text: Vec<String> = tab.lines.iter().map(|l| l.to_string()).collect();
        assert_eq!(
            text,
            [
                "Session info   click values to copy",
                "",
                "Title:",
                "t",
                "",
                "Session ID:",
                "sid-123",
                "",
                "Working directory:",
                "/tmp",
                "",
                "Model: Grok",
                "Context: 1 / 2",
            ]
        );
        // Each copy target is (value line index, copied text). Non-compact rows
        // copy just the value; the compact model/runtime rows copy the whole
        // `Label: value` line.
        let targets: Vec<(usize, &str)> = tab
            .copy_targets
            .iter()
            .map(|t| (t.line_idx, t.value.as_str()))
            .collect();
        assert_eq!(
            targets,
            [
                (3, "t"),
                (6, "sid-123"),
                (9, "/tmp"),
                (11, "Model: Grok"),
                (12, "Context: 1 / 2"),
            ]
        );
    }
}
