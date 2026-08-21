//! Keyboard and mouse handling for the grok-pi settings panel.
//!
//! F2 and Ctrl/Cmd+`,` close from any mode. Esc is mode-dependent: Browse
//! delegates to the modal chrome, every sub-mode returns to Browse.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::layout::Rect;

use super::actions;
use super::state::{Mode, ModeKind, Outcome, PiSettingsState, Row};
use crate::app::actions::Action;
use crate::input::line_editor::LineEditOutcome;
use crate::settings::{SettingKind, SettingValue};

/// Int stepper adornments, also used by the renderer for hit rects.
pub(super) const STEPPER_LEFT: &str = "\u{2039}";
pub(super) const STEPPER_RIGHT: &str = "\u{203A}";

/// Fast-scroll distance for PageUp/PageDown inside sub-panes.
const PAGE_STEP: isize = 10;

// ---------------------------------------------------------------------------
// Keys
// ---------------------------------------------------------------------------

pub fn handle_key(state: &mut PiSettingsState, key: &KeyEvent) -> Outcome {
    if is_close_key(key) {
        return Outcome::Close;
    }
    match state.mode.kind() {
        ModeKind::Browse => browse_key(state, key),
        ModeKind::Search => search_key(state, key),
        ModeKind::Picking => chooser_key(state, key),
        ModeKind::PickingGroup => group_key(state, key),
        ModeKind::EditingString => string_key(state, key),
        ModeKind::EditingInt => int_key(state, key),
        ModeKind::ConfirmReset => confirm_key(state, key),
    }
}

pub fn handle_paste(state: &mut PiSettingsState, text: &str) -> Outcome {
    match &mut state.mode {
        Mode::EditingString { editor, .. } => {
            editor.insert_paste_with_policy(text, safe_char, usize::MAX);
            revalidate_string(state);
            Outcome::Changed
        }
        Mode::Search => {
            let outcome = state
                .query
                .insert_paste_with_policy(text, safe_char, usize::MAX);
            apply_query_edit(state, outcome)
        }
        _ => Outcome::Unchanged,
    }
}

/// Esc is deliberately not matched here: the modal chrome intercepts it in
/// Browse and returns `CloseRequested` before this sees the event.
fn is_close_key(key: &KeyEvent) -> bool {
    key.code == KeyCode::F(2)
        || (key.code == KeyCode::Char(',')
            && (key.modifiers.contains(KeyModifiers::CONTROL)
                || key.modifiers.contains(KeyModifiers::SUPER)))
}

