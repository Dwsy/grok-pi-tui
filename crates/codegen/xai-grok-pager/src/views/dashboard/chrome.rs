//! Dashboard chrome above the row list: the header row (location label, `[Choose …]` hint, state chips, promo CTA)
//! and the primary actions row (`+ New Agent`, `Open Previous /resume`, `Worktree Ctrl+w`).
//! Both paint into rects from [`crate::views::dashboard::layout::DashboardLayout`] and register their click targets on [`DashboardState`].

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::render::line_utils::truncate_line;
use crate::theme::Theme;
use crate::views::dashboard::row::DashboardRow;
use crate::views::dashboard::state::{ActionsFocus, DashboardState, RowState, SPINNER_DIVISOR};
use crate::views::location::{LocationParts, location_parts, worktree_badge};

/// Promo upgrade CTA for the dashboard header, resolved through the shared slot gate by the producer (`app_view`).
#[derive(Clone, Copy)]
pub struct HeaderUpgradeCta<'a> {
    /// The `[label]` button text.
    pub label: &'a str,
    /// True when the promo is non-dismissible, so the `Ctrl+O` override applies.
    pub pinned: bool,
    /// The promo's trimmed `cta.caption` value; painted only when `pinned` is set.
    pub caption: Option<&'a str>,
}

