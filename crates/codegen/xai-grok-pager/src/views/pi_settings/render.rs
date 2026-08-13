//! Rendering for the grok-pi settings panel.
//!
//! ## Browse surface
//!
//! ```text
//! ┌─ Settings ─────────────────────────────────────────────┐
//! │  Appearance  Mouse  Editor  Agent  …                    │  native tab bar
//! │ ──────────────────────────────────────────────────────  │
//! │  Theme       │ Theme                     Grok Night ›   │  sidebar │ pane
//! │  Display     │ Auto dark theme           Grok Night ›   │
//! │  Thinking    │ Compact mode                       on    │  dimmed: other section
//! │                                                         │
//! │  Color theme for the pager UI.                          │  fixed 3-row block
//! │ ──────────────────────────────────────────────────────  │
//! │  ↑/↓ nav · Space toggle · ←/→ tabs · / search · Esc      │
//! └─────────────────────────────────────────────────────────┘
//! ```
//!
//! Every list row is exactly one line high, so the scroll window is a plain
//! slice of `state.visible`. Sub-panes (chooser, editors, reset confirm) take
//! over the whole content area and drop the tab bar for a breadcrumb title.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::layout;
use super::state::{Mode, ModeKind, PiSettingsState, Row};
use crate::render::line_utils::truncate_str;
use crate::settings::{
    CodingDataSharingLock, OwnedEnumChoice, SettingKind, SettingMeta, SettingValue,
};
use crate::theme::Theme;
use crate::views::modal_window::{
    self, ModalContentArea, ModalSizing, ModalWindowConfig, Shortcut,
};

// ---------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------

/// Widest sidebar name we render before truncating.
const SIDEBAR_NAME_CAP: u16 = 22;
const SIDEBAR_INDENT: u16 = 2;
const SIDEBAR_GAP: u16 = 2;
const SIDEBAR_SEPARATOR: &str = "\u{2502} ";
const SIDEBAR_SEPARATOR_W: u16 = 2;
/// Below this the value column starves, so the sidebar is dropped and section
/// names render as inline headings instead.
const MIN_PANE_W: u16 = 46;

/// Cursor gutter — `› ` on the focused row, blank otherwise.
const CURSOR_W: u16 = 2;
/// Chevron column, reserved on every row so the glyph never shifts.
const CHEVRON_W: u16 = 2;
const RIGHT_PAD_W: u16 = 1;
/// Minimum blank columns between the label and value columns.
const GAP_W: u16 = 2;
/// Label column ceiling, so one long label cannot starve the value column.
const LABEL_CAP: u16 = 30;

/// Fixed description rows under the list, plus the blank row above them.
/// Constant so moving between rows with and without long descriptions never
/// reflows the list.
const DESCRIPTION_ROWS: u16 = 3;
const DESCRIPTION_BLOCK: u16 = DESCRIPTION_ROWS + 1;

/// Below this the row list is skipped entirely.
const MIN_CONTENT_W: u16 = 24;
const MAX_MODAL_W: u16 = 118;

/// Value shown for a ZDR-locked row, replacing opt-in/opt-out entirely.
const ZDR_VALUE: &str = "ZDR";
/// Appended to a team-managed row's value.
const ADMIN_SUFFIX: &str = " \u{00B7} Admin Managed";