fn browse_key(state: &mut PiSettingsState, key: &KeyEvent) -> Outcome {
    // While the sidebar owns the keyboard, Up/Down jump whole sections and
    // Tab/Enter/Esc hand focus back to the rows.
    if state.section_focus {
        return match key.code {
            KeyCode::Down | KeyCode::Char('j') => Outcome::changed_if(state.jump_section(1)),
            KeyCode::Up | KeyCode::Char('k') => Outcome::changed_if(state.jump_section(-1)),
            KeyCode::Left | KeyCode::Char('h') => Outcome::changed_if(state.cycle_tab(-1)),
            KeyCode::Right | KeyCode::Char('l') => Outcome::changed_if(state.cycle_tab(1)),
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Enter | KeyCode::Esc => {
                state.section_focus = false;
                Outcome::Changed
            }
            _ => Outcome::Unchanged,
        };
    }

    match key.code {
        // Reached only while a committed query is still filtering the list —
        // otherwise the modal chrome claims Esc and closes the panel.
        KeyCode::Esc if state.searching() => {
            state.sync_tab_to_selection();
            state.query.reset();
            state.refresh_visible();
            state.clamp_selection();
            Outcome::Changed
        }
        KeyCode::Down | KeyCode::Char('j') => Outcome::changed_if(state.step(1)),
        KeyCode::Up | KeyCode::Char('k') => Outcome::changed_if(state.step(-1)),
        // Descriptions live in the fixed block under the list, which frees the
        // horizontal keys to move between tabs.
        KeyCode::Left | KeyCode::Char('h') if key.modifiers.is_empty() => {
            Outcome::changed_if(state.cycle_tab(-1))
        }
        KeyCode::Right | KeyCode::Char('l') if key.modifiers.is_empty() => {
            Outcome::changed_if(state.cycle_tab(1))
        }
        // Tab parks the cursor on the section sidebar for fast hopping.
        KeyCode::Tab | KeyCode::BackTab => Outcome::changed_if(state.toggle_section_focus()),
        KeyCode::PageDown => Outcome::changed_if(state.jump_section(1)),
        KeyCode::PageUp => Outcome::changed_if(state.jump_section(-1)),
        KeyCode::Char('g') if key.modifiers.is_empty() => Outcome::changed_if(state.jump_end(-1)),
        KeyCode::Char('G') => Outcome::changed_if(state.jump_end(1)),
        KeyCode::Char(' ') => match state.toggle_focused_bool() {
            Some(action) => Outcome::Action(action),
            None => Outcome::Unchanged,
        },
        KeyCode::Enter => activate_focused(state),
        // `i` aliases `/`, matching the shared pickers' vim-nav convention.
        KeyCode::Char('/') | KeyCode::Char('i') if key.modifiers.is_empty() => {
            state.to_search();
            Outcome::Changed
        }
        KeyCode::Char('d') if key.modifiers.is_empty() => match state.focused() {
            // Groups have no scalar default, and a locked row is not the
            // user's to change.
            Some((_, meta)) if matches!(meta.kind, SettingKind::Group { .. }) => Outcome::Unchanged,
            Some((key, _)) if state.lock(key).is_some() => Outcome::Unchanged,
            Some((key, _)) => {
                state.mode = Mode::ConfirmReset { key };
                Outcome::Changed
            }
            None => Outcome::Unchanged,
        },
        KeyCode::Backspace => {
            // Keep editing a committed query without re-entering search mode.
            if state.query().is_empty() {
                return Outcome::Unchanged;
            }
            let outcome = state.query.delete_last_grapheme();
            apply_query_edit(state, outcome)
        }
        _ => Outcome::Unchanged,
    }
}

/// Open whatever the focused row leads to: a toggle, a chooser, a sub-sheet,
/// an editor, or another modal entirely.
fn activate_focused(state: &mut PiSettingsState) -> Outcome {
    let Some((key, _)) = state.focused() else {
        return Outcome::Unchanged;
    };
    // Pi resources has its own modal.
    if key == "pi_config" {
        return Outcome::Action(Action::OpenPiConfig);
    }
    if state.open_group() {
        return Outcome::Changed;
    }
    // Side-model slots always use the native searchable /model picker, never
    // this panel's chooser, so the catalog lives in one place.
    if let Some(action) = actions::side_model_picker_action(key) {
        return Outcome::Action(action);
    }
    // Enter on a Bool behaves like Space.
    if let Some(action) = state.toggle_focused_bool() {
        return Outcome::Action(action);
    }
    if state.open_chooser() || state.open_editor() {
        return Outcome::Changed;
    }
    Outcome::Unchanged
}

fn search_key(state: &mut PiSettingsState, key: &KeyEvent) -> Outcome {
    match key.code {
        KeyCode::Esc => {
            if state.searching() {
                // Search doubles as navigation: land on the tab owning the
                // focused result rather than snapping back to the start.
                state.sync_tab_to_selection();
                state.query.reset();
                state.refresh_visible();
                state.clamp_selection();
            }
            state.to_browse();
            Outcome::Changed
        }
        // Commit the query and return to Browse with it still applied, so the
        // user can immediately toggle the focused result.
        KeyCode::Enter => {
            state.to_browse();
            Outcome::Changed
        }
        KeyCode::Down => {
            let moved = state.step(1);
            state.sync_tab_to_selection();
            Outcome::changed_if(moved)
        }
        KeyCode::Up => {
            let moved = state.step(-1);
            state.sync_tab_to_selection();
            Outcome::changed_if(moved)
        }
        KeyCode::PageDown | KeyCode::PageUp => {
            let delta = if key.code == KeyCode::PageDown { 1 } else { -1 };
            let mut moved = false;
            for _ in 0..PAGE_STEP {
                moved |= state.step(delta);
            }
            state.sync_tab_to_selection();
            Outcome::changed_if(moved)
        }
        KeyCode::Tab => Outcome::Unchanged,
        KeyCode::Char('u') if key.modifiers == KeyModifiers::CONTROL => {
            if state.searching() {
                state.query.reset();
                state.refresh_visible();
                state.clamp_selection();
            }
            Outcome::Changed
        }
        _ => {
            let outcome = state.query.handle_key_with_insert_policy(key, safe_char);
            apply_query_edit(state, outcome)
        }
    }
}

