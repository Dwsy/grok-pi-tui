//! Shared prompt-area list overlay: accent bar, bold title, and a scrollable single-line row list with a cursor.
//!
//! `/rewind`'s picker phase and `/jump` each used to keep this row geometry in sync by hand across their render, hit-test, and height functions.
//! Row content stays with the caller (a closure); this module owns the accent bar, title, cursor styling, and the scroll window.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::theme::Theme;

const MAX_ROWS: usize = 15;

/// List geometry: row count and cursor position.
/// Construct per call; every method derives the same scroll window from these two fields, so the render, hit-test, and height paths cannot drift.

pub struct ListOverlay {
    pub len: usize,
    pub selected: usize,
}

pub struct RowCtx {
    pub is_cursor: bool,
    pub row_bg: Color,
    pub content_width: u16,
}

/// Optional search/filter line rendered between the title and the list rows.
pub struct SearchLine<'a> {
    /// The current query text (empty = placeholder shown).
    pub query: &'a str,
    /// Placeholder shown when query is empty.
    pub placeholder: &'a str,
    /// Whether the search input is focused (cursor visible).
    pub focused: bool,
}

impl ListOverlay {
    /// Overlay height: title plus rows (at most [`MAX_ROWS`]), capped at 60% of the screen, plus one padding row.

    pub fn height(&self, screen_h: u16) -> u16 {
        self.height_with_search(screen_h, false)
    }

    /// Height accounting for an optional search line.
    pub fn height_with_search(&self, screen_h: u16, has_search: bool) -> u16 {
        let rows = self.len.min(MAX_ROWS) as u16;
        let search_extra: u16 = if has_search { 1 } else { 0 };
        let height = 2 + search_extra + rows;
        let cap = (screen_h as u32 * 60 / 100).max(6) as u16;
        height.min(cap) + 1
    }

    fn visible_rows(area: Rect, has_search: bool) -> usize {
        let base: u16 = 3;
        let search_extra: u16 = if has_search { 1 } else { 0 };
        area.height.saturating_sub(base + search_extra) as usize
    }

    fn scroll_offset(&self, visible_rows: usize) -> usize {
        if visible_rows > 0 && self.selected >= visible_rows {
            self.selected - visible_rows + 1
        } else {
            0
        }
    }

    /// Row index under a screen position, or `None` when the position misses the rows.

    pub fn row_at(&self, area: Rect, col: u16, row: u16) -> Option<usize> {
        self.row_at_with_search(area, col, row, false)
    }

    /// Row hit-testing accounting for an optional search line.
    pub fn row_at_with_search(
        &self,
        area: Rect,
        col: u16,
        row: u16,
        has_search: bool,
    ) -> Option<usize> {
        if area.height == 0 || area.width < 10 {
            return None;
        }
        if col < area.x || col >= area.x + area.width {
            return None;
        }
        if row < area.y || row >= area.y + area.height {
            return None;
        }
        let search_extra: u16 = if has_search { 1 } else { 0 };
        let first = area.y + 2 + search_extra;
        if row < first {
            return None;
        }
        let visible_rows = Self::visible_rows(area, has_search);
        let relative = (row - first) as usize;
        if relative >= visible_rows {
            return None;
        }
        let index = self.scroll_offset(visible_rows) + relative;
        (index < self.len).then_some(index)
    }

    /// Render the overlay: bg fill, accent bar, title, then the visible window of rows.
    /// `row_line(idx, ctx)` produces each row's content; cursor and row backgrounds are painted here.
    /// Applies the standard unfocus dim, so callers must not blend again.

    pub fn render(
        &self,
        buf: &mut Buffer,
        area: Rect,
        title: &str,
        focused: bool,
        row_line: impl FnMut(usize, &RowCtx) -> Line<'static>,
    ) {
        self.render_with_search(buf, area, title, focused, None, row_line);
    }

