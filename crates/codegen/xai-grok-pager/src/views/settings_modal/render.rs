//! Settings modal rendering.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::state::{
    CONTENT_MIN_WIDTH, MAX_THOUGHTS_WIDTH_WIDENED_MARGIN, MODAL_TITLE, RowEntry,
    STANDARD_MAX_WIDTH, SettingsModalState, SettingsMode, SettingsModeKind,
    TITLE_LEADING_DECORATION_W, effective_enum_choices, group_children, mode_is_consent_chooser,
};
use crate::render::line_utils::truncate_str;
use crate::settings::{
    CodingDataSharingLock, OwnedEnumChoice, SettingKey, SettingKind, SettingMeta, SettingValue,
    StringValidator, dynamic_enum_choices,
};
use crate::theme::Theme;
use crate::views::modal_window::{
    self, ModalContentArea, ModalSizing, ModalWindowConfig, Shortcut,
};

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Overlay for the reset-confirm dialog. Overrides chrome breadcrumb,
/// footer, and search bar with the confirmation prompt.
pub struct ResetConfirmOverlay<'a> {
    pub prompt: &'a str,

    pub breadcrumb_suffix: &'a str,
}

/// Render the settings modal. Returns `true` if the reset-confirm
/// overlay was rendered (caller suppresses row-list mouse events).
pub fn render_settings_modal(
    buf: &mut Buffer,
    full_area: Rect,
    state: &mut SettingsModalState,
    compact: bool,
    overlay: Option<&ResetConfirmOverlay<'_>>,
) -> bool {
    let theme = Theme::current();
    let confirm_shortcuts = build_reset_confirm_shortcuts();
    let normal_shortcuts = build_shortcuts(state);
    let shortcuts: &[Shortcut<'_>] = if overlay.is_some() {
        &confirm_shortcuts
    } else {
        &normal_shortcuts
    };

    // Breadcrumb title for sub-modes: "Settings › <label>".
    let breadcrumb_owned: String;
    let title: &str = if let Some(o) = overlay {
        breadcrumb_owned = format!(
            "{MODAL_TITLE} {} {}",
            crate::glyphs::chevron(),
            o.breadcrumb_suffix
        );
        &breadcrumb_owned
    } else {
        match &state.state.mode {
            SettingsMode::PickingEnum { key, .. } => {
                if let Some(meta) = state.registry.find(key) {
                    breadcrumb_owned =
                        format!("{MODAL_TITLE} {} {}", crate::glyphs::chevron(), meta.label);
                    &breadcrumb_owned
                } else {
                    MODAL_TITLE
                }
            }

            SettingsMode::EditingString { key, .. } | SettingsMode::EditingInt { key, .. } => {
                if let Some(meta) = state.registry.find(key) {
                    breadcrumb_owned =
                        format!("{MODAL_TITLE} {} {}", crate::glyphs::chevron(), meta.label);
                    &breadcrumb_owned
                } else {
                    MODAL_TITLE
                }
            }
            SettingsMode::PickingGroup { key, .. } => {
                if let Some(meta) = state.registry.find(key) {
                    breadcrumb_owned =
                        format!("{MODAL_TITLE} {} {}", crate::glyphs::chevron(), meta.label);
                    &breadcrumb_owned
                } else {
                    MODAL_TITLE
                }
            }
            _ => MODAL_TITLE,
        }
    };

    // Footer sizing: predict shortcut wrap rows, add a gap row above
    // when the docs footer is present. EditingValue suppresses the
    // docs footer. Widen the modal when editing `max_thoughts_width`
    // so the wrap preview is useful at widths above STANDARD_MAX_WIDTH.
    let widen_for_max_thoughts_width = matches!(
        &state.state.mode,
        SettingsMode::EditingInt { key, .. }
            if *key == crate::settings::defs::MAX_THOUGHTS_WIDTH_KEY
    );
    let widened_candidate = full_area
        .width
        .saturating_sub(MAX_THOUGHTS_WIDTH_WIDENED_MARGIN);
    let (max_width, width_pct) =
        if widen_for_max_thoughts_width && widened_candidate > STANDARD_MAX_WIDTH {
            (widened_candidate, 1.0)
        } else {
            // The split sidebar + settings pane needs more room than the old
            // single-column list did.
            (STANDARD_MAX_WIDTH, 0.86)
        };
    let sizing = ModalSizing {
        width_pct,
        max_width,
        min_width: 44,
        v_margin: 3,
        h_pad: 2,
        v_pad: 1,
        footer_lines: 2,
    }
    .with_compact(compact);
    // Must agree with the `docs_footer_area` split below — a mismatch
    // would reserve a row nothing paints (or paint into the body).
    let has_tip_footer = !matches!(
        state.state.mode_kind(),
        SettingsModeKind::EditingString | SettingsModeKind::EditingInt
    ) && !mode_is_consent_chooser(&state.state.mode);
    let footer_lines = if has_tip_footer {
        modal_window::footer_lines_with_tip_gap(full_area, &sizing, shortcuts)
    } else {
        sizing.footer_lines
    };
    let sizing = ModalSizing {
        footer_lines,
        ..sizing
    };

    // The tab bar belongs to Browse; sub-panes take over the whole window and
    // wear a breadcrumb title instead.
    let tab_labels: Vec<&str> = state.tab_labels();
    let browsing = matches!(
        state.state.mode_kind(),
        SettingsModeKind::Browse | SettingsModeKind::FilterFocused
    ) && overlay.is_none();
    let modal_config = ModalWindowConfig {
        title,
        tabs: browsing.then_some(tab_labels.as_slice()),
        shortcuts,
        sizing,
        fold_info: None,
    };

    let Some(ModalContentArea {
        content: content_area,
        ..
    }) =
        modal_window::render_modal_window(buf, full_area, &mut state.window, &modal_config, &theme)
    else {
        // Chrome refused — reset hit-rects for graceful degradation.
        state.reset_hit_rects();
        return overlay.is_some();
    };

    if content_area.height < 2 || content_area.width < CONTENT_MIN_WIDTH {
        state.reset_hit_rects();
        return overlay.is_some();
    }

    if let Some(o) = overlay {
        // Confirmation overlay replaces the search bar; row list
        // renders dimmed underneath. Hit-rects reset so clicks
        // only route to the y/n footer buttons.
        render_reset_confirm_overlay(buf, content_area, state, &theme, o);
        return true;
    }

    let (inner_area, docs_footer_area) = match state.state.mode_kind() {
        SettingsModeKind::EditingString | SettingsModeKind::EditingInt => (content_area, None),
        // A consent chooser shows the disclosure and the choices only.
        _ if mode_is_consent_chooser(&state.state.mode) => (content_area, None),
        _ => modal_window::split_content_for_tip_footer(content_area),
    };

    // Per-mode render dispatch (exhaustive to catch new variants).
    let mode_is_sub_pane = matches!(
        state.state.mode_kind(),
        SettingsModeKind::PickingEnum
            | SettingsModeKind::PickingGroup
            | SettingsModeKind::EditingString
            | SettingsModeKind::EditingInt
    );
    match state.state.mode_kind() {
        SettingsModeKind::PickingEnum => {
            state.reset_hit_rects();
            render_picking_enum(buf, inner_area, state, &theme);
            state.picker_choice_rects = take_picker_choice_rects();
        }
        SettingsModeKind::PickingGroup => {
            state.reset_hit_rects();
            let rects = render_picking_group(buf, inner_area, state, &theme);
            state.picker_choice_rects = rects;
        }
        SettingsModeKind::EditingString | SettingsModeKind::EditingInt => {
            state.reset_hit_rects();
            render_editing_value(buf, inner_area, state, &theme);
        }
        SettingsModeKind::Browse | SettingsModeKind::FilterFocused => {
            // Clear sub-pane hit-rects from prior frames.
            state.picker_choice_rects.clear();
            state.editor_adornment_rects = (Rect::default(), Rect::default());
            state.settings_breadcrumb_rect = None;
            state.list_area = inner_area;
            render_browse(buf, inner_area, state, &theme);
        }
    }

    // Breadcrumb hit-rect for sub-pane back-navigation on click.
    // Repaint with UNDERLINED (+ accent on hover) for affordance.
    state.settings_breadcrumb_rect = if mode_is_sub_pane {
        state.window.popup_area.map(|popup| {
            let title_w = title.width() as u16;
            // Clamp to not extend past the close button.
            let max_w = popup.width.saturating_sub(2 + 2); // borders + " ─" trailing decoration
            Rect {
                x: popup.x + 1 + TITLE_LEADING_DECORATION_W,
                y: popup.y,
                width: title_w.min(max_w),
                height: 1,
            }
        })
    } else {
        None
    };
    if let Some(rect) = state.settings_breadcrumb_rect {
        let fg = if state.breadcrumb_hovered {
            theme.accent_user
        } else {
            theme.text_primary
        };
        let style_mods = Modifier::BOLD | Modifier::UNDERLINED;
        for offset in 0..rect.width {
            let x = rect.x + offset;
            if let Some(cell) = buf.cell_mut((x, rect.y)) {
                let mut s = cell.style();
                s.fg = Some(fg);
                s = s.add_modifier(style_mods);
                cell.set_style(s);
            }
        }
    }

    if let Some(footer_area) = docs_footer_area {
        render_docs_footer(buf, footer_area, &theme);
    }
    false
}

/// Render the reset-confirm overlay: prompt replaces the search bar,
/// row list renders below with the target row at full intensity and
/// all other rows dimmed.
fn render_reset_confirm_overlay(
    buf: &mut Buffer,
    content_area: Rect,
    state: &mut SettingsModalState,
    theme: &Theme,
    overlay: &ResetConfirmOverlay<'_>,
) {
    // Clear hit-rects so clicks only route to y/n buttons.
    state.reset_hit_rects();

    if content_area.height == 0 || content_area.width == 0 {
        return;
    }

    // Row 0: prompt (full width, bold + accent).
    let prompt_area = Rect {
        x: content_area.x,
        y: content_area.y,
        width: content_area.width,
        height: 1,
    };
    let prompt_bg_style = Style::default().bg(theme.bg_visual);
    buf.set_style(prompt_area, prompt_bg_style);
    let prompt_style = Style::default()
        .fg(theme.accent_user)
        .bg(theme.bg_visual)
        .add_modifier(Modifier::BOLD);
    let prompt_text: std::borrow::Cow<'_, str> =
        if overlay.prompt.width() <= content_area.width as usize {
            std::borrow::Cow::Borrowed(overlay.prompt)
        } else {
            std::borrow::Cow::Owned(truncate_str(overlay.prompt, content_area.width as usize))
        };
    let prompt_w = (prompt_text.width() as u16).min(content_area.width);
    buf.set_span(
        content_area.x,
        content_area.y,
        &Span::styled(prompt_text.as_ref(), prompt_style),
        prompt_w,
    );

    // Row 1+: render rows, then dim all except the target.
    if content_area.height < 2 {
        return;
    }
    let list_area = Rect {
        x: content_area.x,
        y: content_area.y + 1,
        width: content_area.width,
        height: content_area.height - 1,
    };
    render_rows(buf, list_area, state, theme);

    let target_rect = state.row_rects.get(state.selected).copied();

    // Dim every line outside the target row's y-range (DIM + blend).
    let target_y_start = target_rect.map(|r| r.y);
    let target_y_end = target_rect.map(|r| r.y.saturating_add(r.height));
    let area_y_end = list_area.y.saturating_add(list_area.height);
    for y in list_area.y..area_y_end {
        if let (Some(ys), Some(ye)) = (target_y_start, target_y_end)
            && y >= ys
            && y < ye
        {
            continue; // inside the target row's y range — stays full intensity
        }
        let strip = Rect {
            x: list_area.x,
            y,
            width: list_area.width,
            height: 1,
        };
        buf.set_style(strip, Style::default().add_modifier(Modifier::DIM));
        crate::render::color::blend_area(buf, strip, Some((theme.bg_base, 0.55)), None);
    }
}

