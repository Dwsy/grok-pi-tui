//! Dropdown list renderer for @-completion results.
//!
//! Rows highlight the selection and the fuzzy-matched characters, long paths truncate with `…`, and a scrollbar appears when results overflow.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use xai_grok_workspace::file_system::FuzzyMatchResult;

use crate::render::scrollbar::render_scrollbar_styled;
use crate::theme::Theme;

use super::context::normalize_display_path;
use super::mentions::MentionEntry;
use super::state::FileSearchState;

/// Maximum number of visible rows in the dropdown (excluding separator).
pub const MAX_DROPDOWN_ROWS: u16 = 8;

/// Render the file search dropdown items into the given area. This renders only the result rows;
/// the caller (AgentView) clears the panel and draws the borders, separator, and count hint. The
/// `area` covers just the item rows.
pub fn render_dropdown(buf: &mut Buffer, area: Rect, file_search: &FileSearchState, theme: &Theme) {
    if area.height == 0 || area.width < 4 || !file_search.is_visible() {
        return;
    }

    let results = file_search.results();
    let topk = &results.topk;
    let mentions = file_search.mention_entries();
    let total = file_search.result_count();
    let selected = file_search.selected();
    let scroll = file_search.scroll_offset();
    let dir_mode = file_search.is_dir_mode();

    // Reserve 2 columns on the right for the scrollbar (gap and track)
    let needs_scrollbar = total > area.height as usize;
    let content_width = if needs_scrollbar {
        area.width.saturating_sub(2)
    } else {
        area.width
    };

    let visible_rows = area.height as usize;

    let hovered = file_search.hovered();
    let hover_bg = theme.bg_hover;

    for row in 0..visible_rows {
        let idx = scroll + row;
        if idx >= total {
            break;
        }

        let y = area.y + row as u16;
        let is_selected = idx == selected;
        let is_hovered = hovered == Some(idx) && !is_selected;

        if let Some(entry) = mentions.get(idx) {
            render_mention_entry(
                buf,
                area.x,
                y,
                content_width,
                entry,
                is_selected,
                is_hovered,
                hover_bg,
                theme,
            );
        } else if let Some(item) = topk.get(idx.saturating_sub(mentions.len())) {
            render_fuzzy_item(
                buf,
                area.x,
                y,
                content_width,
                item,
                is_selected,
                is_hovered,
                hover_bg,
                dir_mode,
                theme,
            );
        }
    }

    // ── Scrollbar ───────────────────────────────────────────────────────

    if needs_scrollbar {
        let scrollbar_area = Rect {
            x: area.x + area.width - 1,
            y: area.y,
            width: 1,
            height: area.height,
        };
        let track_style = Style::default().bg(theme.bg_dark);
        let thumb_style = Style::default().fg(theme.gray_dim).bg(theme.bg_dark);
        render_scrollbar_styled(
            buf,
            Some(scrollbar_area),
            total as u16,
            area.height,
            scroll as u16,
            track_style,
            thumb_style,
        );
    }
}

/// Desired height for the dropdown: the separator row plus min(results, max_rows).
pub fn dropdown_height(file_search: &FileSearchState, max_rows: u16) -> u16 {
    if !file_search.is_visible() {
        return 0;
    }
    let result_rows = (file_search.result_count() as u16).min(max_rows);
    1 + result_rows // separator + results
}

/// Non-selected prefix: same width as the arrow, just spaces.
const ITEM_PREFIX: &str = "  ";
const PREFIX_WIDTH: u16 = crate::glyphs::PROMPT_ARROW_WIDTH;

