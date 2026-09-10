//! Code-review modal: left file list + right BlockViewerPane (edit-viewer TUI).

use super::AgentView;
use crate::app::actions::Action;
use crate::app::app_view::InputOutcome;
use crate::views::review::{
    ReviewFocus, ReviewInput, handle_review_ask_key, handle_review_list_key, handle_review_mouse,
    handle_review_preview_shell_key,
};
use crossterm::event::{KeyEvent, MouseEvent};
use std::path::Path;

impl AgentView {
    fn copy_review_path(&mut self) -> InputOutcome {
        let Some(path) = self
            .review_state
            .as_ref()
            .and_then(|s| s.current_file())
            .map(|f| f.path.clone())
        else {
            self.show_toast("No file to copy");
            return InputOutcome::Changed;
        };
        // copy_to_clipboard shows its own delivery toast.
        self.copy_to_clipboard(&path);
        InputOutcome::Changed
    }

    fn open_review_path(&mut self) -> InputOutcome {
        let path = self
            .review_state
            .as_ref()
            .and_then(|s| s.current_file())
            .map(|f| f.path.clone());
        let Some(path) = path else {
            self.show_toast("No file to open");
            return InputOutcome::Changed;
        };
        if crate::app::link_opener::open_path(Path::new(&path)) {
            self.show_toast(&format!("Opening {path}\u{2026}"));
        } else {
            self.show_toast(&format!("Could not open {path}"));
        }
        InputOutcome::Changed
    }

    /// Map a `ReviewInput` to an `InputOutcome`, handling shared variants.
    fn review_input_outcome(&mut self, input: ReviewInput) -> InputOutcome {
        match input {
            ReviewInput::Dismissed => InputOutcome::Action(Action::ReviewDismiss),
            ReviewInput::ToggleTree => {
                let enabled = self
                    .review_state
                    .as_mut()
                    .map(|s| s.toggle_tree_mode())
                    .unwrap_or(false);
                if let Some(state) = self.review_state.as_mut() {
                    state.ensure_viewer(&self.scrollback);
                }
                InputOutcome::Action(Action::SetReviewFileTree(enabled))
            }
            ReviewInput::ToggleIncludeReads => {
                let enabled = self
                    .review_state
                    .as_ref()
                    .map(|s| !s.filter.includes_reads())
                    .unwrap_or(false);
                InputOutcome::Action(Action::SetReviewIncludeReads(enabled))
            }
            ReviewInput::OpenPath => self.open_review_path(),
            ReviewInput::CopyPath => self.copy_review_path(),
            ReviewInput::AskSubmit(question) => InputOutcome::Action(Action::ReviewAsk(question)),
            ReviewInput::Changed | ReviewInput::Consumed => {
                if let Some(state) = self.review_state.as_mut() {
                    state.ensure_viewer(&self.scrollback);
                }
                InputOutcome::Changed
            }
        }
    }

    pub(super) fn handle_review_key(&mut self, key: &KeyEvent) -> InputOutcome {
        let Some(state) = self.review_state.as_mut() else {
            return InputOutcome::Unchanged;
        };

        match state.focus {
            ReviewFocus::List => {
                let input = handle_review_list_key(state, key);
                self.review_input_outcome(input)
            }
            ReviewFocus::Ask => {
                let input = handle_review_ask_key(state, key);
                self.review_input_outcome(input)
            }
            ReviewFocus::Preview => {
                // Shell keys (← list, n/p file) before viewer.
                if let Some(shell) = handle_review_preview_shell_key(state, key) {
                    return self.review_input_outcome(shell);
                }

                // Esc/q close only when viewer is not in search/filter/visual.
                if let Some(viewer) = state.viewer.as_ref()
                    && viewer.is_close_key(key)
                {
                    // If viewer has active input, is_close_key is false.
                    return InputOutcome::Action(Action::ReviewDismiss);
                }

                // Route to BlockViewerPane (scroll/search/filter/wrap/copy/select).
                let Some(viewer) = state.viewer.as_mut() else {
                    return InputOutcome::Changed;
                };
                if !viewer.handle_key(key) {
                    // Unconsumed Esc while list not focused → go to list.
                    if matches!(key.code, crossterm::event::KeyCode::Esc) {
                        state.focus = ReviewFocus::List;
                        return InputOutcome::Changed;
                    }
                    return InputOutcome::Changed;
                }

                // Process y/Y copy pending (same as fullscreen block viewer).
                let entry_id = viewer.entry_id;
                if let Some(entry) = self.scrollback.get_by_id(entry_id).cloned()
                    && let Some(viewer) = self.review_state.as_mut().and_then(|s| s.viewer.as_mut())
                    && let Some(text) = viewer.process_pending_copy(&entry)
                {
                    self.copy_to_clipboard(&text);
                }
                // Drag copy text.
                if let Some(viewer) = self.review_state.as_mut().and_then(|s| s.viewer.as_mut())
                    && let Some(text) = viewer.drag_copy_text.take()
                {
                    self.copy_to_clipboard(&text);
                }
                InputOutcome::Changed
            }
        }
    }

    pub(super) fn handle_review_mouse(&mut self, mouse: &MouseEvent) -> InputOutcome {
        let Some(state) = self.review_state.as_mut() else {
            return InputOutcome::Unchanged;
        };
        let input = handle_review_mouse(state, mouse);
        // Drain drag copy after mouse up for preview pane.
        if matches!(input, ReviewInput::Changed | ReviewInput::Consumed) {
            if let Some(viewer) = state.viewer.as_mut()
                && let Some(text) = viewer.drag_copy_text.take()
            {
                self.copy_to_clipboard(&text);
            }
        }
        self.review_input_outcome(input)
    }
}