fn safe_char(character: char) -> bool {
    !crate::render::line_utils::is_unsafe_display_char(character)
}

fn apply_query_edit(state: &mut PiSettingsState, outcome: LineEditOutcome) -> Outcome {
    match outcome {
        LineEditOutcome::TextChanged => {
            state.refresh_visible();
            state.clamp_selection();
            state.sync_tab_to_selection();
            Outcome::Changed
        }
        LineEditOutcome::HandledNoChange | LineEditOutcome::CursorChanged => Outcome::Changed,
        LineEditOutcome::Unhandled => Outcome::Unchanged,
    }
}

fn chooser_key(state: &mut PiSettingsState, key: &KeyEvent) -> Outcome {
    let Mode::Picking {
        key: setting,
        index,
        ref original,
        supports_preview,
        ..
    } = state.mode
    else {
        return Outcome::Unchanged;
    };
    let original = original.clone();
    let choices = state.choices_for(setting);
    if choices.is_empty() {
        state.to_browse();
        return Outcome::Changed;
    }

    match key.code {
        KeyCode::Down | KeyCode::Char('j') => {
            move_choice(state, index, choices.len() as isize, 1, supports_preview)
        }
        KeyCode::Up | KeyCode::Char('k') => {
            move_choice(state, index, choices.len() as isize, -1, supports_preview)
        }
        KeyCode::Enter => {
            let canonical = choices[index.min(choices.len() - 1)].canonical.clone();
            // A deep-link open (`/privacy`) is only about this one choice, so
            // committing it dismisses the panel rather than dropping the user
            // into a browse list they never asked for.
            let close = state.close_on_picker_exit;
            state.to_browse();
            match (actions::action_for_enum_commit(setting, &canonical), close) {
                (Some(action), true) => Outcome::ActionThenClose(action),
                (Some(action), false) => Outcome::Action(action),
                (None, true) => Outcome::Close,
                (None, false) => Outcome::Changed,
            }
        }
        KeyCode::Esc => {
            let close = state.close_on_picker_exit;
            let revert = supports_preview
                .then(|| revert_action(setting, &original))
                .flatten();
            state.to_browse();
            match (revert, close) {
                (Some(action), true) => Outcome::ActionThenClose(action),
                (Some(action), false) => Outcome::Action(action),
                (None, true) => Outcome::Close,
                (None, false) => Outcome::Changed,
            }
        }
        _ => Outcome::Unchanged,
    }
}

/// Step the chooser cursor, firing a live preview when the setting supports it.
fn move_choice(
    state: &mut PiSettingsState,
    index: usize,
    len: isize,
    delta: isize,
    supports_preview: bool,
) -> Outcome {
    let next = (index as isize + delta).clamp(0, len - 1) as usize;
    if next == index {
        return Outcome::Unchanged;
    }
    let (setting, canonical) = {
        let Mode::Picking { key, .. } = state.mode else {
            return Outcome::Unchanged;
        };
        let choices = state.choices_for(key);
        (key, choices[next].canonical.clone())
    };
    if let Mode::Picking {
        index: i, scroll, ..
    } = &mut state.mode
    {
        *i = next;
        // Keep the focused choice inside the rendered window.
        if next < *scroll {
            *scroll = next;
        }
    }
    match supports_preview.then(|| actions::action_for_enum_preview(setting, &canonical)) {
        Some(Some(action)) => Outcome::Action(action),
        _ => Outcome::Changed,
    }
}

