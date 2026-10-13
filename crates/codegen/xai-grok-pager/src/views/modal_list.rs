use crate::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::style::{Modifier, Style};
use unicode_width::UnicodeWidthStr;
/// Column of an arrow row's detail line and of `render_detail_line`, past the arrow and dot; a flat row's detail starts at `NAME_INDENT`
pub(crate) const DETAIL_INDENT: u16 = 6;
/// Columns taken by the dot and its space before a row's name
pub(crate) const NAME_INDENT: u16 = 2;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RowStatus {
    Enabled,
    Disabled,
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "Only the Remote Control screen shows a pending device"
        )
    )]
    Pending,
    #[expect(
        dead_code,
        reason = "Only the Remote Control screen shows a ready computer"
    )]
    Ready,
}
/// Badge drawn after the name, skipped whole when it does not fit
#[derive(Clone, Copy, Debug)]
pub(crate) struct RowTag<'a> {
    pub(crate) text: &'a str,
    pub(crate) style: Style,
    /// Blank columns before the text that keep the cells' existing style
    pub(crate) gap: u16,
}
pub(crate) struct ListRow<'a> {
    /// `Some(is_expanded)` draws an arrow; `None` starts the row at the dot
    pub(crate) expand: Option<bool>,
    pub(crate) status: RowStatus,
    pub(crate) name: &'a str,
    pub(crate) tags: &'a [RowTag<'a>],
    pub(crate) detail: Option<&'a str>,
    pub(crate) is_selected: bool,
}
/// Draws `row` at `y`, plus its detail line on `y + 1` that the caller must have room for; returns the rows used
pub(crate) fn render_row(
    buf: &mut Buffer,
    x: u16,
    width: u16,
    y: u16,
    row: &ListRow<'_>,
    theme: &Theme,
) -> u16 {
    let end = x.saturating_add(width);
    if row.is_selected {
        fill_highlight(buf, x, end, y, theme);
    }
    let dot_x = match row.expand {
        Some(is_expanded) => {
            let arrow = if is_expanded {
                "\u{25bc} "
            } else {
                "\u{25b6} "
            };
            put(buf, x, end, y, arrow, Style::default().fg(theme.gray_dim));
            x.saturating_add(2)
        }
        None => x,
    };
    let (glyph, dot_fg) = match row.status {
        RowStatus::Enabled => (crate::glyphs::filled_dot(), theme.accent_success),
        RowStatus::Disabled => ("\u{25cb}", theme.gray_dim),
        RowStatus::Pending => (crate::glyphs::filled_dot(), theme.warning),
        RowStatus::Ready => ("\u{25cb}", theme.accent_success),
    };
    let dot = format!("{glyph} ");
    let dot_style = Style::default().fg(dot_fg);
    put(buf, dot_x, end, y, &dot, dot_style);
    let name_style = Style::default()
        .fg(theme.text_primary)
        .add_modifier(Modifier::BOLD);
    let name_x = dot_x.saturating_add(NAME_INDENT);
    let mut cursor = put(buf, name_x, end, y, row.name, name_style);
    for tag in row.tags {
        let tag_x = cursor.saturating_add(tag.gap);
        if usize::from(end.saturating_sub(tag_x)) >= tag.text.width() {
            cursor = put(buf, tag_x, end, y, tag.text, tag.style);
        }
    }
    let Some(detail) = row.detail else {
        return 1;
    };
    let detail_y = y.saturating_add(1);
    if row.is_selected {
        fill_highlight(buf, x, end, detail_y, theme);
    }
    let detail_x = match row.expand {
        Some(_) => x.saturating_add(DETAIL_INDENT),
        None => name_x,
    };
    put(
        buf,
        detail_x,
        end,
        detail_y,
        detail,
        Style::default().fg(theme.gray),
    );
    2
}
/// Draws one detail line under a row, for callers that wrap and scroll details line by line
pub(crate) fn render_detail_line(
    buf: &mut Buffer,
    x: u16,
    width: u16,
    y: u16,
    text: &str,
    is_selected: bool,
    theme: &Theme,
) {
    let end = x.saturating_add(width);
    if is_selected {
        fill_highlight(buf, x, end, y, theme);
    }
    let detail_x = x.saturating_add(DETAIL_INDENT);
    put(buf, detail_x, end, y, text, Style::default().fg(theme.gray));
}
pub(crate) fn render_section_header(
    buf: &mut Buffer,
    x: u16,
    width: u16,
    y: u16,
    label: &str,
    theme: &Theme,
) {
    let header_style = Style::default()
        .fg(theme.gray)
        .bg(theme.bg_base)
        .add_modifier(Modifier::BOLD);
    let rule_style = Style::default().fg(theme.gray_dim).bg(theme.bg_base);
    let end = x.saturating_add(width);
    let rule_x = put(buf, x, end, y, &format!(" {label} "), header_style);
    let rule = "\u{2500}".repeat(usize::from(end.saturating_sub(rule_x)));
    put(buf, rule_x, end, y, &rule, rule_style);
}
/// Reverse video where the theme has no background band, as under `NO_COLOR`, so the selection still shows
pub(crate) fn highlight(theme: &Theme) -> Style {
    if theme.is_bandless() {
        theme.selection_overlay()
    } else {
        Style::default().bg(theme.bg_highlight)
    }
}
fn fill_highlight(buf: &mut Buffer, x: u16, end: u16, y: u16, theme: &Theme) {
    let fill = highlight(theme);
    for cx in x..end {
        if let Some(cell) = buf.cell_mut((cx, y)) {
            cell.set_style(fill);
        }
    }
}
/// Draws `text` clipped to `end` and returns the column after it
fn put(buf: &mut Buffer, x: u16, end: u16, y: u16, text: &str, style: Style) -> u16 {
    if !(buf.area.top()..buf.area.bottom()).contains(&y) {
        return x;
    }
    let max_width = usize::from(end.saturating_sub(x));
    buf.set_stringn(x, y, text, max_width, style).0
}
#[cfg(test)]
#[path = "modal_list_tests.rs"]
mod tests;
