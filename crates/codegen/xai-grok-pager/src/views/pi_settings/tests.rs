//! Unit tests for the grok-pi settings panel.

use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::state::{Mode, ModeKind, Outcome, PiSettingsState, Row};
use super::{handle_key, render_pi_settings};
use crate::app::actions::Action;
use crate::settings::{PagerLocalSnapshot, SettingCategory, SettingKind, SettingsRegistry};

use xai_grok_shell::agent::config::UiConfig;

fn state() -> PiSettingsState {
    PiSettingsState::new(
        Arc::new(SettingsRegistry::defaults()),
        UiConfig::default(),
        PagerLocalSnapshot::default(),
    )
}

fn press(state: &mut PiSettingsState, code: KeyCode) -> Outcome {
    handle_key(state, &KeyEvent::new(code, KeyModifiers::NONE))
}

/// Render into a buffer and return its rows as plain strings.
fn render_lines(state: &mut PiSettingsState, width: u16, height: u16) -> Vec<String> {
    let area = Rect::new(0, 0, width, height);
    let mut buf = Buffer::empty(area);
    render_pi_settings(&mut buf, area, state, false);
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buf.cell((x, y)).map_or(" ", |c| c.symbol()).to_string())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}

// -- Row table --------------------------------------------------------------

#[test]
fn rows_open_each_tab_with_a_heading_then_its_sections() {
    let state = state();
    let Some(first) = state.rows.first() else {
        panic!("registry must produce rows")
    };
    assert!(
        matches!(first, Row::Tab { .. }),
        "the table must open with a tab heading, got {first:?}",
    );
    // Every section heading is immediately followed by at least one setting:
    // empty sections are never emitted.
    for (i, row) in state.rows.iter().enumerate() {
        if matches!(row, Row::Section { .. }) {
            assert!(
                state.rows.get(i + 1).is_some_and(Row::is_setting),
                "section heading at {i} is not followed by a setting",
            );
        }
    }
}

#[test]
fn browsing_shows_only_the_active_tab_and_hides_tab_headings() {
    let state = state();
    let active = state.active_category().expect("a tab must be active");
    for &row in &state.visible {
        match &state.rows[row] {
            Row::Tab { .. } => panic!("tab headings must be hidden while browsing"),
            Row::Section { category, .. } => assert_eq!(*category, active),
            Row::Setting { key, .. } => {
                let meta = state.meta(key).expect("visible key must be registered");
                assert_eq!(meta.category, active, "`{key}` leaked from another tab");
            }
        }
    }
}

#[test]
fn tabs_cover_every_category_that_has_visible_rows() {
    let state = state();
    let expected: Vec<SettingCategory> = SettingCategory::ALL
        .iter()
        .copied()
        .filter(|cat| {
            state
                .rows
                .iter()
                .any(|r| matches!(r, Row::Tab { category } if category == cat))
        })
        .collect();
    assert_eq!(state.tabs, expected, "tab order must follow ALL order");
    assert!(!state.tabs.is_empty(), "at least one tab must render");
}

#[test]
fn opening_focuses_the_first_setting_not_a_heading() {
    let state = state();
    assert!(
        state.rows[state.selected].is_setting(),
        "the cursor must open on a selectable row",
    );
}

// -- Navigation -------------------------------------------------------------

#[test]
fn stepping_skips_headings_and_stops_at_the_ends() {
    let mut state = state();
    let first = state.selected;
    assert!(!state.step(-1), "already at the top; must not wrap");
    assert_eq!(state.selected, first);

    let mut steps = 0;
    while state.step(1) {
        assert!(
            state.rows[state.selected].is_setting(),
            "stepping must never land on a heading",
        );
        steps += 1;
        assert!(steps < 500, "step(1) failed to terminate");
    }
    assert!(steps > 0, "the first tab must have more than one row");
    assert!(!state.step(1), "already at the bottom; must not wrap");
}