/// Preview action that restores the value a chooser started from.
fn revert_action(key: &'static str, original: &SettingValue) -> Option<Action> {
    let canonical = match original {
        SettingValue::Enum(canonical) => (*canonical).to_string(),
        SettingValue::String(text) => text.clone(),
        _ => return None,
    };
    actions::action_for_enum_preview(key, &canonical)
}

fn group_key(state: &mut PiSettingsState, key: &KeyEvent) -> Outcome {
    let Mode::PickingGroup { key: group, child } = state.mode else {
        return Outcome::Unchanged;
    };
    let children = state.group_children(group);
    if children.is_empty() {
        state.to_browse();
        return Outcome::Changed;
    }
    match key.code {
        KeyCode::Down | KeyCode::Char('j') | KeyCode::Up | KeyCode::Char('k') => {
            let delta: isize = matches!(key.code, KeyCode::Down | KeyCode::Char('j'))
                .then_some(1)
                .unwrap_or(-1);
            let next = (child as isize + delta).clamp(0, children.len() as isize - 1) as usize;
            if next == child {
                return Outcome::Unchanged;
            }
            state.mode = Mode::PickingGroup {
                key: group,
                child: next,
            };
            Outcome::Changed
        }
        // The sheet stays open so several toggles can be flipped in a row.
        KeyCode::Enter | KeyCode::Char(' ') => toggle_group_child(state, children[child]),
        KeyCode::Esc => {
            state.to_browse();
            Outcome::Changed
        }
        _ => Outcome::Unchanged,
    }
}

fn toggle_group_child(state: &PiSettingsState, child: &'static str) -> Outcome {
    match state.value_of(child) {
        Some(SettingValue::Bool(current)) => match actions::action_for_bool(child, !current) {
            Some(action) => Outcome::Action(action),
            None => Outcome::Unchanged,
        },
        _ => Outcome::Unchanged,
    }
}

fn string_key(state: &mut PiSettingsState, key: &KeyEvent) -> Outcome {
    let Mode::EditingString {
        key: setting,
        ref editor,
        ref error,
        ..
    } = state.mode
    else {
        return Outcome::Unchanged;
    };
    match key.code {
        KeyCode::Esc => {
            state.to_browse();
            Outcome::Changed
        }
        KeyCode::Enter => {
            if error.is_some() {
                return Outcome::Unchanged;
            }
            let value = editor.text().to_string();
            state.to_browse();
            match actions::action_for_string(setting, value, &state.pager_snapshot) {
                Some(action) => Outcome::Action(action),
                None => Outcome::Changed,
            }
        }
        _ => {
            let Mode::EditingString { editor, .. } = &mut state.mode else {
                return Outcome::Unchanged;
            };
            let outcome = editor.handle_key_with_insert_policy(key, safe_char);
            match outcome {
                LineEditOutcome::TextChanged => {
                    revalidate_string(state);
                    Outcome::Changed
                }
                LineEditOutcome::HandledNoChange | LineEditOutcome::CursorChanged => {
                    Outcome::Changed
                }
                LineEditOutcome::Unhandled => Outcome::Unchanged,
            }
        }
    }
}

fn revalidate_string(state: &mut PiSettingsState) {
    let models = state.pager_snapshot.available_models.clone();
    if let Mode::EditingString {
        editor,
        validator,
        error,
        ..
    } = &mut state.mode
    {
        *error = actions::validate_string(*validator, editor.text(), &models);
    }
}

