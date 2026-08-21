//! Timeline sidebar: a per-turn tick rail that replaces the scrollbar gutter.

use std::ops::Range;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Widget};

use crate::theme::Theme;

pub const RAIL_WIDTH: u16 = 2;
pub const MIN_TERMINAL_WIDTH: u16 = 60;
pub const MIN_TURNS: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineRail {
    pub rect: Rect,
    pub window: Range<usize>,
    pub ticks_y: u16,
    pub active: Option<usize>,
    pub up_target: Option<usize>,
    pub down_target: Option<usize>,
    pub up_y: u16,
    pub down_y: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimelineHit {
    Tick(usize),
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RailViewport {
    pub active: Option<usize>,
    pub up_target: Option<usize>,
    pub down_target: Option<usize>,
    pub at_bottom: bool,
}

pub fn rail_width(
    show_timeline: bool,
    is_subagent_view: bool,
    area_width: u16,
    turn_count: usize,
) -> u16 {
    if show_timeline
        && !is_subagent_view
        && area_width >= MIN_TERMINAL_WIDTH
        && turn_count >= MIN_TURNS
    {
        RAIL_WIDTH
    } else {
        0
    }
}

pub fn compute_rail(
    scrollback_area: Rect,
    rail_x: u16,
    turn_count: usize,
    viewport: RailViewport,
) -> Option<TimelineRail> {
    if turn_count < MIN_TURNS {
        return None;
    }
    let max_ticks = (scrollback_area.height as usize).checked_sub(2)?;
    if max_ticks == 0 {
        return None;
    }
    let window = if turn_count <= max_ticks {
        0..turn_count
    } else {
        let tail_start = turn_count - max_ticks;
        let start = if viewport.at_bottom {
            viewport
                .active
                .map_or(tail_start, |active| active.min(tail_start))
        } else {
            viewport
                .active
                .unwrap_or(turn_count - 1)
                .saturating_sub(max_ticks / 2)
                .min(tail_start)
        };
        start..start + max_ticks
    };
    let top = scrollback_area.y + ((scrollback_area.height as usize - window.len() - 2) / 2) as u16;
    let ticks_y = top + 1;
    Some(TimelineRail {
        rect: Rect::new(
            rail_x,
            scrollback_area.y,
            RAIL_WIDTH,
            scrollback_area.height,
        ),
        window: window.clone(),
        ticks_y,
        active: viewport.active,
        up_target: viewport.up_target,
        down_target: viewport.down_target,
        up_y: top,
        down_y: ticks_y + window.len() as u16,
    })
}

pub fn chevron_target(rail: &TimelineRail, hit: TimelineHit) -> Option<usize> {
    match hit {
        TimelineHit::Tick(turn_idx) => Some(turn_idx),
        TimelineHit::Up => rail.up_target,
        TimelineHit::Down => rail.down_target,
    }
}

impl TimelineRail {
    pub fn hit(&self, col: u16, row: u16) -> Option<TimelineHit> {
        if !self.rect.contains((col, row).into()) {
            return None;
        }
        if row == self.up_y {
            return Some(TimelineHit::Up);
        }
        if row == self.down_y {
            return Some(TimelineHit::Down);
        }
        (row >= self.ticks_y)
            .then(|| (row - self.ticks_y) as usize)
            .filter(|relative| *relative < self.window.len())
            .map(|relative| TimelineHit::Tick(self.window.start + relative))
    }
}

pub fn render_tick_hover_popup(
    buf: &mut Buffer,
    rail: &TimelineRail,
    scrollback_area: Rect,
    turn_idx: usize,
    preview: &str,
    theme: &Theme,
) {
    if !rail.window.contains(&turn_idx) {
        return;
    }
    let max_text = ((scrollback_area.width / 2).clamp(16, 32)) as usize;
    let mut lines = Vec::new();
    let mut rest = preview.trim();
    while !rest.is_empty() && lines.len() < 2 {
        if lines.len() == 1 {
            lines.push(crate::render::line_utils::truncate_str(rest, max_text));
            break;
        }
        let end = crate::render::line_utils::byte_offset_at_width(rest, max_text);
        lines.push(rest[..end].to_string());
        rest = rest[end..].trim_start();
    }
    if lines.is_empty() {
        return;
    }
    let text_width = lines
        .iter()
        .map(|line| unicode_width::UnicodeWidthStr::width(line.as_str()))
        .max()
        .unwrap_or_default() as u16;
    let card_height = lines.len() as u16 + 2;
    if card_height > scrollback_area.height {
        return;
    }
    let tick_y = rail.ticks_y + (turn_idx - rail.window.start) as u16;
    let card_area = Rect::new(
        rail.rect
            .x
            .saturating_sub(text_width + 5)
            .max(scrollback_area.x),
        tick_y
            .saturating_sub(card_height / 2)
            .max(scrollback_area.y)
            .min((scrollback_area.y + scrollback_area.height).saturating_sub(card_height)),
        text_width + 4,
        card_height,
    );
    let background = theme.bg_base;
    Clear.render(card_area, buf);
    buf.set_style(card_area, Style::default().bg(background));
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.gray).bg(background));
    let inner = block.inner(card_area);
    block.render(card_area, buf);
    for (index, line) in lines.into_iter().enumerate() {
        buf.set_line(
            inner.x + 1,
            inner.y + index as u16,
            &Line::from(Span::styled(
                line,
                Style::default().fg(theme.text_primary).bg(background),
            )),
            text_width,
        );
    }
}

