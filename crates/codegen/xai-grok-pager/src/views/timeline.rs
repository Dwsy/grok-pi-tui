//! Timeline sidebar: prompt and context-compaction markers replacing the scrollbar gutter.

use std::ops::Range;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Widget};

use crate::theme::Theme;

pub const RAIL_WIDTH: u16 = 2;

/// Terminals narrower than this hide the rail (the transcript needs the columns more than the navigator).

pub const MIN_TERMINAL_WIDTH: u16 = 60;
pub const MIN_MARKERS: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineRail {
    pub rect: Rect,
    /// Turn indices currently shown as ticks (windowed around the active turn when the conversation has more turns than rows).
    pub window: Range<usize>,
    pub ticks_y: u16,
    pub active: Option<usize>,
    /// The ▲ target: the nearest turn strictly above the viewport top
    /// ([`ScrollbackState::turn_above_viewport_top`]), NOT `active - 1`.
    pub up_target: Option<usize>,
    /// The ▼ target: the nearest turn below the viewport top
    /// ([`ScrollbackState::turn_below_viewport_top`]). Both go through `jump_to_turn`, which
    /// over-scrolls trailing turns rather than dimming.
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
    marker_count: usize,
) -> u16 {
    if show_timeline
        && !is_subagent_view
        && area_width >= MIN_TERMINAL_WIDTH
        && marker_count >= MIN_MARKERS
    {
        RAIL_WIDTH
    } else {
        0
    }
}

/// Compute rail geometry for this frame, or `None` when the rail should not render.