/// Footer shortcuts for the reset-confirm dialog (y/n are clickable).
fn build_reset_confirm_shortcuts() -> Vec<Shortcut<'static>> {
    use crate::views::modal::{RESET_CONFIRM_NO_ID, RESET_CONFIRM_YES_ID};
    vec![
        Shortcut {
            label: "y reset",
            clickable: true,
            id: RESET_CONFIRM_YES_ID,
        },
        Shortcut {
            label: "n cancel",
            clickable: true,
            id: RESET_CONFIRM_NO_ID,
        },
        Shortcut {
            label: "Esc cancel",
            clickable: false,
            id: 0,
        },
        Shortcut {
            label: "F2 cancel",
            clickable: false,
            id: 0,
        },
    ]
}

// ---------------------------------------------------------------------------
// Browse layout geometry
// ---------------------------------------------------------------------------

/// Longest sidebar section name we will render before truncating.
const SIDEBAR_NAME_CAP: u16 = 22;
/// Leading indent + trailing gap around a sidebar name.
const SIDEBAR_INDENT: u16 = 2;
const SIDEBAR_GAP: u16 = 2;
const SIDEBAR_SEPARATOR: &str = "\u{2502} ";
const SIDEBAR_SEPARATOR_W: u16 = 2;
/// Below this the value column starves, so the sidebar is dropped and the
/// section names render as inline headings instead.
const MIN_PANE_W: u16 = 46;
/// Fixed description rows under the list, plus the blank row above them.
/// Constant so moving between rows with and without long descriptions never
/// reflows the list.
pub(super) const DESCRIPTION_ROWS: u16 = 3;
const BROWSE_FOOTER_ROWS: u16 = DESCRIPTION_ROWS + 1;

/// Sidebar column width, derived from every section name in the layout table
/// rather than just the active tab's, so the divider column never shifts when
/// switching tabs.
fn sidebar_width() -> u16 {
    let widest = crate::settings::SettingCategory::ALL
        .iter()
        .flat_map(|cat| crate::settings::sections_for(*cat))
        .map(|name| name.width() as u16)
        .max()
        .unwrap_or(0);
    widest.min(SIDEBAR_NAME_CAP) + SIDEBAR_INDENT + SIDEBAR_GAP
}

