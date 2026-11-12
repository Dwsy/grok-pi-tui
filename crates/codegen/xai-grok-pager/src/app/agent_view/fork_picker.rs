//! Pi `/fork` list overlay: key/mouse handling (jump-style prompt shell).

use super::AgentView;
use crate::app::actions::Action;
use crate::app::app_view::InputOutcome;
use crate::slash::commands::fork::ForkArgs;
use crate::views::fork_picker::{
    ForkPickerInput, fork_picker_activate, fork_picker_row_at, handle_fork_picker_key, move_cursor,
    set_fork_picker_cursor,
};
use crossterm::event::{KeyEvent, MouseButton, MouseEvent, MouseEventKind};

impl AgentView {
    pub(crate) fn dismiss_fork_picker(&mut self) {
        self.fork_state = None;
    }

    pub(crate) fn fork_slot_taken(&self) -> bool {
        self.jump_state.is_some()
            || self.rewind_state.is_some()
            || self.inline_edit.is_some()
            || self.btw_state.is_some()
            || !self.no_input_overlay_pending()
    }

    pub(super) fn dismiss_fork_picker_if_suppressed(&mut self) -> bool {
        if self.fork_state.is_some() && self.fork_slot_taken() {
            self.dismiss_fork_picker();
            return true;
        }
        false
    }

    pub(super) fn handle_fork_picker_key(&mut self, key: &KeyEvent) -> InputOutcome {
        let Some(state) = self.fork_state.as_ref() else {
            return InputOutcome::Unchanged;
        };
        match handle_fork_picker_key(state, key) {
            ForkPickerInput::MoveUp => {
                if let Some(state) = self.fork_state.as_mut() {
                    move_cursor(state, -1);
                }
                InputOutcome::Changed
            }
            ForkPickerInput::MoveDown => {
                if let Some(state) = self.fork_state.as_mut() {
                    move_cursor(state, 1);
                }
                InputOutcome::Changed
            }
            input => Self::fork_picker_input_to_outcome(input, state.to_dashboard),
        }
    }

    fn fork_picker_input_to_outcome(input: ForkPickerInput, to_dashboard: bool) -> InputOutcome {
        match input {
            ForkPickerInput::Select(entry_id) => {
                let args = ForkArgs {
                    worktree_override: None,
                    directive: Some(entry_id),
                };
                InputOutcome::Action(if to_dashboard {
                    Action::ForkToDashboard(args)
                } else {
                    Action::Fork(args)
                })
            }
            ForkPickerInput::Dismissed => InputOutcome::Action(Action::PiForkDismiss),
            ForkPickerInput::MoveUp | ForkPickerInput::MoveDown | ForkPickerInput::Consumed => {
                InputOutcome::Changed
            }
        }
    }

    pub(super) fn handle_fork_picker_mouse(&mut self, mouse: &MouseEvent) -> InputOutcome {
        let Some(state) = self.fork_state.as_mut() else {
            return InputOutcome::Unchanged;
        };
        let area = self.pane_areas.prompt;
        let Some(index) = fork_picker_row_at(state, area, mouse.column, mouse.row) else {
            return InputOutcome::Unchanged;
        };

        match mouse.kind {
            MouseEventKind::Moved => {
                if set_fork_picker_cursor(state, index) {
                    InputOutcome::Changed
                } else {
                    InputOutcome::Unchanged
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                set_fork_picker_cursor(state, index);
                Self::fork_picker_input_to_outcome(fork_picker_activate(state), state.to_dashboard)
            }
            _ => InputOutcome::Unchanged,
        }
    }
}