#[test]
fn horizontal_keys_cycle_tabs_and_reset_the_focus() {
    let mut state = state();
    let first = state.active_category();
    assert!(matches!(press(&mut state, KeyCode::Right), Outcome::Changed));
    assert_ne!(state.active_category(), first, "→ must move to the next tab");
    assert!(
        state.rows[state.selected].is_setting(),
        "switching tabs must land on a selectable row",
    );
    assert!(matches!(press(&mut state, KeyCode::Left), Outcome::Changed));
    assert_eq!(state.active_category(), first, "← must come back");
}

#[test]
fn tab_cycling_wraps_at_both_ends() {
    let mut state = state();
    let first = state.active_category();
    for _ in 0..state.tabs.len() {
        state.cycle_tab(1);
    }
    assert_eq!(state.active_category(), first, "a full cycle must return home");
    state.cycle_tab(-1);
    assert_eq!(
        state.active_category(),
        state.tabs.last().copied(),
        "stepping back from the first tab must wrap to the last",
    );
}

#[test]
fn tab_key_toggles_section_focus_and_up_down_then_jump_sections() {
    let mut state = state();
    assert!(
        state.sections().len() >= 2,
        "the Appearance tab needs several sections for this test",
    );
    assert!(matches!(press(&mut state, KeyCode::Tab), Outcome::Changed));
    assert!(state.section_focus);

    let before = state.active_section();
    press(&mut state, KeyCode::Down);
    assert_ne!(
        state.active_section(),
        before,
        "with section focus, Down must jump a whole section",
    );

    press(&mut state, KeyCode::Tab);
    assert!(!state.section_focus, "Tab must hand focus back to the rows");
}

#[test]
fn section_focus_needs_at_least_two_sections() {
    let mut state = state();
    let privacy = state
        .tabs
        .iter()
        .position(|c| *c == SettingCategory::Privacy)
        .expect("Privacy tab must exist");
    state.set_active_tab(privacy);
    assert_eq!(state.sections().len(), 1);
    assert!(
        !state.toggle_section_focus(),
        "a single-section tab has nowhere to hop",
    );
    assert!(!state.section_focus);
}

#[test]
fn focus_key_switches_to_the_owning_tab() {
    let mut state = state();
    assert!(state.focus_key("coding_data_sharing"));
    assert_eq!(state.active_category(), Some(SettingCategory::Privacy));
    assert_eq!(
        state.focused().map(|(key, _)| key),
        Some("coding_data_sharing"),
    );
    assert!(
        state.visible.contains(&state.selected),
        "a deep-linked row must be visible, not filtered out",
    );
}

// -- Search -----------------------------------------------------------------

#[test]
fn search_spans_every_tab_and_groups_results_by_tab_heading() {
    let mut state = state();
    press(&mut state, KeyCode::Char('/'));
    assert_eq!(state.mode.kind(), ModeKind::Search);
    state.set_query("model");

    let headings = state
        .visible
        .iter()
        .filter(|&&r| matches!(state.rows[r], Row::Tab { .. }))
        .count();
    assert!(headings >= 1, "results must be grouped under tab headings");
    assert!(
        state
            .visible
            .iter()
            .all(|&r| !matches!(state.rows[r], Row::Section { .. })),
        "section headings are suppressed in search results",
    );
    let matches = state
        .visible
        .iter()
        .filter(|&&r| state.rows[r].is_setting())
        .count();
    assert!(matches > 0, "`model` must match something");
}

#[test]
fn a_tab_heading_is_emitted_once_per_tab_and_only_when_it_matches() {
    let mut state = state();
    state.set_query("theme");
    let mut seen = Vec::new();
    for &row in &state.visible {
        if let Row::Tab { category } = state.rows[row] {
            assert!(
                !seen.contains(&category),
                "{category:?} heading emitted twice",
            );
            seen.push(category);
        }
    }
    // Every heading must be followed by at least one match, never left dangling.
    for (position, &row) in state.visible.iter().enumerate() {
        if matches!(state.rows[row], Row::Tab { .. }) {
            let next = state.visible.get(position + 1).map(|&r| &state.rows[r]);
            assert!(
                next.is_some_and(|r| r.is_setting()),
                "a tab heading with no matches must not be emitted",
            );
        }
    }
}