/// Render the dashboard header row:
/// ```text
///   main worktree ~/wt/wt1 (worktree of ~/proj) [Choose Ctrl+l]   ◆ 2 awaiting │ ⋮ 3 working │ ◇ 1 idle
pub(super) fn render_header(
    buf: &mut Buffer,
    area: Rect,
    theme: &Theme,
    rows: &[DashboardRow],
    state: &mut DashboardState,
    registry: &crate::actions::ActionRegistry,
    upgrade_cta: Option<HeaderUpgradeCta<'_>>,
) {
    use crate::views::agent_status::AgentStatusBar;

    if area.area() == 0 {
        return;
    }
    let bg = Style::default().bg(theme.bg_base);
    let dim = theme.dim().bg(theme.bg_base);
    buf.set_style(area, bg);

    // Count top-level rows per state. Subagents inherit their parent's group so we explicitly skip `indent > 0` rows.
    let mut awaiting = 0usize;
    let mut working = 0usize;
    let mut idle = 0usize;
    let mut done = 0usize;
    let mut failed = 0usize;
    for r in rows.iter().filter(|r| r.indent == 0) {
        match r.state {
            RowState::NeedsInput => awaiting += 1,
            RowState::Working => working += 1,
            RowState::Idle => idle += 1,
            // Inactive (roster-only) sessions get no header chip; the chips show actionable local state
            // The section header already carries the inactive count
            RowState::Inactive => {}
            RowState::Completed => done += 1,
            RowState::Failed => failed += 1,
        }
    }

    // Right-aligned chips, ordered like `RowState::group_priority` (awaiting leftmost).
    // Glyphs match per-row markers; the label keeps each chip readable when colour
    // is the only other cue.
    let frames = crate::glyphs::dot_spinner_frames();
    let spinner = frames[(state.spinner_tick / SPINNER_DIVISOR) as usize % frames.len()];
    let chip_specs = [
        (
            "awaiting",
            crate::glyphs::diamond_filled(),
            theme.warning,
            awaiting,
        ),
        ("working", spinner, theme.accent_running, working),
        (
            "idle",
            crate::glyphs::diamond_hollow(),
            theme.gray_dim,
            idle,
        ),
        (
            "done",
            crate::glyphs::diamond_filled(),
            theme.accent_success,
            done,
        ),
        (
            "failed",
            crate::glyphs::diamond_filled(),
            theme.accent_error,
            failed,
        ),
    ];
    let mut status = AgentStatusBar::new(theme);
    for (label, glyph, color, count) in chip_specs.into_iter().filter(|(_, _, _, count)| *count > 0)
    {
        status.push(
            label,
            Line::from(vec![
                Span::styled(glyph, bg.fg(color)),
                Span::styled(format!(" {count} {label}"), bg.fg(theme.gray)),
            ]),
        );
    }
    // Chips render right-aligned within the header so they share a right edge with the actions row below
    // Capture the per-chip rects so the left label's width budget stops short of the leftmost chip instead of painting over it
    let chip_rects = status.render(buf, area);

    // Paint the current location (git branch and cwd, with worktree label) on the left, mirroring the
    // welcome top bar and the agent status bar.
    let full_label_budget = chip_rects
        .values()
        .map(|r| r.x)
        .min()
        .map(|min_x| min_x.saturating_sub(3).saturating_sub(area.x))
        .unwrap_or(area.width) as usize;
    // Reserve the upgrade CTA (a lead space, the `[label]`, and the pinned-only `cta.caption`) so the location label truncates first
    // The shared painter then clamps to the space left, so it can't overpaint chips
    // The caption shows only for pinned CTAs
    let upgrade_caption = upgrade_cta.and_then(|cta| cta.pinned.then_some(cta.caption).flatten());
    let upgrade_reserve = upgrade_cta.map_or(0usize, |cta| {
        1 + crate::views::announcements::upgrade_cta_reserve(cta.label, upgrade_caption) as usize
    });
    let label_budget = full_label_budget.saturating_sub(upgrade_reserve);

    let LocationParts {
        branch,
        is_worktree,
        cwd_display,
    } = location_parts(&state.cwd);
    let mut location_spans: Vec<Span<'static>> = Vec::new();
    if let Some(branch) = branch {
        location_spans.push(Span::styled(branch, dim));
        location_spans.push(Span::styled(" ", bg));
    }
    if is_worktree {
        location_spans.push(worktree_badge(theme).patch_style(bg));
    }
    location_spans.push(Span::styled(cwd_display, bg.fg(theme.text_secondary)));
    let mut location = truncate_line(Line::from(location_spans), label_budget);
    // Underline on hover so the label reads as a click target (opens the location picker)
    // Underline only visible text: the whitespace separator between the branch and path parts stays bare
    // Hover is mouse-driven on the prior frame
    if state.location_hit.hovered {
        location.spans = underline_location_on_hover(std::mem::take(&mut location.spans));
    }
    let location_w = location.width() as u16;
    buf.set_line(area.x, area.y, &location, location_w);

    let mut choose_hint = hint_line(
        Span::styled("Choose", dim),
        chord_hint(
            theme,
            registry,
            crate::actions::ActionId::DashboardOpenLocationPicker,
        ),
    );
    choose_hint.spans.insert(0, Span::styled(" [", dim));
    choose_hint.spans.push(Span::styled("]", dim));
    let choose_hint_w = choose_hint.width() as u16;
    let hint_w = if (location_w + choose_hint_w) as usize <= label_budget {
        buf.set_line(area.x + location_w, area.y, &choose_hint, choose_hint_w);
        choose_hint_w
    } else {
        0
    };

    // Record the painted label (path plus hint) as a click target so the mouse handler can open the location picker
    // Width is clamped to the label budget so the hit area never extends under the chips
    let label_w = location_w + hint_w;
    let hit_w = label_w.min(label_budget as u16);
    if hit_w > 0 {
        state.location_hit.set(Some(Rect {
            x: area.x,
            y: area.y,
            width: hit_w,
            height: 1,
        }));
    }

    // Upgrade CTA painted right after the location label (free-tier upsell), clamped to the space left before the chips
    // The paint is a lead space then the shared clamping button painter
    // A pointer click opens it with the `Dashboard` CTA surface; Ctrl+O with `Keyboard`
    if let Some(HeaderUpgradeCta { label, .. }) = upgrade_cta {
        let avail = full_label_budget.saturating_sub(label_w as usize);
        if avail > 1 {
            let cta_x = area.x + label_w;
            buf.set_span(cta_x, area.y, &Span::styled(" ", bg), 1);
            let painted = crate::views::announcements::render_cta_button(
                buf,
                theme,
                cta_x + 1,
                area.y,
                (avail - 1) as u16,
                label,
                upgrade_caption,
                state.upgrade_cta_hit.hovered,
            );
            state.upgrade_cta_hit.set(painted);
        }
    }
}