pub fn render_rail(
    buf: &mut Buffer,
    rail: &TimelineRail,
    hovered: Option<TimelineHit>,
    theme: &Theme,
    is_compaction_turn: impl Fn(usize) -> bool,
) {
    let dim = Style::default().fg(theme.gray_dim);
    let normal = Style::default().fg(theme.gray);
    let bright = Style::default().fg(theme.text_primary);
    let compaction = Style::default().fg(theme.accent_tool);
    let up_enabled = rail.up_target.is_some();
    let down_enabled = rail.down_target.is_some();
    let up_style = if hovered == Some(TimelineHit::Up) && up_enabled {
        bright
    } else if up_enabled {
        normal
    } else {
        dim
    };
    let down_style = if hovered == Some(TimelineHit::Down) && down_enabled {
        bright
    } else if down_enabled {
        normal
    } else {
        dim
    };
    let chevron_x = rail.rect.x + RAIL_WIDTH - 1;
    buf.set_span(
        chevron_x,
        rail.up_y,
        &Span::styled(crate::glyphs::timeline_chevron_up(), up_style),
        1,
    );
    buf.set_span(
        chevron_x,
        rail.down_y,
        &Span::styled(crate::glyphs::timeline_chevron_down(), down_style),
        1,
    );
    for (row, turn_idx) in rail.window.clone().enumerate() {
        let y = rail.ticks_y + row as u16;
        let is_active = rail.active == Some(turn_idx);
        let is_hovered = hovered == Some(TimelineHit::Tick(turn_idx));

        // Upstream rail uses horizontal strokes, not dots:
        // active "━━", hover "──", idle right-aligned " ─".
        let (text, style) = if is_active {
            (crate::glyphs::timeline_tick_active(), bright)
        } else if is_hovered {
            (crate::glyphs::timeline_tick_hover(), bright)
        } else {
            // Short dim tick in the rightmost cell (precomposed pad + light).
            (" \u{2500}", dim)
        };
        buf.set_span(rail.rect.x, y, &Span::styled(text, style), RAIL_WIDTH);

        // The rail is two columns wide. Keep the prompt tick in its normal
        // style and use the left column as an additional compaction marker.
        // This preserves turn navigation while showing both semantics at once.
        if is_compaction_turn(turn_idx) {
            buf.set_span(rail.rect.x, y, &Span::styled("\u{2500}", compaction), 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compaction_tick_uses_distinct_tool_accent() {
        let area = Rect::new(0, 0, RAIL_WIDTH, 4);
        let rail = TimelineRail {
            rect: area,
            window: 0..2,
            ticks_y: 1,
            active: None,
            up_target: None,
            down_target: None,
            up_y: 0,
            down_y: 3,
        };
        let theme = Theme::current();
        let mut buf = Buffer::empty(area);

        render_rail(&mut buf, &rail, None, &theme, |turn_idx| turn_idx == 1);

        assert_eq!(buf[(1, 1)].fg, theme.gray_dim, "prompt tick stays neutral");
        assert_eq!(
            buf[(0, 2)].fg,
            theme.accent_tool,
            "compaction marker must use the tool accent"
        );
        assert_eq!(
            buf[(1, 2)].fg,
            theme.gray_dim,
            "prompt tick keeps its neutral color beside compaction"
        );
        assert_ne!(
            buf[(0, 2)].fg,
            buf[(1, 2)].fg,
            "compaction and prompt ticks must remain visually distinct"
        );
    }
}