pub fn compute_rail(
    scrollback_area: Rect,
    rail_x: u16,
    marker_count: usize,
    viewport: RailViewport,
) -> Option<TimelineRail> {
    if marker_count < MIN_MARKERS {
        return None;
    }
    let max_ticks = (scrollback_area.height as usize).checked_sub(2)?;
    if max_ticks == 0 {
        return None;
    }
    let window = if marker_count <= max_ticks {
        0..marker_count
    } else {
        let tail_start = marker_count - max_ticks;
        let start = if viewport.at_bottom {
            viewport
                .active
                .map_or(tail_start, |active| active.min(tail_start))
        } else {
            viewport
                .active
                .unwrap_or(marker_count - 1)
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

/// Display and action therefore cannot disagree; `None` means an end stop (dim chevron, click is a
/// no-op). The chevron therefore matches the click instead of doing nothing.

pub fn chevron_target(rail: &TimelineRail, hit: TimelineHit) -> Option<usize> {
    match hit {
        TimelineHit::Tick(marker_idx) => Some(marker_idx),
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
    marker_idx: usize,
    preview: &str,
    timestamp: Option<chrono::DateTime<chrono::Local>>,
    theme: &Theme,
) {
    if !rail.window.contains(&marker_idx) {
        return;
    }
    // Wider card than the original text-only popup: the timestamp line and a
    // longer preview need more room to stay readable.
    let max_text = ((scrollback_area.width / 2).clamp(24, 48)) as usize;
    let mut lines: Vec<(String, bool)> = Vec::new();
    if let Some(ts) = timestamp {
        lines.push((ts.format("%Y-%m-%d %H:%M").to_string(), true));
    }
    let mut rest = preview.trim();
    while !rest.is_empty() && lines.len() < 4 {
        if lines.len() == 3 {
            lines.push((
                crate::render::line_utils::truncate_str(rest, max_text),
                false,
            ));
            break;
        }
        let end = crate::render::line_utils::byte_offset_at_width(rest, max_text);
        lines.push((rest[..end].to_string(), false));
        rest = rest[end..].trim_start();
    }
    if lines.is_empty() {
        return;
    }
    let text_width = lines
        .iter()
        .map(|(text, _)| unicode_width::UnicodeWidthStr::width(text.as_str()))
        .max()
        .unwrap_or_default() as u16;
    let card_height = lines.len() as u16 + 2;
    if card_height > scrollback_area.height {
        return;
    }
    let tick_y = rail.ticks_y + (marker_idx - rail.window.start) as u16;
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
        let (text, is_timestamp) = line;
        buf.set_line(
            inner.x + 1,
            inner.y + index as u16,
            &Line::from(Span::styled(
                text,
                Style::default()
                    .fg(if is_timestamp {
                        theme.gray
                    } else {
                        theme.text_primary
                    })
                    .bg(background),
            )),
            text_width,
        );
    }
}

/// Render the rail: chevrons and one tick row per windowed turn.
/// The rail draws directly on the scrollback background; a dark track strip read as an awkward empty band, especially with few ticks.

pub fn render_rail(
    buf: &mut Buffer,
    rail: &TimelineRail,
    hovered: Option<TimelineHit>,
    theme: &Theme,
    is_compaction_marker: impl Fn(usize) -> bool,
) {
    let dim = Style::default().fg(theme.gray_dim);
    let normal = Style::default().fg(theme.gray);
    let bright = Style::default().fg(theme.text_primary);
    let compaction = Style::default().fg(theme.accent_assistant);
    // Chevron dim state derives from the same function the click handler uses.
    let up_enabled = chevron_target(rail, TimelineHit::Up).is_some();
    let down_enabled = chevron_target(rail, TimelineHit::Down).is_some();

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
    for (row, marker_idx) in rail.window.clone().enumerate() {
        let y = rail.ticks_y + row as u16;
        let is_active = rail.active == Some(marker_idx);
        let is_hovered = hovered == Some(TimelineHit::Tick(marker_idx));

        if is_compaction_marker(marker_idx) {
            // Compaction is a context-boundary event rather than a user turn.
            // Use both a different glyph and a semantic accent so the marker
            // stays recognizable even in low-contrast or monochrome themes.
            let text = if is_active || is_hovered {
                "◆◆"
            } else {
                " ◆"
            };
            buf.set_span(rail.rect.x, y, &Span::styled(text, compaction), RAIL_WIDTH);
            continue;
        }

        // Prompt markers keep the upstream horizontal-stroke language:
        // active "━━", hover "──", idle right-aligned " ─".
        let (text, style) = if is_active {
            (crate::glyphs::timeline_tick_active(), bright)
        } else if is_hovered {
            (crate::glyphs::timeline_tick_hover(), bright)
        } else {
            // Short dim tick in the rightmost cell: a pad space precomposed with the light horizontal glyph

            (" \u{2500}", dim)
        };
        buf.set_span(rail.rect.x, y, &Span::styled(text, style), RAIL_WIDTH);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compaction_marker_uses_distinct_glyph_and_assistant_accent() {
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

        assert_eq!(
            buf[(1, 1)].symbol(),
            "─",
            "prompt marker keeps its line glyph"
        );
        assert_eq!(
            buf[(1, 1)].fg,
            theme.gray_dim,
            "prompt marker stays neutral"
        );
        assert_eq!(buf[(1, 2)].symbol(), "◆", "compaction uses a diamond glyph");
        assert_eq!(
            buf[(1, 2)].fg,
            theme.accent_assistant,
            "compaction marker uses the assistant accent"
        );
        assert_ne!(
            buf[(1, 2)].symbol(),
            buf[(1, 1)].symbol(),
            "compaction and prompt markers stay distinct without color"
        );
    }
    fn area() -> Rect {
        Rect::new(0, 0, 80, 20)
    }

    fn rail(turn_count: usize, active: Option<usize>) -> Option<TimelineRail> {
        compute_rail(
            area(),
            76,
            turn_count,
            RailViewport {
                active,
                up_target: active.and_then(|i| i.checked_sub(1)),
                down_target: active.and_then(|i| (i + 1 < turn_count).then_some(i + 1)),
                at_bottom: false,
            },
        )
    }

    #[test]
    fn overflow_windows_around_active() {
        // 50 turns, 18 tick rows (20 - 2 chevrons).
        let mid_rail = rail(50, Some(25)).unwrap();
        assert_eq!(mid_rail.window.len(), 18);
        assert!(mid_rail.window.contains(&25));
        // Window is roughly centered on the active turn, and tick rows map to window-relative turn indices
        assert_eq!(mid_rail.window.start, 25 - 9);

        // Active at the end clamps the window to the tail.
        let rail = rail(50, Some(49)).unwrap();
        assert_eq!(rail.window, 32..50);

        // No active turn anchors to the newest.
        let rail = rail(50, None).unwrap();
        assert_eq!(rail.window, 32..50);

        // At the bottom the window prefers the tail, but still includes the viewport-top (active) turn so a tick stays highlighted
        let rail = compute_rail(
            area(),
            76,
            50,
            RailViewport {
                active: Some(25),
                up_target: Some(24),
                down_target: Some(26),
                at_bottom: true,
            },
        )
        .unwrap();
        assert_eq!(rail.window, 25..43);
        assert!(rail.window.contains(&25));

        // Active already in the tail pins to the newest ticks
        let rail = compute_rail(
            area(),
            76,
            50,
            RailViewport {
                active: Some(40),
                up_target: Some(39),
                down_target: Some(41),
                at_bottom: true,
            },
        )
        .unwrap();
        assert_eq!(rail.window, 32..50);
        assert!(rail.window.contains(&40));
    }

    #[test]
    fn hit_maps_chevrons_and_ticks() {
        let rail = rail(4, Some(1)).unwrap();
        // Outside the rail columns (width 2: cols 76-77).
        assert_eq!(rail.hit(75, rail.ticks_y), None);
        assert_eq!(rail.hit(78, rail.ticks_y), None);
        // Chevrons.
        assert_eq!(rail.hit(77, rail.up_y), Some(TimelineHit::Up));
        assert_eq!(rail.hit(77, rail.down_y), Some(TimelineHit::Down));
        // Ticks map window-relative rows to turn indices.
        assert_eq!(rail.hit(76, rail.ticks_y), Some(TimelineHit::Tick(0)));
        assert_eq!(rail.hit(77, rail.ticks_y + 3), Some(TimelineHit::Tick(3)));
        // Rows between chevrons/ticks and rail edges miss.
        assert_eq!(rail.hit(76, rail.up_y - 1), None);
    }

    #[test]
    fn chevron_targets_follow_the_rail_state() {
        use TimelineHit::{Down, Tick, Up};
        let mid = rail(10, Some(3)).unwrap();
        // Ticks jump to themselves.
        assert_eq!(chevron_target(&mid, Tick(7)), Some(7));
        // Chevrons take the rail's viewport-derived targets verbatim.
        assert_eq!(chevron_target(&mid, Up), Some(2));
        assert_eq!(chevron_target(&mid, Down), Some(4));
        // End stops are no-ops (the dim chevrons).
        assert_eq!(chevron_target(&rail(10, Some(0)).unwrap(), Up), None);
        assert_eq!(chevron_target(&rail(10, Some(9)).unwrap(), Down), None);
        // Pre-turn content focuses the first tick, but Down still enters that first turn rather than skipping to the second
        let pre = compute_rail(
            area(),
            76,
            10,
            RailViewport {
                active: Some(0),
                down_target: Some(0),
                ..RailViewport::default()
            },
        )
        .unwrap();
        assert_eq!(chevron_target(&pre, Down), Some(0));
        assert_eq!(chevron_target(&pre, Up), None);
        // At the bottom ▼ still steps to the next turn (jump_to_turn over-scrolls it to the top, matching a tick click); ▲ steps up
        let bottom = compute_rail(
            area(),
            76,
            10,
            RailViewport {
                active: Some(4),
                up_target: Some(3),
                down_target: Some(5),
                at_bottom: true,
            },
        )
        .unwrap();
        assert_eq!(chevron_target(&bottom, Up), Some(3));
        assert_eq!(chevron_target(&bottom, Down), Some(5));
        // ▼ dims only when the last turn already owns the top.
        let last = compute_rail(
            area(),
            76,
            10,
            RailViewport {
                active: Some(9),
                up_target: Some(8),
                down_target: None,
                at_bottom: true,
            },
        )
        .unwrap();
        assert_eq!(chevron_target(&last, Down), None);
    }

    #[test]
    fn rail_width_gates_eligibility() {
        // All conditions met reserves the rail columns
        assert_eq!(rail_width(true, false, 80, 5), RAIL_WIDTH);
        // Setting off / subagent view / narrow terminal / too few turns.
        assert_eq!(rail_width(false, false, 80, 5), 0);
        assert_eq!(rail_width(true, true, 80, 5), 0);
        assert_eq!(rail_width(true, false, MIN_TERMINAL_WIDTH - 1, 5), 0);
        assert_eq!(rail_width(true, false, 80, 1), 0);
    }
}