/// Opacity that blends `gray_dim` toward the background for key hints: on GrokNight this lands on the palette's `FG_GUTTER` (`#414141`),
/// one step fainter than `gray_dim`, which is the colour the design uses for shortcut keys.
const KEY_HINT_BLEND: f32 = 0.66;

/// Style for the key part of a `label Key` hint: a shade fainter than `gray_dim` so the label reads first.
/// Falls back to the polarity-safe dim style on palettes that can't blend (named ANSI colours, the bandless terminal theme's `Reset` slots).
fn key_hint_style(theme: &Theme) -> Style {
    crate::render::color::blend_color(theme.bg_base, theme.gray_dim, KEY_HINT_BLEND)
        .map_or(theme.dim(), |c| Style::default().fg(c))
        .bg(theme.bg_base)
}

/// `{label} {hint}` from two pre-styled spans: the label, then the hint (a chord or a slash command, normally in [`key_hint_style`]).
/// A `None` hint paints only the label. Static labels and hints are borrowed, so a frame allocates only for chord displays.
fn hint_line(label: Span<'static>, hint: Option<Span<'static>>) -> Line<'static> {
    let mut spans = vec![label];
    if let Some(hint) = hint {
        spans.push(Span::styled(" ", hint.style));
        spans.push(hint);
    }
    Line::from(spans)
}

/// The dashboard chord bound to `id` as a key-hint span, or `None` when the action has no binding.
fn chord_hint(
    theme: &Theme,
    registry: &crate::actions::ActionRegistry,
    id: crate::actions::ActionId,
) -> Option<Span<'static>> {
    registry
        .key_for(id)
        .map(|key| Span::styled(key.display(), key_hint_style(theme)))
}