fn int_key(state: &mut PiSettingsState, key: &KeyEvent) -> Outcome {
    let Mode::EditingInt {
        key: setting,
        ref buffer,
        min,
        max,
    } = state.mode
    else {
        return Outcome::Unchanged;
    };
    let current: i64 = buffer.parse().unwrap_or(min);
    let (small, large) = step_sizes(min, max);
    match key.code {
        KeyCode::Esc => {
            state.to_browse();
            Outcome::Changed
        }
        KeyCode::Enter => {
            let value = current.clamp(min, max);
            state.to_browse();
            match actions::action_for_int(setting, value) {
                Some(action) => Outcome::Action(action),
                None => Outcome::Changed,
            }
        }
        KeyCode::Left | KeyCode::Char('h') => set_int(state, current - small, min, max),
        KeyCode::Right | KeyCode::Char('l') => set_int(state, current + small, min, max),
        KeyCode::Down | KeyCode::PageDown => set_int(state, current - large, min, max),
        KeyCode::Up | KeyCode::PageUp => set_int(state, current + large, min, max),
        KeyCode::Home => set_int(state, min, min, max),
        KeyCode::End => set_int(state, max, min, max),
        KeyCode::Backspace => {
            let mut next = buffer.clone();
            next.pop();
            update_int_buffer(state, next);
            Outcome::Changed
        }
        KeyCode::Char(c) if c.is_ascii_digit() => {
            let candidate = format!("{buffer}{c}");
            // Reject overlong input rather than silently clamping mid-typing.
            match candidate.parse::<i64>() {
                Ok(value) if value <= max => {
                    update_int_buffer(state, candidate);
                    Outcome::Changed
                }
                _ => Outcome::Unchanged,
            }
        }
        _ => Outcome::Unchanged,
    }
}

fn set_int(state: &mut PiSettingsState, value: i64, min: i64, max: i64) -> Outcome {
    let clamped = value.clamp(min, max);
    let next = clamped.to_string();
    if state.mode_buffer() == Some(next.as_str()) {
        return Outcome::Unchanged;
    }
    update_int_buffer(state, next);
    Outcome::Changed
}

fn update_int_buffer(state: &mut PiSettingsState, next: String) {
    if let Mode::EditingInt { buffer, .. } = &mut state.mode {
        *buffer = next;
    }
}

/// Stepper increments derived from the range: wide ranges step by 5/10, narrow
/// ones by 1/5, so both feel proportionate.
pub(super) fn step_sizes(min: i64, max: i64) -> (i64, i64) {
    if max - min >= 100 { (5, 10) } else { (1, 5) }
}

fn confirm_key(state: &mut PiSettingsState, key: &KeyEvent) -> Outcome {
    let Mode::ConfirmReset { key: setting } = state.mode else {
        return Outcome::Unchanged;
    };
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
            let action = state.reset_action(setting);
            state.to_browse();
            match action {
                Some(action) => Outcome::Action(action),
                None => Outcome::Changed,
            }
        }
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
            state.to_browse();
            Outcome::Changed
        }
        _ => Outcome::Unchanged,
    }
}

// ---------------------------------------------------------------------------
// Mouse
// ---------------------------------------------------------------------------

pub fn handle_mouse(
    state: &mut PiSettingsState,
    kind: MouseEventKind,
    column: u16,
    row: u16,
) -> Outcome {
    match state.mode.kind() {
        ModeKind::Picking | ModeKind::PickingGroup => choice_mouse(state, kind, column, row),
        ModeKind::EditingInt => stepper_mouse(state, kind, column, row),
        ModeKind::EditingString | ModeKind::ConfirmReset => Outcome::Unchanged,
        ModeKind::Browse | ModeKind::Search => list_mouse(state, kind, column, row),
    }
}