#[test]
fn escaping_search_lands_on_the_tab_owning_the_focused_result() {
    let mut state = state();
    press(&mut state, KeyCode::Char('/'));
    state.set_query("coding data");
    state.select_first_visible();
    let landed = state
        .rows
        .get(state.selected)
        .and_then(|_| state.focused().map(|(key, _)| key));
    assert_eq!(landed, Some("coding_data_sharing"));

    press(&mut state, KeyCode::Esc);
    assert_eq!(state.mode.kind(), ModeKind::Browse);
    assert!(!state.searching(), "Esc must clear the query");
    assert_eq!(
        state.active_category(),
        Some(SettingCategory::Privacy),
        "Esc must land on the tab owning the focused result",
    );
}

/// The modal chrome closes on Esc unless the panel claims it first. Anything
/// with a local "back out" meaning must claim it, or Esc dismisses the panel
/// instead of undoing one step.
#[test]
fn escape_belongs_to_the_panel_while_searching_or_focused_on_sections() {
    let mut state = state();
    assert!(!state.owns_escape(), "plain browse lets the chrome close");

    press(&mut state, KeyCode::Char('/'));
    assert!(state.owns_escape(), "search mode owns Esc");
    press(&mut state, KeyCode::Esc);

    state.set_query("theme");
    assert!(state.owns_escape(), "a committed query still owns Esc");
    press(&mut state, KeyCode::Esc);
    assert!(!state.searching(), "Esc must clear a committed query");
    assert!(!state.owns_escape(), "and hand Esc back to the chrome");

    state.toggle_section_focus();
    assert!(state.owns_escape(), "section focus owns Esc");
    press(&mut state, KeyCode::Esc);
    assert!(!state.section_focus, "Esc must leave section focus");

    state.focus_key("theme");
    press(&mut state, KeyCode::Enter);
    assert!(state.owns_escape(), "an open chooser owns Esc");
}

#[test]
fn switching_tabs_abandons_an_active_search() {
    let mut state = state();
    state.set_query("theme");
    assert!(state.searching());
    state.cycle_tab(1);
    assert!(!state.searching(), "a tab switch leaves search behind");
    assert_eq!(state.mode.kind(), ModeKind::Browse);
}

// -- Activation -------------------------------------------------------------

#[test]
fn space_toggles_the_focused_bool() {
    let mut state = state();
    assert!(state.focus_key("compact_mode"));
    let Outcome::Action(action) = press(&mut state, KeyCode::Char(' ')) else {
        panic!("Space on a Bool row must dispatch an action")
    };
    assert!(
        matches!(action, Action::SetCompactMode(true)),
        "expected SetCompactMode(true), got {action:?}",
    );
}

#[test]
fn enter_on_an_enum_row_opens_the_chooser_on_the_current_value() {
    let mut state = state();
    assert!(state.focus_key("theme"));
    assert!(matches!(press(&mut state, KeyCode::Enter), Outcome::Changed));
    let Mode::Picking { key, index, .. } = state.mode else {
        panic!("Enter on an Enum row must open the chooser")
    };
    assert_eq!(key, "theme");
    let choices = state.choices_for("theme");
    let current = state.value_of("theme");
    let expected = match current {
        Some(crate::settings::SettingValue::Enum(canonical)) => {
            choices.iter().position(|c| c.canonical == canonical).unwrap_or(0)
        }
        _ => 0,
    };
    assert_eq!(index, expected, "the chooser must open on the live value");
}