/// Apply the header location label's hover underline: underline only the visible text.
/// Whitespace-only spans (the separator between the branch and path parts) stay bare.
fn underline_location_on_hover(spans: Vec<Span<'static>>) -> Vec<Span<'static>> {
    spans
        .into_iter()
        .map(|span| {
            if span.content.chars().any(|c| !c.is_whitespace()) {
                span.patch_style(Style::default().add_modifier(Modifier::UNDERLINED))
            } else {
                span
            }
        })
        .collect()
}

/// Render the primary actions row below the header:
/// ```text
///   + New Agent                           Open Previous /resume │ Worktree Ctrl+w
pub(super) fn render_actions_row(
    buf: &mut Buffer,
    area: Rect,
    theme: &Theme,
    state: &mut DashboardState,
    registry: &crate::actions::ActionRegistry,
    workspace_dashboard_enabled: bool,
) {
    if area.area() == 0 {
        fall_back_from_unpainted_item(state);
        return;
    }
    let bg = Style::default().bg(theme.bg_base);
    let key_style = key_hint_style(theme);
    buf.set_style(area, bg);

    // Focused: light green `accent_success` (the affirmative "create a new session" colour), so the focus is obvious
    // Hovered (mouse over, not focused): brighter `text_primary` foreground so the button stands out under the cursor
    // Only the text colour changes on hover (no background fill)
    let button_fg = |focused: bool, hovered: bool, resting: Color| {
        if focused {
            theme.accent_success
        } else if hovered {
            theme.text_primary
        } else {
            resting
        }
    };

    // When worktree mode is on and the cwd is a git repo (so it can actually take effect), the next session goes in a fresh git worktree
    let worktree_armed = state.worktree_armed();
    let new_agent_label = if worktree_armed {
        "+ New Agent in Worktree"
    } else {
        "+ New Agent"
    };
    let new_agent_w = (UnicodeWidthStr::width(new_agent_label) as u16).min(area.width);

    // Right-hand items are laid out before `+ New Agent` so a focus fallback (below) can still colour that button in the same frame
    // Each right-hand item keeps a 2-cell gap from the `+ New Agent` button so the two sides never touch
    let mut right_edge = area.x + area.width;
    let left_limit = area.x + new_agent_w + 2;
    let fits = |right_edge: u16, w: u16| right_edge.checked_sub(w).is_some_and(|x| x >= left_limit);
    let place_right =
        |right_edge: &mut u16, line: &Line<'static>, buf: &mut Buffer| -> Option<Rect> {
            let w = line.width() as u16;
            if !fits(*right_edge, w) {
                return None;
            }
            let x = *right_edge - w;
            buf.set_line(x, area.y, line, w);
            *right_edge = x;
            Some(Rect {
                x,
                y: area.y,
                width: w,
                height: 1,
            })
        };

    let worktree_label = if worktree_armed {
        "Disable Worktree"
    } else {
        "Worktree"
    };
    let worktree_hint = hint_line(
        Span::styled(
            worktree_label,
            bg.fg(button_fg(
                state.worktree_toggle_focused(),
                state.worktree_toggle_hit.hovered,
                theme.gray,
            )),
        ),
        chord_hint(
            theme,
            registry,
            crate::actions::ActionId::DashboardToggleWorktree,
        ),
    );
    let worktree_rect = place_right(&mut right_edge, &worktree_hint, buf);
    state.worktree_toggle_hit.set(worktree_rect);

    // Strict right-to-left priority: once the worktree toggle is out, nothing to its left is tried either, so a narrower
    // `Open Previous` can never take the place of the (wider, armed) toggle
    if workspace_dashboard_enabled && worktree_rect.is_some() {
        // The session picker has no dashboard chord (`Ctrl+R` is rename here), so the hint names the slash command that opens it
        let open_previous = hint_line(
            Span::styled(
                "Open Previous",
                bg.fg(button_fg(
                    state.open_session_button_focused(),
                    state.open_session_button_hit.hovered,
                    theme.gray,
                )),
            ),
            Some(Span::styled("/resume", key_style)),
        );
        let divider = Line::from(Span::styled(" │ ", key_style));
        let needed = (open_previous.width() + divider.width()) as u16;
        let open_rect = fits(right_edge, needed).then(|| {
            place_right(&mut right_edge, &divider, buf);
            place_right(&mut right_edge, &open_previous, buf)
        });
        state.open_session_button_hit.set(open_rect.flatten());
    }
    fall_back_from_unpainted_item(state);

    let new_agent_fg = button_fg(
        state.new_agent_button_focused(),
        state.new_agent_button_hit.hovered,
        theme.text_secondary,
    );
    buf.set_string(
        area.x,
        area.y,
        crate::util::truncate_to_width(new_agent_label, new_agent_w as usize),
        bg.fg(new_agent_fg),
    );
    state.new_agent_button_hit.set(Some(Rect {
        x: area.x,
        y: area.y,
        width: new_agent_w,
        height: 1,
    }));
}

/// A cursor parked on a right-hand item needs a painted item under it; when this frame dropped the item (row too narrow, or no
/// actions row at all), the cursor falls back to `+ New Agent`, which is painted last and so shows the focus colour this frame.
fn fall_back_from_unpainted_item(state: &mut DashboardState) {
    if state
        .actions_focus
        .is_some_and(|item| item != ActionsFocus::NewAgent && !item.is_painted(state))
    {
        state.focus_new_agent_button();
    }
}

#[cfg(test)]
#[path = "chrome_tests.rs"]
mod tests;
