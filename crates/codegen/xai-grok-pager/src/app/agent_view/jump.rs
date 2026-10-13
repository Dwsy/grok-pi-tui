//! `/jump` picker: transcript preview syncing and key/mouse handling.

use super::AgentView;
use crate::app::actions::Action;
use crate::app::app_view::InputOutcome;
use crate::views::jump::{
    JumpInput, JumpPurpose, JumpRestore, handle_jump_key, jump_activate, jump_row_at, move_cursor,
    set_jump_cursor,
};
use crossterm::event::{KeyEvent, MouseButton, MouseEvent, MouseEventKind};

impl AgentView {
    /// Close the `/jump` picker (if open) and restore the viewport it opened from.
    /// Shared by the `Esc` dismiss path and the rewind entry points, so a shadowed picker can't reappear stale.
    pub(crate) fn dismiss_jump_picker(&mut self) {
        if let Some(state) = self.jump_state.take() {
            self.restore_jump_viewport(state.restore);
        }
    }

    pub(crate) fn restore_jump_viewport(&mut self, restore: JumpRestore) {
        self.scrollback.set_selected(restore.selected);
        if let Some(bookmark) = restore.bookmark {
            self.scrollback.restore_scroll_bookmark(bookmark);
        }
        if restore.follow_mode {
            self.scrollback.enable_follow();
        }
    }

    /// True when another prompt overlay owns the input slot, so the `/jump` picker must not open and an open one must be dismissed.
    /// Fork/review overlays are Pi-hosted prompt owners in addition to the upstream rewind, `/btw`, and approval surfaces.
    pub(crate) fn jump_slot_taken(&self) -> bool {
        self.rewind_state.is_some()
            || self.btw_state.is_some()
            || self.fork_state.is_some()
            || self.review_state.is_some()
            || !self.no_input_overlay_pending()
    }

    pub(super) fn dismiss_jump_picker_if_suppressed(&mut self) -> bool {
        if self.jump_state.is_some() && self.jump_slot_taken() {
            self.dismiss_jump_picker();
            return true;
        }
        false
    }

    pub(super) fn sync_jump_preview(&mut self) {
        let Some(prompt_id) = self
            .jump_state
            .as_ref()
            .and_then(|state| state.current_entry())
            .map(|entry| entry.prompt_entry_id)
        else {
            return;
        };
        if let Some(index) = self.scrollback.index_of_id(prompt_id) {
            self.scrollback.scroll_to_entry_top(index);
        }
    }

    pub(super) fn handle_jump_key(&mut self, key: &KeyEvent) -> InputOutcome {
        let Some(state) = self.jump_state.as_ref() else {
            return InputOutcome::Unchanged;
        };
        match handle_jump_key(state, key) {
            JumpInput::MoveUp => {
                if let Some(state) = self.jump_state.as_mut() {
                    move_cursor(state, -1);
                    self.sync_jump_preview();
                }
                InputOutcome::Changed
            }
            JumpInput::MoveDown => {
                if let Some(state) = self.jump_state.as_mut() {
                    move_cursor(state, 1);
                    self.sync_jump_preview();
                }
                InputOutcome::Changed
            }
            JumpInput::SearchChar(c) => {
                if let Some(state) = self.jump_state.as_mut() {
                    state.query.push(c);
                    state.apply_filter();
                    self.sync_jump_preview();
                }
                InputOutcome::Changed
            }
            JumpInput::SearchBackspace => {
                if let Some(state) = self.jump_state.as_mut() {
                    state.query.pop();
                    state.apply_filter();
                    self.sync_jump_preview();
                }
                InputOutcome::Changed
            }
            JumpInput::CopySelected(text) => {
                self.copy_to_clipboard(&text);
                InputOutcome::Changed
            }
            JumpInput::Select(id) => self.jump_select_outcome(id),
            input => Self::jump_input_to_outcome(input),
        }
    }

    fn jump_input_to_outcome(input: JumpInput) -> InputOutcome {
        match input {
            JumpInput::Select(id) => {
                // Purpose is read from jump_state when present; default Navigate.
                // Caller paths that only have JumpInput use Navigate unless
                // handle_jump_key routes Select through jump_select_outcome.
                InputOutcome::Action(Action::JumpPickerSelect(id))
            }
            JumpInput::Dismissed => InputOutcome::Action(Action::JumpDismiss),
            JumpInput::MoveUp
            | JumpInput::MoveDown
            | JumpInput::SearchChar(_)
            | JumpInput::SearchBackspace
            | JumpInput::CopySelected(_)
            | JumpInput::Consumed => InputOutcome::Changed,
        }
    }

    fn jump_select_outcome(&self, id: crate::scrollback::entry::EntryId) -> InputOutcome {
        match self.jump_state.as_ref().map(|s| s.purpose) {
            Some(JumpPurpose::Review) => InputOutcome::Action(Action::ReviewOpenForTurn(id)),
            _ => InputOutcome::Action(Action::JumpPickerSelect(id)),
        }
    }

    pub(super) fn handle_jump_mouse(&mut self, mouse: &MouseEvent) -> InputOutcome {
        let Some(state) = self.jump_state.as_mut() else {
            return InputOutcome::Unchanged;
        };
        let area = self.pane_areas.prompt;
        let Some(index) = jump_row_at(state, area, mouse.column, mouse.row) else {
            return InputOutcome::Unchanged;
        };

        match mouse.kind {
            MouseEventKind::Moved => {
                if set_jump_cursor(state, index) {
                    self.sync_jump_preview();
                    InputOutcome::Changed
                } else {
                    InputOutcome::Unchanged
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                set_jump_cursor(state, index);
                let purpose = state.purpose;
                match jump_activate(state) {
                    JumpInput::Select(id) => match purpose {
                        JumpPurpose::Review => InputOutcome::Action(Action::ReviewOpenForTurn(id)),
                        JumpPurpose::Navigate => InputOutcome::Action(Action::JumpPickerSelect(id)),
                    },
                    other => Self::jump_input_to_outcome(other),
                }
            }
            _ => InputOutcome::Unchanged,
        }
    }
}