#[test]
fn chooser_enter_commits_and_escape_returns_to_browse() {
    let mut state = state();
    state.focus_key("theme");
    press(&mut state, KeyCode::Enter);
    press(&mut state, KeyCode::Down);
    let Outcome::Action(action) = press(&mut state, KeyCode::Enter) else {
        panic!("Enter in the chooser must commit")
    };
    assert!(matches!(action, Action::SetTheme(_)));
    assert_eq!(state.mode.kind(), ModeKind::Browse);

    state.focus_key("theme");
    press(&mut state, KeyCode::Enter);
    press(&mut state, KeyCode::Esc);
    assert_eq!(state.mode.kind(), ModeKind::Browse, "Esc must back out");
}

#[test]
fn stepping_a_preview_enum_dispatches_a_preview_not_a_commit() {
    let mut state = state();
    state.focus_key("theme");
    press(&mut state, KeyCode::Enter);
    let Outcome::Action(action) = press(&mut state, KeyCode::Down) else {
        panic!("a preview-capable chooser must preview on Down")
    };
    assert!(
        matches!(action, Action::PreviewTheme(_)),
        "expected PreviewTheme, got {action:?}",
    );
}

#[test]
fn side_model_slots_defer_to_the_native_model_picker() {
    let mut state = state();
    assert!(state.focus_key("recap_models"));
    // The group sheet holds the slots; open it, then activate a slot.
    assert!(matches!(press(&mut state, KeyCode::Enter), Outcome::Changed));
    assert_eq!(state.mode.kind(), ModeKind::PickingGroup);
}

#[test]
fn enter_on_an_int_row_opens_the_stepper_seeded_with_the_live_value() {
    let mut state = state();
    assert!(state.focus_key("max_thoughts_width"));
    assert!(matches!(press(&mut state, KeyCode::Enter), Outcome::Changed));
    let Mode::EditingInt { buffer, min, max, .. } = &state.mode else {
        panic!("Enter on an Int row must open the stepper")
    };
    assert!(min < max);
    assert!(
        buffer.parse::<i64>().is_ok_and(|v| v >= *min && v <= *max),
        "the stepper must open inside its own bounds, got {buffer:?}",
    );
}

#[test]
fn the_int_stepper_clamps_at_both_bounds() {
    let mut state = state();
    state.focus_key("max_thoughts_width");
    press(&mut state, KeyCode::Enter);
    let Mode::EditingInt { min, max, .. } = state.mode else {
        panic!("stepper must be open")
    };
    for _ in 0..500 {
        press(&mut state, KeyCode::Left);
    }
    assert_eq!(state.mode_int_buffer(), Some(min.to_string()));
    for _ in 0..500 {
        press(&mut state, KeyCode::Right);
    }
    assert_eq!(state.mode_int_buffer(), Some(max.to_string()));
}

#[test]
fn d_asks_before_resetting_and_n_backs_out() {
    let mut state = state();
    state.focus_key("compact_mode");
    assert!(matches!(press(&mut state, KeyCode::Char('d')), Outcome::Changed));
    assert_eq!(state.mode.kind(), ModeKind::ConfirmReset);

    assert!(matches!(press(&mut state, KeyCode::Char('n')), Outcome::Changed));
    assert_eq!(state.mode.kind(), ModeKind::Browse);

    press(&mut state, KeyCode::Char('d'));
    let Outcome::Action(action) = press(&mut state, KeyCode::Char('y')) else {
        panic!("y must dispatch the reset")
    };
    assert!(matches!(action, Action::SetCompactMode(_)));
    assert_eq!(state.mode.kind(), ModeKind::Browse);
}

#[test]
fn f2_closes_from_every_mode() {
    for open in [KeyCode::Char('/'), KeyCode::Enter, KeyCode::Char('d')] {
        let mut state = state();
        state.focus_key("theme");
        press(&mut state, open);
        assert!(
            matches!(press(&mut state, KeyCode::F(2)), Outcome::Close),
            "F2 must close from {:?}",
            state.mode.kind(),
        );
    }
}

// -- Rendering --------------------------------------------------------------