/// Render a single fuzzy match item with character-level match highlighting.
#[allow(clippy::too_many_arguments)]
fn render_fuzzy_item(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    width: u16,
    item: &FuzzyMatchResult,
    is_selected: bool,
    is_hovered: bool,
    hover_bg: ratatui::style::Color,
    dir_mode: bool,
    theme: &Theme,
) {
    if width < PREFIX_WIDTH + 1 {
        return;
    }

    let path_str = item.path.to_string();
    let path = normalize_display_path(&path_str);

    let embed = crate::views::modal_window::embedded_row_style(theme, is_selected);
    let row_bg = match embed {
        Some(e) => e.bg,
        None if is_selected => theme.bg_visual,
        None if is_hovered => hover_bg,
        None => theme.bg_light,
    };
    let text_fg = embed.map_or(theme.text_primary, |e| e.fg(theme.text_primary));
    let bold = if is_selected {
        Modifier::BOLD
    } else {
        Modifier::empty()
    };

    // Fill the row with background.
    for col in x..x + width {
        if let Some(cell) = buf.cell_mut((col, y)) {
            cell.set_char(' ');
            cell.set_style(Style::default().bg(row_bg));
        }
    }

    // Arrow on the selected row, blank gutter on the rest.
    let prefix = if is_selected {
        crate::glyphs::prompt_arrow()
    } else {
        ITEM_PREFIX
    };
    let prefix_style = Style::default().fg(text_fg).bg(row_bg).add_modifier(bold);
    for (i, ch) in prefix.chars().enumerate() {
        let px = x + i as u16;
        if px < x + width
            && let Some(cell) = buf.cell_mut((px, y))
        {
            cell.set_char(ch);
            cell.set_style(if is_selected {
                prefix_style
            } else {
                Style::default().bg(row_bg)
            });
        }
    }

    // Styles: primary FG for text (not dimmed), BLUE for match chars.
    // Selected rows get bold via the modifier.
    let match_style = Style::default()
        .fg(embed.map_or(theme.fuzzy_accent, |e| e.fg(theme.fuzzy_accent)))
        .bg(row_bg)
        .add_modifier(bold);
    let normal_style = Style::default().fg(text_fg).bg(row_bg).add_modifier(bold);

    // Render path characters after prefix, with match highlighting.
    let mut indices = &item.indices[..];
    let mut col = x + PREFIX_WIDTH;
    let max_col = x + width;

    for (char_idx, (byte_idx, ch)) in path.char_indices().enumerate() {
        let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0) as u16;
        if col + ch_width > max_col {
            // Truncation: replace last visible char with '…'
            if col > x + PREFIX_WIDTH
                && let Some(cell) = buf.cell_mut((col.saturating_sub(1), y))
            {
                cell.set_char('…');
            }
            break;
        }

        let is_match = indices.first() == Some(&(char_idx as u32));
        if is_match {
            indices = &indices[1..];
        }

        let style = if is_match { match_style } else { normal_style };

        // Write the character.
        let ch_str = &path[byte_idx..byte_idx + ch.len_utf8()];
        if let Some(cell) = buf.cell_mut((col, y)) {
            cell.set_symbol(ch_str);
            cell.set_style(style);
        }
        // For wide chars, fill continuation cell.
        if ch_width > 1 {
            for w in 1..ch_width {
                if let Some(cell) = buf.cell_mut((col + w, y)) {
                    cell.set_char(' ');
                    cell.set_style(style);
                }
            }
        }
        col += ch_width;
    }

    // Mark directories with a trailing '/' so a folder is distinguishable from
    // a file at a glance instead of only after accepting it. In dir mode every
    // row is a directory the query already ended in '/', so the marker is
    // redundant there but harmless: keep one code path.
    if (item.is_dir || dir_mode)
        && col < max_col
        && let Some(cell) = buf.cell_mut((col, y))
    {
        cell.set_char('/');
        cell.set_style(normal_style);
    }

    // Terminal theme (Reset bands): reverse video; no-op on RGB themes.
    if embed.is_none() {
        let row_rect = Rect {
            x,
            y,
            width,
            height: 1,
        };
        if is_selected {
            buf.set_style(row_rect, theme.selection_overlay());
        } else if is_hovered {
            buf.set_style(row_rect, theme.hover_overlay());
        }
    }
}

/// Write `text` with `style` starting at `col`, returning the updated column.
/// Stops and places `…` if `max_col` would be exceeded.
fn write_styled_segment(
    buf: &mut Buffer,
    y: u16,
    mut col: u16,
    min_col: u16,
    max_col: u16,
    text: &str,
    style: Style,
) -> (u16, bool) {
    for (byte_idx, ch) in text.char_indices() {
        let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0) as u16;
        if col + ch_width > max_col {
            if col > min_col
                && let Some(cell) = buf.cell_mut((col.saturating_sub(1), y))
            {
                cell.set_char('…');
            }
            return (col, true);
        }
        let ch_str = &text[byte_idx..byte_idx + ch.len_utf8()];
        if let Some(cell) = buf.cell_mut((col, y)) {
            cell.set_symbol(ch_str);
            cell.set_style(style);
        }
        if ch_width > 1 {
            for w in 1..ch_width {
                if let Some(cell) = buf.cell_mut((col + w, y)) {
                    cell.set_char(' ');
                    cell.set_style(style);
                }
            }
        }
        col += ch_width;
    }
    (col, false)
}