    /// Render with an optional search/filter line between title and rows.
    pub fn render_with_search(
        &self,
        buf: &mut Buffer,
        area: Rect,
        title: &str,
        focused: bool,
        search: Option<SearchLine<'_>>,
        mut row_line: impl FnMut(usize, &RowCtx) -> Line<'static>,
    ) {
        if area.height == 0 || area.width < 10 {
            return;
        }

        let theme = Theme::current();
        let background = theme.bg_light;
        buf.set_style(area, Style::default().bg(background));

        let accent_style = Style::default().fg(theme.accent_user);
        for row in area.y..area.y + area.height {
            if let Some(cell) = buf.cell_mut((area.x, row)) {
                cell.set_symbol(crate::glyphs::accent_bar());
                cell.set_style(accent_style);
            }
        }

        let content_x = area.x + 3;
        let content_width = area.width.saturating_sub(5);
        let title_style = Style::default()
            .fg(theme.accent_user)
            .add_modifier(Modifier::BOLD);
        let mut row = area.y + 1;
        buf.set_line(
            content_x,
            row,
            &Line::from(Span::styled(title.to_string(), title_style)),
            content_width,
        );
        row += 1;

        // Render search line if present.
        let has_search = search.is_some();
        if let Some(ref search) = search {
            let search_style = if search.query.is_empty() {
                Style::default().fg(theme.gray_dim)
            } else {
                Style::default().fg(theme.text_primary)
            };
            let display = if search.query.is_empty() {
                search.placeholder.to_string()
            } else {
                let cursor = if search.focused { "█" } else { "" };
                format!("{}{}", search.query, cursor)
            };
            let prefix = Span::styled("/ ", Style::default().fg(theme.accent_user));
            let text = Span::styled(display, search_style);
            buf.set_line(
                content_x,
                row,
                &Line::from(vec![prefix, text]),
                content_width,
            );
            row += 1;
        }

        let visible_rows = Self::visible_rows(area, has_search);
        let scroll_offset = self.scroll_offset(visible_rows);
        for index in (scroll_offset..self.len).take(visible_rows) {
            if row >= area.y + area.height {
                break;
            }
            let is_cursor = index == self.selected;
            let row_bg = if is_cursor && focused {
                theme.bg_visual
            } else {
                background
            };

            let row_rect = Rect {
                x: content_x.saturating_sub(1),
                y: row,
                width: content_width + 2,
                height: 1,
            };
            buf.set_style(row_rect, Style::default().bg(row_bg));
            let context = RowCtx {
                is_cursor,
                row_bg,
                content_width,
            };
            let line = row_line(index, &context);
            buf.set_line(content_x, row, &line, content_width);
            if is_cursor && focused {
                buf.set_style(row_rect, theme.selection_overlay());
            }
            row += 1;
        }

        if !focused {
            crate::render::color::recede_area(buf, area, background, 0.66);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area() -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 10,
        }
    }

    #[test]
    fn row_at_maps_rows_and_rejects_chrome() {
        let list = ListOverlay {
            len: 3,
            selected: 0,
        };
        assert_eq!(list.row_at(area(), 5, 1), None);
        assert_eq!(list.row_at(area(), 5, 2), Some(0));
        assert_eq!(list.row_at(area(), 5, 4), Some(2));
        assert_eq!(list.row_at(area(), 5, 5), None);
    }

    #[test]
    fn row_at_respects_scroll_window() {
        // 20 rows, 7 visible (height 10 - 3), cursor at the end: the window starts at 13 so the cursor stays visible

        let list = ListOverlay {
            len: 20,
            selected: 19,
        };
        assert_eq!(list.row_at(area(), 5, 2), Some(13));
        assert_eq!(list.row_at(area(), 5, 8), Some(19));
    }

    /// Terminal theme (zero opaque cells): the cursor row carries reverse
    /// video instead of a painted band. RGB themes keep the `bg_visual`
    /// band and the row's own fgs.
    #[test]
    fn terminal_theme_cursor_row_uses_reverse_video() {
        use ratatui::style::Modifier;

        let _guard = crate::theme::cache::pin_theme();
        let list = ListOverlay {
            len: 3,
            selected: 1,
        };
        let render = || {
            let theme = Theme::current();
            let mut buf = Buffer::empty(area());
            list.render(&mut buf, area(), "Pick", true, |i, ctx| {
                Line::from(Span::styled(
                    format!("row {i}"),
                    Style::default().fg(theme.text_primary).bg(ctx.row_bg),
                ))
            });
            // Rows start at y+2; content at x+3.
            (buf[(3, 3)].style(), buf[(3, 2)].style())
        };

        crate::theme::cache::set(crate::theme::ThemeKind::Terminal);
        let (cursor, normal) = render();
        assert!(
            cursor.add_modifier.contains(Modifier::REVERSED),
            "cursor row uses reverse video, got {cursor:?}"
        );
        assert_eq!(cursor.bg, Some(Color::Reset), "no painted band");
        assert!(!normal.add_modifier.contains(Modifier::REVERSED));

        crate::theme::cache::set(crate::theme::ThemeKind::GrokNight);
        let (cursor, normal) = render();
        let theme = Theme::current();
        assert_eq!(cursor.bg, Some(theme.bg_visual), "RGB keeps the band");
        assert_eq!(cursor.fg, Some(theme.text_primary), "RGB keeps row fgs");
        assert!(!cursor.add_modifier.contains(Modifier::REVERSED));
        assert_eq!(normal.bg, Some(theme.bg_light));
    }

    #[test]
    fn height_caps_at_max_rows_and_screen_fraction() {
        let two = ListOverlay {
            len: 2,
            selected: 0,
        };
        assert_eq!(two.height(40), 5); // title + 2 rows + padding
        let many = ListOverlay {
            len: 30,
            selected: 0,
        };
        assert_eq!(many.height(40), 18); // 15-row cap
        assert_eq!(many.height(12), 8); // 60% screen cap
    }
}