fn list_mouse(state: &mut PiSettingsState, kind: MouseEventKind, column: u16, row: u16) -> Outcome {
    let on_list = contains(state.list_area, column, row);

    if matches!(kind, MouseEventKind::Moved) {
        let hovered = state
            .row_rects
            .iter()
            .position(|r| contains(*r, column, row))
            .filter(|&i| state.rows.get(i).is_some_and(Row::is_setting));
        if hovered != state.hover {
            state.hover = hovered;
            return Outcome::Changed;
        }
        return Outcome::Unchanged;
    }

    match kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if !on_list {
                return Outcome::Unchanged;
            }
            // A sidebar click jumps to that section's first row, mirroring the
            // Tab + Up/Down keyboard path.
            if let Some(section) = state
                .sidebar_rects
                .iter()
                .position(|r| contains(*r, column, row))
            {
                let target = state.sections().get(section).map(|(_, first)| *first);
                state.section_focus = false;
                return match target {
                    Some(target) => Outcome::changed_if(state.select_row(target)),
                    None => Outcome::Unchanged,
                };
            }
            let Some(index) = state
                .row_rects
                .iter()
                .position(|r| contains(*r, column, row))
            else {
                return Outcome::Unchanged;
            };
            if !state.rows[index].is_setting() {
                return Outcome::Unchanged;
            }
            // Two-stage click: a click on an unfocused row only selects it, so
            // the description can be read first. Clicking the already-focused
            // row, or its value column, activates. The value rect is a
            // Fitts's-law nudge around the small glyph.
            let on_value = contains(
                state.value_rects.get(index).copied().unwrap_or_default(),
                column,
                row,
            );
            let was_focused = state.selected == index;
            state.select_row(index);
            state.section_focus = false;
            if on_value || was_focused {
                return activate_focused(state);
            }
            Outcome::Changed
        }
        MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
            if !on_list {
                return Outcome::Unchanged;
            }
            let delta = if matches!(kind, MouseEventKind::ScrollDown) {
                1
            } else {
                -1
            };
            let mut moved = false;
            for _ in 0..3 {
                moved |= state.step(delta);
            }
            Outcome::changed_if(moved)
        }
        _ => Outcome::Unchanged,
    }
}

fn choice_mouse(
    state: &mut PiSettingsState,
    kind: MouseEventKind,
    column: u16,
    row: u16,
) -> Outcome {
    let hit = state
        .choice_rects
        .iter()
        .position(|r| r.height > 0 && contains(*r, column, row));

    if matches!(kind, MouseEventKind::Moved) {
        if hit != state.hover {
            state.hover = hit;
            return Outcome::Changed;
        }
        return Outcome::Unchanged;
    }
    if !matches!(kind, MouseEventKind::Down(MouseButton::Left)) {
        return Outcome::Unchanged;
    }
    let Some(hit) = hit else {
        return Outcome::Unchanged;
    };

    match state.mode {
        // Click moves the chooser cursor (and previews), matching Up/Down;
        // Enter still commits.
        Mode::Picking {
            index,
            supports_preview,
            ..
        } => {
            let len = state.choice_rects.len() as isize;
            move_choice(
                state,
                index,
                len,
                hit as isize - index as isize,
                supports_preview,
            )
        }
        Mode::PickingGroup { key, .. } => {
            let children = state.group_children(key);
            let Some(&child) = children.get(hit) else {
                return Outcome::Unchanged;
            };
            state.mode = Mode::PickingGroup { key, child: hit };
            toggle_group_child(state, child)
        }
        _ => Outcome::Unchanged,
    }
}

fn stepper_mouse(
    state: &mut PiSettingsState,
    kind: MouseEventKind,
    column: u16,
    row: u16,
) -> Outcome {
    if !matches!(kind, MouseEventKind::Down(MouseButton::Left)) {
        return Outcome::Unchanged;
    }
    let Mode::EditingInt {
        ref buffer,
        min,
        max,
        ..
    } = state.mode
    else {
        return Outcome::Unchanged;
    };
    let current: i64 = buffer.parse().unwrap_or(min);
    let (small, _) = step_sizes(min, max);
    let (left, right) = state.stepper_rects;
    if contains(left, column, row) {
        return set_int(state, current - small, min, max);
    }
    if contains(right, column, row) {
        return set_int(state, current + small, min, max);
    }
    Outcome::Unchanged
}

fn contains(rect: Rect, column: u16, row: u16) -> bool {
    rect.width > 0
        && rect.height > 0
        && column >= rect.x
        && column < rect.x + rect.width
        && row >= rect.y
        && row < rect.y + rect.height
}

impl PiSettingsState {
    /// The Int editor's raw buffer, for change detection.
    fn mode_buffer(&self) -> Option<&str> {
        match &self.mode {
            Mode::EditingInt { buffer, .. } => Some(buffer),
            _ => None,
        }
    }
}