#[test]
fn the_wide_layout_draws_the_section_sidebar_and_the_divider() {
    let mut state = state();
    let lines = render_lines(&mut state, 140, 40);
    let body = lines.join("\n");
    assert!(
        body.contains('\u{2502}'),
        "the wide layout must draw a sidebar divider:\n{body}",
    );
    for (name, _) in state.sections() {
        assert!(
            body.contains(name),
            "sidebar must list section `{name}`:\n{body}",
        );
    }
}

#[test]
fn the_narrow_layout_drops_the_sidebar_for_inline_headings() {
    let mut state = state();
    let lines = render_lines(&mut state, 60, 30);
    let body = lines.join("\n");
    // Section names still appear, now as inline headings above their rows.
    let first = state.sections().first().map(|(name, _)| *name);
    if let Some(first) = first {
        assert!(
            body.contains(first),
            "narrow layout must keep section names inline:\n{body}",
        );
    }
}

#[test]
fn the_focused_rows_description_renders_in_the_fixed_block() {
    let mut state = state();
    state.focus_key("compact_mode");
    let meta = state.meta("compact_mode").expect("registered");
    let first_word = meta
        .description
        .split_whitespace()
        .next()
        .expect("description must not be empty");
    let body = render_lines(&mut state, 140, 40).join("\n");
    assert!(
        body.contains(first_word),
        "the description block must show the focused row's description:\n{body}",
    );
}

#[test]
fn the_tab_bar_lists_every_tab_label() {
    let mut state = state();
    let body = render_lines(&mut state, 140, 40).join("\n");
    for label in state.tab_labels() {
        assert!(body.contains(label), "tab bar must show `{label}`:\n{body}");
    }
}

/// `Group` rows carry no scalar value — the registry deliberately gives
/// `current_value_for` no arm for them. They must render as navigation rows,
/// not as the marker that flags a genuinely missing read mapping.
#[test]
fn group_rows_render_as_navigation_not_as_a_skew_marker() {
    let registry = SettingsRegistry::defaults();
    let groups: Vec<(&'static str, &'static str)> = registry
        .all()
        .iter()
        .filter(|meta| matches!(meta.kind, SettingKind::Group { .. }))
        // `external_only` rows are gated on a process-global profile flag that
        // this test must not flip out from under parallel tests. The row
        // painter does not branch on the gate, so the rest still covers it.
        .filter(|meta| !meta.external_only)
        .map(|meta| (meta.key, meta.label))
        .collect();
    assert!(!groups.is_empty(), "the registry must define group rows");

    for (key, label) in groups {
        let mut state = state();
        assert!(state.focus_key(key), "`{key}` must have a row");
        let body = render_lines(&mut state, 140, 40).join("\n");
        assert!(body.contains(label), "`{key}` must render its label:\n{body}");
        assert!(
            !body.contains("no read mapping"),
            "`{key}` rendered the registry-skew marker:\n{body}",
        );
    }
}

#[test]
fn rendering_survives_a_terminal_far_too_small_to_draw() {
    let mut state = state();
    for (w, h) in [(1, 1), (10, 3), (20, 6), (40, 2)] {
        let area = Rect::new(0, 0, w, h);
        let mut buf = Buffer::empty(area);
        render_pi_settings(&mut buf, area, &mut state, false);
    }
}

#[test]
fn every_visible_row_gets_a_hit_rect() {
    let mut state = state();
    render_lines(&mut state, 140, 40);
    let painted = state
        .visible
        .iter()
        .filter(|&&r| state.rows[r].is_setting())
        .filter(|&&r| state.row_rects[r].height > 0)
        .count();
    assert!(painted > 0, "rows must record hit rects for the mouse");
}

impl PiSettingsState {
    /// Test-only view of the Int stepper's buffer.
    fn mode_int_buffer(&self) -> Option<String> {
        match &self.mode {
            Mode::EditingInt { buffer, .. } => Some(buffer.clone()),
            _ => None,
        }
    }
}