/// Sidebar column width, derived from every section name in the layout table
/// rather than the active tab's, so the divider never shifts between tabs.
fn sidebar_width() -> u16 {
    (layout::widest_section_name() as u16).min(SIDEBAR_NAME_CAP) + SIDEBAR_INDENT + SIDEBAR_GAP
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Render the whole panel: modal chrome, then the mode's surface.
pub fn render_pi_settings(
    buf: &mut Buffer,
    full_area: Rect,
    state: &mut PiSettingsState,
    compact: bool,
) {
    let theme = Theme::current();
    let shortcuts = build_shortcuts(state);

    // Sub-panes wear a breadcrumb title and hand the tab row back as content.
    let breadcrumb;
    let title: &str = match state.mode.subject().and_then(|key| state.meta(key)) {
        Some(meta) if state.mode.is_sub_pane() => {
            breadcrumb = format!(
                "{} {} {}",
                super::MODAL_TITLE,
                crate::glyphs::chevron(),
                meta.label
            );
            &breadcrumb
        }
        _ => super::MODAL_TITLE,
    };

    let browsing = !state.mode.is_sub_pane();
    let tab_labels = state.tab_labels();
    let sizing = ModalSizing {
        // The split sidebar plus settings pane needs more room than a plain
        // single-column list.
        width_pct: 0.86,
        max_width: MAX_MODAL_W,
        min_width: 44,
        v_margin: 3,
        h_pad: 2,
        v_pad: 1,
        footer_lines: 2,
    }
    .with_compact(compact);

    let config = ModalWindowConfig {
        title,
        tabs: browsing.then_some(tab_labels.as_slice()),
        shortcuts: &shortcuts,
        sizing,
        fold_info: None,
    };

    let Some(ModalContentArea { content, .. }) =
        modal_window::render_modal_window(buf, full_area, &mut state.window, &config, &theme)
    else {
        state.reset_hit_rects();
        return;
    };
    if content.height < 2 || content.width < MIN_CONTENT_W {
        state.reset_hit_rects();
        return;
    }

    match state.mode.kind() {
        ModeKind::Browse | ModeKind::Search => {
            state.choice_rects.clear();
            state.stepper_rects = (Rect::default(), Rect::default());
            render_browse(buf, content, state, &theme);
        }
        ModeKind::Picking => {
            state.reset_hit_rects();
            render_chooser(buf, content, state, &theme);
        }
        ModeKind::PickingGroup => {
            state.reset_hit_rects();
            render_group_sheet(buf, content, state, &theme);
        }
        ModeKind::EditingString | ModeKind::EditingInt => {
            state.reset_hit_rects();
            render_editor(buf, content, state, &theme);
        }
        ModeKind::ConfirmReset => {
            state.reset_hit_rects();
            render_reset_confirm(buf, content, state, &theme);
        }
    }
}

// ---------------------------------------------------------------------------
// Browse
// ---------------------------------------------------------------------------

fn render_browse(buf: &mut Buffer, content: Rect, state: &mut PiSettingsState, theme: &Theme) {
    let mut area = content;

    // The search banner exists only while a query is live; browsing is entered
    // with `/`, so an always-on empty bar would just cost a row.
    if state.searching() && area.height >= 3 {
        crate::views::picker::render_line_editor_search_bar(
            buf,
            area.x,
            area.y,
            area.width,
            theme,
            &state.query,
            state.mode.kind() == ModeKind::Search,
            true,
            Some(theme.bg_base),
        );
        crate::views::picker::render_divider(
            buf,
            area.x,
            area.y + 1,
            area.width,
            theme,
            Some(theme.bg_base),
        );
        area = Rect {
            y: area.y + 2,
            height: area.height - 2,
            ..area
        };
    }

    // Reserve the description block whenever a usable list still fits.
    let (list_area, description) = if area.height > DESCRIPTION_BLOCK + 1 {
        (
            Rect {
                height: area.height - DESCRIPTION_BLOCK,
                ..area
            },
            Some(Rect {
                y: area.y + area.height - DESCRIPTION_ROWS,
                height: DESCRIPTION_ROWS,
                ..area
            }),
        )
    } else {
        (area, None)
    };

    state.list_area = list_area;
    render_rows(buf, list_area, state, theme);
    if let Some(description) = description {
        render_description(buf, description, state, theme);
    }
}

/// Paint the section sidebar and settings pane, and record their hit rects.
fn render_rows(buf: &mut Buffer, area: Rect, state: &mut PiSettingsState, theme: &Theme) {
    state.row_rects.clear();
    state.row_rects.resize(state.rows.len(), Rect::default());
    state.value_rects.clear();
    state.value_rects.resize(state.rows.len(), Rect::default());
    state.sidebar_rects.clear();

    let viewport = area.height as usize;
    if viewport == 0 {
        return;
    }
    if state.visible.is_empty() {
        render_no_matches(buf, area, state, theme);
        return;
    }

    let sections = state.sections();
    let active_section = state.active_section();
    let sidebar_w = sidebar_width();
    let split = sections.len() >= 2 && area.width >= sidebar_w + SIDEBAR_SEPARATOR_W + MIN_PANE_W;
    let pane = if split {
        render_sidebar(buf, area, sidebar_w, &sections, active_section, state, theme);
        Rect {
            x: area.x + sidebar_w + SIDEBAR_SEPARATOR_W,
            width: area.width - sidebar_w - SIDEBAR_SEPARATOR_W,
            ..area
        }
    } else {
        buf.set_style(area, Style::default().bg(theme.bg_base));
        area
    };

    let active_range = split.then(|| active_section_range(&sections, active_section));
    let label_w = label_column_width(state, pane.width);
    state.scroll = clamp_scroll(state, viewport);
    let end = state.visible.len().min(state.scroll + viewport);
    let window: Vec<usize> = state.visible[state.scroll..end].to_vec();
    let hover = state.hover;
    let section_focus = state.section_focus;

    for (offset, row) in window.into_iter().enumerate() {
        let rect = Rect {
            x: pane.x,
            y: pane.y + offset as u16,
            width: pane.width,
            height: 1,
        };
        state.row_rects[row] = rect;
        // Rows outside the section under the cursor recede so the active
        // section reads as the foreground.
        let dimmed = active_range.is_some_and(|(first, last)| row < first || row > last);

        match &state.rows[row] {
            Row::Tab { category } => render_heading(buf, rect, category.label(), false, theme),
            Row::Section { name, .. } => {
                // The sidebar owns section names in the split layout; inline
                // headings are the narrow-terminal fallback.
                if !split {
                    render_heading(buf, rect, name, dimmed, theme);
                }
            }
            Row::Setting { key, meta } => {
                let key = *key;
                let Some(meta) = state.registry.all().get(*meta) else {
                    continue;
                };
                let selected = row == state.selected && !section_focus;
                let style = RowStyle {
                    selected,
                    hovered: hover == Some(row),
                    dimmed: dimmed && !selected,
                };
                let value_rect = render_setting_row(
                    buf,
                    rect,
                    meta,
                    state.value_of(key).as_ref(),
                    label_w,
                    style,
                    state.lock(key),
                    theme,
                );
                state.value_rects[row] = value_rect;
            }
        }
    }
}

fn render_no_matches(buf: &mut Buffer, area: Rect, state: &PiSettingsState, theme: &Theme) {
    if !state.searching() {
        return;
    }
    let prefix = "No matches for ";
    let budget = (area.width as usize)
        .saturating_sub(prefix.width())
        .saturating_sub(2); // surrounding quotes
    let query = truncate_str(state.query(), budget);
    let message = format!("{prefix}\"{query}\"");
    let style = Style::default().fg(theme.gray_dim).bg(theme.bg_base);
    let w = (message.width() as u16).min(area.width);
    buf.set_span(
        area.x + area.width.saturating_sub(w) / 2,
        area.y + area.height / 2,
        &Span::styled(&message, style),
        w,
    );
}

/// Row index range `[first, last]` of the section under the cursor, including
/// its heading row. Rows outside it render dimmed.
fn active_section_range(sections: &[(&'static str, usize)], active: usize) -> (usize, usize) {
    let first = sections[active].1;
    let last = sections
        .get(active + 1)
        .map(|(_, next)| next.saturating_sub(1))
        .unwrap_or(usize::MAX);
    // The section's heading row sits directly above its first setting.
    (first.saturating_sub(1), last)
}

#[allow(clippy::too_many_arguments)]
fn render_sidebar(
    buf: &mut Buffer,
    area: Rect,
    sidebar_w: u16,
    sections: &[(&'static str, usize)],
    active: usize,
    state: &mut PiSettingsState,
    theme: &Theme,
) {
    buf.set_style(area, Style::default().bg(theme.bg_base));
    let separator = Style::default().fg(theme.gray_dim).bg(theme.bg_base);
    for row in 0..area.height {
        buf.set_span(
            area.x + sidebar_w,
            area.y + row,
            &Span::styled(SIDEBAR_SEPARATOR, separator),
            SIDEBAR_SEPARATOR_W,
        );
    }

    let name_w = sidebar_w.saturating_sub(SIDEBAR_INDENT + SIDEBAR_GAP);
    state.sidebar_rects.resize(sections.len(), Rect::default());
    for (i, (name, _)) in sections.iter().enumerate() {
        if i as u16 >= area.height {
            break;
        }
        let rect = Rect {
            x: area.x,
            y: area.y + i as u16,
            width: sidebar_w,
            height: 1,
        };
        state.sidebar_rects[i] = rect;
        let is_active = i == active;
        let focused = is_active && state.section_focus;
        // With section focus the cursor parks here, so the sidebar becomes the
        // single focus indicator.
        let cursor = if focused {
            format!("{} ", crate::glyphs::chevron())
        } else {
            " ".repeat(SIDEBAR_INDENT as usize)
        };
        let style = match (is_active, focused) {
            (_, true) => Style::default()
                .fg(theme.text_primary)
                .bg(theme.bg_visual)
                .add_modifier(Modifier::BOLD),
            (true, false) => Style::default()
                .fg(theme.accent_user)
                .bg(theme.bg_base)
                .add_modifier(Modifier::BOLD),
            (false, false) => Style::default().fg(theme.gray).bg(theme.bg_base),
        };
        if focused {
            buf.set_style(rect, Style::default().bg(theme.bg_visual));
        }
        let text = format!("{cursor}{}", truncate_str(name, name_w as usize));
        let w = (text.width() as u16).min(sidebar_w);
        buf.set_span(rect.x, rect.y, &Span::styled(&text, style), w);
    }
}

/// Minimal scroll that keeps the focus on screen, pulling in the heading above
/// it when the focus would otherwise sit on the first line.
fn clamp_scroll(state: &PiSettingsState, viewport: usize) -> usize {
    let max = state.visible.len().saturating_sub(viewport);
    let Some(position) = state.visible.iter().position(|&r| r == state.selected) else {
        return state.scroll.min(max);
    };
    let mut scroll = state.scroll;
    if position < scroll {
        scroll = position;
    }
    if position >= scroll + viewport {
        scroll = position + 1 - viewport;
    }
    if scroll == position
        && position > 0
        && matches!(
            state.rows[state.visible[position - 1]],
            Row::Tab { .. } | Row::Section { .. }
        )
    {
        scroll = position - 1;
    }
    scroll.min(max)
}

/// Label column width for the current view.
fn label_column_width(state: &PiSettingsState, pane_w: u16) -> u16 {
    let cap = LABEL_CAP.min(pane_w / 2);
    state
        .visible
        .iter()
        .filter_map(|&row| match &state.rows[row] {
            Row::Setting { meta, .. } => state.registry.all().get(*meta),
            _ => None,
        })
        .map(|meta| meta.label.width() as u16)
        .max()
        .unwrap_or(0)
        .min(cap)
}

/// The focused row's description, in the fixed block under the list.
fn render_description(buf: &mut Buffer, area: Rect, state: &PiSettingsState, theme: &Theme) {
    buf.set_style(area, Style::default().bg(theme.bg_base));
    let Some((key, meta)) = state.focused() else {
        return;
    };
    let wrap_w = area.width.saturating_sub(SIDEBAR_INDENT);
    if wrap_w == 0 {
        return;
    }

    // A locked row's reason replaces the description: it is the only thing the
    // user can act on.
    let owned;
    let text: &str = match state.lock(key).map(CodingDataSharingLock::reason) {
        Some(reason) => reason,
        None if meta.restart_required => {
            owned = format!("{} Takes effect on next start.", meta.description);
            &owned
        }
        None => meta.description,
    };

    let style = Style::default()
        .fg(theme.gray)
        .bg(theme.bg_base)
        .add_modifier(Modifier::ITALIC);
    let wrapped = wrap_text(text, wrap_w);
    let rows = DESCRIPTION_ROWS as usize;
    for (i, line) in wrapped.iter().take(rows).enumerate() {
        let owned_line;
        let text: &str = if i + 1 == rows && wrapped.len() > rows {
            owned_line = format!(
                "{}\u{2026}",
                truncate_str(line, wrap_w.saturating_sub(1) as usize)
            );
            &owned_line
        } else {
            line
        };
        let w = (text.width() as u16).min(wrap_w);
        buf.set_span(
            area.x + SIDEBAR_INDENT,
            area.y + i as u16,
            &Span::styled(text, style),
            w,
        );
    }
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

/// Visual state of a settings row for one frame.
#[derive(Debug, Clone, Copy)]
struct RowStyle {
    selected: bool,
    hovered: bool,
    /// Row belongs to a section other than the one under the cursor.
    dimmed: bool,
}

fn render_heading(buf: &mut Buffer, area: Rect, label: &str, dimmed: bool, theme: &Theme) {
    buf.set_style(area, Style::default().bg(theme.bg_base));
    let style = Style::default()
        .fg(if dimmed { theme.gray_dim } else { theme.gray })
        .bg(theme.bg_base)
        .add_modifier(Modifier::BOLD);
    let text = truncate_str(label, area.width.saturating_sub(CURSOR_W) as usize);
    let w = (text.width() as u16).min(area.width.saturating_sub(CURSOR_W));
    buf.set_span(
        area.x + CURSOR_W.min(area.width),
        area.y,
        &Span::styled(&text, style),
        w,
    );
}

/// Terminal-native themes collapse selection tokens to `Reset`; fall back to
/// ANSI `DarkGray` (not silver `Gray`, which washes out on dark profiles).
fn row_bg(theme: &Theme, selected: bool, hovered: bool) -> Color {
    if crate::theme::cache::terminal_native_locked() || matches!(theme.bg_visual, Color::Reset) {
        return if selected || hovered {
            Color::DarkGray
        } else {
            Color::Reset
        };
    }
    if selected {
        theme.bg_visual
    } else if hovered {
        theme.bg_hover
    } else {
        theme.bg_base
    }
}

/// Value-column text, shared by the row painter and the chooser.
pub(super) fn value_text(
    meta: &SettingMeta,
    value: &SettingValue,
    lock: Option<CodingDataSharingLock>,
) -> String {
    if lock == Some(CodingDataSharingLock::Zdr) {
        return ZDR_VALUE.to_string();
    }
    let mut text = match value {
        SettingValue::Bool(b) => if *b { "on" } else { "off" }.to_string(),
        SettingValue::String(s) => {
            if s.is_empty() && matches!(meta.kind, SettingKind::DynamicEnum { .. }) {
                "(no override)".to_string()
            } else {
                s.clone()
            }
        }
        SettingValue::Enum(canonical) => enum_display(&meta.kind, canonical).to_string(),
        SettingValue::Int(i) => i.to_string(),
        SettingValue::PiBuiltinTools(_) => "Pi built-in tools".to_string(),
    };
    if lock == Some(CodingDataSharingLock::TeamManaged) {
        text.push_str(ADMIN_SUFFIX);
    }
    text
}

/// Display name for an Enum canonical, falling back to the canonical itself so
/// a hand-edited config still renders something.
fn enum_display<'a>(kind: &'a SettingKind, canonical: &'a str) -> &'a str {
    if let SettingKind::Enum { choices, .. } = kind
        && let Some(choice) = choices.iter().find(|c| c.canonical == canonical)
    {
        return choice.display;
    }
    canonical
}

/// One setting row: cursor gutter, padded label column, then the value
/// right-aligned against the reserved chevron column. Always one line high.
/// Returns the value column's hit rect.
fn render_setting_row(
    buf: &mut Buffer,
    area: Rect,
    meta: &SettingMeta,
    value: Option<&SettingValue>,
    label_w: u16,
    row: RowStyle,
    lock: Option<CodingDataSharingLock>,
    theme: &Theme,
) -> Rect {
    let bg = row_bg(theme, row.selected, row.hovered);
    buf.set_style(area, Style::default().bg(bg));

    // A Group is pure navigation into a sub-sheet: the registry gives it no
    // scalar value, so it renders label + chevron with an empty value column.
    let group = matches!(meta.kind, SettingKind::Group { .. });
    // Any other missing value carrier means registry/dispatch skew. Surface it
    // rather than rendering a silently blank row.
    if value.is_none() && !group {
        return render_unmapped_row(buf, area, meta, label_w, bg, theme);
    }

    // Dimmed rows collapse to one flat wash so inner colors do not fight it.
    let (label_style, value_style) = if row.dimmed {
        let dim = Style::default().fg(theme.gray_dim).bg(bg);
        (dim, dim)
    } else {
        let mut label = Style::default().fg(theme.text_primary).bg(bg);
        if row.selected {
            label = label.add_modifier(Modifier::BOLD);
        }
        // Off and locked values recede; everything else carries the accent.
        let value = if lock.is_some() || matches!(value, Some(SettingValue::Bool(false))) {
            Style::default().fg(theme.gray).bg(bg)
        } else {
            Style::default().fg(theme.accent_user).bg(bg)
        };
        (label, value)
    };

    let cursor = if row.selected {
        format!("{} ", crate::glyphs::chevron())
    } else {
        " ".repeat(CURSOR_W as usize)
    };
    buf.set_span(
        area.x,
        area.y,
        &Span::styled(&cursor, Style::default().fg(theme.accent_user).bg(bg)),
        CURSOR_W.min(area.width),
    );

    let label_x = area.x + CURSOR_W.min(area.width);
    let label_avail = area.width.saturating_sub(CURSOR_W);
    let label = truncate_str(meta.label, label_w.min(label_avail) as usize);
    let painted = (label.width() as u16).min(label_avail);
    if painted > 0 {
        buf.set_span(label_x, area.y, &Span::styled(&label, label_style), painted);
    }

    // Chevron marks rows that open a sub-pane; locked rows cannot be entered
    // so they drop the affordance.
    let show_chevron = lock.is_none()
        && matches!(
            meta.kind,
            SettingKind::Enum { .. }
                | SettingKind::String { .. }
                | SettingKind::DynamicEnum { .. }
                | SettingKind::Int { .. }
                | SettingKind::Group { .. }
        );
    let chevron_x = (area.x + area.width).saturating_sub(RIGHT_PAD_W + CHEVRON_W);
    let value_floor = label_x + label_w.min(label_avail) + GAP_W;
    let value_avail = chevron_x.saturating_sub(value_floor);
    let text = value
        .map(|value| truncate_str(&value_text(meta, value, lock), value_avail as usize))
        .unwrap_or_default();
    let value_w = (text.width() as u16).min(value_avail);
    let value_x = chevron_x.saturating_sub(value_w);
    if value_w > 0 {
        buf.set_span(value_x, area.y, &Span::styled(&text, value_style), value_w);
    }
    if show_chevron {
        let style = Style::default()
            .fg(if row.dimmed { theme.gray_dim } else { theme.gray })
            .bg(bg);
        buf.set_span(
            chevron_x,
            area.y,
            &Span::styled(format!(" {}", crate::glyphs::chevron()), style),
            CHEVRON_W,
        );
    }

    Rect {
        x: value_x,
        y: area.y,
        width: value_w.saturating_add(CHEVRON_W),
        height: 1,
    }
}

/// A row whose `current_value_for` returned `None` (registry / dispatch skew).
/// Renders in the error color so the misconfiguration is visible at runtime.
fn render_unmapped_row(
    buf: &mut Buffer,
    area: Rect,
    meta: &SettingMeta,
    label_w: u16,
    bg: Color,
    theme: &Theme,
) -> Rect {
    let style = Style::default()
        .fg(theme.accent_error)
        .bg(bg)
        .add_modifier(Modifier::BOLD);
    let text = format!(
        "  {}  (no read mapping)",
        truncate_str(meta.label, label_w as usize)
    );
    let w = (text.width() as u16).min(area.width);
    buf.set_span(area.x, area.y, &Span::styled(&text, style), w);
    Rect {
        x: area.x,
        y: area.y,
        width: 0,
        height: 1,
    }
}

// ---------------------------------------------------------------------------
// Sub-panes
// ---------------------------------------------------------------------------

/// Shared sub-pane header: bold title, wrapped description, blank gap.
/// Returns the rows consumed so the caller can position its body.
fn render_sub_pane_header(
    buf: &mut Buffer,
    area: Rect,
    theme: &Theme,
    title: &str,
    description: &str,
) -> u16 {
    buf.set_style(area, Style::default().bg(theme.bg_base));
    let title_style = Style::default()
        .fg(theme.text_primary)
        .bg(theme.bg_base)
        .add_modifier(Modifier::BOLD);
    let text = truncate_str(title, area.width as usize);
    let w = (text.width() as u16).min(area.width);
    buf.set_span(area.x, area.y, &Span::styled(&text, title_style), w);

    let wrapped = wrap_text(description, area.width);
    let rows = wrapped.len() as u16;
    // Give the body room first: skip the description if it would crowd it out.
    if rows == 0 || area.height < rows + 3 {
        return 2;
    }
    let style = Style::default().fg(theme.gray_dim).bg(theme.bg_base);
    for (i, line) in wrapped.iter().enumerate() {
        let w = (line.width() as u16).min(area.width);
        buf.set_span(
            area.x,
            area.y + 1 + i as u16,
            &Span::styled(line.as_str(), style),
            w,
        );
    }
    2 + rows
}

/// Radio-style chooser for Enum / DynamicEnum settings.
fn render_chooser(buf: &mut Buffer, area: Rect, state: &mut PiSettingsState, theme: &Theme) {
    let Mode::Picking {
        key, index, scroll, ..
    } = state.mode
    else {
        return;
    };
    let Some(meta) = state.meta(key) else {
        return;
    };
    let (label, description) = (meta.label, meta.description);
    let choices = state.choices_for(key);
    let header = render_sub_pane_header(buf, area, theme, label, description);
    if area.height <= header {
        return;
    }

    let body = Rect {
        y: area.y + header,
        height: area.height - header,
        ..area
    };
    let scroll = scroll.min(choices.len().saturating_sub(1));
    state.choice_rects = render_choice_list(buf, body, &choices, index, scroll, state.hover, theme);
    if let Mode::Picking { scroll: s, .. } = &mut state.mode {
        *s = scroll;
    }
}

/// Render a scrolling radio list. Each choice takes one line for its label
/// plus wrapped description lines; the returned rects span the whole entry.
fn render_choice_list(
    buf: &mut Buffer,
    area: Rect,
    choices: &[OwnedEnumChoice],
    selected: usize,
    scroll: usize,
    hover: Option<usize>,
    theme: &Theme,
) -> Vec<Rect> {
    let mut rects = vec![Rect::default(); choices.len()];
    let mut y = area.y;
    let end_y = area.y + area.height;
    for (i, choice) in choices.iter().enumerate().skip(scroll) {
        if y >= end_y {
            break;
        }
        let is_selected = i == selected;
        let bg = row_bg(theme, is_selected, hover == Some(i));
        let marker = if is_selected {
            "\u{25CF}"
        } else {
            "\u{25CB}"
        };
        let marker_style = Style::default()
            .fg(if is_selected {
                theme.accent_user
            } else {
                theme.gray
            })
            .bg(bg);
        let label_style = Style::default()
            .fg(theme.text_primary)
            .bg(bg)
            .add_modifier(if is_selected {
                Modifier::BOLD
            } else {
                Modifier::empty()
            });

        let description = wrap_text(&choice.description, area.width.saturating_sub(5));
        let height = (1 + description.len() as u16).min(end_y - y);
        let rect = Rect {
            x: area.x,
            y,
            width: area.width,
            height,
        };
        buf.set_style(rect, Style::default().bg(bg));
        rects[i] = rect;

        buf.set_line(
            area.x,
            y,
            &Line::from(vec![
                Span::styled(format!(" {marker}  "), marker_style),
                Span::styled(choice.display.clone(), label_style),
            ]),
            area.width,
        );
        let description_style = Style::default().fg(theme.gray_dim).bg(bg);
        for (line_no, line) in description.iter().enumerate() {
            let line_y = y + 1 + line_no as u16;
            if line_y >= end_y {
                break;
            }
            let w = (line.width() as u16).min(area.width.saturating_sub(5));
            buf.set_span(
                area.x + 5,
                line_y,
                &Span::styled(line.as_str(), description_style),
                w,
            );
        }
        y += height;
    }
    rects
}

/// Sub-sheet listing a `Group` setting's child toggles.
fn render_group_sheet(buf: &mut Buffer, area: Rect, state: &mut PiSettingsState, theme: &Theme) {
    let Mode::PickingGroup { key, child } = state.mode else {
        return;
    };
    let Some(meta) = state.meta(key) else {
        return;
    };
    let header = render_sub_pane_header(buf, area, theme, meta.label, meta.description);
    if area.height <= header {
        return;
    }

    let children = state.group_children(key);
    let mut rects = vec![Rect::default(); children.len()];
    let label_w = children
        .iter()
        .filter_map(|k| state.meta(k))
        .map(|m| m.label.width() as u16)
        .max()
        .unwrap_or(0)
        .min(LABEL_CAP);
    for (i, child_key) in children.iter().enumerate() {
        let y = area.y + header + i as u16;
        if y >= area.y + area.height {
            break;
        }
        let Some(child_meta) = state.meta(child_key) else {
            continue;
        };
        let rect = Rect {
            x: area.x,
            y,
            width: area.width,
            height: 1,
        };
        rects[i] = rect;
        let style = RowStyle {
            selected: i == child,
            hovered: state.hover == Some(i),
            dimmed: false,
        };
        render_setting_row(
            buf,
            rect,
            child_meta,
            state.value_of(child_key).as_ref(),
            label_w,
            style,
            None,
            theme,
        );
    }
    state.choice_rects = rects;
}

/// Inline editor for String and Int settings.
fn render_editor(buf: &mut Buffer, area: Rect, state: &mut PiSettingsState, theme: &Theme) {
    let key = match state.mode.subject() {
        Some(key) => key,
        None => return,
    };
    let Some(meta) = state.meta(key) else {
        return;
    };
    let header = render_sub_pane_header(buf, area, theme, meta.label, meta.description);
    if area.height <= header {
        return;
    }
    let field = Rect {
        x: area.x,
        y: area.y + header,
        width: area.width,
        height: 1,
    };

    // Paint first, then publish the stepper hit rects, so the immutable
    // borrow on `state.mode` is done before the mutable write.
    let stepper = match &state.mode {
        Mode::EditingString { editor, error, .. } => {
            crate::views::picker::render_line_editor_search_bar(
                buf,
                field.x,
                field.y,
                field.width,
                theme,
                editor,
                true,
                false,
                Some(theme.bg_base),
            );
            if let Some(error) = error
                && field.y + 2 < area.y + area.height
            {
                let style = Style::default().fg(theme.accent_error).bg(theme.bg_base);
                let text = truncate_str(error, area.width as usize);
                let w = (text.width() as u16).min(area.width);
                buf.set_span(area.x, field.y + 2, &Span::styled(&text, style), w);
            }
            None
        }
        Mode::EditingInt {
            buffer, min, max, ..
        } => {
            let left_glyph = super::input::STEPPER_LEFT;
            let right_glyph = super::input::STEPPER_RIGHT;
            let text = format!("{left_glyph}  {buffer}  {right_glyph}   ({min}\u{2013}{max})");
            let style = Style::default()
                .fg(theme.text_primary)
                .bg(theme.bg_base)
                .add_modifier(Modifier::BOLD);
            let w = (text.width() as u16).min(area.width);
            buf.set_span(field.x, field.y, &Span::styled(&text, style), w);
            let left = Rect {
                x: field.x,
                y: field.y,
                width: left_glyph.width() as u16,
                height: 1,
            };
            let right = Rect {
                x: field.x + (left_glyph.width() + 2 + buffer.width() + 2) as u16,
                y: field.y,
                width: right_glyph.width() as u16,
                height: 1,
            };
            Some((left, right))
        }
        _ => None,
    };
    if let Some(stepper) = stepper {
        state.stepper_rects = stepper;
    }
}

/// Reset-to-default confirmation, drawn over a dimmed list.
fn render_reset_confirm(buf: &mut Buffer, area: Rect, state: &mut PiSettingsState, theme: &Theme) {
    let Mode::ConfirmReset { key } = state.mode else {
        return;
    };
    let Some(meta) = state.meta(key) else {
        return;
    };
    let default = state
        .default_of(key)
        .map(|value| value_text(meta, &value, None))
        .unwrap_or_else(|| "default".to_string());
    buf.set_style(area, Style::default().bg(theme.bg_base));

    let prompt = format!("Reset \u{201C}{}\u{201D} to {default}?", meta.label);
    let style = Style::default()
        .fg(theme.accent_user)
        .bg(theme.bg_base)
        .add_modifier(Modifier::BOLD);
    let text = truncate_str(&prompt, area.width as usize);
    let w = (text.width() as u16).min(area.width);
    buf.set_span(area.x, area.y, &Span::styled(&text, style), w);

    if area.height >= 3 {
        let current = state
            .value_of(key)
            .map(|value| value_text(meta, &value, None))
            .unwrap_or_default();
        let note = format!("Currently {current}.");
        let note_style = Style::default().fg(theme.gray).bg(theme.bg_base);
        let w = (note.width() as u16).min(area.width);
        buf.set_span(area.x, area.y + 2, &Span::styled(&note, note_style), w);
    }
}

// ---------------------------------------------------------------------------
// Footer
// ---------------------------------------------------------------------------

fn shortcut(label: &'static str) -> Shortcut<'static> {
    Shortcut {
        label,
        clickable: false,
        id: 0,
    }
}

/// Footer hints for the current mode.
fn build_shortcuts(state: &PiSettingsState) -> Vec<Shortcut<'static>> {
    match state.mode.kind() {
        ModeKind::Browse if state.section_focus => vec![
            shortcut("\u{2191}/\u{2193} jump sections"),
            shortcut("Tab/Enter settings"),
            shortcut("\u{2190}/\u{2192} tabs"),
            shortcut("Esc close"),
        ],
        ModeKind::Browse => {
            // A locked row accepts neither the edit keys nor `d`, so it
            // advertises neither; the reason shows in the description block.
            let locked = state
                .focused()
                .is_some_and(|(key, _)| state.lock(key).is_some());
            let mut hints = vec![shortcut("\u{2191}/\u{2193}/j/k nav")];
            if !locked {
                hints.push(shortcut("Space toggle"));
                hints.push(match state.focused() {
                    Some((_, meta)) if matches!(meta.kind, SettingKind::Bool { .. }) => {
                        shortcut("Enter toggle")
                    }
                    _ => shortcut("Enter edit"),
                });
            }
            hints.push(shortcut("\u{2190}/\u{2192} tabs"));
            if state.sections().len() >= 2 {
                hints.push(shortcut("Tab sections"));
            }
            hints.push(shortcut("/ search"));
            if !locked {
                hints.push(shortcut("d reset"));
            }
            hints.push(shortcut("F2/Esc close"));
            hints
        }
        ModeKind::Search => vec![
            shortcut("type to filter"),
            shortcut("\u{2191}/\u{2193} nav"),
            shortcut("Enter commit"),
            shortcut("Esc clear"),
        ],
        ModeKind::Picking => {
            let preview = matches!(
                state.mode,
                Mode::Picking {
                    supports_preview: true,
                    ..
                }
            );
            vec![
                shortcut(if preview {
                    "\u{2191}/\u{2193} try"
                } else {
                    "\u{2191}/\u{2193} nav"
                }),
                shortcut("Enter select"),
                shortcut(if preview { "Esc revert" } else { "Esc cancel" }),
            ]
        }
        ModeKind::PickingGroup => vec![
            shortcut("\u{2191}/\u{2193} nav"),
            shortcut("Space/Enter toggle"),
            shortcut("Esc back"),
        ],
        ModeKind::EditingString => vec![
            shortcut("type to edit"),
            shortcut("Enter save"),
            shortcut("Esc cancel"),
        ],
        ModeKind::EditingInt => vec![
            shortcut("\u{2190}/\u{2192} step"),
            shortcut("Enter save"),
            shortcut("Esc cancel"),
        ],
        ModeKind::ConfirmReset => vec![
            shortcut("y reset"),
            shortcut("n cancel"),
            shortcut("Esc cancel"),
        ],
    }
}

/// Word-wrap into owned lines. Descriptions are single-line by registry
/// contract, so this never has to split on `\n`.
fn wrap_text(text: &str, width: u16) -> Vec<String> {
    if text.is_empty() || width == 0 {
        return Vec::new();
    }
    let line = Line::from(Span::raw(text));
    crate::render::wrapping::word_wrap_line(&line, width as usize)
        .into_iter()
        .map(|l| {
            l.spans
                .into_iter()
                .map(|s| s.content.into_owned())
                .collect::<String>()
        })
        .collect()
}