/// Render one extensible `@` mention row (either a provider category header or a concrete item).
#[allow(clippy::too_many_arguments)]
fn render_mention_entry(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    width: u16,
    entry: &MentionEntry,
    is_selected: bool,
    is_hovered: bool,
    hover_bg: ratatui::style::Color,
    theme: &Theme,
) {
    if width < PREFIX_WIDTH + 1 {
        return;
    }

    let embed = crate::views::modal_window::embedded_row_style(theme, is_selected);
    let row_bg = match embed {
        Some(e) => e.bg,
        None if is_selected => theme.bg_visual,
        None if is_hovered => hover_bg,
        None => theme.bg_light,
    };
    let text_fg = embed.map_or(theme.text_primary, |e| e.fg(theme.text_primary));
    let dim_fg = embed.map_or(theme.gray_dim, |e| e.fg(theme.gray_dim));
    let accent_fg = embed.map_or(theme.fuzzy_accent, |e| e.fg(theme.fuzzy_accent));
    let bold = if is_selected {
        Modifier::BOLD
    } else {
        Modifier::empty()
    };

    for col in x..x + width {
        if let Some(cell) = buf.cell_mut((col, y)) {
            cell.set_char(' ');
            cell.set_style(Style::default().bg(row_bg));
        }
    }

    let prefix = if is_selected {
        crate::glyphs::prompt_arrow()
    } else {
        ITEM_PREFIX
    };
    let prefix_style = Style::default().fg(text_fg).bg(row_bg).add_modifier(bold);
    for (i, ch) in prefix.chars().enumerate() {
        let px = x + i as u16;
        if px < x + width
            && let Some(cell) = buf.cell_mut((px, y))
        {
            cell.set_char(ch);
            cell.set_style(if is_selected {
                prefix_style
            } else {
                Style::default().bg(row_bg)
            });
        }
    }

    let accent_style = Style::default().fg(accent_fg).bg(row_bg).add_modifier(bold);
    let primary_style = Style::default().fg(text_fg).bg(row_bg).add_modifier(bold);
    let dim_style = Style::default().fg(dim_fg).bg(row_bg);

    let min_col = x + PREFIX_WIDTH;
    let max_col = x + width;
    let mut col = min_col;

    match entry {
        MentionEntry::ProviderHeader {
            kind,
            label,
            description,
            item_count,
        } => {
            let badge = format!("@{kind}:");
            let (next_col, truncated) =
                write_styled_segment(buf, y, col, min_col, max_col, &badge, accent_style);
            col = next_col;
            if !truncated && !label.is_empty() && !label.eq_ignore_ascii_case(kind) {
                let seg = format!(" {label}");
                let (c, t) =
                    write_styled_segment(buf, y, col, min_col, max_col, &seg, primary_style);
                col = c;
                if !t && !description.is_empty() {
                    let d = format!(" · {description}");
                    let (c2, t2) =
                        write_styled_segment(buf, y, col, min_col, max_col, &d, dim_style);
                    col = c2;
                    if !t2 && *item_count > 0 {
                        let count = format!(" ({item_count})");
                        let _ =
                            write_styled_segment(buf, y, col, min_col, max_col, &count, dim_style);
                    }
                }
            } else if !truncated && !description.is_empty() {
                let d = format!("  {description}");
                let (c2, t2) = write_styled_segment(buf, y, col, min_col, max_col, &d, dim_style);
                col = c2;
                if !t2 && *item_count > 0 {
                    let count = format!(" ({item_count})");
                    let _ = write_styled_segment(buf, y, col, min_col, max_col, &count, dim_style);
                }
            }
        }
        MentionEntry::Item {
            kind,
            title,
            subtitle,
            ..
        } => {
            let badge = format!("@{kind}:");
            let (next_col, truncated) =
                write_styled_segment(buf, y, col, min_col, max_col, &badge, accent_style);
            col = next_col;
            if !truncated {
                let (c, t) =
                    write_styled_segment(buf, y, col, min_col, max_col, title, primary_style);
                col = c;
                if !t && !subtitle.is_empty() {
                    let sub = format!("  {subtitle}");
                    let _ = write_styled_segment(buf, y, col, min_col, max_col, &sub, dim_style);
                }
            }
        }
    }

    if embed.is_none() {
        let row_rect = Rect {
            x,
            y,
            width,
            height: 1,
        };
        if is_selected {
            buf.set_style(row_rect, theme.selection_overlay());
        } else if is_hovered {
            buf.set_style(row_rect, theme.hover_overlay());
        }
    }
}