/// Render the Browse / FilterFocused surface: an optional search banner, the
/// section sidebar beside the settings pane, and the focused row's
/// description pinned to the bottom.
pub(super) fn render_browse(
    buf: &mut Buffer,
    content_area: Rect,
    state: &mut SettingsModalState,
    theme: &Theme,
) {
    let mut area = content_area;

    // The search banner only exists while a query is being typed or held —
    // browsing is entered by typing, so an always-on empty search bar would
    // just cost a row.
    let searching =
        state.state.mode_kind() == SettingsModeKind::FilterFocused || !state.query().is_empty();
    if searching && area.height >= 3 {
        crate::views::picker::render_line_editor_search_bar(
            buf,
            area.x,
            area.y,
            area.width,
            theme,
            &state.state.filter,
            state.state.mode_kind() == SettingsModeKind::FilterFocused,
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

    // Reserve the description block at the bottom whenever there is room for
    // it plus a usable list; otherwise the list takes everything.
    let (list_area, description_area) = if area.height > BROWSE_FOOTER_ROWS + 1 {
        (
            Rect {
                height: area.height - BROWSE_FOOTER_ROWS,
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
    if let Some(description_area) = description_area {
        render_focused_description(buf, description_area, state, theme);
    }
}

/// Render the focused row's description into the fixed bottom block.
fn render_focused_description(
    buf: &mut Buffer,
    area: Rect,
    state: &SettingsModalState,
    theme: &Theme,
) {
    buf.set_style(area, Style::default().bg(theme.bg_base));
    let Some((key, meta)) = state.focused_setting() else {
        return;
    };
    let indent = SIDEBAR_INDENT.min(area.width);
    let wrap_w = area.width.saturating_sub(indent);
    if wrap_w == 0 {
        return;
    }

    // A locked row's reason replaces the description: it is the only thing
    // the user can act on.
    let owned;
    let text: &str = match state.row_lock(key).map(CodingDataSharingLock::reason) {
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
    let wrapped = wrap_description(text, wrap_w);
    for (i, line) in wrapped.iter().take(DESCRIPTION_ROWS as usize).enumerate() {
        let is_last_visible = i + 1 == DESCRIPTION_ROWS as usize;
        let overflows = wrapped.len() > DESCRIPTION_ROWS as usize;
        let owned_line;
        let text: &str = if is_last_visible && overflows {
            owned_line = format!("{}\u{2026}", truncate_str(line, wrap_w as usize - 1));
            &owned_line
        } else {
            line
        };
        let w = (text.width() as u16).min(wrap_w);
        buf.set_span(
            area.x + indent,
            area.y + i as u16,
            &Span::styled(text, style),
            w,
        );
    }
}

pub(super) fn render_docs_footer(buf: &mut Buffer, area: Rect, theme: &Theme) {
    const LONG: &str =
        "Tip · Ask Grok: \"change theme to grokday\" or \"what does compact mode do?\"";
    const SHORT: &str = "Tip · Ask Grok to change a setting";
    let text = modal_window::fit_tip_line(&[LONG, SHORT], area.width as usize);
    modal_window::render_centered_tip_footer(buf, area, theme, text.as_ref());
}

/// Render the settings pane, with the section sidebar beside it when the
/// width allows. Every row is exactly one line high, so scroll math is a
/// plain window over `filtered_cache`.
pub(super) fn render_rows(
    buf: &mut Buffer,
    area: Rect,
    state: &mut SettingsModalState,
    theme: &Theme,
) {
    state.row_rects.clear();
    state.row_rects.resize(state.rows.len(), Rect::default());
    state.value_hit_rects.clear();
    state
        .value_hit_rects
        .resize(state.rows.len(), Rect::default());
    state.sidebar_rects.clear();

    let visible_h = area.height as usize;
    if visible_h == 0 {
        return;
    }
    if state.filtered_cache.is_empty() {
        render_no_matches(buf, area, state, theme);
        return;
    }

    // Sidebar only in browse mode: search results are grouped by tab, not by
    // section, so there is nothing for it to index.
    let sections = state.sections();
    let sidebar_w = sidebar_width();
    let split = sections.len() >= 2
        && area.width >= sidebar_w + SIDEBAR_SEPARATOR_W + MIN_PANE_W
        && state.query().is_empty();
    let pane = if split {
        let pane_x = area.x + sidebar_w + SIDEBAR_SEPARATOR_W;
        render_sidebar(buf, area, sidebar_w, &sections, state, theme);
        Rect {
            x: pane_x,
            width: area.width - sidebar_w - SIDEBAR_SEPARATOR_W,
            ..area
        }
    } else {
        buf.set_style(area, Style::default().bg(theme.bg_base));
        area
    };

    let active_range = split.then(|| active_section_range(state, &sections));
    let max_label_w = compute_max_label_w(state, pane.width);
    let selected_fpos = state
        .filtered_cache
        .iter()
        .position(|&i| i == state.selected);
    state.scroll_offset = clamp_scroll(state, selected_fpos, visible_h);
    let end = state
        .filtered_cache
        .len()
        .min(state.scroll_offset + visible_h);
    let visible: Vec<usize> = state.filtered_cache[state.scroll_offset..end].to_vec();
    let hover_row = state.hover_row;
    let section_focus = state.section_focus;

    for (offset, row_idx) in visible.into_iter().enumerate() {
        let row_rect = Rect {
            x: pane.x,
            y: pane.y + offset as u16,
            width: pane.width,
            height: 1,
        };
        // Rows outside the section under the cursor recede so the active
        // section reads as the foreground.
        let dimmed = active_range.is_some_and(|(first, last)| row_idx < first || row_idx > last);
        state.row_rects[row_idx] = row_rect;

        match &state.rows[row_idx] {
            RowEntry::Header { category } => {
                render_heading_row(buf, row_rect, category.label(), false, theme);
            }
            RowEntry::Section { name, .. } => {
                // The sidebar owns section names in the split layout; inline
                // headings are the narrow-terminal fallback.
                if !split {
                    render_heading_row(buf, row_rect, name, dimmed, theme);
                }
            }
            RowEntry::Setting { key, meta_index } => {
                let key = *key;
                let Some(meta) = state.registry.all().get(*meta_index) else {
                    continue;
                };
                let selected = row_idx == state.selected && !section_focus;
                let hovered = hover_row == Some(row_idx);
                let value_rect = render_setting_row(
                    buf,
                    row_rect,
                    meta,
                    state.value_for(key).as_ref(),
                    max_label_w,
                    RowStyle {
                        selected,
                        hovered,
                        dimmed: dimmed && !selected,
                    },
                    state.row_lock(key),
                    theme,
                );
                state.value_hit_rects[row_idx] = value_rect;
            }
        }
    }
}

/// Centered "no results" notice for an empty filter.
fn render_no_matches(buf: &mut Buffer, area: Rect, state: &SettingsModalState, theme: &Theme) {
    if state.query().is_empty() {
        return;
    }
    let prefix = "No matches for ";
    let available = (area.width as usize)
        .saturating_sub(prefix.width())
        .saturating_sub(2); // surrounding quotes
    let query = if state.query().width() <= available {
        state.query().to_owned()
    } else {
        truncate_str(state.query(), available)
    };
    let msg = format!("{prefix}\"{query}\"");
    let style = Style::default().fg(theme.gray_dim).bg(theme.bg_base);
    let msg_w = (msg.width() as u16).min(area.width);
    let x = area.x + area.width.saturating_sub(msg_w) / 2;
    let y = area.y + area.height / 2;
    buf.set_span(x, y, &Span::styled(&msg, style), msg_w);
}

/// Row index range `[first, last]` of the section under the cursor. Rows
/// outside it render dimmed.
fn active_section_range(
    state: &SettingsModalState,
    sections: &[(&'static str, usize)],
) -> (usize, usize) {
    let active = state.active_section_index();
    let first = sections[active].1;
    let last = sections
        .get(active + 1)
        .map(|(_, next_first)| next_first.saturating_sub(1))
        .unwrap_or(usize::MAX);
    // The section's own heading row sits directly above its first setting.
    (first.saturating_sub(1), last)
}

/// Paint the section sidebar and the divider column between it and the pane.
fn render_sidebar(
    buf: &mut Buffer,
    area: Rect,
    sidebar_w: u16,
    sections: &[(&'static str, usize)],
    state: &mut SettingsModalState,
    theme: &Theme,
) {
    buf.set_style(area, Style::default().bg(theme.bg_base));
    let active = state.active_section_index();
    let separator_style = Style::default().fg(theme.gray_dim).bg(theme.bg_base);
    for row in 0..area.height {
        buf.set_span(
            area.x + sidebar_w,
            area.y + row,
            &Span::styled(SIDEBAR_SEPARATOR, separator_style),
            SIDEBAR_SEPARATOR_W,
        );
    }

    let name_w = sidebar_w.saturating_sub(SIDEBAR_INDENT + SIDEBAR_GAP);
    state.sidebar_rects.resize(sections.len(), Rect::default());
    for (i, (name, _)) in sections.iter().enumerate() {
        if i as u16 >= area.height {
            break;
        }
        let y = area.y + i as u16;
        let is_active = i == active;
        let rect = Rect {
            x: area.x,
            y,
            width: sidebar_w,
            height: 1,
        };
        state.sidebar_rects[i] = rect;

        // With section focus the cursor parks here instead of on a row, so
        // the sidebar is the single focus indicator.
        let cursor = if state.section_focus && is_active {
            format!("{} ", crate::glyphs::chevron())
        } else {
            " ".repeat(SIDEBAR_INDENT as usize)
        };
        let style = match (is_active, state.section_focus) {
            (true, true) => Style::default()
                .fg(theme.text_primary)
                .bg(theme.bg_visual)
                .add_modifier(Modifier::BOLD),
            (true, false) => Style::default()
                .fg(theme.accent_user)
                .bg(theme.bg_base)
                .add_modifier(Modifier::BOLD),
            (false, _) => Style::default().fg(theme.gray).bg(theme.bg_base),
        };
        if is_active && state.section_focus {
            buf.set_style(rect, Style::default().bg(theme.bg_visual));
        }
        let label = truncate_str(name, name_w as usize);
        let text = format!("{cursor}{label}");
        let w = (text.width() as u16).min(sidebar_w);
        buf.set_span(area.x, y, &Span::styled(&text, style), w);
    }
}

/// Minimal scroll that keeps the focused row on screen, pulling in the
/// section heading above it when the focus sits at the top of the window.
fn clamp_scroll(
    state: &SettingsModalState,
    selected_fpos: Option<usize>,
    visible_h: usize,
) -> usize {
    let len = state.filtered_cache.len();
    let max_offset = len.saturating_sub(visible_h);
    let Some(fpos) = selected_fpos else {
        return state.scroll_offset.min(max_offset);
    };
    let mut offset = state.scroll_offset;
    if fpos < offset {
        offset = fpos;
    }
    if fpos >= offset + visible_h {
        offset = fpos + 1 - visible_h;
    }
    // Keep the owning heading visible when the focused row would otherwise
    // be the first line of the window.
    if offset == fpos
        && fpos > 0
        && matches!(
            state.rows[state.filtered_cache[fpos - 1]],
            RowEntry::Header { .. } | RowEntry::Section { .. }
        )
    {
        offset = fpos - 1;
    }
    offset.min(max_offset)
}

/// Label column width for the current view, capped so one long label cannot
/// push the value column off screen.
fn compute_max_label_w(state: &SettingsModalState, pane_w: u16) -> u16 {
    const CAP: u16 = 30;
    let cap = CAP.min(pane_w / 2);
    state
        .filtered_cache
        .iter()
        .filter_map(|&i| match &state.rows[i] {
            RowEntry::Setting { meta_index, .. } => state.registry.all().get(*meta_index),
            _ => None,
        })
        .map(|meta| meta.label.width() as u16)
        .max()
        .unwrap_or(0)
        .min(cap)
}

// Picker prefix width templates (glyphs are drawn separately).
const PICKER_PREFIX_SELECTED: &str = " \u{25CF}  ";
const PICKER_PREFIX_UNSELECTED: &str = " \u{25CB}  ";

pub(super) const PICKER_PREFIX_W: u16 = 4;
const PICKER_SEPARATOR: &str = " \u{00B7} ";
const PICKER_SEPARATOR_W: u16 = 3;

const PICKER_MARKER_W: u16 = 1;

/// Render the shared sub-pane header (bold title row + word-wrapped description)
/// used by all four sub-pane renderers — the enum chooser, the group sub-sheet,
/// and the string/int editors. Returns `header_rows`: the rows consumed (title +
/// optional description + the 1-row gap), so the caller positions its body at
/// `area.y + header_rows`. The bodies differ and stay in each caller.
///
/// `min_non_desc_rows` is the vertical budget (excluding the description rows
/// themselves) that must fit before the description renders at all: `2` for the
/// choosers (title + gap), `3` for the editors (title + gap + the input/stepper
/// row). Callers keep their own `if area.height <= header_rows { return; }`.
fn render_sub_pane_header(
    buf: &mut Buffer,
    area: Rect,
    theme: &Theme,
    title: &str,
    description: &str,
    min_non_desc_rows: u16,
) -> u16 {
    // ── Row 0: title (truncated with `…`). ────────────────────────
    let title_style = Style::default()
        .fg(theme.text_primary)
        .bg(theme.bg_base)
        .add_modifier(Modifier::BOLD);
    let title_text: std::borrow::Cow<'_, str> = if title.width() <= area.width as usize {
        std::borrow::Cow::Borrowed(title)
    } else {
        std::borrow::Cow::Owned(truncate_str(title, area.width as usize))
    };
    let title_w = (title_text.width() as u16).min(area.width);
    buf.set_span(
        area.x,
        area.y,
        &Span::styled(title_text.as_ref(), title_style),
        title_w,
    );

    // ── Row 1+: word-wrapped description ──────────────────────────
    let description_wrapped = wrap_description(description, area.width);
    let desc_rows: u16 = description_wrapped.len() as u16;
    let has_description =
        desc_rows > 0 && area.height >= min_non_desc_rows.saturating_add(desc_rows);
    if has_description {
        let desc_style = Style::default().fg(theme.gray_dim).bg(theme.bg_base);
        for (i, wrap_line) in description_wrapped.iter().enumerate() {
            let y = area.y + 1 + i as u16;
            if y >= area.y + area.height {
                break;
            }
            let w = (wrap_line.width() as u16).min(area.width);
            buf.set_span(area.x, y, &Span::styled(wrap_line.as_str(), desc_style), w);
        }
    }

    if has_description { 2 + desc_rows } else { 2 }
}

/// Render the Enum chooser sub-pane. Title + description + radio-style
/// choice list with scrolling and `… N more` overflow indicator.
pub(super) fn render_picking_enum(
    buf: &mut Buffer,
    area: Rect,
    state: &SettingsModalState,
    theme: &Theme,
) {
    debug_assert_eq!(
        PICKER_PREFIX_SELECTED.width(),
        PICKER_PREFIX_W as usize,
        "PICKER_PREFIX_W drifted from PICKER_PREFIX_SELECTED width",
    );
    debug_assert_eq!(
        PICKER_PREFIX_UNSELECTED.width(),
        PICKER_PREFIX_W as usize,
        "PICKER_PREFIX_W drifted from PICKER_PREFIX_UNSELECTED width",
    );
    debug_assert_eq!(
        PICKER_SEPARATOR.width(),
        PICKER_SEPARATOR_W as usize,
        "PICKER_SEPARATOR_W drifted from PICKER_SEPARATOR width",
    );

    let (setting_key, choices_idx, original_value) = match &state.state.mode {
        SettingsMode::PickingEnum {
            key,
            choices_idx,
            original_value,
            ..
        } => (*key, *choices_idx, original_value),
        _ => unreachable!("picker renderer requires PickingEnum state"),
    };
    let committed_canonical: Option<&str> = match original_value {
        SettingValue::Enum(s) => Some(s),
        SettingValue::String(s) => Some(s.as_str()),
        _ => None,
    };
    let Some(meta) = state.registry.find(setting_key) else {
        return;
    };

    let choices: Vec<OwnedEnumChoice> = match &meta.kind {
        SettingKind::Enum { choices, .. } => {
            effective_enum_choices(setting_key, choices, &state.pager_snapshot)
                .into_iter()
                .map(|c| OwnedEnumChoice {
                    canonical: c.canonical.to_string(),
                    display: c.display.to_string(),
                    description: c.description.to_string(),
                })
                .collect()
        }
        SettingKind::DynamicEnum { source, .. } => {
            dynamic_enum_choices(*source, &state.pager_snapshot)
        }
        _ => return,
    };

    if area.width == 0 || area.height == 0 {
        return;
    }

    // Choosers need title + gap (2) before the description renders.
    let header_rows = render_sub_pane_header(buf, area, theme, meta.label, meta.description, 2);
    if area.height <= header_rows {
        return;
    }
    let choices_y = area.y + header_rows;
    let max_choices_h = area.height.saturating_sub(header_rows) as usize;
    if max_choices_h == 0 {
        return;
    }

    // ── Per-choice wrapped layout ─────────────────────────────────
    let layouts: Vec<PickerChoiceLayout> = choices
        .iter()
        .map(|choice| compute_picker_choice_layout(choice, area.width))
        .collect();
    let total_h: u16 = layouts.iter().map(|l| l.height).sum();

    // ── Scroll offset (variable per-choice height) ────────────────
    let needs_overflow = total_h as usize > max_choices_h;
    let available_h: u16 = if needs_overflow {
        (max_choices_h as u16).saturating_sub(1).max(1)
    } else {
        max_choices_h as u16
    };
    let scroll_offset = picker_scroll_offset(&layouts, choices_idx, available_h);

    let mut visible_end = scroll_offset;
    let mut consumed_h: u16 = 0;
    for (i, layout) in layouts.iter().enumerate().skip(scroll_offset) {
        let next = consumed_h.saturating_add(layout.height);
        if next > available_h {
            break;
        }
        consumed_h = next;
        visible_end = i + 1;
    }
    // Defensive: always show the focused choice even if it's clipped.
    if visible_end <= choices_idx {
        visible_end = choices_idx + 1;
    }
    let _ = consumed_h; // height bookkeeping kept for future tuning

    // ── Hit-rect bookkeeping ──────────────────────────────────────
    let mut picker_choice_rects: Vec<Rect> = vec![Rect::default(); choices.len()];

    // ── Choice rows ───────────────────────────────────────────────
    let fg_primary = theme.text_primary;
    let fg_gray = theme.gray;
    let fg_accent = theme.accent_user;

    let mut y_cursor = choices_y;
    for (choice_i, layout) in layouts
        .iter()
        .enumerate()
        .skip(scroll_offset)
        .take(visible_end - scroll_offset)
    {
        let choice = &choices[choice_i];
        let is_focused = choice_i == choices_idx;
        let is_current = committed_canonical.is_some_and(|c| c == choice.canonical);

        let is_hovered = !is_focused && state.hover_row == Some(choice_i);
        let bg = settings_list_row_bg(theme, is_focused, is_hovered);

        let display_style = if is_focused {
            Style::default()
                .fg(fg_primary)
                .bg(bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(fg_primary).bg(bg)
        };
        let desc_style = Style::default().fg(fg_gray).bg(bg);
        let marker_style = if is_current {
            Style::default().fg(fg_accent).bg(bg)
        } else {
            Style::default().fg(fg_gray).bg(bg)
        };
        let marker = if is_current {
            crate::glyphs::filled_dot()
        } else {
            "\u{25CB}"
        };

        let block_rect = Rect {
            x: area.x,
            y: y_cursor,
            width: area.width,
            height: layout.height,
        };
        buf.set_style(block_rect, Style::default().bg(bg));
        picker_choice_rects[choice_i] = block_rect;

        // ── Line 1: prefix + display + (· + first wrap line) ──────
        let y = y_cursor;
        if area.width > 0 {
            // Leading space (col 0 of the row).
            buf.set_span(
                area.x,
                y,
                &Span::styled(" ", display_style),
                1.min(area.width),
            );
        }
        if area.width > 1 {
            // Marker glyph at col 1.
            buf.set_span(
                area.x + 1,
                y,
                &Span::styled(marker, marker_style),
                PICKER_MARKER_W.min(area.width.saturating_sub(1)),
            );
        }
        if area.width > 2 {
            // Trailing two spaces at cols 2-3.
            let pad_w = 2u16.min(area.width.saturating_sub(2));
            buf.set_span(area.x + 2, y, &Span::styled("  ", display_style), pad_w);
        }

        // Display name (truncated).
        let display_x = area.x.saturating_add(PICKER_PREFIX_W);
        let display_room = (area.x + area.width).saturating_sub(display_x) as usize;
        if display_room == 0 {
            y_cursor = y_cursor.saturating_add(layout.height);
            continue;
        }
        let display_text: std::borrow::Cow<'_, str> = if choice.display.width() <= display_room {
            std::borrow::Cow::Borrowed(choice.display.as_str())
        } else {
            std::borrow::Cow::Owned(truncate_str(&choice.display, display_room))
        };
        let display_w =
            (display_text.width() as u16).min(area.width.saturating_sub(PICKER_PREFIX_W));
        buf.set_span(
            display_x,
            y,
            &Span::styled(display_text.as_ref(), display_style),
            display_w,
        );

        let has_choice_desc = !choice.description.trim().is_empty();
        if !has_choice_desc {
            y_cursor = y_cursor.saturating_add(layout.height);
            continue;
        }
        let after_display_x = display_x.saturating_add(display_w);
        let sep_room = (area.x + area.width).saturating_sub(after_display_x);
        if sep_room == 0 {
            y_cursor = y_cursor.saturating_add(layout.height);
            continue;
        }
        let sep_w = PICKER_SEPARATOR_W.min(sep_room);
        buf.set_span(
            after_display_x,
            y,
            &Span::styled(PICKER_SEPARATOR, desc_style),
            sep_w,
        );

        let desc_x = after_display_x + sep_w;
        let desc_room = (area.x + area.width).saturating_sub(desc_x) as usize;
        if desc_room == 0 {
            y_cursor = y_cursor.saturating_add(layout.height);
            continue;
        }

        // Narrow fallback: truncate on one line if wrapping fails.
        if layout.wrap_lines.is_empty() {
            let desc_text: std::borrow::Cow<'_, str> = if choice.description.width() <= desc_room {
                std::borrow::Cow::Borrowed(choice.description.as_str())
            } else {
                std::borrow::Cow::Owned(truncate_str(&choice.description, desc_room))
            };
            let desc_w = (desc_text.width() as u16).min(area.x + area.width - desc_x);
            buf.set_span(
                desc_x,
                y,
                &Span::styled(desc_text.as_ref(), desc_style),
                desc_w,
            );
            y_cursor = y_cursor.saturating_add(layout.height);
            continue;
        }

        // Line 1: first wrap line at the description column.
        let first_line = &layout.wrap_lines[0];
        let first_w = (first_line.width() as u16).min(area.x + area.width - desc_x);
        buf.set_span(
            desc_x,
            y,
            &Span::styled(first_line.as_str(), desc_style),
            first_w,
        );

        // Lines 2..N: continuation lines aligned under first_line.
        for (cont_i, wrap_line) in layout.wrap_lines.iter().enumerate().skip(1) {
            let cont_y = y + cont_i as u16;
            if cont_y >= area.y + area.height {
                break;
            }
            let cont_w = (wrap_line.width() as u16).min(area.x + area.width - desc_x);
            buf.set_span(
                desc_x,
                cont_y,
                &Span::styled(wrap_line.as_str(), desc_style),
                cont_w,
            );
        }

        y_cursor = y_cursor.saturating_add(layout.height);
    }

    // ── Overflow indicator: "… N more" on the row right below the
    //    last rendered choice. ─────────────────────────────────────
    if needs_overflow && visible_end < choices.len() {
        let more_count = choices.len() - visible_end;
        let overflow_y = y_cursor;
        if overflow_y < choices_y + max_choices_h as u16 && overflow_y < area.y + area.height {
            let overflow_style = Style::default().fg(theme.gray_dim).bg(theme.bg_base);
            let raw = format!("\u{2026} {more_count} more");
            let overflow_text: std::borrow::Cow<'_, str> = if raw.width() <= area.width as usize {
                std::borrow::Cow::Owned(raw)
            } else {
                std::borrow::Cow::Owned(truncate_str(&raw, area.width as usize))
            };
            let overflow_w = (overflow_text.width() as u16).min(area.width);
            let overflow_rect = Rect {
                x: area.x,
                y: overflow_y,
                width: area.width,
                height: 1,
            };
            buf.set_style(overflow_rect, Style::default().bg(theme.bg_base));
            buf.set_span(
                area.x,
                overflow_y,
                &Span::styled(overflow_text.as_ref(), overflow_style),
                overflow_w,
            );
        }
    }

    PICKER_RECTS_SCRATCH.with(|cell| {
        *cell.borrow_mut() = picker_choice_rects;
    });
    let _ = total_h; // suppress unused-var warning on some builds
}

// Thread-local scratch to ferry hit-rects out of `render_picking_enum`
// (which takes `&state`) into `state.picker_choice_rects`.
thread_local! {
    static PICKER_RECTS_SCRATCH: std::cell::RefCell<Vec<Rect>>
        = const { std::cell::RefCell::new(Vec::new()) };
}

/// Read-and-clear the most recent per-choice hit-rects produced by
/// `render_picking_enum`. Returns an empty Vec when called before
/// the first picker render (or after a non-picker frame reset the
/// scratch).
pub(super) fn take_picker_choice_rects() -> Vec<Rect> {
    PICKER_RECTS_SCRATCH.with(|cell| std::mem::take(&mut *cell.borrow_mut()))
}

/// Render the group sub-sheet: title + description + one row per child
/// (`Bool` → on/off, `DynamicEnum`/String → current value or "(no override)").
/// Returns the per-child hit-rects for mouse routing.
fn render_picking_group(
    buf: &mut Buffer,
    area: Rect,
    state: &SettingsModalState,
    theme: &Theme,
) -> Vec<Rect> {
    let (group_key, child_idx) = match &state.state.mode {
        SettingsMode::PickingGroup { key, child_idx } => (*key, *child_idx),
        _ => unreachable!("group renderer requires PickingGroup state"),
    };
    let Some(group_meta) = state.registry.find(group_key) else {
        return Vec::new();
    };
    let children = group_children(state, group_key);
    if area.width == 0 || area.height == 0 {
        return Vec::new();
    }

    // Chooser shape: title + gap (2) before the description renders.
    let header_rows = render_sub_pane_header(
        buf,
        area,
        theme,
        group_meta.label,
        group_meta.description,
        2,
    );
    if area.height <= header_rows {
        return Vec::new();
    }
    let mut y = area.y + header_rows;
    let area_end = area.y + area.height;

    // ── Child toggle rows. ────────────────────────────────────────
    let mut rects: Vec<Rect> = vec![Rect::default(); children.len()];
    for (i, child_key) in children.iter().enumerate() {
        if y >= area_end {
            break;
        }
        let Some(child_meta) = state.registry.find(child_key) else {
            continue;
        };
        let is_focused = i == child_idx;
        let is_hovered = !is_focused && state.hover_row == Some(i);
        let bg = settings_list_row_bg(theme, is_focused, is_hovered);
        let row_rect = Rect {
            x: area.x,
            y,
            width: area.width,
            height: 1,
        };
        buf.set_style(row_rect, Style::default().bg(bg));
        rects[i] = row_rect;

        let marker = if is_focused {
            crate::glyphs::filled_dot()
        } else {
            "\u{25CB}"
        };
        let marker_style = if is_focused {
            Style::default().fg(theme.accent_user).bg(bg)
        } else {
            Style::default().fg(theme.gray).bg(bg)
        };
        let label_style = if is_focused {
            Style::default()
                .fg(theme.text_primary)
                .bg(bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text_primary).bg(bg)
        };

        // Value read live from the snapshot (refreshed after each change).
        let (value_text, value_style) = match state.value_for(child_key) {
            Some(SettingValue::Bool(true)) => (
                "on".to_string(),
                Style::default().fg(theme.accent_user).bg(bg),
            ),
            Some(SettingValue::Bool(false)) => {
                ("off".to_string(), Style::default().fg(theme.gray).bg(bg))
            }
            Some(SettingValue::String(s)) if !s.is_empty() => {
                (s, Style::default().fg(theme.accent_user).bg(bg))
            }
            Some(SettingValue::String(_)) | None => (
                "(no override)".to_string(),
                Style::default().fg(theme.gray).bg(bg),
            ),
            other => (format!("{other:?}"), Style::default().fg(theme.gray).bg(bg)),
        };

        // " <marker>  <label> … <value> " (value right-aligned with a pad).
        buf.set_span(
            area.x,
            y,
            &Span::styled(" ", label_style),
            1.min(area.width),
        );
        if area.width > 1 {
            buf.set_span(
                area.x + 1,
                y,
                &Span::styled(marker, marker_style),
                PICKER_MARKER_W.min(area.width - 1),
            );
        }
        let label_x = area.x.saturating_add(PICKER_PREFIX_W);
        let value_w = value_text.as_str().width() as u16;
        let value_x = (area.x + area.width)
            .saturating_sub(value_w + 1)
            .max(label_x);
        if value_x > label_x {
            let label_room = (value_x - label_x).saturating_sub(1) as usize;
            let label_text: std::borrow::Cow<'_, str> = if child_meta.label.width() <= label_room {
                std::borrow::Cow::Borrowed(child_meta.label)
            } else {
                std::borrow::Cow::Owned(truncate_str(child_meta.label, label_room))
            };
            let label_w = (label_text.width() as u16).min((value_x - label_x).saturating_sub(1));
            buf.set_span(
                label_x,
                y,
                &Span::styled(label_text.as_ref(), label_style),
                label_w,
            );
        }
        if value_x + value_w <= area.x + area.width {
            buf.set_span(
                value_x,
                y,
                &Span::styled(value_text.as_str(), value_style),
                value_w,
            );
        }
        y = y.saturating_add(1);
    }
    rects
}

/// Layout metadata for one picker choice.
struct PickerChoiceLayout {
    height: u16,
    wrap_lines: Vec<String>,
}

/// Compute layout for one picker choice (height + wrapped desc lines).
fn compute_picker_choice_layout(choice: &OwnedEnumChoice, area_width: u16) -> PickerChoiceLayout {
    // No description → 1 line, symbol + display only.
    if choice.description.trim().is_empty() {
        return PickerChoiceLayout {
            height: 1,
            wrap_lines: Vec::new(),
        };
    }

    // Compute the desc column = PICKER_PREFIX_W + display_width + PICKER_SEPARATOR_W.
    // Display gets truncated if it'd overflow; mirror that for layout math.
    let display_room = (area_width as usize).saturating_sub(PICKER_PREFIX_W as usize);
    let display_w = choice.display.width().min(display_room) as u16;
    let after_display = PICKER_PREFIX_W.saturating_add(display_w);
    let after_sep = after_display.saturating_add(PICKER_SEPARATOR_W);

    if after_sep >= area_width {
        // No room for description — single-line fallback.
        return PickerChoiceLayout {
            height: 1,
            wrap_lines: Vec::new(),
        };
    }

    let desc_width = area_width.saturating_sub(after_sep) as usize;
    if desc_width == 0 {
        return PickerChoiceLayout {
            height: 1,
            wrap_lines: Vec::new(),
        };
    }

    let line = Line::from(Span::raw(choice.description.as_str()));
    let wrapped = crate::render::wrapping::word_wrap_line(&line, desc_width);

    let wrap_lines: Vec<String> = wrapped
        .into_iter()
        .map(|l| {
            l.spans
                .into_iter()
                .map(|s| s.content.into_owned())
                .collect::<String>()
        })
        .collect();

    // Defensive: treat empty wrap result as no-wrap.
    if wrap_lines.is_empty() {
        return PickerChoiceLayout {
            height: 1,
            wrap_lines: Vec::new(),
        };
    }

    PickerChoiceLayout {
        height: wrap_lines.len() as u16,
        wrap_lines,
    }
}

/// Smallest scroll offset that keeps the focused choice fully visible.
fn picker_scroll_offset(
    layouts: &[PickerChoiceLayout],
    choices_idx: usize,
    available_h: u16,
) -> usize {
    if layouts.is_empty() || available_h == 0 {
        return 0;
    }
    let n = layouts.len();
    let mut offset = 0usize;
    loop {
        // Sum heights starting at `offset` until adding the next
        // choice would exceed `available_h`.
        let mut consumed: u16 = 0;
        let mut last_visible = offset;
        for (i, layout) in layouts.iter().enumerate().skip(offset) {
            let next = consumed.saturating_add(layout.height);
            if next > available_h {
                break;
            }
            consumed = next;
            last_visible = i + 1;
        }
        // `last_visible` is the exclusive upper bound. If
        // `choices_idx` is in `[offset, last_visible)`, this offset
        // works.
        if choices_idx < last_visible {
            return offset;
        }
        // Otherwise advance. Stop when we've exhausted the list
        // (defensive — shouldn't happen since `choices_idx < n`).
        if offset + 1 >= n {
            return offset;
        }
        offset += 1;
    }
}

/// Min width to draw the Int stepper's `‹`/`›` adornments.
const INT_STEPPER_ADORNMENT_MIN_WIDTH: u16 = 8;

/// Wide-range Int stepper defaults (span > 100): Up/Down small, Left/Right large.
const INT_STEPPER_WIDE_SMALL_STEP: i64 = 5;
const INT_STEPPER_WIDE_LARGE_STEP: i64 = 10;

/// Derive (small, large) step sizes from an Int setting's `[min, max]` span.
/// Narrow dials use unit fine-steps so every in-range value is reachable;
/// wide ranges keep the original ±5 / ±10 feel.
pub(super) fn int_step_sizes(min: i64, max: i64) -> (i64, i64) {
    let span = max.saturating_sub(min).max(0);
    if span <= 20 {
        // scroll_lines 1..=10 (span 9): unit steps on both small and large.
        (1, (span / 5).max(1))
    } else if span <= 100 {
        // scroll_speed 1..=100 (span 99): unit fine, ±5 coarse.
        (1, 5)
    } else {
        // max_thoughts_width 40..=500 (span 460).
        (INT_STEPPER_WIDE_SMALL_STEP, INT_STEPPER_WIDE_LARGE_STEP)
    }
}

/// Footer labels for the Int stepper (must be `'static` for `Shortcut`).
fn int_step_footer_labels(min: i64, max: i64) -> (&'static str, &'static str) {
    let (small, large) = int_step_sizes(min, max);
    match (small, large) {
        (1, 1) => ("\u{2191}/\u{2193} +/-1", "\u{2190}/\u{2192} +/-1"),
        (1, 5) => ("\u{2191}/\u{2193} +/-1", "\u{2190}/\u{2192} +/-5"),
        (5, 10) => ("\u{2191}/\u{2193} +/-5", "\u{2190}/\u{2192} +/-10"),
        // Defensive fallback if thresholds change without new static pairs.
        (1, _) => ("\u{2191}/\u{2193} +/-1", "\u{2190}/\u{2192} step"),
        (5, _) => ("\u{2191}/\u{2193} +/-5", "\u{2190}/\u{2192} step"),
        _ => ("\u{2191}/\u{2193} step", "\u{2190}/\u{2192} step"),
    }
}

// ‹ / › (U+2039 / U+203A) — fall back to ASCII `<` / `>` on legacy ConHost.
pub(super) fn int_stepper_left_glyph() -> &'static str {
    crate::glyphs::chevron_left()
}

fn int_stepper_right_glyph() -> &'static str {
    crate::glyphs::chevron()
}

/// Sample text for the `max_thoughts_width` live wrap preview.
const MAX_THOUGHTS_WIDTH_PREVIEW_SAMPLE: &str = "Let me trace through the call sites. First, \
    I'll need to look at how the dispatch flow handles the new variant. Then I'll verify the \
    rollback path preserves the previous state correctly.";

/// Min width to render the wrap preview.
pub(super) const MAX_THOUGHTS_WIDTH_PREVIEW_MIN_WIDTH: u16 = 30;

/// Min remaining height to render the wrap preview below the stepper.
pub(super) const MAX_THOUGHTS_WIDTH_PREVIEW_MIN_HEIGHT: u16 = 5;

/// Render the inline editor. Int settings use a stepper; String
/// settings use a text input with cursor and validation feedback.
pub(super) fn render_editing_value(
    buf: &mut Buffer,
    area: Rect,
    state: &mut SettingsModalState,
    theme: &Theme,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    state.editor_adornment_rects = (Rect::default(), Rect::default());

    if let SettingsMode::EditingInt {
        key: setting_key,
        buffer,
        ..
    } = &state.state.mode
    {
        let setting_key = *setting_key;
        let buffer = buffer.clone();
        let Some(meta) = state.registry.find(setting_key) else {
            return;
        };
        // Snapshot meta fields to release registry borrow.
        let label = meta.label;
        let description = meta.description;
        render_int_stepper(
            buf,
            area,
            state,
            setting_key,
            label,
            description,
            &buffer,
            theme,
        );
        return;
    }

    let SettingsMode::EditingString {
        key: setting_key,
        editor,
        validation_error,
        ..
    } = &state.state.mode
    else {
        unreachable!("editor renderer requires String or Int state");
    };
    let setting_key = *setting_key;
    let buffer = editor.text();
    let validation_error = validation_error.as_deref();
    let Some(meta) = state.registry.find(setting_key) else {
        return;
    };

    // Editors reserve title + gap + the input row (3) before the description.
    let header_rows = render_sub_pane_header(buf, area, theme, meta.label, meta.description, 3);
    if area.height <= header_rows {
        return;
    }
    let input_y = area.y + header_rows;

    // ── Row 3: input line. ────────────────────────────────────────
    let has_error = validation_error.is_some();
    let input_bg = theme.bg_visual;
    let input_fg = if has_error {
        theme.accent_error
    } else {
        theme.text_primary
    };
    let cursor_style = Style::default().fg(theme.accent_user).bg(input_bg);
    let input_style = Style::default().fg(input_fg).bg(input_bg);

    let input_row_rect = Rect {
        x: area.x,
        y: input_y,
        width: area.width,
        height: 1,
    };
    buf.set_style(input_row_rect, Style::default().bg(theme.bg_base));

    let input_x = area.x;
    let buffer_room_end_x = area.x + area.width;
    let buffer_room = buffer_room_end_x.saturating_sub(input_x) as usize;
    if buffer_room == 0 {
        return; // No room to render the buffer.
    }

    let input_strip_rect = Rect {
        x: input_x,
        y: input_y,
        width: buffer_room as u16,
        height: 1,
    };
    buf.set_style(input_strip_rect, Style::default().bg(input_bg));

    let cursor_reserve = 1usize;
    let visible_buffer_w = buffer_room.saturating_sub(cursor_reserve);

    // Empty-buffer placeholder.
    if buffer.is_empty() {
        let placeholder = match &meta.kind {
            SettingKind::String { validator, .. } => match validator {
                StringValidator::KnownModel => "<empty — use shell default>",
                StringValidator::PromptCursor => {
                    "<native, block, underline, bar, or one character>"
                }
                StringValidator::NonEmptyToken => "<type a value>",
                StringValidator::Any => "<type a value>",
            },
            _ => "",
        };
        if !placeholder.is_empty() && visible_buffer_w > 0 {
            let placeholder_text: std::borrow::Cow<'_, str> =
                if placeholder.width() <= visible_buffer_w {
                    std::borrow::Cow::Borrowed(placeholder)
                } else {
                    std::borrow::Cow::Owned(truncate_str(placeholder, visible_buffer_w))
                };
            let placeholder_w = (placeholder_text.width() as u16).min(visible_buffer_w as u16);
            let placeholder_style = Style::default().fg(theme.gray_dim).bg(input_bg);
            buf.set_span(
                input_x,
                input_y,
                &Span::styled(placeholder_text.as_ref(), placeholder_style),
                placeholder_w,
            );
        }
        // Render the cursor at the start.
        let cursor_x = input_x;
        buf.set_span(
            cursor_x,
            input_y,
            &Span::styled(crate::glyphs::selection_bar(), cursor_style),
            1,
        );
    } else {
        let viewport = editor.viewport(buffer_room);
        let visible = &buffer[viewport.visible_byte_range];
        let visible_width = (visible.width() as u16).min(buffer_room as u16);
        buf.set_span(
            input_x,
            input_y,
            &Span::styled(visible, input_style),
            visible_width,
        );

        let cursor_x =
            input_x + (viewport.cursor_display_column as u16).min(buffer_room as u16 - 1);
        buf.set_span(
            cursor_x,
            input_y,
            &Span::styled(crate::glyphs::selection_bar(), cursor_style),
            1,
        );
    }

    // ── Row 4: validation error. ──────────────────────────────────
    if area.height > header_rows + 1
        && let Some(err) = validation_error
    {
        let err_y = input_y + 1;
        let err_style = Style::default().fg(theme.accent_error).bg(theme.bg_base);
        let err_text: std::borrow::Cow<'_, str> = if err.width() <= area.width as usize {
            std::borrow::Cow::Borrowed(err)
        } else {
            std::borrow::Cow::Owned(truncate_str(err, area.width as usize))
        };
        let err_w = (err_text.width() as u16).min(area.width);
        buf.set_span(
            area.x,
            err_y,
            &Span::styled(err_text.as_ref(), err_style),
            err_w,
        );
    }
}

/// Render the Int stepper: title + description + centered `‹ N ›`.
/// Populates `editor_adornment_rects` for mouse click targets.
#[allow(clippy::too_many_arguments)]
fn render_int_stepper(
    buf: &mut Buffer,
    area: Rect,
    state: &mut SettingsModalState,
    setting_key: SettingKey,
    label: &'static str,
    description: &'static str,
    buffer: &str,
    theme: &Theme,
) {
    // Editors reserve title + gap + the stepper row (3) before the description.
    let header_rows = render_sub_pane_header(buf, area, theme, label, description, 3);
    if area.height <= header_rows {
        return;
    }
    let stepper_y = area.y + header_rows;

    // ── Row 3: centered stepper "‹  N  ›". ────────────────────────
    let value_text = if buffer.is_empty() {
        // Defensive — try_enter_editing_value seeds buffer from the
        // current value, so this branch should be unreachable, but
        // a blank cell would be confusing if a future refactor
        // dropped the seed.
        "-".to_string()
    } else {
        buffer.to_string()
    };
    let value_style = Style::default()
        .fg(theme.accent_user)
        .bg(theme.bg_base)
        .add_modifier(Modifier::BOLD);
    let arrow_style = Style::default().fg(theme.accent_user).bg(theme.bg_base);

    let left_w = int_stepper_left_glyph().width() as u16;
    let right_w = int_stepper_right_glyph().width() as u16;
    let value_w = value_text.width() as u16;
    // Layout: "‹  N  ›" — 2 cells between each glyph for breathing
    // room. Total width = left + 2 + value + 2 + right.
    let inter_pad: u16 = 2;
    let total_w = left_w + inter_pad + value_w + inter_pad + right_w;
    let render_arrows = area.width >= INT_STEPPER_ADORNMENT_MIN_WIDTH;

    if render_arrows && total_w <= area.width {
        // Center the full layout.
        let stepper_x = area.x + (area.width - total_w) / 2;
        let left_x = stepper_x;
        let value_x = left_x + left_w + inter_pad;
        let right_x = value_x + value_w + inter_pad;

        buf.set_span(
            left_x,
            stepper_y,
            &Span::styled(int_stepper_left_glyph(), arrow_style),
            left_w,
        );
        buf.set_span(
            value_x,
            stepper_y,
            &Span::styled(value_text.as_str(), value_style),
            value_w,
        );
        buf.set_span(
            right_x,
            stepper_y,
            &Span::styled(int_stepper_right_glyph(), arrow_style),
            right_w,
        );

        state.editor_adornment_rects = (
            Rect {
                x: left_x,
                y: stepper_y,
                width: left_w,
                height: 1,
            },
            Rect {
                x: right_x,
                y: stepper_y,
                width: right_w,
                height: 1,
            },
        );
    } else {
        // Too narrow for arrows — render the value alone, centered.
        let v_w = value_w.min(area.width);
        let value_x = area.x + (area.width - v_w) / 2;
        buf.set_span(
            value_x,
            stepper_y,
            &Span::styled(value_text.as_str(), value_style),
            v_w,
        );
    }

    // **In-pane hint dropped.** Earlier revisions
    // rendered a centered `↑/↓ +/-5   ←/→ +/-10   Enter commit · Esc
    // cancel` strip here, but the chrome footer's
    // `build_int_editor_shortcuts` already exposes the same content
    // at the bottom of the modal. On tall viewports both rendered
    // simultaneously — same keys, different separator (`·` vs `|`),
    // duplicate visual noise. We rely on the chrome footer alone
    // now; if the chrome ever fails to render its shortcut row (a
    // future regression), the user can still discover the keys via
    // the shortcuts cheatsheet (`?`).

    // ── Live wrap preview for max_thoughts_width. ─────────────────
    //
    // When the user is stepping `max_thoughts_width`, render a
    // sample thinking-text preview directly below the stepper that
    // wraps live at the current pending value. The preview sits in
    // the rows immediately after the stepper (1 blank row + title +
    // N content rows); any rows below the last content row of the
    // preview stay blank — the chrome footer sits below
    // `inner_area`, not inside `area`.
    //
    // Gated on `setting_key == MAX_THOUGHTS_WIDTH_KEY` so future
    // Int settings don't inherit the preview behavior implicitly.
    // The string equality is sufficient because key uniqueness is
    // enforced at registry-load time (see
    // `SettingsRegistry::defaults` / `::from_entries`).
    if setting_key == crate::settings::defs::MAX_THOUGHTS_WIDTH_KEY {
        let stepper_end_y = stepper_y.saturating_add(1);
        let area_end_y = area.y.saturating_add(area.height);
        let preview_h = area_end_y.saturating_sub(stepper_end_y);
        if preview_h >= MAX_THOUGHTS_WIDTH_PREVIEW_MIN_HEIGHT {
            let preview_area = Rect {
                x: area.x,
                y: stepper_end_y,
                width: area.width,
                height: preview_h,
            };
            let pending_value = parse_max_thoughts_width_buffer(buffer);
            render_max_thoughts_width_preview(buf, preview_area, pending_value, theme);
        }
    }
}

/// Parse the Int stepper's buffer back into a `u16` clamped to the
/// `max_thoughts_width` registered bounds. Defensive — the stepper's
/// step path keeps the buffer in range, but a synthetic test fixture
/// or a future code path could seed an out-of-range buffer.
///
/// Both `MIN = 40` and `MAX = 500` fit inside `u16`, so the `clamp`
/// result is always non-negative and ≤ `u16::MAX`. We use
/// `u16::try_from(...).unwrap_or(u16::MAX)` instead of `as u16` so
/// a future bump to `MAX > u16::MAX` saturates rather than silently
/// truncating mod 65536 (security suggestion).
fn parse_max_thoughts_width_buffer(buffer: &str) -> u16 {
    let clamped = buffer
        .parse::<i64>()
        .unwrap_or(crate::settings::defs::MAX_THOUGHTS_WIDTH_MIN)
        .clamp(
            crate::settings::defs::MAX_THOUGHTS_WIDTH_MIN,
            crate::settings::defs::MAX_THOUGHTS_WIDTH_MAX,
        );
    u16::try_from(clamped).unwrap_or(u16::MAX)
}

/// Render the `max_thoughts_width` live wrap preview block.
///
/// **Vertical layout inside `area`.** Top-anchored. Row 0 of `area`
/// is a 1-row blank gap separating the preview from whatever sits
/// directly above (in the live caller, the stepper row); row 1 is
/// the title; rows 2..(2+content_rows) hold the wrapped content.
/// When `pending_value > area.width` (the preview is clamped) and
/// there are at least two rows of vertical slack below the
/// content, a 1-row blank gap and then a note row carry the text
/// `note: clamped at N cols`. Any rows below the note
/// stay blank — that empty space is intentional and stays
/// unpainted (the chrome footer sits below `inner_area`).
///
/// **Edge cases (per spec).**
/// - `area.width < MAX_THOUGHTS_WIDTH_PREVIEW_MIN_WIDTH` (30): omit
///   the preview entirely — too narrow for readable wrapped text.
/// - `area.height < MAX_THOUGHTS_WIDTH_PREVIEW_MIN_HEIGHT` (5):
///   omit — insufficient vertical budget for the gap + title +
///   2 content rows layout.
/// - `pending_value > area.width`: clamp the preview width to
///   `area.width`. The title stays plain `preview`; the clamp
///   amount is surfaced via a `note: clamped at N cols` row
///   rendered below the content when there's a row of slack
///   below it. Content takes priority over the note — if there's
///   no room for the note row, it's silently omitted.
/// - Active setting key gating happens at the call site
///   (`setting_key == "max_thoughts_width"`); this helper is pure
///   on the `pending_value` it receives.
///
/// **Theme tokens.**
/// - Title bg: `theme.bg_visual` — the heavier / more saturated of
///   the two "block" bg tokens; matches selection-bg saturation.
/// - Content bg: `theme.bg_highlight` — the lighter / less
///   saturated of the two; still distinguishable from
///   `theme.bg_base` so the preview reads as a contained block.
/// - Title fg + content fg: `theme.text_primary` — same color the
///   scrollback's thinking output renders in. Italic + bold +
///   underlined for the title (the UNDERLINED gives consistent
///   additional visual weight on themes where `bg_visual` vs
///   `bg_highlight` is mostly a hue shift rather than a luma shift,
///   e.g. TokyoNight); italic only for content.
fn render_max_thoughts_width_preview(
    buf: &mut Buffer,
    area: Rect,
    pending_value: u16,
    theme: &Theme,
) {
    // Edge case 1: terminal area too narrow → omit.
    if area.width < MAX_THOUGHTS_WIDTH_PREVIEW_MIN_WIDTH {
        return;
    }
    // Edge case 2: terminal area too short → omit.
    if area.height < MAX_THOUGHTS_WIDTH_PREVIEW_MIN_HEIGHT {
        return;
    }
    // Defensive guard: catch future editors who add `\n` / `\t` (or
    // any other control char that bypasses word_wrap_line's flow)
    // to the sample. `wrap_description` has the same debug_assert
    // for the same reason.
    debug_assert!(
        !MAX_THOUGHTS_WIDTH_PREVIEW_SAMPLE.contains('\n')
            && !MAX_THOUGHTS_WIDTH_PREVIEW_SAMPLE.contains('\t'),
        "MAX_THOUGHTS_WIDTH_PREVIEW_SAMPLE must not contain `\\n` or `\\t`; \
         word_wrap_line flattens spans byte-for-byte and would render control \
         cells as glyphs",
    );

    // Effective preview content width = min(pending, available).
    let pending_w = pending_value.max(1);
    let effective_width = pending_w.min(area.width);
    let clamped = pending_w > area.width;

    // Wrap the sample text at the effective width.
    let sample_line = Line::from(Span::raw(MAX_THOUGHTS_WIDTH_PREVIEW_SAMPLE));
    let wrapped = crate::render::wrapping::word_wrap_line(&sample_line, effective_width as usize);
    // Defensive: a degenerate wrap (zero lines) means we have no
    // meaningful preview to show. The MIN_WIDTH=30 gate above makes
    // this practically unreachable.
    if wrapped.is_empty() {
        return;
    }
    // Layout budget: 1 row blank gap (above title) + 1 row title +
    // N content rows. Cap N at the available rows; the rest of
    // `area` (below the last content row) stays blank. The
    // MIN_HEIGHT=5 gate above guarantees `area.height >= 5`, so
    // `available_content_rows >= 3` here — always enough room for
    // the minimum 2 content rows. We cap at `wrapped.len()` so a
    // short wrap doesn't leave us painting beyond the wrap shape.
    let available_content_rows = area.height.saturating_sub(2) as usize;
    let visible_content = wrapped.len().min(available_content_rows);
    render_preview_block(
        buf,
        area,
        effective_width,
        clamped,
        &wrapped[..visible_content],
        theme,
    );
}

/// Inner painter for the preview block — split out so the
/// caller's edge-case dispatch (omit / truncate / full-fit) stays
/// readable and the rendering logic isn't duplicated.
///
/// Caller guarantees:
/// - `effective_width <= area.width`.
/// - `wrapped.len() >= 1` AND `wrapped.len() + 2 <= area.height`
///   (1 row gap above + 1 row title + `wrapped.len()` content rows
///   must all fit inside `area`).
/// - `area.width >= MAX_THOUGHTS_WIDTH_PREVIEW_MIN_WIDTH`.
///
/// Clamped-state surfacing: when `clamped` is true and there are
/// at least 2 rows of vertical slack below the content (i.e.
/// `area.height >= wrapped.len() + 4`), a 1-row blank gap and
/// then a `note: clamped at N cols` row are rendered below the
/// last content row
/// in `theme.text_secondary` with no modifier and on
/// `theme.bg_base` (no block-tinted bg — the note lives in the
/// chrome strip below the preview, not inside the two-tone
/// preview block). When the slack is unavailable, the note is
/// silently omitted; content takes priority.
fn render_preview_block(
    buf: &mut Buffer,
    area: Rect,
    effective_width: u16,
    clamped: bool,
    wrapped: &[Line<'_>],
    theme: &Theme,
) {
    debug_assert!(
        area.height >= (wrapped.len() as u16).saturating_add(2),
        "render_preview_block caller-guarantee violated: \
         area.height={} < wrapped.len()+2={}",
        area.height,
        wrapped.len() + 2,
    );
    // Top-anchor: row 0 of `area` is a blank gap, row 1 holds the
    // title, rows 2..(2+content_rows) hold the wrapped content.
    // Any rows below the last content row stay blank, except for
    // the optional clamped-note row described at the bottom of
    // this function.
    let title_y = area.y.saturating_add(1);

    // ── Title row. ────────────────────────────────────────────────
    let title_bg = theme.bg_visual;
    let content_bg = theme.bg_highlight;
    let title_fg = theme.text_primary;
    let content_fg = theme.text_primary;

    // Paint title bg first so partial-trailing-whitespace stays
    // tinted (the bg extends to the FULL effective_width on every
    // row, including any title columns past the text).
    let title_rect = Rect {
        x: area.x,
        y: title_y,
        width: effective_width,
        height: 1,
    };
    buf.set_style(title_rect, Style::default().bg(title_bg));

    // Title is always plain lowercase `preview`. The previous
    // implementation appended ` · clamped to N cols` to the title
    // when the preview clamped to a narrower terminal width; the
    // clamp signal has been moved to a note row below the content
    // (see the bottom of this function) so the title carries the
    // same shape regardless of clamp state.
    let title_text: &str = "preview";
    let title_text_truncated: std::borrow::Cow<'_, str> =
        if title_text.width() <= effective_width as usize {
            std::borrow::Cow::Borrowed(title_text)
        } else {
            std::borrow::Cow::Owned(truncate_str(title_text, effective_width as usize))
        };
    let title_w = (title_text_truncated.width() as u16).min(effective_width);
    // BOLD + ITALIC + UNDERLINED on the title. UNDERLINED gives
    // additional visual weight independent of the bg luma — on
    // TokyoNight `bg_visual` vs `bg_highlight` is
    // mostly a hue shift, not a luma shift, so the underline
    // carries the "this is the title" cue on its own.
    let title_style = Style::default()
        .fg(title_fg)
        .bg(title_bg)
        .add_modifier(Modifier::BOLD | Modifier::ITALIC | Modifier::UNDERLINED);
    buf.set_span(
        area.x,
        title_y,
        &Span::styled(title_text_truncated.as_ref(), title_style),
        title_w,
    );

    // ── Content rows. ─────────────────────────────────────────────
    let content_style = Style::default()
        .fg(content_fg)
        .bg(content_bg)
        .add_modifier(Modifier::ITALIC);
    for (i, wrap_line) in wrapped.iter().enumerate() {
        let row_y = title_y + 1 + i as u16;
        // Paint bg first across `effective_width` so trailing
        // whitespace on a wrap line is still tinted.
        let row_rect = Rect {
            x: area.x,
            y: row_y,
            width: effective_width,
            height: 1,
        };
        buf.set_style(row_rect, Style::default().bg(content_bg));
        // Flatten the wrapped line's spans back to a plain string
        // (the sample text has no inline styles, so we don't lose
        // any styling). Then re-style with our italic + content_fg.
        let text: String = wrap_line
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>();
        let text_w = (text.width() as u16).min(effective_width);
        if text_w > 0 {
            buf.set_span(area.x, row_y, &Span::styled(text, content_style), text_w);
        }
    }

    // ── Clamped note (optional, height-permitting). ───────────────
    //
    // When `clamped`, surface the clamp in a low-key note row
    // immediately below the last content row. The note is
    // height-aware: content takes priority, so if there's no row
    // of slack below the content we omit the note entirely. The
    // note sits OUTSIDE the two-tone preview bg block — it uses
    // `theme.bg_base` so it visually reads as chrome/tip text,
    // not as part of the wrap preview. Left-aligned at the same
    // x-offset as content (`area.x`).
    if clamped {
        // One blank row sits between the last content row and the
        // note so the note reads as a separate annotation, not a
        // continuation of the wrap preview.
        let note_y = title_y
            .saturating_add(1)
            .saturating_add(wrapped.len() as u16)
            .saturating_add(1);
        let area_end_y = area.y.saturating_add(area.height);
        if note_y < area_end_y {
            let note_text = format!("note: clamped at {effective_width} cols");
            let note_text_truncated: std::borrow::Cow<'_, str> =
                if note_text.width() <= area.width as usize {
                    std::borrow::Cow::Borrowed(note_text.as_str())
                } else {
                    std::borrow::Cow::Owned(truncate_str(&note_text, area.width as usize))
                };
            let note_w = (note_text_truncated.width() as u16).min(area.width);
            // No modifier, no bg tint — the note reads as chrome
            // text aligned with the preview's left edge.
            let note_style = Style::default().fg(theme.text_secondary).bg(theme.bg_base);
            buf.set_span(
                area.x,
                note_y,
                &Span::styled(note_text_truncated.as_ref(), note_style),
                note_w,
            );
        }
    }
}

/// Look up the user-friendly display string for an Enum canonical
/// against the setting's own `EnumChoice` catalog. Falls back to the
/// canonical verbatim if the lookup misses (defense-in-depth: a
/// hand-edited corrupted config with an unknown canonical still
/// renders without an empty string, mirroring
/// `display_name_for_canonical`'s pattern).
///
/// Look up the display name for an Enum canonical via the registry.
fn display_for_enum_canonical<'a>(kind: &'a SettingKind, canonical: &'a str) -> &'a str {
    if let SettingKind::Enum { choices, .. } = kind {
        for c in *choices {
            if c.canonical == canonical {
                return c.display;
            }
        }
    }
    // Fallback: render the canonical verbatim. Defensive — catches a
    // schema-vs-renderer drift without crashing the modal.
    canonical
}

/// Word-wrap a description string. Returns owned lines for re-styling.
/// Asserts descriptions are single-line (no `\n`/`\t`).
pub(super) fn wrap_description(description: &str, width: u16) -> Vec<String> {
    if description.is_empty() || width == 0 {
        return Vec::new();
    }
    debug_assert!(
        !description.contains('\n') && !description.contains('\t'),
        "SettingMeta::description is single-line / no tabs by contract: \
         description={description:?}. Word-wrap doesn't split on \\n or \\t — \
         such chars would render as control codes in a buffer cell. \
         Pre-split + iterate if multi-line descriptions become useful.",
    );
    let line = Line::from(Span::raw(description));
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

// Row chrome: a 2-cell cursor gutter on the left, the label column, then the
// value right-aligned against a reserved chevron column.

/// Cursor gutter — `› ` on the focused row, blank otherwise.
pub(super) const ROW_CURSOR_W: u16 = 2;
pub(super) const ROW_RIGHT_PAD_W: u16 = 1;
/// Chevron column width — reserved for all rows for alignment.
pub(super) const ROW_CHEVRON_COL_W: u16 = 2;
/// Minimum blank columns between the label and the value.
const ROW_GAP_W: u16 = 2;
/// Appended to the value column of a locked row (see `SettingsModalState::row_lock`).
pub(super) const ROW_ADMIN_MANAGED_SUFFIX: &str = " \u{00B7} Admin Managed";
/// Value column for ZDR-locked rows — replaces the opt-in/out value entirely.
pub(super) const ROW_ZDR_VALUE: &str = "ZDR";

/// Value-column text, shared by layout, scroll math, and paint.
pub(super) fn value_display(
    meta: &SettingMeta,
    value: &SettingValue,
    lock: Option<CodingDataSharingLock>,
) -> String {
    if lock == Some(CodingDataSharingLock::Zdr) {
        return ROW_ZDR_VALUE.to_string();
    }
    let mut display = match value {
        SettingValue::Bool(b) => if *b { "on" } else { "off" }.to_string(),
        SettingValue::String(s) => {
            if s.is_empty() && matches!(meta.kind, SettingKind::DynamicEnum { .. }) {
                "(no override)".to_string()
            } else {
                s.clone()
            }
        }
        SettingValue::Enum(e) => display_for_enum_canonical(&meta.kind, e).to_string(),
        SettingValue::Int(i) => i.to_string(),
        SettingValue::PiBuiltinTools(_) => "Pi built-in tools".to_string(),
    };
    if lock == Some(CodingDataSharingLock::TeamManaged) {
        display.push_str(ROW_ADMIN_MANAGED_SUFFIX);
    }
    display
}

/// Terminal-native themes collapse selection tokens to `Reset`; use ANSI
/// `DarkGray` (not silver `Gray`, which washes out default fg on dark profiles).
pub(super) fn settings_list_row_bg(theme: &Theme, is_selected: bool, is_hovered: bool) -> Color {
    if crate::theme::cache::terminal_native_locked() || matches!(theme.bg_visual, Color::Reset) {
        return if is_selected || is_hovered {
            Color::DarkGray
        } else {
            Color::Reset
        };
    }
    if is_selected {
        theme.bg_visual
    } else if is_hovered {
        theme.bg_hover
    } else {
        theme.bg_base
    }
}

/// Visual state of a settings row for one frame.
#[derive(Debug, Clone, Copy)]
pub(super) struct RowStyle {
    /// Keyboard focus is on this row.
    pub selected: bool,
    /// Pointer is over this row.
    pub hovered: bool,
    /// Row belongs to a section other than the one under the cursor.
    pub dimmed: bool,
}

/// Render a tab or section heading row.
fn render_heading_row(buf: &mut Buffer, area: Rect, label: &str, dimmed: bool, theme: &Theme) {
    buf.set_style(area, Style::default().bg(theme.bg_base));
    let style = Style::default()
        .fg(if dimmed { theme.gray_dim } else { theme.gray })
        .bg(theme.bg_base)
        .add_modifier(Modifier::BOLD);
    let text = truncate_str(label, area.width.saturating_sub(ROW_CURSOR_W) as usize);
    let w = (text.width() as u16).min(area.width);
    buf.set_span(
        area.x + ROW_CURSOR_W.min(area.width),
        area.y,
        &Span::styled(&text, style),
        w,
    );
}

/// Render one setting row: cursor gutter, padded label column, then the value
/// right-aligned against the reserved chevron column. Always exactly one line.
/// Returns the value column's hit-rect.
pub(super) fn render_setting_row(
    buf: &mut Buffer,
    area: Rect,
    meta: &SettingMeta,
    value: Option<&SettingValue>,
    max_label_w: u16,
    row: RowStyle,
    lock: Option<CodingDataSharingLock>,
    theme: &Theme,
) -> Rect {
    let bg = settings_list_row_bg(theme, row.selected, row.hovered);
    buf.set_style(area, Style::default().bg(bg));

    // A row with no value carrier means registry/dispatch skew. Surface it in
    // the value column rather than silently rendering a blank.
    let Some(value) = value else {
        return render_setting_row_unmapped(buf, area, meta, max_label_w, bg, theme);
    };
    let value_text = value_display(meta, value, lock);

    // Rows outside the active section collapse to one flat wash so inner
    // label/value colors do not fight the dimming.
    let (label_style, value_style) = if row.dimmed {
        let dim = Style::default().fg(theme.gray_dim).bg(bg);
        (dim, dim)
    } else {
        let mut label = Style::default().fg(theme.text_primary).bg(bg);
        if row.selected {
            label = label.add_modifier(Modifier::BOLD);
        }
        // Off and locked values recede; everything else carries the accent.
        let value = if lock.is_some() || matches!(value, SettingValue::Bool(false)) {
            Style::default().fg(theme.gray).bg(bg)
        } else {
            Style::default().fg(theme.accent_user).bg(bg)
        };
        (label, value)
    };

    // Chevron marks rows that open a sub-pane. Locked rows cannot be entered,
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

    // ── Cursor gutter ────────────────────────────────────────────────────
    let cursor = if row.selected {
        format!("{} ", crate::glyphs::chevron())
    } else {
        " ".repeat(ROW_CURSOR_W as usize)
    };
    let cursor_style = Style::default().fg(theme.accent_user).bg(bg);
    buf.set_span(
        area.x,
        area.y,
        &Span::styled(&cursor, cursor_style),
        ROW_CURSOR_W.min(area.width),
    );

    // ── Label column ─────────────────────────────────────────────────────
    let label_x = area.x + ROW_CURSOR_W.min(area.width);
    let label_avail = area.width.saturating_sub(ROW_CURSOR_W);
    let label = truncate_str(meta.label, max_label_w.min(label_avail) as usize);
    let label_w = (label.width() as u16).min(label_avail);
    if label_w > 0 {
        buf.set_span(label_x, area.y, &Span::styled(&label, label_style), label_w);
    }

    // ── Value column, right-aligned against the chevron gutter ───────────
    let chevron_x = (area.x + area.width).saturating_sub(ROW_RIGHT_PAD_W + ROW_CHEVRON_COL_W);
    let value_left_bound = label_x + max_label_w.min(label_avail) + ROW_GAP_W;
    let value_avail = chevron_x.saturating_sub(value_left_bound);
    let value_text = truncate_str(&value_text, value_avail as usize);
    let value_w = (value_text.width() as u16).min(value_avail);
    let value_x = chevron_x.saturating_sub(value_w);
    if value_w > 0 {
        buf.set_span(
            value_x,
            area.y,
            &Span::styled(&value_text, value_style),
            value_w,
        );
    }
    if show_chevron {
        let chevron = format!(" {}", crate::glyphs::chevron());
        let style = Style::default()
            .fg(if row.dimmed {
                theme.gray_dim
            } else {
                theme.gray
            })
            .bg(bg);
        buf.set_span(
            chevron_x,
            area.y,
            &Span::styled(&chevron, style),
            ROW_CHEVRON_COL_W,
        );
    }

    Rect {
        x: value_x,
        y: area.y,
        width: value_w.saturating_add(ROW_CHEVRON_COL_W),
        height: 1,
    }
}

/// Fallback for a row whose `current_value_for` returned `None` (registry /
/// dispatch skew). Renders the label in the error color so the
/// misconfiguration is visible at runtime; `every_setting_has_dispatch_arm`
/// catches the case at CI time.
fn render_setting_row_unmapped(
    buf: &mut Buffer,
    area: Rect,
    meta: &SettingMeta,
    max_label_w: u16,
    bg: Color,
    theme: &Theme,
) -> Rect {
    let style = Style::default()
        .fg(theme.accent_error)
        .bg(bg)
        .add_modifier(Modifier::BOLD);
    let label = truncate_str(meta.label, max_label_w as usize);
    let text = format!("  {label}  (no read mapping)");
    let w = (text.width() as u16).min(area.width);
    buf.set_span(area.x, area.y, &Span::styled(&text, style), w);
    Rect {
        x: area.x,
        y: area.y,
        width: 0,
        height: 1,
    }
}

/// Build the footer shortcut row. Enter label varies by focused row kind.
pub(super) fn build_shortcuts(state: &SettingsModalState) -> Vec<Shortcut<'static>> {
    match &state.state.mode {
        SettingsMode::Browse => {
            // A locked row (ZDR / team-managed) accepts neither the edit keys
            // nor `d`, so it advertises neither. The reason shows in the
            // description block instead.
            let locked = state
                .focused_setting()
                .is_some_and(|(key, _)| state.row_lock(key).is_some());
            let enter_label = match state.focused_setting() {
                Some((_, meta)) if matches!(meta.kind, SettingKind::Bool { .. }) => "Enter toggle",
                _ => "Enter edit",
            };
            if state.section_focus {
                return vec![
                    Shortcut {
                        label: "\u{2191}/\u{2193} jump sections",
                        clickable: false,
                        id: 0,
                    },
                    Shortcut {
                        label: "Tab/Enter settings",
                        clickable: false,
                        id: 0,
                    },
                    Shortcut {
                        label: "\u{2190}/\u{2192} switch tabs",
                        clickable: false,
                        id: 0,
                    },
                    Shortcut {
                        label: "Esc close",
                        clickable: false,
                        id: 0,
                    },
                ];
            }
            let mut shortcuts = vec![Shortcut {
                label: "\u{2191}/\u{2193}/j/k nav",
                clickable: false,
                id: 0,
            }];
            if !locked {
                shortcuts.push(Shortcut {
                    label: "Space toggle",
                    clickable: false,
                    id: 0,
                });
                shortcuts.push(Shortcut {
                    label: enter_label,
                    clickable: false,
                    id: 0,
                });
            }
            shortcuts.push(Shortcut {
                label: "\u{2190}/\u{2192} tabs",
                clickable: false,
                id: 0,
            });
            if state.sections().len() >= 2 {
                shortcuts.push(Shortcut {
                    label: "Tab sections",
                    clickable: false,
                    id: 0,
                });
            }
            shortcuts.push(Shortcut {
                label: "type to search",
                clickable: false,
                id: 0,
            });
            if !locked {
                shortcuts.push(Shortcut {
                    label: "d reset",
                    clickable: false,
                    id: 0,
                });
            }
            shortcuts.push(Shortcut {
                label: "F2/Esc close",
                clickable: false,
                id: 0,
            });
            shortcuts
        }
        SettingsMode::FilterFocused => vec![
            Shortcut {
                label: "type to filter",
                clickable: false,
                id: 0,
            },
            Shortcut {
                label: "\u{2191}/\u{2193} nav",
                clickable: false,
                id: 0,
            },
            Shortcut {
                label: "Backspace edit",
                clickable: false,
                id: 0,
            },
            Shortcut {
                label: "Enter commit",
                clickable: false,
                id: 0,
            },
            Shortcut {
                label: "Esc clear",
                clickable: false,
                id: 0,
            },
        ],
        SettingsMode::PickingEnum {
            supports_preview: sp,
            key,
            ..
        } => {
            // Labels depend on whether the Enum supports live preview.
            let nav_label = if *sp {
                "\u{2191}/\u{2193} try"
            } else {
                "\u{2191}/\u{2193} nav"
            };
            let esc_label = if *sp { "Esc revert" } else { "Esc cancel" };
            let consent = crate::settings::is_consent_chooser(key);
            let mut shortcuts = vec![
                Shortcut {
                    label: nav_label,
                    clickable: false,
                    id: 0,
                },
                // A chooser picks one of the offered answers, so Enter
                // "selects". The filter bar and the value editors, where
                // Enter really does commit typed input, keep that wording.
                Shortcut {
                    label: "Enter select",
                    clickable: false,
                    id: 0,
                },
                Shortcut {
                    label: esc_label,
                    clickable: false,
                    id: 0,
                },
            ];
            // Consent choosers hide reset; the key is disabled there too, so
            // this stays a description of what actually works on the pane.
            if !consent {
                shortcuts.push(Shortcut {
                    label: "d reset",
                    clickable: false,
                    id: 0,
                });
            }
            shortcuts
        }

        SettingsMode::EditingInt { min, max, .. } => {
            let (small_label, large_label) = int_step_footer_labels(*min, *max);
            vec![
                Shortcut {
                    label: small_label,
                    clickable: false,
                    id: 0,
                },
                Shortcut {
                    label: large_label,
                    clickable: false,
                    id: 0,
                },
                Shortcut {
                    label: "Enter commit",
                    clickable: false,
                    id: 0,
                },
                Shortcut {
                    label: "Esc cancel",
                    clickable: false,
                    id: 0,
                },
                Shortcut {
                    label: "d reset",
                    clickable: false,
                    id: 0,
                },
            ]
        }
        SettingsMode::EditingString { .. } => vec![
            Shortcut {
                label: "type to edit",
                clickable: false,
                id: 0,
            },
            Shortcut {
                label: "\u{2190}/\u{2192} cursor",
                clickable: false,
                id: 0,
            },
            Shortcut {
                label: "Enter commit",
                clickable: false,
                id: 0,
            },
            Shortcut {
                label: "Esc cancel",
                clickable: false,
                id: 0,
            },
        ],
        SettingsMode::PickingGroup { .. } => vec![
            Shortcut {
                label: "\u{2191}/\u{2193}/j/k nav",
                clickable: false,
                id: 0,
            },
            Shortcut {
                label: "Space/Enter open",
                clickable: false,
                id: 0,
            },
            Shortcut {
                label: "Esc back",
                clickable: false,
                id: 0,
            },
        ],
    }
}
