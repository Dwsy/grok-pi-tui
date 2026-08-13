//! Modal dialog handling for [`AgentView`]: the `handle_modal_key` /
//! `handle_modal_mouse` input dispatchers, the command palette / arg picker /
//! doc picker input handlers, and the active-modal draw dispatch.
//!
//! Extracted from `agent_view.rs` as a sibling `impl AgentView` block (same
//! pattern as `queue_edit.rs` and `mouse.rs`).

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::Widget;

use super::actions::Action;
use super::agent_view::{
    AgentView, active_contexts_for_pane, apply_pi_settings_outcome, apply_settings_outcome,
};
use super::app_view::InputOutcome;

use crate::theme::Theme;
use crate::views::modal::{self, ActiveModal, ArgPickerSelection};

impl AgentView {
    /// `suggest_args` falls back to model rows when the query is not in effort
    /// phase. Model-phase reasoning rows use a trailing space in `insert_text`;
    /// effort rows do not. Require a non-empty list with no trailing-space
    /// rows before treating the picker as effort phase.
    fn arg_items_look_like_effort_phase(items: &[crate::slash::command::ArgItem]) -> bool {
        !items.is_empty()
            && items
                .iter()
                .all(|item| !item.insert_text.ends_with(char::is_whitespace))
    }

    /// Step the model ArgPicker from effort phase back to the model list.
    /// Returns `true` if the modal was updated (caller should not fully close).
    fn try_arg_picker_step_back_from_effort(&mut self) -> bool {
        Self::try_arg_picker_step_back_from_effort_modal(
            &mut self.active_modal,
            &self.prompt.slash_controller,
            &self.session.models,
            &self.session.cwd,
        )
    }

    fn try_arg_picker_step_back_from_effort_modal(
        active_modal: &mut Option<ActiveModal>,
        slash_controller: &crate::slash::SlashController,
        models: &crate::acp::model_state::ModelState,
        cwd: &std::path::Path,
    ) -> bool {
        let Some(ActiveModal::ArgPicker {
            command,
            args_query,
            ..
        }) = active_modal.as_ref()
        else {
            return false;
        };
        if args_query.is_empty() || !matches!(command.as_str(), "model" | "m") {
            return false;
        }
        let command = command.clone();
        let Some(cmd) = slash_controller.registry().get(&command) else {
            return false;
        };
        let ctx = crate::slash::command::AppCtx {
            models,
            cwd,
            has_session_announcements: slash_controller.has_session_announcements(),
            billing_surface_visible: slash_controller.billing_surface_visible(),
            usage_command_visible: slash_controller.usage_command_visible(),
            workflows_available: slash_controller.workflows_available(),
            screen_mode: slash_controller.screen_mode(),
        };
        let Some(model_items) = cmd.suggest_args(&ctx, "") else {
            return false;
        };
        if model_items.is_empty() {
            return false;
        }
        if let Some(ActiveModal::ArgPicker {
            args_query,
            items,
            original_items,
            state,
            ..
        }) = active_modal.as_mut()
        {
            args_query.clear();
            *items = model_items.clone();
            *original_items = model_items;
            // Model list is type-to-find: reopen input-default like the initial /model open.
            *state = crate::views::picker::PickerState::input_active();
        }
        true
    }

    /// Handle a key press while a modal dialog is active.
    ///
    /// Matches the pressed character against the modal's options and resolves
    /// the result. All non-matching keys are consumed (blocked).
    #[cfg(test)]
    pub(super) fn handle_modal_key(&mut self, key: &KeyEvent) -> InputOutcome {
        let registry = crate::actions::ActionRegistry::defaults();
        self.handle_modal_key_with_registry(key, &registry)
    }

    /// Handle modal input using the live action registry that dispatched it.
    pub(super) fn handle_modal_key_with_registry(
        &mut self,
        key: &KeyEvent,
        registry: &crate::actions::ActionRegistry,
    ) -> InputOutcome {
        use crate::views::modal::ActiveModal;
        use crate::views::modal_window::{self as mw, ModalWindowOutcome};

        // Peek at the modal type to decide dispatch strategy.
        let Some(ref mut modal) = self.active_modal else {
            return InputOutcome::Changed;
        };

        // Session tree has its own key handler (filters/search/label/detail).
        if matches!(modal, ActiveModal::SessionTree { .. }) {
            let ev = crossterm::event::Event::Key(*key);
            return self.handle_session_tree_input(&ev);
        }

        // Tree map has its own simple key handler (↑↓/Enter/Esc).
        if matches!(modal, ActiveModal::TreeMap { .. }) {
            return self.handle_tree_map_input(key);
        }

        // Picker-based modals: route Esc through ModalWindow chrome first,
        // then delegate remaining keys to the picker input handler.
        if matches!(
            modal,
            ActiveModal::CommandPalette { .. }
                | ActiveModal::ArgPicker { .. }
                | ActiveModal::Notifications { .. }
                | ActiveModal::SessionPicker { .. }
                | ActiveModal::DocPicker { .. }
        ) {
            // Extract window state for handle_modal_key.
            let (window, query_empty, esc_clears, external_session_picker) = match modal {
                ActiveModal::CommandPalette { window, state, .. } => {
                    (window, state.query().is_empty(), true, false)
                }
                ActiveModal::ArgPicker { window, state, .. } => {
                    (window, state.query().is_empty(), false, false)
                }
                ActiveModal::Notifications { window, state } => {
                    (window, state.picker.query().is_empty(), true, false)
                }
                ActiveModal::SessionPicker {
                    window,
                    state,
                    source_filter,
                    ..
                } => (
                    window,
                    state.query().is_empty(),
                    false,
                    *source_filter == crate::views::session_picker::SourceFilter::External,
                ),
                ActiveModal::DocPicker { window, state, .. } => {
                    (window, state.query().is_empty(), true, false)
                }
                _ => unreachable!(),
            };
            // These modals don't use fold; fold_info is None so
            // Left/Right/h/l return Unhandled and reach the picker.
            let external_tabs = ["Current folder", "All"];
            let chrome_cfg = mw::ModalWindowConfig {
                title: "",
                tabs: external_session_picker.then_some(&external_tabs),
                shortcuts: &[],
                sizing: mw::ModalSizing::default(),
                fold_info: None,
            };
            let outcome = mw::handle_modal_key(window, key, &chrome_cfg);
            match outcome {
                ModalWindowOutcome::TabChanged(index) if external_session_picker => {
                    window.active_tab = index;
                    return InputOutcome::Action(Action::RefreshExternalSessionCatalog);
                }
                ModalWindowOutcome::CloseRequested => {
                    // If query non-empty and esc_clears_query: clear query first.
                    if esc_clears && !query_empty {
                        match modal {
                            ActiveModal::CommandPalette { state, .. } => {
                                state.set_query("");
                                state.selected = 0;
                                state.scroll_offset = None;
                            }
                            ActiveModal::DocPicker { state, .. } => {
                                state.set_query("");
                                state.selected = 0;
                                state.scroll_offset = None;
                            }
                            ActiveModal::Notifications { state, .. } => {
                                state.picker.clear_query();
                            }
                            _ => {}
                        }
                        return InputOutcome::Changed;
                    }
                    // Otherwise delegate close to the picker handler which
                    // knows about palette snapshots / restore logic.
                    let ev = crossterm::event::Event::Key(*key);
                    if matches!(self.active_modal, Some(ActiveModal::DocPicker { .. })) {
                        return self.handle_doc_input(&ev);
                    }
                    if matches!(self.active_modal, Some(ActiveModal::Notifications { .. })) {
                        return self.handle_notifications_input(&ev);
                    }
                    return self.handle_palette_or_arg_input_with_registry(&ev, registry);
                }
                ModalWindowOutcome::Unhandled => {
                    // Non-Esc key (including Left/Right/h/l):
                    // forward to picker input handler.
                    let ev = crossterm::event::Event::Key(*key);
                    if matches!(self.active_modal, Some(ActiveModal::DocPicker { .. })) {
                        return self.handle_doc_input(&ev);
                    }
                    if matches!(self.active_modal, Some(ActiveModal::Notifications { .. })) {
                        return self.handle_notifications_input(&ev);
                    }
                    return self.handle_palette_or_arg_input_with_registry(&ev, registry);
                }
                _ => return InputOutcome::Changed,
            }
        }

        // RememberNoteReview: modal preview for # remember notes.
        if let ActiveModal::RememberNoteReview {
            ref mut scroll,
            ref mut showing_enhanced,
            ref enhanced_content,
            ref mut cached_lines,
            ref mut window,
            ..
        } = *modal
        {
            let chrome_cfg = mw::ModalWindowConfig {
                title: "",
                tabs: None,
                shortcuts: &[],
                sizing: mw::ModalSizing::default(),
                fold_info: None,
            };
            match mw::handle_modal_key(window, key, &chrome_cfg) {
                mw::ModalWindowOutcome::CloseRequested => {
                    self.active_modal = None;
                    return InputOutcome::Changed;
                }
                mw::ModalWindowOutcome::Handled => return InputOutcome::Changed,
                mw::ModalWindowOutcome::Unhandled => {}
                _ => {}
            }

            match key.code {
                KeyCode::Enter => {
                    return InputOutcome::Action(Action::SaveRememberNoteFromModal);
                }
                KeyCode::Char('y') if key.modifiers.is_empty() => {
                    return InputOutcome::Action(Action::SaveRememberNoteFromModal);
                }
                KeyCode::Tab => {
                    if enhanced_content.is_some() {
                        *showing_enhanced = !*showing_enhanced;
                        *cached_lines = None;
                        *scroll = 0;
                        return InputOutcome::Changed;
                    }
                    return InputOutcome::Unchanged;
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    *scroll = scroll.saturating_add(1);
                    return InputOutcome::Changed;
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    *scroll = scroll.saturating_sub(1);
                    return InputOutcome::Changed;
                }
                KeyCode::PageDown => {
                    *scroll = scroll.saturating_add(10);
                    return InputOutcome::Changed;
                }
                KeyCode::PageUp => {
                    *scroll = scroll.saturating_sub(10);
                    return InputOutcome::Changed;
                }
                _ => return InputOutcome::Unchanged,
            }
        }

        // DocViewer / ContextInfo: route through ModalWindow chrome, then handle scroll.
        // Cache graph keys (1/2/3/s/e/r/v/0) when metrics present.
        if matches!(modal, ActiveModal::ContextInfo { .. }) {
            use crate::views::cache_graph::{CacheGraphView, build_cache_stats_csv};
            use crossterm::event::KeyCode;

            let ActiveModal::ContextInfo {
                window,
                scroll,
                cache_metrics,
                view,
                export_cwd,
                export_basename,
                ..
            } = modal
            else {
                unreachable!();
            };
            let chrome_cfg = mw::ModalWindowConfig {
                title: "",
                tabs: None,
                shortcuts: &[],
                sizing: mw::ModalSizing::default(),
                fold_info: None,
            };
            match mw::handle_modal_key(window, key, &chrome_cfg) {
                ModalWindowOutcome::CloseRequested => {
                    self.active_modal = None;
                    return InputOutcome::Changed;
                }
                ModalWindowOutcome::Unhandled => {
                    if crate::views::modal::apply_doc_scroll(key.code, scroll) {
                        return InputOutcome::Changed;
                    }
                    // View switching / export when cache metrics are attached.
                    if cache_metrics.is_some() {
                        let next = match key.code {
                            KeyCode::Char('0') | KeyCode::Char('c') => {
                                Some(CacheGraphView::Breakdown)
                            }
                            KeyCode::Char('1') => Some(CacheGraphView::PerTurn),
                            KeyCode::Char('2') => Some(CacheGraphView::CumulativePercent),
                            KeyCode::Char('3') => Some(CacheGraphView::CumulativeTotal),
                            KeyCode::Char('s') => Some(CacheGraphView::Stats),
                            KeyCode::Char('v') => Some(view.cycle_forward()),
                            _ => None,
                        };
                        if let Some(next_view) = next {
                            if next_view != *view {
                                *view = next_view;
                                *scroll = 0;
                            }
                            return InputOutcome::Changed;
                        }
                        if matches!(key.code, KeyCode::Char('e')) {
                            if let Some(metrics) = cache_metrics.as_ref() {
                                let path = std::path::Path::new(export_cwd)
                                    .join(format!("{export_basename}.csv"));
                                let csv = build_cache_stats_csv(metrics);
                                match std::fs::write(&path, csv) {
                                    Ok(()) => {
                                        self.show_toast(&format!(
                                            "Exported cache stats to {}",
                                            path.display()
                                        ));
                                    }
                                    Err(err) => {
                                        self.show_toast(&format!("Cache export failed: {err}"));
                                    }
                                }
                                return InputOutcome::Changed;
                            }
                        }
                        if matches!(key.code, KeyCode::Char('r')) {
                            // Re-fetch session/info; view preserved in complete handler.
                            return InputOutcome::Action(
                                crate::app::actions::Action::ShowContextInfo,
                            );
                        }
                    }
                    return InputOutcome::Unchanged;
                }
                _ => return InputOutcome::Changed,
            }
        }
        if let ActiveModal::DocViewer {
            window,
            previous_palette,
            standalone,
            ..
        } = modal
        {
            let chrome_cfg = mw::ModalWindowConfig {
                title: "",
                tabs: None,
                shortcuts: &[],
                sizing: mw::ModalSizing::default(),
                fold_info: None,
            };
            let outcome = mw::handle_modal_key(window, key, &chrome_cfg);
            match outcome {
                ModalWindowOutcome::CloseRequested => {
                    if *standalone {
                        self.active_modal = None;
                    } else {
                        // Esc in DocViewer -> back to DocPicker list.
                        // Shuttle the palette snapshot so DocPicker can restore it on its own Esc.
                        let prev = previous_palette.take();
                        self.active_modal = Some(crate::views::modal::howto_list_modal(prev));
                    }
                    return InputOutcome::Changed;
                }
                ModalWindowOutcome::Unhandled => {
                    // Scroll keys (no fold for doc viewer).
                    let ev = crossterm::event::Event::Key(*key);
                    return self.handle_doc_input(&ev);
                }
                _ => return InputOutcome::Changed,
            }
        }
        // ShortcutsHelp: modal window chrome first, then picker / detail.
        if let ActiveModal::ShortcutsHelp {
            entries,
            state,
            window,
            filter_active,
            collapsed_sections,
            expanded_ids,
            mode,
        } = modal
        {
            use crate::views::shortcuts_help::{self, ShortcutsHelpOutcome};
            let searching = state.search_active || !state.query().is_empty();
            if mode.is_browse() && searching && key.code == KeyCode::Esc {
                state.set_query("");
                state.search_active = false;
                state.selected = 0;
                return InputOutcome::Changed;
            }
            // Detail owns Esc (back to browse); skip chrome so it doesn't close the modal.
            let footer = if mode.is_detail() {
                shortcuts_help::modal_footer_detail()
            } else {
                shortcuts_help::modal_footer(*filter_active)
            };
            let chrome_cfg = mw::ModalWindowConfig {
                title: "Keyboard Shortcuts",
                tabs: None,
                shortcuts: &footer,
                sizing: crate::views::shortcuts_help::modal_sizing(
                    self.scrollback.appearance().prompt.compact,
                ),
                fold_info: None,
            };
            if mode.is_browse() {
                match mw::handle_modal_key(window, key, &chrome_cfg) {
                    mw::ModalWindowOutcome::CloseRequested => {
                        self.active_modal = None;
                        return InputOutcome::Changed;
                    }
                    mw::ModalWindowOutcome::Unhandled => {}
                    _ => return InputOutcome::Changed,
                }
            }
            match shortcuts_help::handle_input(
                key,
                entries,
                state,
                *filter_active,
                collapsed_sections,
                expanded_ids,
                mode,
            ) {
                ShortcutsHelpOutcome::Close => {
                    self.active_modal = None;
                    return InputOutcome::Changed;
                }
                ShortcutsHelpOutcome::ToggleFilter => {
                    *filter_active = !*filter_active;
                    state.selected = 0;
                    return InputOutcome::Changed;
                }
                ShortcutsHelpOutcome::ToggleSection(idx) => {
                    shortcuts_help::toggle_membership(collapsed_sections, idx);
                    return InputOutcome::Changed;
                }
                ShortcutsHelpOutcome::ToggleExpand(action_id) => {
                    shortcuts_help::toggle_membership(expanded_ids, action_id);
                    return InputOutcome::Changed;
                }
                ShortcutsHelpOutcome::Changed => return InputOutcome::Changed,
                ShortcutsHelpOutcome::Unchanged => return InputOutcome::Unchanged,
            }
        }

        // MemoryBrowser: route through ModalWindow chrome, then delegate.
        if let ActiveModal::MemoryBrowser { state } = modal {
            // When the filter input is focused, Esc exits filter mode
            // instead of closing the modal. Handle before modal chrome.
            if matches!(
                state.mode,
                crate::views::memory_modal::MemoryModalMode::FilterFocused
            ) {
                return crate::views::memory_modal::handle_memory_key(state, key);
            }
            let chrome_cfg = mw::ModalWindowConfig {
                title: "",
                tabs: None,
                shortcuts: &[],
                sizing: mw::ModalSizing::default(),
                fold_info: None,
            };
            let outcome = mw::handle_modal_key(&mut state.window, key, &chrome_cfg);
            match outcome {
                ModalWindowOutcome::CloseRequested => {
                    self.active_modal = None;
                    return InputOutcome::Changed;
                }
                ModalWindowOutcome::Unhandled => {
                    return crate::views::memory_modal::handle_memory_key(state, key);
                }
                _ => return InputOutcome::Changed,
            }
        }

        // Settings: route through ModalWindow chrome, then delegate.
        if let ActiveModal::Settings { state } = modal {
            // Sub-mode short-circuit: FilterFocused, PickingEnum, PickingGroup,
            // and EditingValue own their own Esc/keystroke semantics.
            if matches!(
                state.mode(),
                crate::views::settings_modal::SettingsModalMode::FilterFocused
                    | crate::views::settings_modal::SettingsModalMode::PickingEnum { .. }
                    | crate::views::settings_modal::SettingsModalMode::PickingGroup { .. }
                    | crate::views::settings_modal::SettingsModalMode::EditingValue { .. }
            ) {
                let out = crate::views::settings_modal::handle_settings_key(state, key);
                return apply_settings_outcome(self, out);
            }
            let chrome_cfg = mw::ModalWindowConfig {
                title: "",
                tabs: None,
                shortcuts: &[],
                sizing: mw::ModalSizing::default(),
                fold_info: None,
            };
            let chrome_outcome = mw::handle_modal_key(&mut state.window, key, &chrome_cfg);
            match chrome_outcome {
                ModalWindowOutcome::CloseRequested => {
                    self.active_modal = None;
                    return InputOutcome::Changed;
                }
                ModalWindowOutcome::Unhandled => {
                    let out = crate::views::settings_modal::handle_settings_key(state, key);
                    return apply_settings_outcome(self, out);
                }
                _ => return InputOutcome::Changed,
            }
        }

        // grok-pi settings panel: the chrome owns close and tab switching,
        // then the panel handles the rest. Sub-panes own their own Esc, so
        // they bypass the chrome entirely.
        if let ActiveModal::PiSettings { state } = modal {
            use crate::views::pi_settings;
            // Search, section focus, and sub-panes give Esc a local meaning,
            // so they bypass the chrome's Esc-closes-the-modal rule.
            if state.owns_escape() {
                let out = pi_settings::handle_key(state, key);
                return apply_pi_settings_outcome(self, out);
            }
            let chrome_cfg = mw::ModalWindowConfig {
                title: "",
                tabs: None,
                shortcuts: &[],
                sizing: mw::ModalSizing::default(),
                fold_info: None,
            };
            return match mw::handle_modal_key(&mut state.window, key, &chrome_cfg) {
                ModalWindowOutcome::CloseRequested => {
                    self.active_modal = None;
                    InputOutcome::Changed
                }
                ModalWindowOutcome::Unhandled => {
                    let out = pi_settings::handle_key(state, key);
                    apply_pi_settings_outcome(self, out)
                }
                _ => InputOutcome::Changed,
            };
        }

        if let ActiveModal::PiConfig { state } = modal {
            return match crate::views::pi_config::PiConfigModalState::handle_key(state, key) {
                crate::views::pi_config::PiConfigOutcome::Close => {
                    state.complete_picker(false);
                    self.active_modal = None;
                    InputOutcome::Changed
                }
                crate::views::pi_config::PiConfigOutcome::PickerSubmit => {
                    state.complete_picker(true);
                    self.active_modal = None;
                    InputOutcome::Changed
                }
                crate::views::pi_config::PiConfigOutcome::Changed => InputOutcome::Changed,
            };
        }

        if let ActiveModal::PiModels { state } = modal {
            return match state.handle_key(key) {
                crate::views::pi_models::PiModelsOutcome::Close => {
                    self.active_modal = None;
                    InputOutcome::Changed
                }
                crate::views::pi_models::PiModelsOutcome::Changed => InputOutcome::Changed,
                crate::views::pi_models::PiModelsOutcome::Reload => {
                    InputOutcome::Action(Action::PiReload)
                }
                crate::views::pi_models::PiModelsOutcome::Activate(query) => {
                    let Some(model_id) = self.session.models.resolve_by_name_or_id(&query) else {
                        state.set_error(format!(
                            "Model '{query}' is not in the live catalog. Save/reload, then try again."
                        ));
                        return InputOutcome::Changed;
                    };
                    InputOutcome::Action(Action::SwitchModel {
                        model_id,
                        effort: None,
                    })
                }
            };
        }

        // ResetSettingsConfirm: y/n routing. Handled before generic
        // char-match so Esc/F2/Ctrl+, route to Cancel (not modal close).
        if let Some(ActiveModal::ResetSettingsConfirm { modal, .. }) = self.active_modal.as_ref() {
            let resolved = match key.code {
                KeyCode::Esc => Some(crate::views::modal::ResetSettingsResult::Cancel),
                KeyCode::F(2) => Some(crate::views::modal::ResetSettingsResult::Cancel),
                KeyCode::Char(',')
                    if key.modifiers.contains(KeyModifiers::CONTROL)
                        || key.modifiers.contains(KeyModifiers::SUPER) =>
                {
                    Some(crate::views::modal::ResetSettingsResult::Cancel)
                }
                // Only bare keystrokes — Ctrl+Y must not fire Reset.
                KeyCode::Char(c) if key.modifiers.is_empty() => modal.resolve(c).copied(),
                _ => None,
            };
            return match resolved {
                Some(choice) => InputOutcome::Action(Action::ConfirmResetSetting { choice }),
                None => InputOutcome::Changed,
            };
        }

        // EditConfirm: single char matching.
        let ch = match key.code {
            KeyCode::Char(c) => c,
            KeyCode::Esc => {
                self.active_modal = None;
                return InputOutcome::Changed;
            }
            _ => return InputOutcome::Changed, // consume, ignore
        };

        // Take the modal so we can match on it and modify self.
        let Some(modal) = self.active_modal.take() else {
            return InputOutcome::Changed;
        };

        match modal {
            ActiveModal::EditConfirm {
                modal: confirm,
                pending_target,
            } => self.handle_edit_confirm_choice(confirm, pending_target, ch),
            ActiveModal::CommandPalette { .. }
            | ActiveModal::ArgPicker { .. }
            | ActiveModal::Notifications { .. }
            | ActiveModal::SessionTree { .. }
            | ActiveModal::TreeMap { .. }
            | ActiveModal::SessionPicker { .. }
            | ActiveModal::DocPicker { .. }
            | ActiveModal::DocViewer { .. }
            | ActiveModal::ContextInfo { .. }
            | ActiveModal::ShortcutsHelp { .. }
            | ActiveModal::MemoryBrowser { .. }
            | ActiveModal::Settings { .. }
            | ActiveModal::PiSettings { .. }
            | ActiveModal::PiConfig { .. }
            | ActiveModal::PiModels { .. }
            | ActiveModal::ResetSettingsConfirm { .. }
            | ActiveModal::RememberNoteReview { .. } => unreachable!(),
        }
    }

    pub(super) fn handle_modal_paste(
        &mut self,
        text: &str,
        registry: &crate::actions::ActionRegistry,
    ) -> InputOutcome {
        use crate::views::modal::ActiveModal;

        let event = crossterm::event::Event::Paste(text.to_owned());
        if matches!(self.active_modal, Some(ActiveModal::DocPicker { .. })) {
            return self.handle_doc_input(&event);
        }
        if matches!(
            self.active_modal,
            Some(
                ActiveModal::CommandPalette { .. }
                    | ActiveModal::ArgPicker { .. }
                    | ActiveModal::SessionPicker { .. }
            )
        ) {
            return self.handle_palette_or_arg_input_with_registry(&event, registry);
        }

        if let Some(ActiveModal::ShortcutsHelp { state, mode, .. }) = self.active_modal.as_mut() {
            return match crate::views::shortcuts_help::handle_paste(text, state, mode) {
                crate::views::shortcuts_help::ShortcutsHelpOutcome::Changed => {
                    InputOutcome::Changed
                }
                _ => InputOutcome::Unchanged,
            };
        }
        if let Some(ActiveModal::MemoryBrowser { state }) = self.active_modal.as_mut() {
            return crate::views::memory_modal::handle_memory_paste(state, text);
        }
        if let Some(ActiveModal::PiModels { state }) = self.active_modal.as_mut() {
            state.handle_paste(text);
            return InputOutcome::Changed;
        }
        let pi_settings_outcome = match self.active_modal.as_mut() {
            Some(ActiveModal::PiSettings { state }) => {
                Some(crate::views::pi_settings::handle_paste(state, text))
            }
            _ => None,
        };
        if let Some(outcome) = pi_settings_outcome {
            return apply_pi_settings_outcome(self, outcome);
        }
        let settings_outcome = match self.active_modal.as_mut() {
            Some(ActiveModal::Settings { state }) => Some(
                crate::views::settings_modal::handle_settings_paste(state, text),
            ),
            _ => None,
        };
        if let Some(outcome) = settings_outcome {
            return apply_settings_outcome(self, outcome);
        }
        if self.active_modal.is_some() {
            InputOutcome::Changed
        } else {
            InputOutcome::Unchanged
        }
    }

    /// Arg picker input (separate from command palette to avoid borrow conflicts
    /// when stepping back from the model effort phase via slash registry + session).
    fn handle_arg_picker_input(&mut self, ev: &crossterm::event::Event) -> InputOutcome {
        use crate::views::picker::{PickerConfig, PickerOutcome, handle_picker_input};

        enum ArgPickerStep {
            Selected(crate::slash::command::ArgItem),
            Closed,
            FilterChanged,
        }

        let (command_clone, in_effort_phase, entry_count, selection) =
            match self.active_modal.as_ref() {
                Some(ActiveModal::ArgPicker {
                    command,
                    args_query,
                    items,
                    selection,
                    ..
                }) => (
                    command.clone(),
                    !args_query.is_empty(),
                    items.len(),
                    *selection,
                ),
                _ => return InputOutcome::Changed,
            };

        let config = PickerConfig {
            title: None,
            show_search_hint: false,
            expandable: false,
            esc_clears_query: false,
            shortcuts: Some(crate::views::picker::picker_shortcuts()),
            pending_hint: None,
            non_selectable: &[],
            non_selectable_clickable: &[],
            shortcuts_area: None,
            tabs: None,
            active_tab: 0,
            filter_label: None,
            filter_key_hint: None,
            filter_active: false,
            header_note: None,
            action_keys: &[],
            disable_search: false,
            compact_bottom_bar: false,
            search_only_on_slash: false,
            vim_normal_first: crate::appearance::cache::load_vim_mode(),
        };

        let step = {
            let Some(ActiveModal::ArgPicker { items, state, .. }) = self.active_modal.as_mut()
            else {
                return InputOutcome::Changed;
            };
            match handle_picker_input(ev, state, entry_count, &config) {
                PickerOutcome::Selected(i) => match items.get(i).cloned() {
                    Some(item) => ArgPickerStep::Selected(item),
                    None => return InputOutcome::Changed,
                },
                PickerOutcome::Closed => ArgPickerStep::Closed,
                PickerOutcome::QueryChanged => ArgPickerStep::FilterChanged,
                PickerOutcome::Changed => return InputOutcome::Changed,
                PickerOutcome::Unchanged => return InputOutcome::Unchanged,
                _ => return InputOutcome::Changed,
            }
        };

        match step {
            ArgPickerStep::FilterChanged => {
                if let Some(ActiveModal::ArgPicker {
                    command,
                    args_query,
                    items,
                    original_items,
                    state,
                    ..
                }) = self.active_modal.as_mut()
                {
                    let query = state.query().to_owned();
                    if matches!(command.as_str(), "model" | "m") && args_query.is_empty() {
                        // Keep the model picker aligned with Pi TUI: fuzzyFilter
                        // ranks all matching rows instead of requiring the raw
                        // query to occur as one contiguous substring.
                        let indexes = crate::slash::matcher::FuzzyMatcher::new()
                            .rank_pi_model_selector(original_items, &query, |item| {
                                item.match_text.as_str()
                            });
                        *items = indexes
                            .into_iter()
                            .filter_map(|index| original_items.get(index).cloned())
                            .collect();
                    } else {
                        let q = query.to_lowercase();
                        *items = original_items
                            .iter()
                            .filter(|item| {
                                q.is_empty()
                                    || item.match_text.to_lowercase().contains(&q)
                                    || item.display.to_lowercase().contains(&q)
                                    || item.description.to_lowercase().contains(&q)
                            })
                            .cloned()
                            .collect();
                    }
                    state.selected = state.selected.min(items.len().saturating_sub(1));
                }
                InputOutcome::Changed
            }
            ArgPickerStep::Closed => {
                if in_effort_phase && self.try_arg_picker_step_back_from_effort() {
                    return InputOutcome::Changed;
                }
                let (snapshot, settings) = match self.active_modal.as_mut() {
                    Some(ActiveModal::ArgPicker {
                        previous_palette,
                        previous_settings,
                        ..
                    }) => (previous_palette.take(), previous_settings.take()),
                    _ => (None, None),
                };
                if let Some(state) = settings {
                    // Refresh values so the just-cleared/changed slot paints.
                    // (commit path also refreshes via PersistSetting effects.)
                    self.active_modal = Some(ActiveModal::Settings { state });
                } else if let Some(snapshot) = snapshot {
                    self.active_modal = Some(ActiveModal::CommandPalette {
                        entries: snapshot.entries,
                        state: snapshot.state,
                        window: crate::views::modal_window::ModalWindowState::new(),
                    });
                } else {
                    self.active_modal = None;
                }
                InputOutcome::Changed
            }
            ArgPickerStep::Selected(item) => {
                if matches!(selection, ArgPickerSelection::ToggleScopedModel) {
                    if item.insert_text.eq_ignore_ascii_case("all") {
                        self.session.models.clear_scoped_models();
                    } else if let Some(model_id) =
                        crate::slash::commands::model::resolve_model_for_arg_item(
                            &self.session.models,
                            &item,
                        )
                    {
                        self.session.models.toggle_scoped_model(model_id);
                    }
                    if let Some(cmd) = self.prompt.slash_controller.registry().get(&command_clone) {
                        let ctx = self.prompt.slash_controller.app_ctx(&self.session.models);
                        if let Some(updated) = cmd.suggest_args(&ctx, "")
                            && let Some(ActiveModal::ArgPicker {
                                items,
                                original_items,
                                state,
                                ..
                            }) = self.active_modal.as_mut()
                        {
                            *original_items = updated;
                            let query = state.query().to_lowercase();
                            *items = original_items
                                .iter()
                                .filter(|candidate| {
                                    query.is_empty()
                                        || candidate.match_text.to_lowercase().contains(&query)
                                        || candidate.display.to_lowercase().contains(&query)
                                        || candidate.description.to_lowercase().contains(&query)
                                })
                                .cloned()
                                .collect();
                            state.selected = state.selected.min(items.len().saturating_sub(1));
                        }
                    }
                    return InputOutcome::Changed;
                }
                if let ArgPickerSelection::SetModelSlot(slot_key) = selection {
                    let settings = match self.active_modal.as_mut() {
                        Some(ActiveModal::ArgPicker {
                            previous_settings, ..
                        }) => previous_settings.take(),
                        _ => None,
                    };
                    let restore = |this: &mut Self, settings: Option<Box<_>>| {
                        if let Some(state) = settings {
                            this.active_modal = Some(ActiveModal::Settings { state });
                        } else {
                            this.active_modal = None;
                        }
                    };
                    if item.insert_text.is_empty() {
                        let action = match slot_key {
                            "recap_model" => Action::ClearRecapModel,
                            "recap_model_2" => Action::ClearRecapModel2,
                            "recap_model_3" => Action::ClearRecapModel3,
                            "btw_model" => Action::ClearBtwModel,
                            "btw_model_2" => Action::ClearBtwModel2,
                            "btw_model_3" => Action::ClearBtwModel3,
                            other => {
                                tracing::error!(key = other, "unknown model slot clear");
                                restore(self, settings);
                                return InputOutcome::Changed;
                            }
                        };
                        restore(self, settings);
                        return InputOutcome::Action(action);
                    }
                    let Some(model_id) = crate::slash::commands::model::resolve_model_for_arg_item(
                        &self.session.models,
                        &item,
                    ) else {
                        restore(self, settings);
                        return InputOutcome::Changed;
                    };
                    let id = model_id.0.to_string();
                    let action = match slot_key {
                        "recap_model" => Action::SetRecapModel(model_id),
                        "recap_model_2" => Action::SetRecapModel2(id),
                        "recap_model_3" => Action::SetRecapModel3(id),
                        "btw_model" => Action::SetBtwModel(id),
                        "btw_model_2" => Action::SetBtwModel2(id),
                        "btw_model_3" => Action::SetBtwModel3(id),
                        other => {
                            tracing::error!(key = other, "unknown model slot set");
                            restore(self, settings);
                            return InputOutcome::Changed;
                        }
                    };
                    restore(self, settings);
                    return InputOutcome::Action(action);
                }
                let chains_to_effort = matches!(command_clone.as_str(), "model" | "m")
                    && item.insert_text.ends_with(char::is_whitespace);
                if chains_to_effort {
                    let next_query = item.insert_text.clone();
                    if let Some(cmd) = self.prompt.slash_controller.registry().get(&command_clone) {
                        let ctx = self.prompt.slash_controller.app_ctx(&self.session.models);
                        if let Some(effort_items) = cmd.suggest_args(&ctx, &next_query)
                            && Self::arg_items_look_like_effort_phase(&effort_items)
                        {
                            if let Some(ActiveModal::ArgPicker {
                                args_query,
                                items,
                                original_items,
                                state,
                                ..
                            }) = self.active_modal.as_mut()
                            {
                                *args_query = next_query;
                                *items = effort_items.clone();
                                *original_items = effort_items;
                                // Effort sub-step is part of the type-to-find /model picker: open input-focused (cursor + type-to-filter), matching the rest of the flow.
                                *state = crate::views::picker::PickerState::input_active();
                            }
                            return InputOutcome::Changed;
                        }
                    }
                }
                let full = format!("/{} {}", command_clone, item.insert_text.trim_end());
                self.active_modal = None;
                InputOutcome::Action(Action::SendSlashCommandPreservingDraft(full))
            }
        }
    }

    /// Unified input handler for command palette and arg picker modals.
    #[cfg(test)]
    fn handle_palette_or_arg_input(&mut self, ev: &crossterm::event::Event) -> InputOutcome {
        let registry = crate::actions::ActionRegistry::defaults();
        self.handle_palette_or_arg_input_with_registry(ev, &registry)
    }

    fn handle_palette_or_arg_input_with_registry(
        &mut self,
        ev: &crossterm::event::Event,
        registry: &crate::actions::ActionRegistry,
    ) -> InputOutcome {
        use crate::views::modal::{ActiveModal, PaletteCommand};
        use crate::views::picker::{PickerConfig, PickerOutcome, handle_picker_input};

        if matches!(self.active_modal, Some(ActiveModal::ArgPicker { .. })) {
            return self.handle_arg_picker_input(ev);
        }

        let Some(ref mut modal) = self.active_modal else {
            return InputOutcome::Changed;
        };

        match modal {
            ActiveModal::CommandPalette {
                entries: _, state, ..
            } => {
                // Build filtered entries for count and non-selectable indices.
                let filtered = crate::views::modal::filter_palette_entries(
                    state.query(),
                    self.sharing_enabled,
                    &self.prompt.slash_controller,
                );
                let non_sel: Vec<bool> = filtered
                    .iter()
                    .map(|e| matches!(e.command, PaletteCommand::SectionHeader(_)))
                    .collect();
                let entry_count = filtered.len();

                let config = PickerConfig {
                    title: None,
                    show_search_hint: false,
                    expandable: false,
                    esc_clears_query: true,
                    shortcuts: Some(crate::views::picker::picker_shortcuts()),
                    pending_hint: None,
                    non_selectable: &non_sel,
                    non_selectable_clickable: &[],
                    shortcuts_area: None,
                    tabs: None,
                    active_tab: 0,
                    filter_label: None,
                    filter_key_hint: None,
                    filter_active: false,
                    header_note: None,
                    action_keys: &[],
                    disable_search: false,
                    compact_bottom_bar: false,
                    search_only_on_slash: false,
                    vim_normal_first: crate::appearance::cache::load_vim_mode(),
                };

                match handle_picker_input(ev, state, entry_count, &config) {
                    PickerOutcome::Selected(i) => {
                        if i >= filtered.len() {
                            return InputOutcome::Changed;
                        }
                        if matches!(filtered[i].command, PaletteCommand::SectionHeader(_)) {
                            return InputOutcome::Changed;
                        }
                        let cmd = filtered[i].command.clone();
                        match cmd {
                            PaletteCommand::NewSession => {
                                self.active_modal = None;
                                InputOutcome::Action(Action::NewSession)
                            }
                            PaletteCommand::NewSessionInWorktree => {
                                self.active_modal = None;
                                InputOutcome::Action(Action::NewWorktreeSession {
                                    load_session_id: None,
                                    label: None,
                                    git_ref: None,
                                })
                            }
                            PaletteCommand::Home => {
                                self.active_modal = None;
                                InputOutcome::Action(Action::ExitSessionConfirmed)
                            }
                            PaletteCommand::Quit => {
                                self.active_modal = None;
                                InputOutcome::Action(Action::QuitConfirmed)
                            }
                            PaletteCommand::HowTo => {
                                // Save palette state for Esc restore (same pattern as /resume).
                                let prev = {
                                    let ActiveModal::CommandPalette { entries, state, .. } =
                                        self.active_modal.as_ref().unwrap()
                                    else {
                                        unreachable!()
                                    };
                                    Some(crate::views::modal::PaletteSnapshot {
                                        entries: entries.clone(),
                                        state: state.clone(),
                                    })
                                };
                                self.active_modal =
                                    Some(crate::views::modal::howto_list_modal(prev));
                                InputOutcome::Changed
                            }
                            PaletteCommand::KeyboardShortcuts => {
                                use crate::views::shortcuts_help;
                                let mut contexts = active_contexts_for_pane(self.active_pane);
                                // Same overlay-context push as the Ctrl+.
                                // path (`handle_agent_action`,
                                // `ActionId::ShortcutsHelp`).
                                if self.in_dashboard_overlay {
                                    contexts.push(crate::actions::When::DashboardOverlay);
                                }
                                let entries = shortcuts_help::build_entries(
                                    &contexts,
                                    registry,
                                    self.vim_mode,
                                );
                                let state = shortcuts_help::build_initial_picker_state(&entries);
                                self.active_modal = Some(ActiveModal::ShortcutsHelp {
                                    entries,
                                    state,
                                    window: Default::default(),
                                    filter_active: false,
                                    collapsed_sections:
                                        crate::views::shortcuts_help::default_collapsed(),
                                    expanded_ids: std::collections::HashSet::new(),
                                    mode: crate::views::shortcuts_help::ShortcutsHelpMode::Browse,
                                });
                                InputOutcome::Changed
                            }
                            PaletteCommand::Memory => {
                                self.active_modal = None;
                                InputOutcome::Action(Action::OpenMemoryModal)
                            }
                            PaletteCommand::OpenExtensionsTab(tab) => {
                                self.active_modal = None;
                                InputOutcome::Action(Action::OpenExtensionsModal {
                                    tab,
                                    trigger: xai_grok_telemetry::events::ExtensionsModalTrigger::CommandPalette,
                                })
                            }
                            PaletteCommand::OpenSettings => {
                                self.active_modal = None;
                                InputOutcome::Action(Action::OpenPiSettings)
                            }
                            PaletteCommand::OpenAgentsModal => {
                                self.active_modal = None;
                                InputOutcome::Action(Action::OpenConfigAgentsModal(None))
                            }
                            PaletteCommand::EditPromptExternal => {
                                self.active_modal = None;
                                InputOutcome::Action(Action::EditPromptExternal)
                            }
                            PaletteCommand::SlashCommand(text) => {
                                let trimmed = text
                                    .trim_start_matches('/')
                                    .trim_end_matches(' ')
                                    .to_string();

                                if trimmed == "resume" {
                                    let prev = {
                                        let ActiveModal::CommandPalette { entries, state, .. } =
                                            self.active_modal.as_ref().unwrap()
                                        else {
                                            unreachable!()
                                        };
                                        Some(crate::views::modal::PaletteSnapshot {
                                            entries: entries.clone(),
                                            state: state.clone(),
                                        })
                                    };
                                    self.active_modal = Some(ActiveModal::SessionPicker {
                                        state: crate::views::picker::PickerState::default(),
                                        entries: None,
                                        loading: true,
                                        lanes: Default::default(),
                                        previous_palette: prev,
                                        window: crate::views::modal_window::ModalWindowState::new(),
                                        content_results: None,
                                        content_loading: false,
                                        deep_search_seq: 0,
                                        entries_query: None,
                                        source_filter:
                                            crate::views::session_picker::SourceFilter::default(),
                                        pending_delete: None,
                                        preview_scroll: 0,
                                        search_mode: false,
                                        preview_mode: false,
                                        preview_messages: None,
                                    });
                                    return InputOutcome::Action(Action::FetchSessionList);
                                }

                                let is_picker =
                                    matches!(trimmed.as_str(), "model" | "m" | "theme" | "t");
                                if is_picker
                                    && let Some(command) =
                                        self.prompt.slash_controller.registry().get(&trimmed)
                                {
                                    let ctx =
                                        self.prompt.slash_controller.app_ctx(&self.session.models);
                                    if let Some(items) = command.suggest_args(&ctx, "")
                                        && !items.is_empty()
                                    {
                                        // Save palette state for Esc restore.
                                        let prev = {
                                            let ActiveModal::CommandPalette {
                                                entries, state, ..
                                            } = self.active_modal.as_ref().unwrap()
                                            else {
                                                unreachable!()
                                            };
                                            Some(crate::views::modal::PaletteSnapshot {
                                                entries: entries.clone(),
                                                state: state.clone(),
                                            })
                                        };
                                        self.active_modal = Some(ActiveModal::ArgPicker {
                                            command: trimmed,
                                            args_query: String::new(),
                                            items: items.clone(),
                                            original_items: items,
                                            // Type-to-find: open in input mode (vim: Esc→nav, i→input).
                                            state: crate::views::picker::PickerState::input_active(
                                            ),
                                            previous_palette: prev,
                                            previous_settings: None,
                                            selection: ArgPickerSelection::RunCommand,
                                            window:
                                                crate::views::modal_window::ModalWindowState::new(),
                                        });
                                        return InputOutcome::Changed;
                                    }
                                }
                                self.active_modal = None;
                                InputOutcome::Action(Action::SendSlashCommandPreservingDraft(text))
                            }
                            PaletteCommand::SectionHeader(_) => InputOutcome::Changed,
                        }
                    }
                    PickerOutcome::Closed => {
                        self.active_modal = None;
                        InputOutcome::Changed
                    }
                    PickerOutcome::QueryChanged => {
                        // Re-filter entries based on updated query.
                        let sharing_enabled = self.sharing_enabled;
                        if let Some(ActiveModal::CommandPalette { entries, state, .. }) =
                            self.active_modal.as_mut()
                        {
                            *entries = crate::views::modal::filter_palette_entries(
                                state.query(),
                                sharing_enabled,
                                &self.prompt.slash_controller,
                            );
                            state.selected = state.selected.min(entries.len().saturating_sub(1));
                        }
                        InputOutcome::Changed
                    }
                    PickerOutcome::Changed => InputOutcome::Changed,
                    PickerOutcome::Unchanged => InputOutcome::Unchanged,
                    _ => InputOutcome::Changed,
                }
            }
            ActiveModal::ArgPicker { .. } => unreachable!("routed via handle_arg_picker_input"),
            ActiveModal::SessionTree { .. } => {
                // Modal already restored by caller for owned-match paths; for
                // ref matches this arm is unused because early routing handles it.
                self.handle_session_tree_input(ev)
            }
            ActiveModal::SessionPicker {
                entries,
                state,
                loading: _,
                previous_palette,
                content_results,
                content_loading,
                entries_query,
                source_filter,
                pending_delete,
                window,
                search_mode,
                preview_mode,
                preview_messages,
                preview_scroll,
                ..
            } => {
                use crate::views::session_picker::{
                    CONTENT_EXPAND_OFFSET, PickerItem, SessionPickerWorktreeSelection,
                    build_entry_map, effective_filter_query, session_picker_worktree_selection,
                    sync_session_picker_query_expansion,
                };

                // Build grouped mapping using shared helper (now with content).
                // Pin the current session's repo group using the live agent cwd.
                let current_repo = crate::views::session_picker::repo_name_from_cwd(
                    &self.session.cwd.to_string_lossy(),
                );
                let entry_map = build_entry_map(
                    entries.as_deref(),
                    content_results.as_deref(),
                    effective_filter_query(state.query(), entries_query.as_deref()),
                    true,
                    *content_loading,
                    *source_filter,
                    Some(current_repo.as_str()),
                );
                let entry_count = entry_map.len();
                let non_sel: Vec<bool> = entry_map.iter().map(|e| e.is_none()).collect();
                let focused_is_foreign = match entry_map
                    .get(state.selected)
                    .and_then(|entry| entry.as_ref())
                {
                    Some(PickerItem::Fuzzy { original_index }) => entries
                        .as_ref()
                        .and_then(|entries| entries.get(*original_index))
                        .is_some_and(|entry| {
                            crate::app::foreign_sessions::is_foreign_picker_source(&entry.source)
                        }),
                    _ => false,
                };

                // Chat-mode picker lists conversations only: the source
                // filter and local-disk delete are dead weight there.
                let chat_mode = self.app_chat_mode;
                let external_picker =
                    *source_filter == crate::views::session_picker::SourceFilter::External;
                let external_tabs = ["Current folder", "All"];
                let config = PickerConfig {
                    title: Some("Resume session"),
                    show_search_hint: true,
                    expandable: true,
                    esc_clears_query: false, // Esc returns to palette or closes
                    shortcuts: Some(crate::views::picker::picker_shortcuts()),
                    pending_hint: None,
                    non_selectable: &non_sel,
                    non_selectable_clickable: &[],
                    shortcuts_area: None,
                    tabs: external_picker.then_some(&external_tabs),
                    active_tab: if external_picker {
                        window.active_tab
                    } else {
                        0
                    },
                    filter_label: (!chat_mode && !external_picker).then(|| source_filter.label()),
                    filter_key_hint: (!chat_mode && !external_picker).then_some("f"),
                    filter_active: !chat_mode && !external_picker && source_filter.is_active(),
                    header_note: None,
                    action_keys: if external_picker {
                        &[('s', "sort")]
                    } else if chat_mode || focused_is_foreign {
                        &[]
                    } else {
                        &[('d', "delete"), ('s', "sort")]
                    },
                    disable_search: false,
                    compact_bottom_bar: false,
                    search_only_on_slash: false,
                    vim_normal_first: crate::appearance::cache::load_vim_mode(),
                };

                match crate::views::session_picker::handle_pending_delete_key(pending_delete, ev) {
                    crate::views::session_picker::PendingDeleteKey::Confirm(pd) => {
                        return InputOutcome::Action(Action::DeleteSession {
                            source: pd.source,
                            session_id: pd.session_id,
                            cwd: pd.cwd,
                        });
                    }
                    crate::views::session_picker::PendingDeleteKey::Cancel => {
                        return InputOutcome::Changed;
                    }
                    crate::views::session_picker::PendingDeleteKey::Disarmed
                    | crate::views::session_picker::PendingDeleteKey::NotArmed => {}
                }

                // Ctrl+F full-text page: External + F2 `psm_resume_index` only.
                // Adapter still requires PSM online (port 52131).
                let psm_features = crate::appearance::cache::load_psm_resume_index();
                if psm_features
                    && *source_filter == crate::views::session_picker::SourceFilter::External
                    && let crossterm::event::Event::Key(key) = ev
                    && key.kind == KeyEventKind::Press
                    && key
                        .modifiers
                        .contains(crossterm::event::KeyModifiers::CONTROL)
                    && matches!(key.code, crossterm::event::KeyCode::Char('f'))
                {
                    *search_mode = !*search_mode;
                    if *search_mode {
                        state.search_active = true;
                        state.set_query("");
                        *content_results = None;
                        *content_loading = false;
                        state.selected = 0;
                        state.scroll_offset = None;
                    }
                    return InputOutcome::Changed;
                }

                // ── Preview mode (PSM/external picker only) ──
                // Right loads messages from PSM and enters preview; Left exits.
                // Right loads messages from PSM and enters preview; Left exits.
                if psm_features
                    && *source_filter == crate::views::session_picker::SourceFilter::External
                    && let crossterm::event::Event::Key(key) = ev
                    && matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
                    && key.modifiers.is_empty()
                {
                    match key.code {
                        crossterm::event::KeyCode::Right
                            if !*preview_mode && key.kind == KeyEventKind::Press =>
                        {
                            let selected = entry_map
                                .get(state.selected)
                                .and_then(|entry| entry.as_ref());
                            match selected {
                                Some(PickerItem::Fuzzy { original_index }) => {
                                    if let Some(entry) =
                                        entries.as_ref().and_then(|ents| ents.get(*original_index))
                                    {
                                        return InputOutcome::Action(Action::LoadSessionPreview {
                                            session_id: entry.id.clone(),
                                            session_path: entry.session_path.clone(),
                                        });
                                    }
                                }
                                Some(PickerItem::Content { hit_index }) => {
                                    if let Some(hit) = content_results
                                        .as_ref()
                                        .and_then(|hits| hits.get(*hit_index))
                                    {
                                        return InputOutcome::Action(Action::LoadSessionPreview {
                                            session_id: hit.session_id.clone(),
                                            session_path: None,
                                        });
                                    }
                                }
                                _ => {}
                            }
                            // No selectable row — still enter empty preview.
                            *preview_mode = true;
                            *preview_messages = Some(Vec::new());
                            *preview_scroll = 0;
                            return InputOutcome::Changed;
                        }
                        crossterm::event::KeyCode::Esc | crossterm::event::KeyCode::Left
                            if *preview_mode && key.kind == KeyEventKind::Press =>
                        {
                            *preview_mode = false;
                            *preview_messages = None;
                            *preview_scroll = 0;
                            return InputOutcome::Changed;
                        }
                        // Scroll keys: same dispatch as Read/DocViewer (supports key-repeat).
                        code if *preview_mode
                            && crate::views::modal::apply_doc_scroll(code, preview_scroll) =>
                        {
                            return InputOutcome::Changed;
                        }
                        // y / c — copy full transcript
                        crossterm::event::KeyCode::Char(ch)
                            if *preview_mode
                                && key.kind == KeyEventKind::Press
                                && matches!(ch, 'y' | 'c') =>
                        {
                            if let Some(msgs) = preview_messages.as_ref() {
                                let text = format_preview_transcript(msgs);
                                self.copy_to_clipboard(&text);
                            }
                            return InputOutcome::Changed;
                        }
                        // Enter — resume the focused session from preview
                        crossterm::event::KeyCode::Enter
                            if *preview_mode && key.kind == KeyEventKind::Press =>
                        {
                            let selected = entry_map
                                .get(state.selected)
                                .and_then(|entry| entry.as_ref());
                            match selected {
                                Some(PickerItem::Fuzzy { original_index }) => {
                                    return InputOutcome::Action(Action::PickSession(
                                        *original_index,
                                    ));
                                }
                                Some(PickerItem::Content { hit_index }) => {
                                    if let Some(hit) = content_results
                                        .as_ref()
                                        .and_then(|hits| hits.get(*hit_index))
                                    {
                                        return InputOutcome::Action(Action::PickContentSession {
                                            session_id: hit.session_id.clone(),
                                            cwd: hit.cwd.clone(),
                                        });
                                    }
                                }
                                _ => {}
                            }
                            return InputOutcome::Changed;
                        }
                        _ => {}
                    }
                }

                // ── Search mode input handling ──
                // When search_mode is active, all keys are consumed by the
                // full-text search page (query input, Tab scope, nav, Esc).
                if *search_mode {
                    if let crossterm::event::Event::Key(key) = ev
                        && matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
                    {
                        use crossterm::event::KeyCode;
                        match key.code {
                            // Esc / Ctrl+F — exit search mode
                            KeyCode::Esc if key.kind == KeyEventKind::Press => {
                                *search_mode = false;
                                *content_results = None;
                                *content_loading = false;
                                return InputOutcome::Changed;
                            }
                            KeyCode::Char('f')
                                if key
                                    .modifiers
                                    .contains(crossterm::event::KeyModifiers::CONTROL) =>
                            {
                                *search_mode = false;
                                *content_results = None;
                                *content_loading = false;
                                return InputOutcome::Changed;
                            }
                            // Tab — toggle cwd/all scope (re-trigger search)
                            KeyCode::Tab if key.kind == KeyEventKind::Press => {
                                // Cycle source filter to toggle scope
                                return InputOutcome::Action(Action::CycleSessionSourceFilter);
                            }
                            // Enter — resume selected search result
                            KeyCode::Enter if key.kind == KeyEventKind::Press => {
                                if let Some(hits) = content_results.as_ref()
                                    && let Some(hit) = hits.get(state.selected)
                                {
                                    let session_id = hit.session_id.clone();
                                    let cwd = hit.cwd.clone();
                                    return InputOutcome::Action(Action::PickContentSession {
                                        session_id,
                                        cwd,
                                    });
                                }
                                return InputOutcome::Changed;
                            }
                            // ↑/↓ — navigate results
                            KeyCode::Up | KeyCode::Char('k')
                                if !key
                                    .modifiers
                                    .contains(crossterm::event::KeyModifiers::CONTROL) =>
                            {
                                if state.selected > 0 {
                                    state.selected -= 1;
                                    if let Some(off) = state.scroll_offset.as_mut() {
                                        if state.selected < *off {
                                            *off = state.selected;
                                        }
                                    }
                                }
                                return InputOutcome::Changed;
                            }
                            KeyCode::Down | KeyCode::Char('j')
                                if !key
                                    .modifiers
                                    .contains(crossterm::event::KeyModifiers::CONTROL) =>
                            {
                                let max = content_results
                                    .as_ref()
                                    .map_or(0, |r| r.len())
                                    .saturating_sub(1);
                                if state.selected < max {
                                    state.selected += 1;
                                }
                                return InputOutcome::Changed;
                            }
                            // Backspace — delete last char from query
                            KeyCode::Backspace if key.kind == KeyEventKind::Press => {
                                let q = state.query().to_string();
                                if !q.is_empty() {
                                    let new_q: String =
                                        q.chars().take(q.chars().count() - 1).collect();
                                    state.set_query(&new_q);
                                    state.selected = 0;
                                    state.scroll_offset = None;
                                    return InputOutcome::Action(Action::TriggerDeepSearch);
                                }
                                return InputOutcome::Changed;
                            }
                            // Regular char — append to query
                            KeyCode::Char(c)
                                if !key
                                    .modifiers
                                    .contains(crossterm::event::KeyModifiers::CONTROL) =>
                            {
                                let mut q = state.query().to_string();
                                q.push(c);
                                state.set_query(&q);
                                state.selected = 0;
                                state.scroll_offset = None;
                                return InputOutcome::Action(Action::TriggerDeepSearch);
                            }
                            _ => {}
                        }
                    }
                    return InputOutcome::Changed;
                }

                if let crossterm::event::Event::Key(key) = ev
                    && !state.search_active
                    && key.kind == KeyEventKind::Press
                    && key.modifiers.is_empty()
                    && matches!(key.code, crossterm::event::KeyCode::Char('s'))
                    && let Some(entries) = entries.as_mut()
                {
                    state.session_sort = state.session_sort.next();
                    crate::views::session_picker::sort_session_entries(entries, state.session_sort);
                    state.selected = 0;
                    state.scroll_offset = None;
                    return InputOutcome::Changed;
                }

                if let crossterm::event::Event::Key(key) = ev
                    && let Some(selection) = session_picker_worktree_selection(
                        key,
                        state,
                        &entry_map,
                        &non_sel,
                        entries.as_deref(),
                        content_results.as_deref(),
                    )
                {
                    return InputOutcome::Action(match selection {
                        SessionPickerWorktreeSelection::Fuzzy(original_index) => {
                            Action::PickSessionInWorktree(original_index)
                        }
                        SessionPickerWorktreeSelection::Content { session_id, cwd } => {
                            Action::PickContentSessionInWorktree { session_id, cwd }
                        }
                        SessionPickerWorktreeSelection::Unavailable => {
                            return InputOutcome::Changed;
                        }
                    });
                }

                let selected_before = state.selected;
                let outcome = handle_picker_input(ev, state, entry_count, &config);
                if pending_delete.is_some() && state.selected != selected_before {
                    *pending_delete = None;
                }
                match outcome {
                    PickerOutcome::Selected(i) => {
                        match entry_map.get(i).and_then(|e| e.as_ref()) {
                            Some(PickerItem::Fuzzy { original_index }) => {
                                // Don't clear active_modal here — dispatch_pick_session
                                // reads entries from it before clearing.
                                InputOutcome::Action(Action::PickSession(*original_index))
                            }
                            Some(PickerItem::Content { hit_index }) => {
                                if let Some(hits) = content_results.as_ref()
                                    && let Some(hit) = hits.get(*hit_index)
                                {
                                    InputOutcome::Action(Action::PickContentSession {
                                        session_id: hit.session_id.clone(),
                                        cwd: hit.cwd.clone(),
                                    })
                                } else {
                                    InputOutcome::Changed
                                }
                            }
                            None => InputOutcome::Changed,
                        }
                    }
                    PickerOutcome::SubmitQuery => {
                        // Free-text load only for a UUID session id.
                        // Own the id before clearing the modal (state is a
                        // reborrow of `active_modal`).
                        let load_id =
                            crate::views::session_picker::session_id_for_direct_load(state.query())
                                .map(str::to_owned);
                        if let Some(sid) = load_id {
                            self.active_modal = None;
                            InputOutcome::Action(Action::LoadSession(sid, None, false))
                        } else {
                            InputOutcome::Unchanged
                        }
                    }
                    PickerOutcome::Closed => {
                        if let Some(snapshot) = previous_palette.take() {
                            self.active_modal = Some(ActiveModal::CommandPalette {
                                entries: snapshot.entries,
                                state: snapshot.state,
                                window: crate::views::modal_window::ModalWindowState::new(),
                            });
                        } else {
                            self.active_modal = None;
                        }
                        // A search/list fetch may still be in flight; the
                        // dispatch layer must invalidate it now that the
                        // modal (its landing surface) is gone.
                        InputOutcome::Action(Action::SessionPickerClosed)
                    }
                    PickerOutcome::Expand(i) => match entry_map.get(i).and_then(|e| e.as_ref()) {
                        Some(PickerItem::Fuzzy { original_index }) => {
                            if let Some(ents) = entries.as_ref()
                                && let Some(entry) = ents.get(*original_index)
                                && !crate::app::foreign_sessions::is_foreign_picker_source(
                                    &entry.source,
                                )
                                && !state.expanded.contains(original_index)
                            {
                                InputOutcome::Action(Action::ExpandSessionCard {
                                    source: entry.source.clone(),
                                    session_id: entry.id.clone(),
                                })
                            } else {
                                InputOutcome::Changed
                            }
                        }
                        Some(PickerItem::Content { hit_index }) => {
                            if let Some(hits) = content_results.as_ref()
                                && let Some(hit) = hits.get(*hit_index)
                            {
                                InputOutcome::Action(Action::ExpandSessionCard {
                                    source: "local".into(),
                                    session_id: hit.session_id.clone(),
                                })
                            } else {
                                InputOutcome::Changed
                            }
                        }
                        None => InputOutcome::Changed,
                    },
                    PickerOutcome::Collapse(i) => match entry_map.get(i).and_then(|e| e.as_ref()) {
                        Some(PickerItem::Fuzzy { original_index }) => {
                            if let Some(ents) = entries.as_ref()
                                && let Some(entry) = ents.get(*original_index)
                                && state.expanded.contains(original_index)
                            {
                                InputOutcome::Action(Action::ExpandSessionCard {
                                    source: entry.source.clone(),
                                    session_id: entry.id.clone(),
                                })
                            } else {
                                InputOutcome::Changed
                            }
                        }
                        Some(PickerItem::Content { hit_index }) => {
                            let key = CONTENT_EXPAND_OFFSET + hit_index;
                            if state.expanded.contains(&key)
                                && let Some(hits) = content_results.as_ref()
                                && let Some(hit) = hits.get(*hit_index)
                            {
                                InputOutcome::Action(Action::ExpandSessionCard {
                                    source: "local".into(),
                                    session_id: hit.session_id.clone(),
                                })
                            } else {
                                InputOutcome::Changed
                            }
                        }
                        None => InputOutcome::Changed,
                    },
                    PickerOutcome::Copy(i) => {
                        if let Some(Some(PickerItem::Fuzzy { original_index })) = entry_map.get(i) {
                            InputOutcome::Action(Action::CopySessionId(*original_index))
                        } else {
                            InputOutcome::Changed
                        }
                    }
                    PickerOutcome::QueryChanged => {
                        sync_session_picker_query_expansion(
                            entries.as_deref(),
                            content_results.as_deref(),
                            entries_query.as_deref(),
                            state,
                            true,
                            *content_loading,
                            *source_filter,
                            Some(current_repo.as_str()),
                        );
                        InputOutcome::Action(Action::TriggerDeepSearch)
                    }
                    PickerOutcome::Changed => InputOutcome::Changed,
                    PickerOutcome::Unchanged => {
                        if let crossterm::event::Event::Key(key) = ev
                            && key.kind == KeyEventKind::Press
                            && crate::key!('/', CONTROL).matches(key)
                            && !state.query().trim().is_empty()
                        {
                            return InputOutcome::Action(Action::ForceDeepSearch);
                        }
                        InputOutcome::Unchanged
                    }
                    PickerOutcome::FilterCycled => {
                        InputOutcome::Action(Action::CycleSessionSourceFilter)
                    }
                    PickerOutcome::TabChanged(index) if external_picker => {
                        window.active_tab = index;
                        InputOutcome::Action(Action::RefreshExternalSessionCatalog)
                    }
                    PickerOutcome::Action('d') => {
                        *pending_delete =
                            crate::views::session_picker::pending_delete_from_selection(
                                state.selected,
                                &entry_map,
                                entries.as_deref(),
                                content_results.as_deref(),
                            );
                        InputOutcome::Changed
                    }
                    PickerOutcome::NonSelectableClick(_)
                    | PickerOutcome::TabChanged(_)
                    | PickerOutcome::Action(_) => InputOutcome::Changed,
                }
            }
            _ => InputOutcome::Changed,
        }
    }

    fn handle_notifications_input(&mut self, ev: &crossterm::event::Event) -> InputOutcome {
        use crate::views::picker::{PickerConfig, PickerOutcome, handle_picker_input};

        let Some(ActiveModal::Notifications { state, .. }) = self.active_modal.as_mut() else {
            return InputOutcome::Changed;
        };
        let entry_count = state.filtered_notifications().len();
        let config = PickerConfig {
            title: None,
            // Browse-only list: always type-to-search (ignore global vim nav),
            // so e/y/←/→ stay expandable actions until search is focused.
            show_search_hint: true,
            expandable: true,
            esc_clears_query: true,
            shortcuts: Some(crate::views::picker::picker_shortcuts()),
            pending_hint: None,
            non_selectable: &[],
            non_selectable_clickable: &[],
            shortcuts_area: None,
            tabs: None,
            active_tab: 0,
            filter_label: None,
            filter_key_hint: None,
            filter_active: false,
            header_note: None,
            action_keys: &[],
            disable_search: false,
            compact_bottom_bar: false,
            search_only_on_slash: false,
            // Forced off: global vim would swallow type-to-search on this modal.
            vim_normal_first: false,
        };
        match handle_picker_input(ev, &mut state.picker, entry_count, &config) {
            PickerOutcome::Closed => {
                self.active_modal = None;
                InputOutcome::Changed
            }
            // e / → / Enter toggle expand; ← / Shift+e collapse.
            PickerOutcome::Expand(i) | PickerOutcome::Selected(i) => {
                if let Some(ActiveModal::Notifications { state, .. }) = self.active_modal.as_mut() {
                    if !state.picker.expanded.insert(i) {
                        state.picker.expanded.remove(&i);
                    }
                }
                InputOutcome::Changed
            }
            PickerOutcome::Collapse(i) => {
                if let Some(ActiveModal::Notifications { state, .. }) = self.active_modal.as_mut() {
                    state.picker.expanded.remove(&i);
                }
                InputOutcome::Changed
            }
            PickerOutcome::Copy(i) => {
                if let Some(ActiveModal::Notifications { state, .. }) = self.active_modal.as_mut() {
                    let filtered = state.filtered_notifications();
                    if let Some(notification) = filtered.get(i) {
                        let text = notification.message.clone();
                        let _ = crate::clipboard::SystemClipboard::try_set(&text);
                        self.show_toast("Copied notification");
                    }
                }
                InputOutcome::Changed
            }
            PickerOutcome::Unchanged => InputOutcome::Unchanged,
            _ => InputOutcome::Changed,
        }
    }

    /// Basic input handler for documentation modals (DocPicker list / DocViewer panel).
    fn handle_doc_input(&mut self, ev: &crossterm::event::Event) -> InputOutcome {
        use crate::views::modal::ActiveModal;
        use crate::views::picker::{
            PickerConfig, PickerEntry, PickerOutcome, PickerRow, handle_picker_input,
        };

        // DocPicker list: use unified picker for nav/select
        if let Some(ActiveModal::DocPicker {
            entries,
            state,
            previous_palette,
            ..
        }) = &mut self.active_modal
        {
            // Filter entries based on search query
            let filtered: Vec<_> = if state.query().is_empty() {
                entries.iter().enumerate().collect()
            } else {
                let q = state.query().to_lowercase();
                entries
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| {
                        e.title.to_lowercase().contains(&q)
                            || e.description.to_lowercase().contains(&q)
                    })
                    .collect()
            };
            let entry_count = filtered.len();
            let non_sel: Vec<bool> = vec![false; entry_count];
            let _picker_entries: Vec<PickerEntry> = filtered
                .iter()
                .map(|(i, e)| {
                    PickerEntry::Row(PickerRow {
                        label: &e.title,
                        right_label: &e.description,
                        selected: filtered
                            .get(state.selected)
                            .map(|(o, _)| *o)
                            .unwrap_or(usize::MAX)
                            == *i,
                        expanded: false,
                        dimmed: false,
                        indent: 0,
                        label_color: None,
                        badge: "",
                        badge_color: None,
                        collapsible: false,
                        underline_last_desc: false,
                        fields: &[],
                        description_lines: &[],
                        summary_lines: &[],
                    })
                })
                .collect();
            let config = PickerConfig {
                title: Some("How-to Guides"),
                show_search_hint: false,
                expandable: false,
                esc_clears_query: true,
                shortcuts: Some(crate::views::picker::picker_shortcuts()),
                pending_hint: None,
                non_selectable: &non_sel,
                non_selectable_clickable: &[],
                shortcuts_area: None,
                tabs: None,
                active_tab: 0,
                filter_label: None,
                filter_key_hint: None,
                filter_active: false,
                header_note: None,
                action_keys: &[],
                disable_search: false,
                compact_bottom_bar: false,
                search_only_on_slash: false,
                vim_normal_first: crate::appearance::cache::load_vim_mode(),
            };
            // Handle input
            match handle_picker_input(ev, state, entry_count, &config) {
                PickerOutcome::Selected(i) => {
                    if let Some((orig_idx, _)) = filtered.get(i)
                        && let Some(doc) = entries.get(*orig_idx)
                    {
                        let title = doc.title.clone();
                        let content = doc.content.to_string();
                        // Shuttle the palette snapshot through DocViewer so it can
                        // be passed back to DocPicker when the user presses Esc.
                        let prev = previous_palette.take();
                        self.active_modal = Some(ActiveModal::DocViewer {
                            title,
                            content,
                            scroll: 0,
                            window: crate::views::modal_window::ModalWindowState::new(),
                            cached_lines: None,
                            previous_palette: prev,
                            standalone: false,
                        });
                    }
                    InputOutcome::Changed
                }
                PickerOutcome::Closed => {
                    // Restore the command palette if we have a saved snapshot
                    // (same pattern as ArgPicker / SessionPicker).
                    if let Some(snapshot) = previous_palette.take() {
                        self.active_modal = Some(ActiveModal::CommandPalette {
                            entries: snapshot.entries,
                            state: snapshot.state,
                            window: crate::views::modal_window::ModalWindowState::new(),
                        });
                    } else {
                        self.active_modal = None;
                    }
                    InputOutcome::Changed
                }
                _ => InputOutcome::Changed,
            }
        } else if let Some(ActiveModal::DocViewer { scroll, .. }) = &mut self.active_modal {
            if let Event::Key(KeyEvent { code, .. }) = ev
                && modal::apply_doc_scroll(*code, scroll)
            {
                return InputOutcome::Changed;
            }
            InputOutcome::Changed
        } else {
            InputOutcome::Changed
        }
    }
    /// Handle mouse events while a modal is active.
    ///
    /// Click on a button → same as pressing that key.
    /// Hover → update `modal_hovered_key` for highlight.
    pub(super) fn handle_modal_mouse_with_registry(
        &mut self,
        mouse: &crossterm::event::MouseEvent,
        registry: &crate::actions::ActionRegistry,
    ) -> InputOutcome {
        use crate::views::modal::ActiveModal;
        use crate::views::modal_window::{self as mw, ModalWindowOutcome};
        use crossterm::event::MouseEventKind;

        // Picker-based modals: route through ModalWindow chrome first,
        // then delegate content events to the picker input handler.
        if matches!(
            self.active_modal,
            Some(
                ActiveModal::CommandPalette { .. }
                    | ActiveModal::ArgPicker { .. }
                    | ActiveModal::Notifications { .. }
                    | ActiveModal::SessionPicker { .. }
                    | ActiveModal::DocPicker { .. }
                    | ActiveModal::DocViewer { .. }
                    | ActiveModal::ContextInfo { .. }
                    | ActiveModal::ShortcutsHelp { .. }
                    | ActiveModal::RememberNoteReview { .. }
            )
        ) {
            // Extract window for handle_modal_mouse.
            let window = match self.active_modal.as_mut() {
                Some(ActiveModal::CommandPalette { window, .. }) => window,
                Some(ActiveModal::ArgPicker { window, .. }) => window,
                Some(ActiveModal::Notifications { window, .. }) => window,
                Some(ActiveModal::SessionPicker { window, .. }) => window,
                Some(ActiveModal::DocPicker { window, .. }) => window,
                Some(ActiveModal::DocViewer { window, .. }) => window,
                Some(ActiveModal::ContextInfo { window, .. }) => window,
                Some(ActiveModal::ShortcutsHelp { window, .. }) => window,
                Some(ActiveModal::RememberNoteReview { window, .. }) => window,
                _ => unreachable!(),
            };
            let outcome = mw::handle_modal_mouse(window, mouse.kind, mouse.column, mouse.row);
            match outcome {
                ModalWindowOutcome::TabChanged(_)
                    if matches!(
                        self.active_modal,
                        Some(ActiveModal::SessionPicker {
                            source_filter: crate::views::session_picker::SourceFilter::External,
                            ..
                        })
                    ) =>
                {
                    return InputOutcome::Action(Action::RefreshExternalSessionCatalog);
                }
                ModalWindowOutcome::CloseRequested => {
                    // Match keyboard Esc: step back from model effort phase
                    // before fully dismissing the ArgPicker.
                    if self.try_arg_picker_step_back_from_effort() {
                        return InputOutcome::Changed;
                    }
                    // Match keyboard Esc: a closed SessionPicker may still
                    // have a list/search fetch in flight — the dispatch
                    // layer must invalidate it (its landing surface is gone).
                    let closed_session_picker =
                        matches!(self.active_modal, Some(ActiveModal::SessionPicker { .. }));
                    // Single take() handles all modal types to avoid the
                    // double-take bug where the first consume drops the value
                    // before the second branch can match.
                    match self.active_modal.take() {
                        Some(ActiveModal::DocViewer {
                            previous_palette,
                            standalone,
                            ..
                        }) => {
                            if !standalone {
                                self.active_modal =
                                    Some(crate::views::modal::howto_list_modal(previous_palette));
                            }
                        }
                        Some(ActiveModal::ContextInfo { .. }) => {}
                        Some(ActiveModal::ArgPicker {
                            previous_settings: Some(state),
                            ..
                        }) => {
                            self.active_modal = Some(ActiveModal::Settings { state });
                        }
                        Some(
                            ActiveModal::ArgPicker {
                                previous_palette: Some(snap),
                                ..
                            }
                            | ActiveModal::SessionPicker {
                                previous_palette: Some(snap),
                                ..
                            }
                            | ActiveModal::DocPicker {
                                previous_palette: Some(snap),
                                ..
                            },
                        ) => {
                            // Restore previous command palette from snapshot.
                            self.active_modal = Some(ActiveModal::CommandPalette {
                                entries: snap.entries,
                                state: snap.state,
                                window: crate::views::modal_window::ModalWindowState::new(),
                            });
                        }
                        _ => {
                            // No snapshot — close entirely (take() already set to None).
                        }
                    }
                    if closed_session_picker {
                        return InputOutcome::Action(Action::SessionPickerClosed);
                    }
                    return InputOutcome::Changed;
                }
                ModalWindowOutcome::Handled => {
                    return InputOutcome::Changed;
                }
                ModalWindowOutcome::Unhandled => {
                    // DocViewer / RememberNoteReview: wheel scrolls the markdown body.
                    if let Some(
                        ActiveModal::DocViewer { scroll, .. }
                        | ActiveModal::ContextInfo { scroll, .. }
                        | ActiveModal::RememberNoteReview { scroll, .. },
                    ) = self.active_modal.as_mut()
                    {
                        if modal::apply_doc_mouse_scroll(mouse.kind, scroll) {
                            return InputOutcome::Changed;
                        }
                        return InputOutcome::Changed;
                    }
                    // Session preview mode: same wheel semantics as Read/DocViewer.
                    if let Some(ActiveModal::SessionPicker {
                        preview_mode: true,
                        preview_scroll,
                        ..
                    }) = self.active_modal.as_mut()
                    {
                        if modal::apply_doc_mouse_scroll(mouse.kind, preview_scroll) {
                            return InputOutcome::Changed;
                        }
                        // Swallow other mouse in preview so it doesn't hit list picker.
                        return InputOutcome::Changed;
                    }
                    // Content area events — delegate to picker input.
                    let ev = crossterm::event::Event::Mouse(*mouse);
                    if matches!(self.active_modal, Some(ActiveModal::DocPicker { .. })) {
                        return self.handle_doc_input(&ev);
                    }
                    if matches!(self.active_modal, Some(ActiveModal::Notifications { .. })) {
                        return self.handle_notifications_input(&ev);
                    }
                    if let Some(ActiveModal::ShortcutsHelp {
                        entries,
                        state,
                        filter_active,
                        collapsed_sections,
                        expanded_ids,
                        mode,
                        ..
                    }) = &mut self.active_modal
                    {
                        use crate::views::shortcuts_help::{self, ShortcutsHelpOutcome};
                        return match shortcuts_help::handle_mouse(
                            mouse,
                            entries,
                            state,
                            *filter_active,
                            collapsed_sections,
                            mode,
                        ) {
                            ShortcutsHelpOutcome::Close => {
                                self.active_modal = None;
                                InputOutcome::Changed
                            }
                            ShortcutsHelpOutcome::ToggleFilter => {
                                *filter_active = !*filter_active;
                                state.selected = 0;
                                InputOutcome::Changed
                            }
                            ShortcutsHelpOutcome::ToggleSection(idx) => {
                                shortcuts_help::toggle_membership(collapsed_sections, idx);
                                InputOutcome::Changed
                            }
                            // Unreachable today: handle_mouse never yields ToggleExpand (a row click opens detail); kept for exhaustiveness.
                            ShortcutsHelpOutcome::ToggleExpand(action_id) => {
                                shortcuts_help::toggle_membership(expanded_ids, action_id);
                                InputOutcome::Changed
                            }
                            ShortcutsHelpOutcome::Changed => InputOutcome::Changed,
                            ShortcutsHelpOutcome::Unchanged => InputOutcome::Unchanged,
                        };
                    }
                    let ev = crossterm::event::Event::Mouse(*mouse);
                    return self.handle_palette_or_arg_input_with_registry(&ev, registry);
                }
                _ => return InputOutcome::Changed,
            }
        }

        // SessionTree: chrome first, then list click / scroll / double-click go.
        if let Some(ActiveModal::SessionTree { state, window }) = &mut self.active_modal {
            let skip_summary = state.skip_summary_prompt;
            let outcome = mw::handle_modal_mouse(window, mouse.kind, mouse.column, mouse.row);
            match outcome {
                ModalWindowOutcome::CloseRequested => {
                    self.active_modal = None;
                    return InputOutcome::Action(Action::SessionTreeClosed);
                }
                ModalWindowOutcome::Handled => return InputOutcome::Changed,
                ModalWindowOutcome::Unhandled => {
                    return Self::handle_session_tree_mouse(state, mouse, skip_summary);
                }
                _ => return InputOutcome::Changed,
            }
        }

        // TreeMap: chrome first, then row click / scroll.
        if matches!(self.active_modal, Some(ActiveModal::TreeMap { .. })) {
            return self.handle_tree_map_mouse(mouse);
        }

        // MemoryBrowser: route through ModalWindow chrome, then delegate.
        if let Some(ActiveModal::MemoryBrowser { state }) = &mut self.active_modal {
            let outcome =
                mw::handle_modal_mouse(&mut state.window, mouse.kind, mouse.column, mouse.row);
            match outcome {
                ModalWindowOutcome::CloseRequested => {
                    self.active_modal = None;
                    return InputOutcome::Changed;
                }
                ModalWindowOutcome::Handled => return InputOutcome::Changed,
                ModalWindowOutcome::Unhandled => {
                    return crate::views::memory_modal::handle_memory_mouse(
                        state,
                        mouse.kind,
                        mouse.column,
                        mouse.row,
                    );
                }
                _ => return InputOutcome::Changed,
            }
        }

        // Settings: route through ModalWindow chrome, then delegate.
        if let Some(ActiveModal::Settings { state }) = &mut self.active_modal {
            let outcome =
                mw::handle_modal_mouse(&mut state.window, mouse.kind, mouse.column, mouse.row);
            match outcome {
                ModalWindowOutcome::CloseRequested => {
                    self.active_modal = None;
                    return InputOutcome::Changed;
                }
                ModalWindowOutcome::Handled => {
                    if matches!(mouse.kind, MouseEventKind::Moved) {
                        state.hover_row = None;
                    }
                    return InputOutcome::Changed;
                }
                ModalWindowOutcome::Unhandled => {
                    let out = crate::views::settings_modal::handle_settings_mouse(
                        state,
                        mouse.kind,
                        mouse.column,
                        mouse.row,
                    );
                    return apply_settings_outcome(self, out);
                }
                _ => return InputOutcome::Changed,
            }
        }

        // Pi resources: modal chrome owns close/tabs, then the native resource
        // tree receives clicks and wheel input.
        if let Some(ActiveModal::PiConfig { state }) = &mut self.active_modal {
            let outcome =
                mw::handle_modal_mouse(&mut state.window, mouse.kind, mouse.column, mouse.row);
            return match outcome {
                ModalWindowOutcome::CloseRequested => {
                    state.complete_picker(false);
                    self.active_modal = None;
                    InputOutcome::Changed
                }
                ModalWindowOutcome::TabChanged(index) => {
                    state.select_tab(index);
                    InputOutcome::Changed
                }
                ModalWindowOutcome::Handled => InputOutcome::Changed,
                ModalWindowOutcome::Unhandled => match state.handle_mouse(mouse) {
                    crate::views::pi_config::PiConfigOutcome::Close => {
                        state.complete_picker(false);
                        self.active_modal = None;
                        InputOutcome::Changed
                    }
                    crate::views::pi_config::PiConfigOutcome::PickerSubmit => {
                        state.complete_picker(true);
                        self.active_modal = None;
                        InputOutcome::Changed
                    }
                    crate::views::pi_config::PiConfigOutcome::Changed => InputOutcome::Changed,
                },
                _ => InputOutcome::Changed,
            };
        }

        // grok-pi settings panel: the chrome owns close and tab clicks, the
        // panel owns sidebar, row, and chooser hit testing.
        if let Some(ActiveModal::PiSettings { state }) = &mut self.active_modal {
            let outcome =
                mw::handle_modal_mouse(&mut state.window, mouse.kind, mouse.column, mouse.row);
            return match outcome {
                ModalWindowOutcome::CloseRequested => {
                    self.active_modal = None;
                    InputOutcome::Changed
                }
                ModalWindowOutcome::TabChanged(index) => {
                    state.set_active_tab(index);
                    InputOutcome::Changed
                }
                ModalWindowOutcome::Handled => {
                    if matches!(mouse.kind, MouseEventKind::Moved) {
                        state.hover = None;
                    }
                    InputOutcome::Changed
                }
                ModalWindowOutcome::Unhandled => {
                    let out = crate::views::pi_settings::handle_mouse(
                        state,
                        mouse.kind,
                        mouse.column,
                        mouse.row,
                    );
                    apply_pi_settings_outcome(self, out)
                }
                _ => InputOutcome::Changed,
            };
        }

        // Pi models: modal chrome owns the frame; the model center owns
        // dirty-close confirmation and content hit testing.
        if let Some(ActiveModal::PiModels { state }) = &mut self.active_modal {
            let outcome =
                mw::handle_modal_mouse(&mut state.window, mouse.kind, mouse.column, mouse.row);
            return match outcome {
                ModalWindowOutcome::CloseRequested => {
                    let key = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
                    match state.handle_key(&key) {
                        crate::views::pi_models::PiModelsOutcome::Close => {
                            self.active_modal = None;
                            InputOutcome::Changed
                        }
                        _ => InputOutcome::Changed,
                    }
                }
                ModalWindowOutcome::Handled => InputOutcome::Changed,
                ModalWindowOutcome::Unhandled => {
                    state.handle_mouse(mouse);
                    InputOutcome::Changed
                }
                _ => InputOutcome::Changed,
            };
        }

        // ResetSettingsConfirm: route mouse events through the
        // modal-window chrome.
        if let Some(ActiveModal::ResetSettingsConfirm { settings_state, .. }) =
            &mut self.active_modal
        {
            let outcome = mw::handle_modal_mouse(
                &mut settings_state.window,
                mouse.kind,
                mouse.column,
                mouse.row,
            );
            return match outcome {
                ModalWindowOutcome::CloseRequested => {
                    // Close-button (X) click → Cancel. Mirrors Esc /
                    // F2 / Ctrl+, keyboard semantics.
                    InputOutcome::Action(Action::ConfirmResetSetting {
                        choice: crate::views::modal::ResetSettingsResult::Cancel,
                    })
                }
                ModalWindowOutcome::ShortcutActivated(id) => {
                    use crate::views::modal::{
                        RESET_CONFIRM_NO_ID, RESET_CONFIRM_YES_ID, ResetSettingsResult,
                    };
                    let choice = if id == RESET_CONFIRM_YES_ID {
                        ResetSettingsResult::Reset
                    } else if id == RESET_CONFIRM_NO_ID {
                        ResetSettingsResult::Cancel
                    } else {
                        return InputOutcome::Changed;
                    };
                    InputOutcome::Action(Action::ConfirmResetSetting { choice })
                }
                ModalWindowOutcome::Handled => InputOutcome::Changed,
                _ => InputOutcome::Unchanged,
            };
        }

        // Standard modal mouse handling (EditConfirm).
        match mouse.kind {
            MouseEventKind::Down(crossterm::event::MouseButton::Left) => {
                for btn in &self.modal_buttons {
                    if btn.rect.contains((mouse.column, mouse.row).into()) {
                        let key = KeyEvent::new(KeyCode::Char(btn.key), KeyModifiers::NONE);
                        return self.handle_modal_key_with_registry(&key, registry);
                    }
                }
                InputOutcome::Changed
            }
            MouseEventKind::Moved => {
                let new_hover = self
                    .modal_buttons
                    .iter()
                    .find(|btn| btn.rect.contains((mouse.column, mouse.row).into()))
                    .map(|btn| btn.key);
                if new_hover != self.modal_hovered_key {
                    self.modal_hovered_key = new_hover;
                    InputOutcome::Changed
                } else {
                    InputOutcome::Unchanged
                }
            }
            _ => InputOutcome::Changed,
        }
    }

    /// Draw the active modal overlay: the per-`ActiveModal`-variant render
    /// dispatch, called from `draw` which early-returns afterwards.
    ///
    /// `pub(crate)` so minimal mode's overlay host can reuse the exact same
    /// centered-popup rendering (hosting the command palette / shortcuts help /
    /// settings / pickers in its grown live viewport — see
    /// `crate::minimal::overlay::render_app_modal`).
    // Allow inherited from `draw`: covers the nested picker render helpers.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn draw_active_modal(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        theme: Theme,
        compact: bool,
    ) {
        if let Some(ref mut active_modal) = self.active_modal {
            use crate::views::modal_window::{
                self as mw, ModalSizing, ModalWindowConfig, Shortcut,
            };
            use crate::views::picker::{self, PickerEntry, PickerRow};

            // Standard footer shortcuts for picker-style modals.
            let mut picker_shortcuts: Vec<Shortcut> = vec![
                Shortcut {
                    label: "\u{2191}/\u{2193} nav",
                    clickable: false,
                    id: 0,
                },
                Shortcut {
                    label: "Enter select",
                    clickable: false,
                    id: 0,
                },
                Shortcut {
                    label: "Esc close",
                    clickable: false,
                    id: 0,
                },
            ];

            // EditConfirm has no draw arm and is no longer armed anywhere (the
            // dirty pane-switch lock blocks instead) — arming it would capture
            // all input invisibly.
            if let modal::ActiveModal::CommandPalette {
                entries: _,
                state,
                window,
            } = active_modal
            {
                // Command palette: ModalWindow chrome + picker content.
                let filtered = modal::filter_palette_entries(
                    state.query(),
                    self.sharing_enabled,
                    &self.prompt.slash_controller,
                );
                let non_sel: Vec<bool> = filtered
                    .iter()
                    .map(|e| matches!(e.command, modal::PaletteCommand::SectionHeader(_)))
                    .collect();
                let picker_entries: Vec<PickerEntry> = filtered
                    .iter()
                    .enumerate()
                    .map(|(i, e)| {
                        if matches!(e.command, modal::PaletteCommand::SectionHeader(_)) {
                            PickerEntry::Header { label: &e.label }
                        } else {
                            PickerEntry::Row(PickerRow {
                                label: &e.label,
                                right_label: &e.shortcut,
                                selected: state.hovered == Some(i)
                                    || (state.hovered.is_none() && i == state.selected),
                                expanded: false,
                                fields: &[],
                                description_lines: &[],
                                summary_lines: &[],
                                dimmed: false,
                                indent: 0,
                                label_color: None,
                                badge: "",
                                badge_color: None,
                                collapsible: false,
                                underline_last_desc: false,
                            })
                        }
                    })
                    .collect();
                let compact = self.scrollback.appearance().prompt.compact;
                // Surface `i search` in the footer when vim nav mode is active.
                mw::push_vim_nav_search_hint(&mut picker_shortcuts, state.search_active);
                let modal_config = ModalWindowConfig {
                    title: "Commands",
                    tabs: None,
                    shortcuts: &picker_shortcuts,
                    sizing: ModalSizing {
                        width_pct: 0.50,
                        max_width: 80,
                        min_width: 44,
                        v_margin: 4,
                        h_pad: 2,
                        v_pad: 1,
                        footer_lines: 2,
                    }
                    .with_compact(compact),
                    fold_info: None,
                };
                if let Some(mca) = mw::render_modal_window(buf, area, window, &modal_config, &theme)
                {
                    picker::render_picker_in_modal(
                        buf,
                        mca.content,
                        mca.inner_x,
                        mca.inner_width,
                        &theme,
                        state,
                        &picker_entries,
                        &non_sel,
                        false,
                    );
                }
            } else if let modal::ActiveModal::ArgPicker {
                command,
                args_query,
                items,
                state,
                window,
                ..
            } = active_modal
            {
                // Arg picker: ModalWindow chrome + picker content.
                let title = match command.as_str() {
                    "model" | "m" if !args_query.is_empty() => "Pick reasoning effort",
                    "model" | "m" => "Pick model",
                    "theme" | "t" => "Pick theme",
                    _ => "Pick option",
                };
                // Model list rows show the model name and provider. Metadata for
                // the hovered/selected model is rendered in a bottom detail pane
                // (pi-model-selector-x), not as a right-column label.
                let is_model_list =
                    matches!(command.as_str(), "model" | "m") && args_query.is_empty();
                let empty_right = "";
                let picker_entries: Vec<PickerEntry> = items
                    .iter()
                    .enumerate()
                    .map(|(i, item)| {
                        PickerEntry::Row(PickerRow {
                            label: &item.display,
                            right_label: if is_model_list {
                                empty_right
                            } else {
                                &item.description
                            },
                            selected: state.hovered == Some(i)
                                || (state.hovered.is_none() && i == state.selected),
                            expanded: false,
                            fields: &[],
                            description_lines: &[],
                            summary_lines: &[],
                            dimmed: false,
                            indent: 0,
                            label_color: None,
                            badge: "",
                            badge_color: None,
                            collapsible: false,
                            underline_last_desc: false,
                        })
                    })
                    .collect();
                let compact = self.scrollback.appearance().prompt.compact;
                // Surface `i search` in the footer when vim nav mode is active.
                mw::push_vim_nav_search_hint(&mut picker_shortcuts, state.search_active);
                // Model picker is shorter than generic arg pickers (theme, etc.).
                let is_model_picker = matches!(command.as_str(), "model" | "m");
                let sizing = ModalSizing {
                    width_pct: if is_model_list { 0.55 } else { 0.50 },
                    max_width: if is_model_list { 88 } else { 80 },
                    min_width: 44,
                    // Keep the model list at roughly 70% of the terminal height,
                    // including when compact prompt mode is enabled.
                    v_margin: if is_model_list {
                        area.height.saturating_mul(15).saturating_add(99) / 100
                    } else if is_model_picker {
                        8
                    } else {
                        4
                    },
                    h_pad: 2,
                    v_pad: 1,
                    footer_lines: 2,
                };
                let modal_config = ModalWindowConfig {
                    title,
                    tabs: None,
                    shortcuts: &picker_shortcuts,
                    sizing: if is_model_list {
                        sizing
                    } else {
                        sizing.with_compact(compact)
                    },
                    fold_info: None,
                };
                if let Some(mca) = mw::render_modal_window(buf, area, window, &modal_config, &theme)
                {
                    let detail_lines = if is_model_list {
                        selected_model_detail_lines(items, state, &self.session.models)
                    } else {
                        Vec::new()
                    };
                    let (list_area, detail_area) =
                        split_model_picker_content(mca.content, detail_lines.len());
                    picker::render_picker_in_modal(
                        buf,
                        list_area,
                        mca.inner_x,
                        mca.inner_width,
                        &theme,
                        state,
                        &picker_entries,
                        &[],
                        false,
                    );
                    if let Some(detail_area) = detail_area {
                        render_model_picker_detail(buf, detail_area, &detail_lines, &theme);
                    }
                }
            } else if let modal::ActiveModal::Notifications { state, window } = active_modal {
                // label = first line preview; expand body = full multi-line message.
                let rows: Vec<(String, String, Vec<String>)> = state
                    .filtered_notifications()
                    .into_iter()
                    .map(|notification| {
                        let kind = notification.kind.clone().unwrap_or_else(|| "info".into());
                        let body_lines: Vec<String> = if notification.message.is_empty() {
                            vec![String::new()]
                        } else {
                            notification.message.lines().map(str::to_owned).collect()
                        };
                        let label = body_lines
                            .first()
                            .cloned()
                            .unwrap_or_else(|| notification.message.clone());
                        (label, kind, body_lines)
                    })
                    .collect();
                let desc_lines: Vec<Vec<&str>> = rows
                    .iter()
                    .map(|(_, _, body)| body.iter().map(String::as_str).collect())
                    .collect();
                let picker_entries: Vec<PickerEntry> = rows
                    .iter()
                    .enumerate()
                    .map(|(index, (label, kind, _))| {
                        let is_expanded = state.picker.expanded.contains(&index);
                        PickerEntry::Row(PickerRow {
                            label,
                            right_label: kind,
                            selected: state.picker.hovered == Some(index)
                                || (state.picker.hovered.is_none()
                                    && index == state.picker.selected),
                            expanded: is_expanded,
                            fields: &[],
                            description_lines: if is_expanded { &desc_lines[index] } else { &[] },
                            summary_lines: &[],
                            dimmed: false,
                            indent: 0,
                            label_color: None,
                            badge: "",
                            badge_color: None,
                            collapsible: true,
                            underline_last_desc: false,
                        })
                    })
                    .collect();
                picker_shortcuts.push(Shortcut {
                    label: "e/→/↵ expand",
                    clickable: false,
                    id: 0,
                });
                picker_shortcuts.push(Shortcut {
                    label: "← collapse",
                    clickable: false,
                    id: 0,
                });
                picker_shortcuts.push(Shortcut {
                    label: "y copy",
                    clickable: false,
                    id: 0,
                });
                picker_shortcuts.push(Shortcut {
                    label: "type search",
                    clickable: false,
                    id: 0,
                });
                let modal_config = ModalWindowConfig {
                    title: "Notifications",
                    tabs: None,
                    shortcuts: &picker_shortcuts,
                    sizing: ModalSizing {
                        width_pct: 0.70,
                        max_width: 120,
                        min_width: 48,
                        v_margin: 4,
                        h_pad: 2,
                        v_pad: 1,
                        footer_lines: 2,
                    }
                    .with_compact(compact),
                    fold_info: None,
                };
                if let Some(mca) = mw::render_modal_window(buf, area, window, &modal_config, &theme)
                {
                    picker::render_picker_in_modal(
                        buf,
                        mca.content,
                        mca.inner_x,
                        mca.inner_width,
                        &theme,
                        &mut state.picker,
                        &picker_entries,
                        &[],
                        false,
                    );
                }
            } else if let modal::ActiveModal::SessionTree { state, window } = active_modal {
                use crate::views::session_tree::render_session_tree;
                let shortcuts = [
                    mw::Shortcut {
                        label: "↑/↓ nav",
                        clickable: false,
                        id: 0,
                    },
                    mw::Shortcut {
                        label: "click select",
                        clickable: false,
                        id: 0,
                    },
                    mw::Shortcut {
                        label: "dblclick go",
                        clickable: false,
                        id: 0,
                    },
                    mw::Shortcut {
                        label: "Tab fold",
                        clickable: false,
                        id: 0,
                    },
                    mw::Shortcut {
                        label: "Esc close",
                        clickable: false,
                        id: 0,
                    },
                ];
                let title = if state.loading {
                    "Session tree · loading".to_string()
                } else {
                    format!("Session tree · {}", state.filter.label())
                };
                let modal_config = ModalWindowConfig {
                    title: title.as_str(),
                    tabs: None,
                    shortcuts: &shortcuts,
                    sizing: ModalSizing {
                        width_pct: 0.72,
                        max_width: 120,
                        min_width: 56,
                        v_margin: 2,
                        h_pad: 1,
                        v_pad: 0,
                        footer_lines: 2,
                    }
                    .with_compact(compact),
                    fold_info: None,
                };
                if let Some(mca) = mw::render_modal_window(buf, area, window, &modal_config, &theme)
                {
                    render_session_tree(buf, mca.content, state, &theme);
                }
            } else if let modal::ActiveModal::TreeMap { state, window } = active_modal {
                use crate::views::tree_map::render_tree_map;
                let shortcuts = [
                    mw::Shortcut {
                        label: "↑/↓ nav",
                        clickable: false,
                        id: 0,
                    },
                    mw::Shortcut {
                        label: "click go",
                        clickable: false,
                        id: 0,
                    },
                    mw::Shortcut {
                        label: "Esc close",
                        clickable: false,
                        id: 0,
                    },
                ];
                let title = if state.loading {
                    "Branch map · loading".to_string()
                } else {
                    format!("Branch map · {} messages", state.nodes.len())
                };
                let modal_config = ModalWindowConfig {
                    title: title.as_str(),
                    tabs: None,
                    shortcuts: &shortcuts,
                    sizing: ModalSizing {
                        width_pct: 0.60,
                        max_width: 90,
                        min_width: 44,
                        v_margin: 3,
                        h_pad: 1,
                        v_pad: 0,
                        footer_lines: 1,
                    }
                    .with_compact(compact),
                    fold_info: None,
                };
                if let Some(mca) = mw::render_modal_window(buf, area, window, &modal_config, &theme)
                {
                    render_tree_map(buf, mca.content, state, &theme);
                }
            } else if let modal::ActiveModal::SessionPicker {
                entries,
                state,
                loading,
                lanes,
                window,
                content_results,
                content_loading,
                entries_query,
                source_filter,
                pending_delete,
                preview_scroll,
                search_mode,
                preview_mode,
                preview_messages,
                ..
            } = active_modal
            {
                // ── Preview mode: markdown message stream (reuses MarkdownContent) ──
                if *preview_mode {
                    let compact = self.scrollback.appearance().prompt.compact;
                    let preview_shortcuts: Vec<Shortcut> = vec![
                        Shortcut {
                            label: "\u{2191}\u{2193} scroll",
                            clickable: false,
                            id: 0,
                        },
                        Shortcut {
                            label: "PgUp/PgDn page",
                            clickable: false,
                            id: 0,
                        },
                        Shortcut {
                            label: "y copy",
                            clickable: false,
                            id: 0,
                        },
                        Shortcut {
                            label: "\u{2190} back",
                            clickable: false,
                            id: 0,
                        },
                        Shortcut {
                            label: "\u{23ce} resume",
                            clickable: false,
                            id: 0,
                        },
                    ];
                    let modal_config = ModalWindowConfig {
                        title: "Session preview",
                        tabs: None,
                        shortcuts: &preview_shortcuts,
                        sizing: ModalSizing {
                            width_pct: 0.85,
                            max_width: 180,
                            min_width: 48,
                            v_margin: 3,
                            h_pad: 1,
                            v_pad: 0,
                            footer_lines: 2,
                        }
                        .with_compact(compact),
                        fold_info: None,
                    };
                    if let Some(mca) =
                        mw::render_modal_window(buf, area, window, &modal_config, &theme)
                    {
                        render_session_message_preview(
                            buf,
                            mca.content,
                            preview_messages.as_deref(),
                            preview_scroll,
                            &theme,
                        );
                    }
                } else
                // ── Search mode: dedicated full-text search page ──
                // ── Search mode: dedicated full-text search page ──
                if *search_mode {
                    let compact = self.scrollback.appearance().prompt.compact;
                    let external =
                        *source_filter == crate::views::session_picker::SourceFilter::External;
                    let scope_label = if external { "cwd" } else { "all" };
                    let hits = content_results.as_deref().unwrap_or(&[]);
                    let result_count = hits.len();
                    let title = if *content_loading {
                        format!("Full-text search · {scope_label} · searching…")
                    } else if result_count > 0 {
                        format!("Full-text search · {scope_label} · {result_count} results")
                    } else {
                        format!("Full-text search · {scope_label}")
                    };
                    let search_shortcuts: Vec<Shortcut> = vec![
                        Shortcut {
                            label: "↑↓ nav",
                            clickable: false,
                            id: 0,
                        },
                        Shortcut {
                            label: "Tab scope",
                            clickable: false,
                            id: 0,
                        },
                        Shortcut {
                            label: "Enter resume",
                            clickable: false,
                            id: 0,
                        },
                        Shortcut {
                            label: "Esc back",
                            clickable: false,
                            id: 0,
                        },
                    ];
                    let modal_config = ModalWindowConfig {
                        title: title.as_str(),
                        tabs: None,
                        shortcuts: &search_shortcuts,
                        sizing: ModalSizing {
                            width_pct: 0.85,
                            max_width: 180,
                            min_width: 48,
                            v_margin: 3,
                            h_pad: 1,
                            v_pad: 0,
                            footer_lines: 2,
                        }
                        .with_compact(compact),
                        fold_info: None,
                    };
                    if let Some(mca) =
                        mw::render_modal_window(buf, area, window, &modal_config, &theme)
                    {
                        let content_area = mca.content;
                        // Search input bar
                        picker::render_picker_search_bar(
                            buf,
                            content_area.x,
                            content_area.y,
                            content_area.width,
                            &theme,
                            state,
                            true, // always active in search mode
                            true,
                            Some(theme.bg_base),
                        );
                        let sep_y = content_area.y + 1;
                        if sep_y < content_area.y + content_area.height {
                            picker::render_divider(
                                buf,
                                mca.inner_x,
                                sep_y,
                                mca.inner_width,
                                &theme,
                                Some(theme.bg_base),
                            );
                        }
                        let results_area = Rect {
                            x: content_area.x,
                            y: sep_y + 1,
                            width: content_area.width,
                            height: content_area
                                .height
                                .saturating_sub(sep_y + 1 - content_area.y),
                        };
                        // Multi-line rich hits (title + wrapped snippet + short meta).
                        let hits = content_results.as_deref().unwrap_or(&[]);
                        if hits.is_empty() && !*content_loading {
                            let msg = if state.query().trim().is_empty() {
                                "  Type to search across all session messages…"
                            } else {
                                "  No results found"
                            };
                            let line = ratatui::text::Line::from(ratatui::text::Span::styled(
                                msg,
                                ratatui::style::Style::default().fg(theme.gray_dim),
                            ));
                            ratatui::widgets::Paragraph::new(vec![line]).render(results_area, buf);
                        } else {
                            let (item_rects, entry_indices) = render_search_result_hits(
                                buf,
                                results_area,
                                hits,
                                state.selected,
                                &mut state.scroll_offset,
                                *content_loading,
                                &theme,
                            );
                            state.hit_areas = Some(picker::PickerHitAreas {
                                close_button: Rect::default(),
                                search_bar: Rect::new(
                                    content_area.x,
                                    content_area.y,
                                    content_area.width,
                                    1,
                                ),
                                item_rects,
                                entry_indices,
                                tab_rects: vec![],
                                filter_rect: None,
                            });
                        }
                    }
                } else {
                    // Session picker: ModalWindow chrome + picker content.
                    use crate::app::app_view::filter_session_entries;
                    use crate::views::picker::PickerField;
                    use crate::views::session_picker::{
                        build_content_entry_data, build_content_header_label,
                    };
                    // While a delete confirmation is armed, the footer swaps to a
                    // "y confirm / n cancel" prompt. Otherwise show the normal
                    // hints plus the `d delete` action. Chat mode drops the
                    // deep-search / filter / delete hints (local-disk-row actions).
                    let chat_mode = self.app_chat_mode;
                    let mut session_shortcuts: Vec<Shortcut> = if pending_delete.is_some() {
                        vec![
                            Shortcut {
                                label: "y confirm delete",
                                clickable: false,
                                id: 0,
                            },
                            Shortcut {
                                label: "n cancel",
                                clickable: false,
                                id: 0,
                            },
                        ]
                    } else {
                        let external =
                            *source_filter == crate::views::session_picker::SourceFilter::External;
                        let mut shortcuts = vec![Shortcut {
                            label: "\u{2191}\u{2193} nav",
                            clickable: false,
                            id: 0,
                        }];
                        shortcuts.extend([
                            Shortcut {
                                label: "\u{23ce} resume",
                                clickable: false,
                                id: 0,
                            },
                            Shortcut {
                                label: "e expand",
                                clickable: false,
                                id: 0,
                            },
                            Shortcut {
                                label: "/ search",
                                clickable: false,
                                id: 0,
                            },
                            Shortcut {
                                label: "s sort",
                                clickable: false,
                                id: 0,
                            },
                        ]);
                        let psm_on = crate::appearance::cache::load_psm_resume_index();
                        if external && psm_on {
                            shortcuts.push(Shortcut {
                                label: "→ preview",
                                clickable: false,
                                id: 0,
                            });
                            shortcuts.push(Shortcut {
                                label: "Ctrl+F full-text",
                                clickable: false,
                                id: 0,
                            });
                        }
                        if !chat_mode && !external {
                            shortcuts.push(Shortcut {
                                label: "f filter",
                                clickable: false,
                                id: 0,
                            });
                            shortcuts.push(Shortcut {
                                label: "y copy",
                                clickable: false,
                                id: 0,
                            });
                            shortcuts.push(Shortcut {
                                label: "d delete",
                                clickable: false,
                                id: 0,
                            });
                        }
                        shortcuts.push(Shortcut {
                            label: "Esc close",
                            clickable: false,
                            id: 0,
                        });
                        shortcuts
                    };
                    // Surface `i search` in the footer when vim nav mode is active.
                    if pending_delete.is_none() {
                        mw::push_vim_nav_search_hint(&mut session_shortcuts, state.search_active);
                    }
                    let compact = self.scrollback.appearance().prompt.compact;
                    let external =
                        *source_filter == crate::views::session_picker::SourceFilter::External;
                    let external_tabs = ["Current folder", "All"];
                    let modal_config = ModalWindowConfig {
                        title: "Resume session",
                        tabs: external.then_some(&external_tabs),
                        shortcuts: &session_shortcuts,
                        // Keep the welcome picker’s full catalogue controls, but
                        // leave a modest frame around the in-session surface.
                        sizing: ModalSizing {
                            width_pct: 0.85,
                            max_width: 180,
                            min_width: 48,
                            v_margin: 3,
                            h_pad: 1,
                            v_pad: 0,
                            footer_lines: 2,
                        }
                        .with_compact(compact),
                        fold_info: None,
                    };
                    if let Some(mca) =
                        mw::render_modal_window(buf, area, window, &modal_config, &theme)
                    {
                        let content_area = mca.content;
                        picker::render_picker_search_bar(
                            buf,
                            content_area.x,
                            content_area.y,
                            content_area.width,
                            &theme,
                            state,
                            state.search_active,
                            true,
                            Some(theme.bg_base),
                        );
                        // Render filter indicator on the search bar row (hidden in
                        // chat mode — every row is a conversation).
                        if chat_mode {
                            state.filter_area = None;
                        } else {
                            let filter_rect = picker::render_filter_indicator(
                                buf,
                                content_area.x,
                                content_area.y,
                                content_area.width,
                                &theme,
                                source_filter.label(),
                                "f",
                                source_filter.is_active(),
                                state.filter_hovered,
                            );
                            state.filter_area = Some(filter_rect);
                        }
                        // Divider — spans full inner width.
                        let sep_y = content_area.y + 1;
                        if sep_y < content_area.y + content_area.height {
                            picker::render_divider(
                                buf,
                                mca.inner_x,
                                sep_y,
                                mca.inner_width,
                                &theme,
                                Some(theme.bg_base),
                            );
                        }
                        let entries_start_y = sep_y + 1;
                        let search_bar_rect =
                            Rect::new(content_area.x, content_area.y, content_area.width, 1);

                        // Build session picker entries (shared helper). The same
                        // effective query must drive filtering AND the content
                        // header/rows gates below, or this render disagrees with
                        // the input handler's `build_entry_map` (which receives
                        // the effective query) on row indices.
                        let filter_query = crate::views::session_picker::effective_filter_query(
                            state.query(),
                            entries_query.as_deref(),
                        );
                        let entries_data = entries.as_deref().unwrap_or(&[]);
                        let filtered_indices = filter_session_entries(
                            entries.as_deref(),
                            filter_query,
                            *source_filter,
                        );
                        let built = crate::views::session_picker::build_session_entry_data(
                            entries_data,
                            &filtered_indices,
                            state,
                            content_area.width,
                        );
                        let fields_vecs: Vec<Vec<PickerField>> = built
                            .iter()
                            .map(|b| {
                                b.field_data
                                    .iter()
                                    .map(|(l, v)| PickerField { label: l, value: v })
                                    .collect()
                            })
                            .collect();
                        let current_repo = crate::views::session_picker::repo_name_from_cwd(
                            &self.session.cwd.to_string_lossy(),
                        );
                        let (mut picker_entries, mut non_sel_flags) =
                            crate::views::session_picker::build_grouped_picker_entries(
                                entries_data,
                                &filtered_indices,
                                &built,
                                &fields_vecs,
                                state,
                                Some(current_repo.as_str()),
                            );

                        // Append content search result rows (same pattern as welcome).
                        let content_start = picker_entries.len() + 1;
                        let content_entry_data = if let Some(hits) = content_results.as_deref()
                            && !filter_query.is_empty()
                        {
                            build_content_entry_data(
                                hits,
                                entries_data,
                                &filtered_indices,
                                state,
                                content_start,
                            )
                        } else {
                            Vec::new()
                        };
                        let has_content_rows = !content_entry_data.is_empty();
                        let effective_content_loading = *content_loading;
                        let spinner_label = build_content_header_label(
                            effective_content_loading,
                            has_content_rows,
                            self.scrollback.tick_count(),
                        );
                        let show_content_header = has_content_rows
                            || (effective_content_loading && !filter_query.trim().is_empty());
                        if show_content_header {
                            picker_entries.push(PickerEntry::Header {
                                label: &spinner_label,
                            });
                            non_sel_flags.push(true);
                        }
                        let content_fields: Vec<Vec<PickerField>> = content_entry_data
                            .iter()
                            .map(|b| {
                                b.field_data
                                    .iter()
                                    .map(|(l, v)| PickerField { label: l, value: v })
                                    .collect()
                            })
                            .collect();
                        let content_snippets: Vec<[&str; 1]> = content_entry_data
                            .iter()
                            .map(|b| [b.snippet_preview.as_deref().unwrap_or("")])
                            .collect();

                        for (i, (b, fields)) in content_entry_data
                            .iter()
                            .zip(content_fields.iter())
                            .enumerate()
                        {
                            let has_snippet = b.snippet_preview.is_some();
                            picker_entries.push(PickerEntry::Row(PickerRow {
                                label: &b.summary,
                                right_label: &b.right_text,
                                selected: b.is_selected,
                                expanded: b.is_expanded,
                                fields,
                                description_lines: if has_snippet {
                                    &content_snippets[i]
                                } else {
                                    &[]
                                },
                                summary_lines: &[],
                                dimmed: false,
                                indent: 1,
                                label_color: None,
                                badge: if has_snippet { "match" } else { "" },
                                badge_color: Some(theme.accent_user),
                                collapsible: true,
                                underline_last_desc: false,
                            }));
                            non_sel_flags.push(false);
                        }

                        // Split content into list (top ~75%) + preview (bottom ~25%).
                        let hidden_hint = if chat_mode {
                            None
                        } else {
                            crate::views::session_picker::hidden_external_hint(
                                entries.as_deref(),
                                *source_filter,
                            )
                        };
                        let mut entries_start_y = entries_start_y;
                        let mut total_list_height = content_area
                            .height
                            .saturating_sub(entries_start_y.saturating_sub(content_area.y));
                        if let Some(hint) = hidden_hint.as_deref()
                            && total_list_height > 0
                        {
                            buf.set_stringn(
                                content_area.x + 1,
                                entries_start_y,
                                hint,
                                content_area.width.saturating_sub(1) as usize,
                                ratatui::style::Style::default()
                                    .fg(theme.gray_dim)
                                    .bg(theme.bg_base),
                            );
                            entries_start_y += 1;
                            total_list_height -= 1;
                        }
                        let preview_height = if total_list_height >= 8 {
                            (total_list_height / 4).max(3).min(12)
                        } else {
                            0
                        };
                        let list_height = total_list_height.saturating_sub(preview_height);
                        let entries_area = Rect {
                            x: content_area.x,
                            y: entries_start_y,
                            width: content_area.width,
                            height: list_height,
                        };
                        // Resolve the selected row through the same entry map the
                        // input handler uses, so group headers / content rows map
                        // to the correct backing session entry. Computed before the
                        // mutable render call to avoid a borrow conflict on `state`.
                        let selected_entry_owned = if preview_height >= 3 {
                            let entry_map = crate::views::session_picker::build_entry_map(
                                entries.as_deref(),
                                content_results.as_deref(),
                                filter_query,
                                true,
                                effective_content_loading,
                                *source_filter,
                                Some(current_repo.as_str()),
                            );
                            entry_map
                                .get(state.selected)
                                .and_then(|item| item.as_ref())
                                .and_then(|item| match item {
                                    crate::views::session_picker::PickerItem::Fuzzy {
                                        original_index,
                                    } => entries_data.get(*original_index).cloned(),
                                    crate::views::session_picker::PickerItem::Content {
                                        ..
                                    } => None,
                                })
                        } else {
                            None
                        };
                        let content_hit = picker::render_picker_content_with_scrollbar_x(
                            buf,
                            entries_area,
                            &theme,
                            state,
                            &picker_entries,
                            &non_sel_flags,
                            &[],
                            Some(theme.bg_base),
                            crate::views::session_picker::loading_spinner_active(
                                entries.as_deref(),
                                *source_filter,
                                *loading,
                                lanes,
                            ),
                            self.scrollback.tick_count(),
                            mca.inner_x + mca.inner_width - 1,
                        );
                        // Render bottom preview pane for the selected session.
                        if preview_height >= 3 {
                            let preview_area = Rect {
                                x: content_area.x,
                                y: entries_start_y + list_height,
                                width: content_area.width,
                                height: preview_height,
                            };
                            render_session_preview_pane(
                                buf,
                                preview_area,
                                selected_entry_owned.as_ref(),
                                *preview_scroll,
                                &theme,
                            );
                        }
                        state.hit_areas = Some(picker::PickerHitAreas {
                            close_button: Rect::default(),
                            search_bar: search_bar_rect,
                            item_rects: content_hit.item_rects,
                            entry_indices: content_hit.entry_indices,
                            tab_rects: vec![],
                            filter_rect: None,
                        });
                    }
                } // end else (normal picker mode)
            } else if let modal::ActiveModal::DocPicker {
                entries,
                state,
                window,
                ..
            } = active_modal
            {
                let compact = self.scrollback.appearance().prompt.compact;
                modal::render_doc_picker_overlay(
                    buf, area, window, entries, state, compact, &theme,
                );
            } else if let modal::ActiveModal::DocViewer {
                title,
                content,
                scroll,
                window,
                cached_lines,
                ..
            } = active_modal
            {
                let compact = self.scrollback.appearance().prompt.compact;
                modal::render_doc_viewer_overlay(
                    buf,
                    area,
                    window,
                    title,
                    content,
                    scroll,
                    cached_lines,
                    compact,
                    &theme,
                );
            } else if let modal::ActiveModal::ContextInfo {
                block,
                scroll,
                window,
                cache_metrics,
                view,
                ..
            } = active_modal
            {
                let compact = self.scrollback.appearance().prompt.compact;
                // F2 gate is applied when metrics are attached (open path).
                let cache_enabled = cache_metrics.is_some();
                modal::render_context_info_overlay(
                    buf,
                    area,
                    window,
                    block,
                    scroll,
                    compact,
                    &theme,
                    cache_metrics.as_ref(),
                    *view,
                    cache_enabled,
                );
            } else if let modal::ActiveModal::RememberNoteReview {
                ref raw_content,
                ref enhanced_content,
                showing_enhanced,
                ref mut scroll,
                ref mut window,
                ref mut cached_lines,
                ..
            } = *active_modal
            {
                use crate::views::modal_window::{self as mw, Shortcut};

                let has_enhanced = enhanced_content.is_some();
                let tab_label = if showing_enhanced {
                    "Tab raw"
                } else if has_enhanced {
                    "Tab enhanced"
                } else {
                    "enhancing\u{2026}"
                };

                let shortcuts: Vec<Shortcut> = vec![
                    Shortcut {
                        label: "\u{2191}/\u{2193} scroll",
                        clickable: false,
                        id: 0,
                    },
                    Shortcut {
                        label: "Enter save",
                        clickable: false,
                        id: 0,
                    },
                    Shortcut {
                        label: tab_label,
                        clickable: false,
                        id: 0,
                    },
                    Shortcut {
                        label: "Esc cancel",
                        clickable: false,
                        id: 0,
                    },
                ];

                let compact = self.scrollback.appearance().prompt.compact;
                let modal_config = mw::ModalWindowConfig {
                    title: "Memory Note",
                    tabs: None,
                    shortcuts: &shortcuts,
                    sizing: mw::ModalSizing {
                        width_pct: 0.65,
                        max_width: 100,
                        min_width: 40,
                        v_margin: 4,
                        h_pad: 2,
                        v_pad: 1,
                        footer_lines: 2,
                    }
                    .with_compact(compact),
                    fold_info: None,
                };

                if let Some(mw::ModalContentArea {
                    content: content_area,
                    ..
                }) = mw::render_modal_window(buf, area, window, &modal_config, &theme)
                {
                    let display_content = if showing_enhanced {
                        enhanced_content.as_deref().unwrap_or(raw_content)
                    } else {
                        raw_content
                    };

                    let w = content_area.width;
                    let needs_reparse = cached_lines
                        .as_ref()
                        .is_none_or(|(cached_w, _)| *cached_w != w);
                    if needs_reparse {
                        let mc = crate::scrollback::blocks::markdown_content::MarkdownContent::new(
                            display_content.to_string(),
                        );
                        let output = mc.output(w as usize);
                        let lines: Vec<ratatui::text::Line<'static>> =
                            output.lines.into_iter().map(|b| b.content).collect();
                        *cached_lines = Some((w, lines));
                    }
                    let all_lines = &cached_lines.as_ref().unwrap().1;
                    let max_scroll = all_lines.len().saturating_sub(content_area.height as usize);
                    *scroll = (*scroll as usize).min(max_scroll) as u16;
                    let start = *scroll as usize;
                    let visible: Vec<Line> = all_lines
                        .iter()
                        .skip(start)
                        .take(content_area.height as usize)
                        .cloned()
                        .collect();
                    let para = ratatui::widgets::Paragraph::new(visible)
                        .wrap(ratatui::widgets::Wrap { trim: false });
                    para.render(content_area, buf);
                }
            } else if let modal::ActiveModal::ShortcutsHelp {
                entries,
                state,
                window,
                filter_active,
                collapsed_sections,
                expanded_ids,
                mode,
            } = active_modal
            {
                use crate::views::shortcuts_help;
                // Detail screen reuses the same modal chrome with a different footer (pattern B).
                if mode.is_detail() {
                    shortcuts_help::render_detail(buf, area, window, mode, &theme, compact);
                    return;
                }
                let rows = shortcuts_help::CheatsheetRows::build(
                    entries,
                    state.query(),
                    *filter_active,
                    collapsed_sections,
                );
                let help_refs = rows.help_refs();
                let picker_entries = rows.picker_entries(state, expanded_ids, &help_refs);
                let non_sel: Vec<bool> = vec![false; picker_entries.len()];
                let footer = shortcuts_help::modal_footer(*filter_active);
                let modal_config = mw::ModalWindowConfig {
                    title: "Keyboard Shortcuts",
                    tabs: None,
                    shortcuts: &footer,
                    sizing: shortcuts_help::modal_sizing(compact),
                    fold_info: None,
                };
                if let Some(mca) = mw::render_modal_window(buf, area, window, &modal_config, &theme)
                {
                    let searching = state.search_active || !state.query().is_empty();
                    picker::render_picker_in_modal_inner(
                        buf,
                        mca.content,
                        mca.inner_x,
                        mca.inner_width,
                        &theme,
                        state,
                        &picker_entries,
                        &non_sel,
                        false,
                        searching,
                        !searching,
                    );
                }
            } else if let modal::ActiveModal::MemoryBrowser { state: mem_state } = active_modal {
                crate::views::memory_modal::render_memory_modal(buf, area, mem_state, compact);
            } else if let modal::ActiveModal::PiSettings { state } = active_modal {
                crate::views::pi_settings::render_pi_settings(buf, area, state, compact);
            } else if let modal::ActiveModal::PiConfig { state } = active_modal {
                crate::views::pi_config::render_pi_config_modal(buf, area, state, compact);
            } else if let modal::ActiveModal::PiModels { state } = active_modal {
                state.set_current_model(
                    self.session
                        .models
                        .current_model_id_str()
                        .map(str::to_owned),
                );
                crate::views::pi_models::render_pi_models_modal(buf, area, state, compact);
            } else if let modal::ActiveModal::Settings {
                state: settings_state,
            } = active_modal
            {
                crate::views::settings_modal::render_settings_modal(
                    buf,
                    area,
                    settings_state,
                    compact,
                    None,
                );
            } else if matches!(
                active_modal,
                modal::ActiveModal::ResetSettingsConfirm { .. }
            ) {
                // Render settings modal with reset-confirm overlay.
                let prompt = crate::views::modal::reset_confirm_prompt(active_modal)
                    .unwrap_or_else(|| "Reset setting to default?".to_owned());
                let breadcrumb = crate::views::modal::reset_confirm_breadcrumb(active_modal)
                    .unwrap_or_else(|| "Reset setting".to_owned());
                if let modal::ActiveModal::ResetSettingsConfirm { settings_state, .. } =
                    active_modal
                {
                    let overlay = crate::views::settings_modal::ResetConfirmOverlay {
                        prompt: &prompt,
                        breadcrumb_suffix: &breadcrumb,
                    };
                    crate::views::settings_modal::render_settings_modal(
                        buf,
                        area,
                        settings_state,
                        compact,
                        Some(&overlay),
                    );
                }
            }
        }
    }

    fn handle_session_tree_mouse(
        state: &mut crate::views::session_tree::SessionTreeState,
        mouse: &crossterm::event::MouseEvent,
        skip_summary: bool,
    ) -> InputOutcome {
        use crate::views::session_tree::SessionTreeFocus;
        use crossterm::event::{MouseButton, MouseEventKind};
        use std::time::{Duration, Instant};

        // Simple double-click detection via thread-local last click.
        thread_local! {
            static LAST_CLICK: std::cell::RefCell<Option<(Instant, usize)>> = const { std::cell::RefCell::new(None) };
        }

        match mouse.kind {
            MouseEventKind::ScrollUp => {
                if state.detail_expanded {
                    state.detail_scroll = state.detail_scroll.saturating_sub(1);
                } else {
                    state.move_selection(-1);
                }
                return InputOutcome::Changed;
            }
            MouseEventKind::ScrollDown => {
                if state.detail_expanded {
                    state.detail_scroll = state.detail_scroll.saturating_add(1);
                } else {
                    state.move_selection(1);
                }
                return InputOutcome::Changed;
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(index) = state.hit_test_list_row(mouse.column, mouse.row) {
                    let is_double = LAST_CLICK.with(|cell| {
                        let mut last = cell.borrow_mut();
                        let now = Instant::now();
                        let dbl = matches!(
                            *last,
                            Some((t, i)) if i == index && now.duration_since(t) < Duration::from_millis(400)
                        );
                        *last = Some((now, index));
                        dbl
                    });
                    state.selected = index;
                    state.detail_scroll = 0;
                    state.focus = SessionTreeFocus::List;
                    state.ensure_visible(state.list_viewport.max(1));
                    if is_double {
                        if let Some(entry_id) = state.selected_id() {
                            if Some(entry_id.as_str()) == state.leaf_id.as_deref() {
                                return InputOutcome::Changed;
                            }
                            if skip_summary {
                                return InputOutcome::Action(Action::NavigateSessionTree {
                                    entry_id,
                                    summarize: false,
                                    custom_instructions: None,
                                });
                            }
                            state.begin_summarize_prompt(entry_id);
                            return InputOutcome::Changed;
                        }
                    }
                    return InputOutcome::Changed;
                }
                InputOutcome::Unchanged
            }
            _ => InputOutcome::Unchanged,
        }
    }

    fn handle_session_tree_input(&mut self, ev: &crossterm::event::Event) -> InputOutcome {
        use crate::app::actions::{Action, SessionTreeFilter};
        use crate::views::session_tree::{SessionTreeEsc, SessionTreeFocus};
        use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};

        let Event::Key(key) = ev else {
            return InputOutcome::Unchanged;
        };
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return InputOutcome::Unchanged;
        }

        let Some(ActiveModal::SessionTree { state, .. }) = self.active_modal.as_mut() else {
            return InputOutcome::Unchanged;
        };

        // Skip-summary preference is stored on the tree state (set when the
        // modal opened) so we don't need AppView access here.
        let skip_summary_prompt = state.skip_summary_prompt;

        if matches!(state.focus, SessionTreeFocus::LabelEdit) {
            match key.code {
                KeyCode::Esc => {
                    let _ = state.clear_search_or_cancel_edit();
                    return InputOutcome::Changed;
                }
                KeyCode::Enter => {
                    let entry_id = state.selected_id();
                    let label = {
                        let draft = state.label_draft.trim().to_string();
                        if draft.is_empty() { None } else { Some(draft) }
                    };
                    state.focus = SessionTreeFocus::List;
                    state.label_draft.clear();
                    if let Some(entry_id) = entry_id {
                        return InputOutcome::Action(Action::LabelSessionTreeEntry {
                            entry_id,
                            label,
                        });
                    }
                    return InputOutcome::Changed;
                }
                KeyCode::Backspace => {
                    state.label_draft.pop();
                    return InputOutcome::Changed;
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    state.label_draft.push(c);
                    return InputOutcome::Changed;
                }
                _ => return InputOutcome::Unchanged,
            }
        }

        if matches!(state.focus, SessionTreeFocus::SummarizePrompt) {
            match key.code {
                KeyCode::Esc => {
                    state.cancel_summarize();
                    return InputOutcome::Changed;
                }
                KeyCode::Up => {
                    state.summarize_move(-1);
                    return InputOutcome::Changed;
                }
                KeyCode::Down => {
                    state.summarize_move(1);
                    return InputOutcome::Changed;
                }
                KeyCode::Enter => {
                    use crate::views::session_tree::SummarizeConfirmAction;
                    match state.summarize_confirm() {
                        SummarizeConfirmAction::Navigate {
                            entry_id,
                            summarize,
                            custom_instructions,
                        } => {
                            return InputOutcome::Action(Action::NavigateSessionTree {
                                entry_id,
                                summarize,
                                custom_instructions,
                            });
                        }
                        SummarizeConfirmAction::EnterCustomEditor => {
                            return InputOutcome::Changed;
                        }
                    }
                }
                _ => return InputOutcome::Unchanged,
            }
        }

        if matches!(state.focus, SessionTreeFocus::SummarizeCustom) {
            match key.code {
                KeyCode::Esc => {
                    state.cancel_summarize();
                    return InputOutcome::Changed;
                }
                KeyCode::Enter => {
                    use crate::views::session_tree::SummarizeConfirmAction;
                    if let SummarizeConfirmAction::Navigate {
                        entry_id,
                        summarize,
                        custom_instructions,
                    } = state.summarize_custom_confirm()
                    {
                        return InputOutcome::Action(Action::NavigateSessionTree {
                            entry_id,
                            summarize,
                            custom_instructions,
                        });
                    }
                    return InputOutcome::Changed;
                }
                KeyCode::Backspace => {
                    state.summarize_custom_draft.pop();
                    return InputOutcome::Changed;
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    state.summarize_custom_draft.push(c);
                    return InputOutcome::Changed;
                }
                _ => return InputOutcome::Unchanged,
            }
        }

        if state.detail_expanded {
            match key.code {
                KeyCode::Esc => {
                    state.detail_expanded = false;
                    state.focus = SessionTreeFocus::List;
                    return InputOutcome::Changed;
                }
                KeyCode::Up => {
                    state.detail_scroll = state.detail_scroll.saturating_sub(1);
                    return InputOutcome::Changed;
                }
                KeyCode::Down => {
                    state.detail_scroll = state.detail_scroll.saturating_add(1);
                    return InputOutcome::Changed;
                }
                KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    state.detail_expanded = false;
                    state.focus = SessionTreeFocus::List;
                    return InputOutcome::Changed;
                }
                _ => {}
            }
        }

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);

        match key.code {
            KeyCode::Esc => match state.clear_search_or_cancel_edit() {
                SessionTreeEsc::Close => {
                    self.active_modal = None;
                    InputOutcome::Action(Action::SessionTreeClosed)
                }
                SessionTreeEsc::Consumed => InputOutcome::Changed,
            },
            KeyCode::Up => {
                state.move_selection(-1);
                InputOutcome::Changed
            }
            KeyCode::Down => {
                state.move_selection(1);
                InputOutcome::Changed
            }
            KeyCode::Left if key.modifiers.contains(KeyModifiers::ALT) => {
                let _ = state.fold_or_navigate(crate::views::session_tree::FoldDirection::Up);
                InputOutcome::Changed
            }
            KeyCode::Right if key.modifiers.contains(KeyModifiers::ALT) => {
                let _ = state.fold_or_navigate(crate::views::session_tree::FoldDirection::Down);
                InputOutcome::Changed
            }
            KeyCode::Left | KeyCode::PageUp => {
                state.page(-1, 10);
                InputOutcome::Changed
            }
            KeyCode::Right | KeyCode::PageDown => {
                state.page(1, 10);
                InputOutcome::Changed
            }
            KeyCode::Tab | KeyCode::BackTab => {
                let dir = if matches!(key.code, KeyCode::BackTab) {
                    crate::views::session_tree::FoldDirection::Up
                } else {
                    crate::views::session_tree::FoldDirection::Down
                };
                let _ = state.fold_or_navigate(dir);
                InputOutcome::Changed
            }
            KeyCode::Enter => {
                if let Some(entry_id) = state.selected_id() {
                    if Some(entry_id.as_str()) == state.leaf_id.as_deref() {
                        self.show_toast("Already at this point");
                        InputOutcome::Changed
                    } else if skip_summary_prompt {
                        InputOutcome::Action(Action::NavigateSessionTree {
                            entry_id,
                            summarize: false,
                            custom_instructions: None,
                        })
                    } else {
                        state.begin_summarize_prompt(entry_id);
                        InputOutcome::Changed
                    }
                } else {
                    InputOutcome::Changed
                }
            }
            KeyCode::Char('c') if !ctrl => {
                if let Some(node) = state.selected_node() {
                    let text = if node.detail.is_empty() {
                        node.preview.clone()
                    } else {
                        node.detail.clone()
                    };
                    let _ = crate::clipboard::SystemClipboard::try_set(&text);
                    self.show_toast("Copied tree entry");
                }
                InputOutcome::Changed
            }
            KeyCode::Char('r') if !ctrl && !shift => {
                if let Some(entry_id) = state.selected_id() {
                    InputOutcome::Action(Action::RollbackFilesPreview { entry_id })
                } else {
                    InputOutcome::Changed
                }
            }
            KeyCode::Char('r') if shift && !ctrl => {
                if let Some(entry_id) = state.selected_id() {
                    InputOutcome::Action(Action::RollbackFilesExecute { entry_id })
                } else {
                    InputOutcome::Changed
                }
            }
            KeyCode::Char('l') if !ctrl => {
                state.begin_label_edit();
                InputOutcome::Changed
            }
            KeyCode::Char('t') if shift && !ctrl => {
                state.show_label_timestamps = !state.show_label_timestamps;
                InputOutcome::Changed
            }
            KeyCode::Char('o') if ctrl && shift => {
                state.cycle_filter_backward();
                InputOutcome::Changed
            }
            KeyCode::Char('o') if ctrl => {
                state.cycle_filter_forward();
                InputOutcome::Changed
            }
            KeyCode::Char('d') if ctrl => {
                state.set_filter(SessionTreeFilter::Default);
                InputOutcome::Changed
            }
            KeyCode::Char('t') if ctrl => {
                state.set_filter(SessionTreeFilter::NoTools);
                InputOutcome::Changed
            }
            KeyCode::Char('u') if ctrl => {
                state.set_filter(SessionTreeFilter::UserOnly);
                InputOutcome::Changed
            }
            KeyCode::Char('l') if ctrl => {
                state.set_filter(SessionTreeFilter::LabeledOnly);
                InputOutcome::Changed
            }
            KeyCode::Char('a') if ctrl => {
                state.set_filter(SessionTreeFilter::All);
                InputOutcome::Changed
            }
            KeyCode::Char('r') if ctrl => {
                state.detail_expanded = !state.detail_expanded;
                state.focus = if state.detail_expanded {
                    SessionTreeFocus::DetailExpanded
                } else {
                    SessionTreeFocus::List
                };
                state.detail_scroll = 0;
                InputOutcome::Changed
            }
            KeyCode::Char('/') if !ctrl => {
                state.focus = SessionTreeFocus::Search;
                InputOutcome::Changed
            }
            KeyCode::Backspace => {
                if !state.search_query.is_empty() {
                    state.search_query.pop();
                    state.folded.clear();
                    state.selected = 0;
                    state.scroll = 0;
                    state.clamp_selected();
                    InputOutcome::Changed
                } else {
                    InputOutcome::Unchanged
                }
            }
            KeyCode::Char(c) if !ctrl => {
                state.focus = SessionTreeFocus::Search;
                state.search_query.push(c);
                state.folded.clear();
                state.selected = 0;
                state.scroll = 0;
                state.clamp_selected();
                InputOutcome::Changed
            }
            _ => InputOutcome::Unchanged,
        }
    }

    fn handle_tree_map_input(&mut self, key: &KeyEvent) -> InputOutcome {
        use crossterm::event::{KeyCode, KeyEventKind};

        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return InputOutcome::Unchanged;
        }

        let Some(ActiveModal::TreeMap { state, .. }) = self.active_modal.as_mut() else {
            return InputOutcome::Unchanged;
        };

        match key.code {
            KeyCode::Esc => {
                self.active_modal = None;
                InputOutcome::Action(Action::SessionTreeClosed)
            }
            KeyCode::Up => {
                state.move_selection(-1);
                InputOutcome::Changed
            }
            KeyCode::Down => {
                state.move_selection(1);
                InputOutcome::Changed
            }
            KeyCode::PageUp => {
                state.move_selection(-10);
                InputOutcome::Changed
            }
            KeyCode::PageDown => {
                state.move_selection(10);
                InputOutcome::Changed
            }
            KeyCode::Enter => {
                if let Some(entry_id) = state.selected_entry_id() {
                    InputOutcome::Action(Action::NavigateSessionTree {
                        entry_id,
                        summarize: false,
                        custom_instructions: None,
                    })
                } else {
                    InputOutcome::Changed
                }
            }
            _ => InputOutcome::Unchanged,
        }
    }

    fn handle_tree_map_mouse(&mut self, mouse: &crossterm::event::MouseEvent) -> InputOutcome {
        use crate::views::modal_window as mw;
        use crate::views::modal_window::ModalWindowOutcome;
        use crossterm::event::{MouseButton, MouseEventKind};

        let Some(ActiveModal::TreeMap { state, window }) = self.active_modal.as_mut() else {
            return InputOutcome::Unchanged;
        };

        // Modal chrome (close button, etc.)
        let outcome = mw::handle_modal_mouse(window, mouse.kind, mouse.column, mouse.row);
        match outcome {
            ModalWindowOutcome::CloseRequested => {
                self.active_modal = None;
                return InputOutcome::Action(Action::SessionTreeClosed);
            }
            ModalWindowOutcome::Handled => return InputOutcome::Changed,
            ModalWindowOutcome::Unhandled => {}
            _ => return InputOutcome::Changed,
        }

        // Content area: row selection
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(index) = state.hit_test_row(mouse.column, mouse.row) {
                    state.selected = index;
                    if let Some(entry_id) = state.selected_entry_id() {
                        return InputOutcome::Action(Action::NavigateSessionTree {
                            entry_id,
                            summarize: false,
                            custom_instructions: None,
                        });
                    }
                }
                InputOutcome::Changed
            }
            MouseEventKind::ScrollUp => {
                state.move_selection(-1);
                InputOutcome::Changed
            }
            MouseEventKind::ScrollDown => {
                state.move_selection(1);
                InputOutcome::Changed
            }
            _ => InputOutcome::Unchanged,
        }
    }
}

/// Split the model-list content area into list + bottom detail pane.
fn split_model_picker_content(content: Rect, detail_line_count: usize) -> (Rect, Option<Rect>) {
    if detail_line_count == 0 || content.height < 6 {
        return (content, None);
    }
    // Separator row + detail lines (capped so the list keeps breathing room).
    let detail_h = (detail_line_count as u16).saturating_add(1).min(5);
    let max_detail = content.height.saturating_sub(4);
    let detail_h = detail_h.min(max_detail);
    if detail_h < 2 {
        return (content, None);
    }
    let list = Rect {
        height: content.height.saturating_sub(detail_h),
        ..content
    };
    let detail = Rect {
        y: content.y + list.height,
        height: detail_h,
        ..content
    };
    (list, Some(detail))
}

fn selected_model_detail_lines(
    items: &[crate::slash::command::ArgItem],
    state: &crate::views::picker::PickerState,
    models: &crate::acp::model_state::ModelState,
) -> Vec<String> {
    use crate::slash::commands::model::{model_picker_detail_lines, resolve_model_for_arg_item};

    let idx = state.hovered.unwrap_or(state.selected);
    let Some(item) = items.get(idx) else {
        return Vec::new();
    };
    let Some(id) = resolve_model_for_arg_item(models, item) else {
        return Vec::new();
    };
    let Some(info) = models.available.get(&id) else {
        return Vec::new();
    };
    model_picker_detail_lines(info)
}

/// Render the bottom preview pane for the selected session in the resume picker.
/// Shows session metadata and first message as a quick glance (like the model
/// picker's detail pane but richer).

/// Render PSM full-text hits as multi-line cards:
///   title (left) + badge (right)
///   wrapped snippet (2–3 lines)
///   short repo · relative meta (no full path waste)
///
/// Returns (item_rects, entry_indices) for click hit-testing.
fn render_search_result_hits(
    buf: &mut Buffer,
    area: Rect,
    hits: &[xai_grok_shell::extensions::session_search::SearchSessionHit],
    selected: usize,
    scroll_offset: &mut Option<usize>,
    content_loading: bool,
    theme: &Theme,
) -> (Vec<Rect>, Vec<usize>) {
    use crate::render::SafeBuf;
    use crate::render::line_utils::truncate_str;
    use crate::render::scrollbar::render_scrollbar;
    use crate::views::session_picker::repo_name_from_cwd;
    use ratatui::style::{Modifier, Style};
    use ratatui::text::{Line as TuiLine, Span};
    use unicode_width::UnicodeWidthStr;

    const SNIP_LINES: usize = 2;
    const CARD_GAP: usize = 1; // blank line between cards
    // header + snip + meta + gap
    let card_rows = 1 + SNIP_LINES + 1 + CARD_GAP;

    if area.width == 0 || area.height == 0 {
        return (Vec::new(), Vec::new());
    }

    let need_sb = hits.len() * card_rows > area.height as usize && area.width > 4;
    let paint_w = if need_sb {
        area.width.saturating_sub(1)
    } else {
        area.width
    };
    let inner_w = paint_w.saturating_sub(2) as usize; // left pad

    // Keep selected card visible.
    let mut offset = scroll_offset.unwrap_or(0);
    let sel_top = selected.saturating_mul(card_rows);
    let sel_bot = sel_top + card_rows.saturating_sub(1);
    let view_h = area.height as usize;
    if sel_top < offset {
        offset = sel_top;
    } else if sel_bot >= offset + view_h {
        offset = sel_bot.saturating_sub(view_h.saturating_sub(1));
    }
    *scroll_offset = Some(offset);

    let mut item_rects = Vec::new();
    let mut entry_indices = Vec::new();
    for (i, hit) in hits.iter().enumerate() {
        let card_top = i * card_rows;
        let card_bottom = card_top + card_rows - CARD_GAP; // exclusive of gap
        // Skip cards fully above viewport
        if card_bottom <= offset {
            continue;
        }
        if card_top >= offset + view_h {
            break;
        }

        let selected_row = i == selected;
        let bg = if selected_row {
            theme.bg_light
        } else {
            theme.bg_base
        };
        let title_fg = if selected_row {
            theme.accent_user
        } else {
            theme.text_primary
        };
        let meta_fg = theme.gray_dim;
        let snip_fg = theme.text_secondary;

        // Visible start row for this card within area
        let vis_start = card_top.saturating_sub(offset);
        let card_y0 = area.y + vis_start as u16;

        // Track hit rect for click (visible portion)
        let visible_card_h = card_rows
            .saturating_sub(CARD_GAP)
            .min(view_h.saturating_sub(vis_start)) as u16;
        if visible_card_h > 0 {
            item_rects.push(Rect {
                x: area.x,
                y: card_y0,
                width: paint_w,
                height: visible_card_h,
            });
            entry_indices.push(i);
        }

        // ── Line 0: title + badge ──
        if card_top >= offset && card_top < offset + view_h {
            let y = area.y + (card_top - offset) as u16;
            let badge = if hit.matched_fields.iter().any(|f| f == "content") {
                "match"
            } else if hit
                .matched_fields
                .iter()
                .any(|f| f == "name" || f == "title")
            {
                "title"
            } else {
                "hit"
            };
            let badge_w = badge.width() + 2; // [badge]
            let title_budget = inner_w.saturating_sub(badge_w + 1);
            let title = truncate_str(
                if hit.summary.trim().is_empty() {
                    "(no title)"
                } else {
                    hit.summary.trim()
                },
                title_budget,
            );
            let title_w = title.width();
            let mut spans = vec![
                Span::styled(" ", Style::default().bg(bg)),
                Span::styled(
                    title,
                    Style::default()
                        .fg(title_fg)
                        .bg(bg)
                        .add_modifier(if selected_row {
                            Modifier::BOLD
                        } else {
                            Modifier::empty()
                        }),
                ),
            ];
            // pad then badge right-aligned
            let used = 1 + title_w;
            let pad = inner_w.saturating_sub(used + badge_w);
            if pad > 0 {
                spans.push(Span::styled(" ".repeat(pad), Style::default().bg(bg)));
            }
            spans.push(Span::styled(
                format!("[{badge}]"),
                Style::default().fg(theme.accent_user).bg(bg),
            ));
            // fill rest of row with bg
            let line = TuiLine::from(spans);
            buf.set_line_safe(area.x, y, &line, paint_w);
            // paint bg for full width
            if let Some(cell) = buf.cell_mut((area.x, y)) {
                let _ = cell;
            }
            for x in area.x..area.x + paint_w {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    if cell.bg == ratatui::style::Color::Reset {
                        cell.set_bg(bg);
                    }
                }
            }
        }

        // ── Snippet lines ──
        let snip_raw = hit.snippet.as_deref().unwrap_or("").replace('\n', " ");
        let snip_lines = wrap_search_snippet(&snip_raw, inner_w.saturating_sub(2), SNIP_LINES);
        for (si, snip_line) in snip_lines.iter().enumerate() {
            let abs = card_top + 1 + si;
            if abs < offset || abs >= offset + view_h {
                continue;
            }
            let y = area.y + (abs - offset) as u16;
            let styled = style_snippet_with_markers(snip_line, snip_fg, theme.accent_user, bg);
            let mut spans = vec![Span::styled("  ", Style::default().bg(bg))];
            spans.extend(styled);
            buf.set_line_safe(area.x, y, &TuiLine::from(spans), paint_w);
            for x in area.x..area.x + paint_w {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    if cell.bg == ratatui::style::Color::Reset {
                        cell.set_bg(bg);
                    }
                }
            }
        }
        // pad empty snip lines with bg if short
        for si in snip_lines.len()..SNIP_LINES {
            let abs = card_top + 1 + si;
            if abs < offset || abs >= offset + view_h {
                continue;
            }
            let y = area.y + (abs - offset) as u16;
            for x in area.x..area.x + paint_w {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_bg(bg);
                    cell.set_symbol(" ");
                }
            }
        }

        // ── Meta: repo · short id ──
        let meta_abs = card_top + 1 + SNIP_LINES;
        if meta_abs >= offset && meta_abs < offset + view_h {
            let y = area.y + (meta_abs - offset) as u16;
            let repo = repo_name_from_cwd(&hit.cwd);
            let short_id = if hit.session_id.len() > 8 {
                format!("{}…", &hit.session_id[..8])
            } else {
                hit.session_id.clone()
            };
            let meta = if hit.updated_at.is_empty() {
                format!("{repo}  ·  {short_id}")
            } else {
                // Keep ISO date prefix if present (YYYY-MM-DD…)
                let when = hit.updated_at.get(..10).unwrap_or(hit.updated_at.as_str());
                format!("{repo}  ·  {when}  ·  {short_id}")
            };
            let meta = truncate_str(&meta, inner_w.saturating_sub(2));
            let line = TuiLine::from(vec![
                Span::styled("  ", Style::default().bg(bg)),
                Span::styled(meta, Style::default().fg(meta_fg).bg(bg)),
            ]);
            buf.set_line_safe(area.x, y, &line, paint_w);
            for x in area.x..area.x + paint_w {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    if cell.bg == ratatui::style::Color::Reset {
                        cell.set_bg(bg);
                    }
                }
            }
        }
    }

    if need_sb {
        let total_rows = hits.len() * card_rows;
        let sb = Rect {
            x: area.x + area.width - 1,
            y: area.y,
            width: 1,
            height: area.height,
        };
        render_scrollbar(
            buf,
            Some(sb),
            total_rows.min(u16::MAX as usize) as u16,
            view_h.min(u16::MAX as usize) as u16,
            offset.min(u16::MAX as usize) as u16,
            false,
        );
    }

    if content_loading && hits.is_empty() {
        let line = TuiLine::from(Span::styled(
            "  Searching…",
            Style::default().fg(theme.gray_dim),
        ));
        buf.set_line_safe(area.x, area.y, &line, paint_w);
    }

    (item_rects, entry_indices)
}

/// Wrap snippet into up to `max_lines` display lines (char-safe).
fn wrap_search_snippet(text: &str, width: usize, max_lines: usize) -> Vec<String> {
    if width == 0 || max_lines == 0 || text.is_empty() {
        return Vec::new();
    }
    use unicode_width::UnicodeWidthStr;
    let mut lines = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0usize;
    for ch in text.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if cur_w + cw > width && !cur.is_empty() {
            lines.push(std::mem::take(&mut cur));
            cur_w = 0;
            if lines.len() >= max_lines {
                break;
            }
        }
        if lines.len() >= max_lines {
            break;
        }
        cur.push(ch);
        cur_w += cw;
    }
    if !cur.is_empty() && lines.len() < max_lines {
        lines.push(cur);
    }
    // Ellipsis on last if truncated
    if text.width() > width * max_lines
        && let Some(last) = lines.last_mut()
    {
        if last.width() + 1 > width {
            // trim a char
            while last.width() + 1 > width && last.pop().is_some() {}
        }
        last.push('…');
    }
    lines
}

/// Highlight FTS markers `[term]` with accent color.
fn style_snippet_with_markers(
    text: &str,
    normal_fg: ratatui::style::Color,
    mark_fg: ratatui::style::Color,
    bg: ratatui::style::Color,
) -> Vec<ratatui::text::Span<'static>> {
    use ratatui::style::{Modifier, Style};
    use ratatui::text::Span;
    let mut spans = Vec::new();
    let mut rest = text;
    loop {
        if let Some(start) = rest.find('[') {
            if let Some(end_rel) = rest[start + 1..].find(']') {
                let end = start + 1 + end_rel;
                if start > 0 {
                    spans.push(Span::styled(
                        rest[..start].to_string(),
                        Style::default().fg(normal_fg).bg(bg),
                    ));
                }
                // highlight without brackets
                let inner = &rest[start + 1..end];
                spans.push(Span::styled(
                    inner.to_string(),
                    Style::default()
                        .fg(mark_fg)
                        .bg(bg)
                        .add_modifier(Modifier::BOLD),
                ));
                rest = &rest[end + 1..];
                continue;
            }
        }
        if !rest.is_empty() {
            spans.push(Span::styled(
                rest.to_string(),
                Style::default().fg(normal_fg).bg(bg),
            ));
        }
        break;
    }
    spans
}

/// Build a plain-text transcript from structured preview messages.
fn format_preview_transcript(msgs: &[crate::views::modal::SessionPreviewMessage]) -> String {
    let mut out = String::new();
    for (i, m) in msgs.iter().enumerate() {
        if i > 0 {
            out.push_str("\n\n");
        }
        let label = match m.role.as_str() {
            "user" => "You",
            "assistant" => "Assistant",
            other => other,
        };
        out.push_str(label);
        out.push_str(":\n");
        out.push_str(m.content.trim());
    }
    out
}

/// Full-screen session message preview: role headers + MarkdownContent body.
/// Layout mirrors scrollback: user left gutter, assistant markdown, spacing.
fn render_session_message_preview(
    buf: &mut Buffer,
    area: Rect,
    messages: Option<&[crate::views::modal::SessionPreviewMessage]>,
    scroll: &mut u16,
    theme: &Theme,
) {
    use crate::render::SafeBuf;
    use crate::render::scrollbar::render_scrollbar;
    use crate::scrollback::blocks::markdown_content::MarkdownContent;
    use ratatui::style::{Modifier, Style};
    use ratatui::text::{Line as TuiLine, Span};
    use ratatui::widgets::Widget;

    if area.width == 0 || area.height == 0 {
        return;
    }

    match messages {
        None => {
            let line = TuiLine::from(Span::styled(
                "  Loading messages…",
                Style::default().fg(theme.gray_dim),
            ));
            ratatui::widgets::Paragraph::new(vec![line]).render(area, buf);
            return;
        }
        Some([]) => {
            let line = TuiLine::from(Span::styled(
                "  No messages to preview",
                Style::default().fg(theme.gray_dim),
            ));
            ratatui::widgets::Paragraph::new(vec![line]).render(area, buf);
            return;
        }
        Some(msgs) => {
            let full_w = area.width as usize;
            let content_w = full_w.saturating_sub(2).max(8);

            // Flatten to painted lines: header + markdown body + blank separator.
            let mut painted: Vec<TuiLine> = Vec::new();
            for (i, msg) in msgs.iter().enumerate() {
                let is_user = msg.role == "user";
                let (label, label_fg) = if is_user {
                    ("You", theme.accent_user)
                } else {
                    ("Assistant", theme.accent_assistant)
                };

                painted.push(TuiLine::from(vec![
                    Span::styled(
                        if is_user { "┃ " } else { "  " },
                        Style::default().fg(theme.accent_user),
                    ),
                    Span::styled(
                        label,
                        Style::default().fg(label_fg).add_modifier(Modifier::BOLD),
                    ),
                ]));

                let body_w = content_w.saturating_sub(2).max(1);
                let md = MarkdownContent::new_with_table_width(msg.content.as_str(), Some(body_w));
                md.with_wrapped_lines(body_w, |wrapped| {
                    for line in wrapped.lines.iter() {
                        let mut spans = vec![Span::styled(
                            if is_user { "┃ " } else { "  " },
                            Style::default().fg(if is_user {
                                theme.accent_user
                            } else {
                                theme.gray_dim
                            }),
                        )];
                        for span in line.spans.iter() {
                            spans.push(span.clone());
                        }
                        painted.push(TuiLine::from(spans));
                    }
                });

                if i + 1 < msgs.len() {
                    painted.push(TuiLine::from(Span::raw("")));
                }
            }

            let total = painted.len();
            let visible_h = area.height as usize;
            let max_scroll = total.saturating_sub(visible_h);
            *scroll = (*scroll as usize).min(max_scroll) as u16;
            let scroll = *scroll as usize;
            let need_sb = total > visible_h && area.width > 4;
            let paint_w = if need_sb {
                area.width.saturating_sub(1)
            } else {
                area.width
            };

            for (row, line) in painted.into_iter().skip(scroll).take(visible_h).enumerate() {
                let y = area.y + row as u16;
                buf.set_line_safe(area.x, y, &line, paint_w);
            }

            if need_sb {
                let sb = Rect {
                    x: area.x + area.width - 1,
                    y: area.y,
                    width: 1,
                    height: area.height,
                };
                render_scrollbar(
                    buf,
                    Some(sb),
                    total.min(u16::MAX as usize) as u16,
                    visible_h.min(u16::MAX as usize) as u16,
                    scroll.min(u16::MAX as usize) as u16,
                    false,
                );
            }
        }
    }
}

fn render_session_preview_pane(
    buf: &mut Buffer,
    area: Rect,
    entry: Option<&crate::app::app_view::SessionPickerEntry>,
    scroll: u16,
    theme: &Theme,
) {
    use ratatui::style::{Modifier, Style};
    use ratatui::text::{Line as TuiLine, Span};
    use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

    if area.width == 0 || area.height == 0 {
        return;
    }
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(theme.selection_border));
    let inner = block.inner(area);
    block.render(area, buf);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let Some(entry) = entry else {
        let placeholder = TuiLine::from(Span::styled(
            "  No session selected",
            Style::default().fg(theme.gray_dim),
        ));
        Paragraph::new(vec![placeholder]).render(inner, buf);
        return;
    };

    let mut tui_lines: Vec<TuiLine> = Vec::new();

    // Title line: session name or summary
    let title = entry
        .name
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(&entry.summary);
    tui_lines.push(TuiLine::from(Span::styled(
        format!("  {title}"),
        Style::default()
            .fg(theme.accent_user)
            .add_modifier(Modifier::BOLD),
    )));

    // Metadata line: model · messages · tokens
    let mut meta_parts: Vec<String> = Vec::new();
    if let Some(model) = &entry.model_id {
        meta_parts.push(model.clone());
    }
    if entry.num_messages > 0 {
        meta_parts.push(format!("{} msgs", entry.num_messages));
    }
    if let Some(tokens) = entry.total_tokens {
        if tokens > 0 {
            meta_parts.push(format!("{} tok", tokens));
        }
    }
    if let Some(cost) = entry.total_cost {
        if cost > 0.0 {
            meta_parts.push(format!("${cost:.2}"));
        }
    }
    if !meta_parts.is_empty() {
        tui_lines.push(TuiLine::from(Span::styled(
            format!("  {}", meta_parts.join(" · ")),
            Style::default().fg(theme.text_secondary),
        )));
    }

    // Time line: created · updated
    let created_str = entry.created_at.format("%Y-%m-%d %H:%M").to_string();
    let updated_str = entry
        .last_active_at
        .unwrap_or(entry.updated_at)
        .format("%Y-%m-%d %H:%M")
        .to_string();
    tui_lines.push(TuiLine::from(vec![
        Span::styled("  created ", Style::default().fg(theme.gray_dim)),
        Span::styled(&created_str, Style::default().fg(theme.text_secondary)),
        Span::styled("  ·  updated ", Style::default().fg(theme.gray_dim)),
        Span::styled(&updated_str, Style::default().fg(theme.text_secondary)),
    ]));

    // CWD + branch line
    if !entry.cwd.is_empty() {
        let mut cwd_line = vec![
            Span::styled("  ", Style::default()),
            Span::styled(&entry.cwd, Style::default().fg(theme.gray_dim)),
        ];
        if let Some(branch) = &entry.branch {
            cwd_line.push(Span::styled(
                format!("  ({branch})"),
                Style::default().fg(theme.gray_dim),
            ));
        }
        tui_lines.push(TuiLine::from(cwd_line));
    }

    // First message preview
    if let Some(first_msg) = &entry.first_message {
        let msg = first_msg.trim();
        if !msg.is_empty() {
            tui_lines.push(TuiLine::from(Span::raw("")));
            // Truncate to fit remaining lines (char-safe for CJK/emoji)
            let max_chars = (inner.width as usize).saturating_sub(2) * 2;
            let truncated: String = if msg.chars().count() > max_chars {
                msg.chars().take(max_chars).collect::<String>() + "…"
            } else {
                msg.to_string()
            };
            tui_lines.push(TuiLine::from(Span::styled(
                format!("  {truncated}"),
                Style::default().fg(theme.text_primary),
            )));
        }
    }

    // Apply scroll offset
    let visible: Vec<TuiLine> = tui_lines
        .into_iter()
        .skip(scroll as usize)
        .take(inner.height as usize)
        .collect();

    Paragraph::new(visible)
        .wrap(Wrap { trim: false })
        .render(inner, buf);
}

fn render_model_picker_detail(buf: &mut Buffer, area: Rect, lines: &[String], theme: &Theme) {
    use ratatui::style::{Modifier, Style};
    use ratatui::text::{Line as TuiLine, Span};
    use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

    if area.width == 0 || area.height == 0 || lines.is_empty() {
        return;
    }
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(theme.selection_border));
    let inner = block.inner(area);
    block.render(area, buf);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let mut tui_lines = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let style = if i == 0 {
            Style::default()
                .fg(theme.accent_user)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text_secondary)
        };
        tui_lines.push(TuiLine::from(Span::styled(format!("  {line}"), style)));
    }
    Paragraph::new(tui_lines)
        .wrap(Wrap { trim: false })
        .render(inner, buf);
}

#[cfg(test)]
mod session_picker_delete_tests {
    use crate::app::actions::Action;
    use crate::app::agent_view::AgentView;
    use crate::app::agent_view::test_fixtures::make_agent;
    use crate::app::app_view::{InputOutcome, SessionPickerEntry};
    use crate::views::modal::ActiveModal;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};

    fn entry(id: &str) -> SessionPickerEntry {
        SessionPickerEntry {
            id: id.into(),
            summary: id.into(),
            name: None,
            first_message: None,
            session_path: None,
            total_tokens: None,
            total_cost: None,
            updated_at: chrono::Utc::now(),
            created_at: chrono::Utc::now(),
            cwd: "/repo".into(),
            hostname: None,
            source: "local".into(),
            model_id: None,
            num_messages: 0,
            last_active_at: None,
            branch: None,
            repo_name: "repo".into(),
            worktree_label: None,
            parent_session_path: None,
            last_turn_summary: None,
            card_detail: None,
        }
    }

    fn open_picker(agent: &mut AgentView, entries: Vec<SessionPickerEntry>) {
        agent.active_modal = Some(ActiveModal::SessionPicker {
            state: crate::views::picker::PickerState::default(),
            entries: Some(entries),
            loading: false,
            lanes: Default::default(),
            previous_palette: None,
            window: crate::views::modal_window::ModalWindowState::new(),
            content_results: None,
            content_loading: false,
            deep_search_seq: 0,
            entries_query: None,
            source_filter: crate::views::session_picker::SourceFilter::default(),
            pending_delete: None,
            preview_scroll: 0,
            search_mode: false,
            preview_mode: false,
            preview_messages: None,
        });
    }

    fn key(c: char) -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
    }

    fn pending(agent: &AgentView) -> Option<String> {
        match agent.active_modal.as_ref() {
            Some(ActiveModal::SessionPicker { pending_delete, .. }) => {
                pending_delete.as_ref().map(|pd| pd.session_id.clone())
            }
            _ => None,
        }
    }

    #[test]
    fn session_preview_render_clamps_persistent_scroll_to_bottom() {
        let area = ratatui::layout::Rect::new(0, 0, 24, 4);
        let mut buf = ratatui::buffer::Buffer::empty(area);
        let messages = [crate::views::modal::SessionPreviewMessage {
            role: "user".into(),
            content: (0..20)
                .map(|line| format!("line {line}"))
                .collect::<Vec<_>>()
                .join("\n"),
        }];
        let theme = crate::theme::Theme::default();
        let mut scroll = u16::MAX;

        super::render_session_message_preview(&mut buf, area, Some(&messages), &mut scroll, &theme);

        assert!(
            scroll < u16::MAX,
            "render must persist the real bottom offset"
        );
        let bottom = scroll;
        assert!(bottom >= 3, "fixture must produce scrollable content");

        crate::views::modal::apply_doc_mouse_scroll(MouseEventKind::ScrollUp, &mut scroll);
        super::render_session_message_preview(&mut buf, area, Some(&messages), &mut scroll, &theme);

        assert_eq!(
            scroll,
            bottom - 3,
            "one upward wheel event must leave bottom"
        );
    }

    #[test]
    fn escape_returns_external_preview_to_session_list() {
        crate::appearance::cache::set_psm_resume_index(true);
        let mut agent = make_agent();
        let mut session = entry("pi-session");
        session.source = "pi".into();
        open_picker(&mut agent, vec![session]);
        if let Some(ActiveModal::SessionPicker {
            source_filter,
            preview_mode,
            preview_messages,
            preview_scroll,
            ..
        }) = agent.active_modal.as_mut()
        {
            *source_filter = crate::views::session_picker::SourceFilter::External;
            *preview_mode = true;
            *preview_messages = Some(vec![crate::views::modal::SessionPreviewMessage {
                role: "user".into(),
                content: "preview".into(),
            }]);
            *preview_scroll = 2;
        }

        let outcome = agent.handle_modal_key(&KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(outcome, InputOutcome::Changed));
        let Some(ActiveModal::SessionPicker {
            preview_mode,
            preview_messages,
            preview_scroll,
            ..
        }) = agent.active_modal.as_ref()
        else {
            panic!("Esc must return to the session list, not close the modal");
        };
        assert!(!preview_mode);
        assert!(preview_messages.is_none());
        assert_eq!(*preview_scroll, 0);
        crate::appearance::cache::set_psm_resume_index(false);
    }

    #[test]
    fn d_arms_then_y_confirms_delete() {
        let mut agent = make_agent();
        open_picker(&mut agent, vec![entry("s0"), entry("s1")]);

        // `d` arms the confirmation on the first selectable row (s0).
        let out = agent.handle_palette_or_arg_input(&key('d'));
        assert!(matches!(out, InputOutcome::Changed));
        assert_eq!(pending(&agent).as_deref(), Some("s0"));

        // `y` confirms — fires DeleteSession for the armed session.
        let out = agent.handle_palette_or_arg_input(&key('y'));
        assert!(
            matches!(
                out,
                InputOutcome::Action(Action::DeleteSession {
                    ref source,
                    ref session_id,
                    ref cwd,
                }) if source == "local" && session_id == "s0" && cwd == "/repo"
            ),
            "y must confirm deletion of the armed session"
        );
        assert!(pending(&agent).is_none(), "pending cleared after confirm");
    }

    #[test]
    fn d_arms_then_n_cancels() {
        let mut agent = make_agent();
        open_picker(&mut agent, vec![entry("s0")]);

        agent.handle_palette_or_arg_input(&key('d'));
        assert_eq!(pending(&agent).as_deref(), Some("s0"));

        let out = agent.handle_palette_or_arg_input(&key('n'));
        assert!(matches!(out, InputOutcome::Changed));
        assert!(pending(&agent).is_none(), "n cancels the confirmation");
    }

    #[test]
    fn other_key_cancels_pending_delete() {
        let mut agent = make_agent();
        open_picker(&mut agent, vec![entry("s0"), entry("s1")]);

        agent.handle_palette_or_arg_input(&key('d'));
        assert_eq!(pending(&agent).as_deref(), Some("s0"));

        // A navigation key cancels the armed confirmation.
        agent.handle_palette_or_arg_input(&key('j'));
        assert!(
            pending(&agent).is_none(),
            "any non-y/d key cancels the pending delete"
        );
    }

    #[test]
    fn mouse_move_keeps_pending_delete() {
        let mut agent = make_agent();
        open_picker(&mut agent, vec![entry("s0"), entry("s1")]);
        agent.handle_palette_or_arg_input(&key('d'));
        agent.handle_palette_or_arg_input(&Event::Mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        }));
        assert_eq!(pending(&agent).as_deref(), Some("s0"));
    }

    #[test]
    fn y_without_armed_confirmation_does_not_delete() {
        let mut agent = make_agent();
        open_picker(&mut agent, vec![entry("s0")]);

        // No `d` first — `y` is the copy hotkey, never a delete.
        let out = agent.handle_palette_or_arg_input(&key('y'));
        assert!(
            !matches!(out, InputOutcome::Action(Action::DeleteSession { .. })),
            "y alone must not delete"
        );
        assert!(pending(&agent).is_none());
    }

    /// Plain close (Esc) must surface `SessionPickerClosed` so the dispatch
    /// layer can invalidate an in-flight list/search fetch — its landing
    /// surface (the modal) is gone.
    #[test]
    fn esc_close_emits_session_picker_closed_action() {
        let mut agent = make_agent();
        open_picker(&mut agent, vec![entry("s0")]);
        let esc = Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        let out = agent.handle_palette_or_arg_input(&esc);
        assert!(
            matches!(out, InputOutcome::Action(Action::SessionPickerClosed)),
            "close must emit the fetch-invalidation action, got {out:?}"
        );
        assert!(agent.active_modal.is_none(), "modal cleared on close");
    }

    /// Chat-mode picker is conversations-only: `d` (local delete) must not
    /// arm a confirmation and `f` must not cycle the hidden source filter.
    #[test]
    fn chat_mode_disables_delete_and_filter_keys() {
        let mut agent = make_agent();
        agent.app_chat_mode = true;
        open_picker(&mut agent, vec![entry("c0"), entry("c1")]);

        agent.handle_palette_or_arg_input(&key('d'));
        assert!(
            pending(&agent).is_none(),
            "d must not arm delete under chat mode"
        );

        agent.handle_palette_or_arg_input(&key('f'));
        let filter = match agent.active_modal.as_ref() {
            Some(ActiveModal::SessionPicker { source_filter, .. }) => *source_filter,
            _ => panic!("expected open session picker"),
        };
        assert_eq!(
            filter,
            crate::views::session_picker::SourceFilter::Grok,
            "f must not cycle the hidden source filter under chat mode"
        );
    }

    #[test]
    fn ctrl_w_resumes_session_while_search_is_focused() {
        let mut agent = make_agent();
        open_picker(&mut agent, vec![entry("s0")]);
        if let Some(ActiveModal::SessionPicker { state, .. }) = agent.active_modal.as_mut() {
            state.selected = 1;
            state.search_active = true;
            state.set_query("s");
        }

        let worktree = Event::Key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL));
        let outcome = agent.handle_palette_or_arg_input(&worktree);
        assert!(matches!(
            outcome,
            InputOutcome::Action(Action::PickSessionInWorktree(0))
        ));
    }

    #[test]
    fn foreign_row_refuses_delete_detail_and_worktree_actions() {
        let mut agent = make_agent();
        let mut foreign = entry("codex-session");
        foreign.source = "codex".into();
        open_picker(&mut agent, vec![foreign]);
        // Pin All: the refusals only fire when the foreign row is focusable.
        if let Some(ActiveModal::SessionPicker { source_filter, .. }) = agent.active_modal.as_mut()
        {
            *source_filter = crate::views::session_picker::SourceFilter::All;
        }

        let delete = agent.handle_palette_or_arg_input(&key('d'));
        assert!(matches!(delete, InputOutcome::Changed));
        assert!(pending(&agent).is_none(), "foreign delete must not arm");

        let expand = agent.handle_palette_or_arg_input(&key('e'));
        assert!(
            !matches!(
                expand,
                InputOutcome::Action(Action::ExpandSessionCard { .. })
            ),
            "foreign rows have no transcript detail"
        );

        let worktree = Event::Key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL));
        let outcome = agent.handle_palette_or_arg_input(&worktree);
        assert!(
            !matches!(
                outcome,
                InputOutcome::Action(Action::PickSessionInWorktree(_))
            ),
            "foreign rows cannot be resumed in worktrees"
        );
        assert!(
            agent.active_modal.is_some(),
            "refused actions keep picker open"
        );
    }

    /// A server search matches conversation *content* too: a hit whose title
    /// doesn't fuzzy-match the query must stay pickable in the modal
    /// (`effective_filter_query` skips the local re-filter).
    #[test]
    fn server_search_hit_with_unrelated_title_is_pickable() {
        let mut agent = make_agent();
        agent.app_chat_mode = true;
        let mut e = entry("conv-content-1");
        e.summary = "Quarterly roadmap notes".into(); // no "hit" in the title
        e.source = "conversation".into();
        open_picker(&mut agent, vec![e]);
        if let Some(ActiveModal::SessionPicker {
            state,
            entries_query,
            content_loading,
            ..
        }) = agent.active_modal.as_mut()
        {
            state.set_query("hit");
            *entries_query = Some("hit".into());
            // A re-search of the stamped query may be in flight: with the
            // effective query empty, the input map appends NO "Searching…"
            // header (same gate the renders use), so indices don't shift.
            *content_loading = true;
            // Grouped map: [repo header, row] — the row sits at index 1.
            state.selected = 1;
        }
        let out = agent.handle_palette_or_arg_input(&key_code(KeyCode::Enter));
        assert!(
            matches!(out, InputOutcome::Action(Action::PickSession(0))),
            "content-only search hit must be pickable, got {out:?}"
        );
    }

    /// Canary: entries WITHOUT a matching fetch-query stamp keep the local
    /// fuzzy filter — an unrelated title stays hidden from Enter.
    #[test]
    fn unstamped_entries_keep_local_fuzzy_filter() {
        let mut agent = make_agent();
        agent.app_chat_mode = true;
        let mut e = entry("conv-content-1");
        e.summary = "Quarterly roadmap notes".into();
        e.source = "conversation".into();
        open_picker(&mut agent, vec![e]);
        if let Some(ActiveModal::SessionPicker { state, .. }) = agent.active_modal.as_mut() {
            state.set_query("hit");
            state.selected = 1;
        }
        let out = agent.handle_palette_or_arg_input(&key_code(KeyCode::Enter));
        assert!(
            !matches!(out, InputOutcome::Action(Action::PickSession(_))),
            "unstamped entries must still be fuzzy-filtered, got {out:?}"
        );
    }

    fn key_code(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    /// Borrow the picker state of the open session picker for assertions.
    fn picker_state(agent: &AgentView) -> &crate::views::picker::PickerState {
        match agent.active_modal.as_ref() {
            Some(ActiveModal::SessionPicker { state, .. }) => state,
            _ => panic!("expected open session picker"),
        }
    }

    #[test]
    fn up_at_top_focuses_search_and_clears_selection() {
        // Pin vim-mode off; this test asserts the non-vim picker path.
        crate::appearance::cache::set_vim_mode(false);
        let mut agent = make_agent();
        open_picker(&mut agent, vec![entry("s0"), entry("s1")]);

        // Up from the first row moves focus to the search bar and hides the
        // list selection highlight (the row should no longer look selected).
        agent.handle_palette_or_arg_input(&key_code(KeyCode::Up));
        let st = picker_state(&agent);
        assert!(st.search_active, "search bar takes focus");
        assert!(st.selection_hidden, "list selection highlight is cleared");

        // Down from the search bar returns focus to the list and restores
        // the highlight.
        agent.handle_palette_or_arg_input(&key_code(KeyCode::Down));
        let st = picker_state(&agent);
        assert!(!st.search_active, "focus returns to the list");
        assert!(!st.selection_hidden, "selection highlight is restored");
    }

    #[test]
    fn down_at_bottom_focuses_search_and_clears_selection() {
        // Pin vim-mode off; this test asserts the non-vim picker path.
        crate::appearance::cache::set_vim_mode(false);
        let mut agent = make_agent();
        open_picker(&mut agent, vec![entry("s0"), entry("s1")]);

        // Move to the last selectable row, then Down again to reach search.
        agent.handle_palette_or_arg_input(&key_code(KeyCode::Down));
        agent.handle_palette_or_arg_input(&key_code(KeyCode::Down));
        let st = picker_state(&agent);
        assert!(st.search_active, "search bar takes focus from the bottom");
        assert!(st.selection_hidden, "list selection highlight is cleared");
    }

    #[test]
    fn typing_a_query_restores_selection() {
        // Pin vim-mode off; this test asserts the non-vim picker path.
        crate::appearance::cache::set_vim_mode(false);
        let mut agent = make_agent();
        open_picker(&mut agent, vec![entry("s0"), entry("s1")]);

        // Arrow into the search bar (selection hidden), then type — a query
        // makes the top match meaningful again, so the highlight returns.
        agent.handle_palette_or_arg_input(&key_code(KeyCode::Up));
        assert!(picker_state(&agent).selection_hidden);

        agent.handle_palette_or_arg_input(&key('s'));
        let st = picker_state(&agent);
        assert!(st.search_active, "still typing in the search bar");
        assert!(
            !st.selection_hidden,
            "typing a query restores the selection highlight"
        );
    }

    /// Paste garbage + Enter with no rows must not LoadSession.
    #[test]
    fn enter_with_garbage_query_does_not_load_session() {
        let mut agent = make_agent();
        open_picker(&mut agent, vec![]);
        if let Some(ActiveModal::SessionPicker { state, .. }) = agent.active_modal.as_mut() {
            state.set_query("this is pasted garbage!!!");
        }
        let out = agent.handle_palette_or_arg_input(&key_code(KeyCode::Enter));
        assert!(
            matches!(out, InputOutcome::Unchanged),
            "garbage query must be a no-op, got {out:?}"
        );
        assert!(
            matches!(agent.active_modal, Some(ActiveModal::SessionPicker { .. })),
            "picker must stay open"
        );
    }

    #[test]
    fn enter_with_uuid_query_loads_session() {
        let mut agent = make_agent();
        open_picker(&mut agent, vec![]);
        let sid = "019fb61a-85a5-7ba0-a4ec-24647dca1893";
        if let Some(ActiveModal::SessionPicker { state, .. }) = agent.active_modal.as_mut() {
            state.set_query(sid);
        }
        let out = agent.handle_palette_or_arg_input(&key_code(KeyCode::Enter));
        assert!(
            matches!(
                out,
                InputOutcome::Action(Action::LoadSession(ref id, None, false)) if id == sid
            ),
            "UUID query should direct-load, got {out:?}"
        );
    }
}

#[cfg(test)]
mod command_palette_vim_input_tests {
    use crate::actions::ActionRegistry;
    use crate::app::agent_view::AgentView;
    use crate::app::agent_view::test_fixtures::make_agent;
    use crate::app::app_view::InputOutcome;
    use crate::views::modal::ActiveModal;
    use crate::views::picker::PickerState;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

    // Open the command palette exactly as the Ctrl+P handler does: type-to-find
    // INPUT mode (`input_active`) over the full palette entries.
    fn open_command_palette(agent: &mut AgentView) {
        agent.active_modal = Some(ActiveModal::CommandPalette {
            entries: crate::views::modal::default_palette_entries(
                agent.sharing_enabled,
                &agent.prompt.slash_controller,
            ),
            state: PickerState::input_active(),
            window: crate::views::modal_window::ModalWindowState::new(),
        });
    }

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn esc() -> KeyEvent {
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
    }

    // Borrow the open command palette's picker state for assertions.
    fn palette_state(agent: &AgentView) -> &PickerState {
        match agent.active_modal.as_ref() {
            Some(ActiveModal::CommandPalette { state, .. }) => state,
            _ => panic!("expected open command palette"),
        }
    }

    #[test]
    fn minimal_palette_shortcuts_uses_live_configured_registry() {
        let mut agent = make_agent();
        agent
            .prompt
            .set_screen_mode(crate::app::ScreenMode::Minimal);
        agent.active_modal = Some(ActiveModal::CommandPalette {
            entries: crate::views::modal::default_palette_entries(
                agent.sharing_enabled,
                &agent.prompt.slash_controller,
            ),
            state: {
                let mut state = PickerState::input_active();
                state.set_query("keyboard shortcuts");
                state.selected = 1; // matching section header is row 0
                state
            },
            window: crate::views::modal_window::ModalWindowState::new(),
        });
        // Start from the real minimal set, then inject the existing config-gated
        // action in a supported context. This pins that modal dispatch preserves
        // the exact live registry rather than reconstructing any defaults.
        let mut actions =
            crate::actions::ActionRegistry::defaults_for(crate::app::ScreenMode::Minimal)
                .all()
                .to_vec();
        let mut config_gated = crate::actions::ActionRegistry::defaults_with_config(true)
            .find(crate::actions::ActionId::ToggleMouseCapture)
            .expect("config-gated action")
            .clone();
        config_gated.context = crate::actions::When::AgentScreen;
        actions.push(config_gated);
        let registry = crate::actions::ActionRegistry::new(actions);
        let out = agent.handle_modal_key_with_registry(
            &KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &registry,
        );
        assert!(matches!(out, InputOutcome::Changed));

        let Some(ActiveModal::ShortcutsHelp { entries, .. }) = &agent.active_modal else {
            panic!("expected shortcuts help modal");
        };
        let action_ids: Vec<_> = entries
            .iter()
            .filter_map(|entry| match entry {
                crate::views::shortcuts_help::ShortcutsHelpEntry::Hint {
                    action_id: Some(id),
                    ..
                } => Some(*id),
                _ => None,
            })
            .collect();
        assert!(action_ids.contains(&crate::actions::ActionId::EditPromptExternal));
        assert!(!action_ids.contains(&crate::actions::ActionId::ToggleTasks));
        assert!(action_ids.contains(&crate::actions::ActionId::ToggleMouseCapture));
        assert!(!action_ids.contains(&crate::actions::ActionId::OpenDashboard));
    }

    #[test]
    fn minimal_edit_prompt_palette_selection_preserves_draft() {
        let mut agent = make_agent();
        agent
            .prompt
            .set_screen_mode(crate::app::ScreenMode::Minimal);
        agent.prompt.set_text("keep this draft");
        agent.active_modal = Some(ActiveModal::CommandPalette {
            entries: crate::views::modal::default_palette_entries(
                agent.sharing_enabled,
                &agent.prompt.slash_controller,
            ),
            state: {
                let mut state = PickerState::input_active();
                // Contiguous substring of the label ("Edit Prompt in External Editor").
                state.set_query("external editor");
                state.selected = 1; // matching section header is row 0
                state
            },
            window: crate::views::modal_window::ModalWindowState::new(),
        });
        let out = agent.handle_modal_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(
            out,
            InputOutcome::Action(crate::app::actions::Action::EditPromptExternal)
        ));
        assert_eq!(agent.prompt.text(), "keep this draft");
        assert!(agent.active_modal.is_none());
    }

    /// Headline command-palette vim flow — a CI-runnable mirror of the ignored
    /// PTY scenario `vim_modal_command_palette.yaml`. Drives the real modal seam
    /// (`handle_modal_key`) so both the chrome Esc handling and the picker's
    /// `vim_normal_first: load_vim_mode()` wiring are exercised end to end.
    #[test]
    fn vim_command_palette_input_then_esc_to_nav_then_i_reenters() {
        // CI defaults vim off and this dev machine's config sets it on, so pin.
        crate::appearance::cache::set_vim_mode(true);
        let mut agent = make_agent();
        open_command_palette(&mut agent);

        // Opens in INPUT mode: a letter types/filters immediately.
        assert!(palette_state(&agent).search_active, "opens in input mode");
        agent.handle_modal_key(&key('a'));
        let st = palette_state(&agent);
        assert_eq!(st.query(), "a", "input mode: a letter filters");
        assert!(st.search_active);

        // First Esc clears the query via the modal chrome but stays in input.
        agent.handle_modal_key(&esc());
        let st = palette_state(&agent);
        assert!(st.query().is_empty(), "Esc clears the query");
        assert!(st.search_active, "still input after the first Esc");

        // Second Esc (empty query) drops to NAV via the picker's vim Esc.
        agent.handle_modal_key(&esc());
        let st = palette_state(&agent);
        assert!(!st.search_active, "second Esc drops to nav");
        assert!(st.query().is_empty());

        // NAV: a bare printable key must NOT type.
        let out = agent.handle_modal_key(&key('b'));
        let st = palette_state(&agent);
        assert!(st.query().is_empty(), "nav: a bare letter does not filter");
        assert!(!st.search_active);
        assert!(
            matches!(out, InputOutcome::Unchanged),
            "nav letter is inert"
        );

        // `i` re-enters INPUT without typing; a letter then filters again.
        agent.handle_modal_key(&key('i'));
        assert!(palette_state(&agent).search_active, "i re-enters search");
        assert!(palette_state(&agent).query().is_empty(), "i does not type");
        agent.handle_modal_key(&key('c'));
        assert_eq!(palette_state(&agent).query(), "c", "typing filters again");
        // Reset the global vim pin so it can't leak to later tests (libtest reuses threads).
        crate::appearance::cache::set_vim_mode(false);
    }

    /// `/` is the other vim search-entry key: from NAV it re-enters INPUT.
    #[test]
    fn vim_command_palette_slash_reenters_search_from_nav() {
        crate::appearance::cache::set_vim_mode(true);
        let mut agent = make_agent();
        open_command_palette(&mut agent);

        // Drop to nav: type, then two Escs (clear query, then nav).
        agent.handle_modal_key(&key('a'));
        agent.handle_modal_key(&esc());
        agent.handle_modal_key(&esc());
        assert!(!palette_state(&agent).search_active, "in nav mode");

        agent.handle_modal_key(&key('/'));
        assert!(palette_state(&agent).search_active, "/ re-enters search");
        assert!(palette_state(&agent).query().is_empty(), "/ does not type");
        // Reset the global vim pin so it can't leak to later tests (libtest reuses threads).
        crate::appearance::cache::set_vim_mode(false);
    }

    /// Vim OFF: the command palette stays type-to-filter — there is no nav mode,
    /// so a letter keeps filtering even after Esc clears the query.
    #[test]
    fn non_vim_command_palette_stays_type_to_filter() {
        crate::appearance::cache::set_vim_mode(false);
        let mut agent = make_agent();
        open_command_palette(&mut agent);

        agent.handle_modal_key(&key('a'));
        let st = palette_state(&agent);
        assert_eq!(st.query(), "a", "a letter filters");
        assert!(st.search_active);

        // Esc clears the query (chrome) but never drops to a nav mode.
        agent.handle_modal_key(&esc());
        assert!(
            palette_state(&agent).query().is_empty(),
            "Esc clears the query"
        );

        // A bare letter still types — no vim nav-mode suppression.
        agent.handle_modal_key(&key('b'));
        let st = palette_state(&agent);
        assert_eq!(st.query(), "b", "still type-to-filter (no nav mode)");
        assert!(st.search_active);
    }

    #[test]
    fn command_palette_bracketed_paste_targets_only_active_query() {
        crate::appearance::cache::set_vim_mode(false);
        let mut agent = make_agent();
        agent.prompt.set_text("hidden prompt");
        open_command_palette(&mut agent);
        if let Some(ActiveModal::CommandPalette { state, .. }) = agent.active_modal.as_mut() {
            state.set_query("ab");
        }
        let registry = ActionRegistry::defaults();
        let _ = agent.handle_input(
            &Event::Key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE)),
            &registry,
        );
        let outcome = agent.handle_input(&Event::Paste("中\r\n".to_owned()), &registry);
        assert!(matches!(outcome, InputOutcome::Changed));
        assert_eq!(palette_state(&agent).query(), "a中b");
        assert_eq!(agent.prompt.text(), "hidden prompt");

        if let Some(ActiveModal::CommandPalette { state, .. }) = agent.active_modal.as_mut() {
            state.set_query("");
            state.search_active = false;
        }
        crate::appearance::cache::set_vim_mode(true);
        let outcome = agent.handle_input(&Event::Paste("ignored".to_owned()), &registry);
        assert!(matches!(outcome, InputOutcome::Unchanged));
        assert!(palette_state(&agent).query().is_empty());
        assert_eq!(agent.prompt.text(), "hidden prompt");
        crate::appearance::cache::set_vim_mode(false);
    }

    // Drives the REAL command-palette render seam (draw_active_modal →
    // picker::render_picker_in_modal → render_search_bar)
    // — the path the bug was on — and asserts the cursor tracks focus.
    #[test]
    fn command_palette_search_bar_cursor_only_when_focused() {
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;

        let render_palette_search_row = |search_active: bool| -> (bool, String) {
            let mut agent = make_agent();
            open_command_palette(&mut agent);
            if let Some(ActiveModal::CommandPalette { state, .. }) = agent.active_modal.as_mut() {
                state.search_active = search_active;
            }
            let area = Rect::new(0, 0, 80, 24);
            let mut buf = Buffer::empty(area);
            agent.draw_active_modal(area, &mut buf, crate::theme::Theme::current(), false);

            let theme = crate::theme::Theme::current();
            let search_bar = match agent.active_modal.as_ref() {
                Some(ActiveModal::CommandPalette { state, .. }) => {
                    state
                        .hit_areas
                        .as_ref()
                        .expect("render sets hit_areas")
                        .search_bar
                }
                _ => panic!("expected an open command palette"),
            };
            let y = search_bar.y;
            let mut has_cursor = false;
            let mut text = String::new();
            for x in search_bar.x..search_bar.x + search_bar.width {
                if let Some(cell) = buf.cell((x, y)) {
                    text.push_str(cell.symbol());
                    // The cursor is an inverse-video cell (bg == text_primary).
                    if cell.bg == theme.text_primary {
                        has_cursor = true;
                    }
                }
            }
            (has_cursor, text)
        };

        let (focused_cursor, _) = render_palette_search_row(true);
        assert!(
            focused_cursor,
            "command palette search bar should render a cursor when search_active",
        );

        let (unfocused_cursor, unfocused_text) = render_palette_search_row(false);
        assert!(
            !unfocused_cursor,
            "command palette search bar must not render a cursor when not search_active",
        );
        assert!(
            unfocused_text.contains("/ to search"),
            "unfocused command palette should show the `/ to search` placeholder, got {unfocused_text:?}",
        );
    }
}

#[cfg(test)]
mod settings_memory_paste_routing_tests {
    use std::sync::Arc;

    use crate::actions::ActionRegistry;
    use crate::app::agent_view::test_fixtures::make_agent;
    use crate::app::app_view::InputOutcome;
    use crate::settings::{PagerLocalSnapshot, SettingsRegistry};
    use crate::views::memory_modal::{MemoryModalMode, MemoryModalState};
    use crate::views::modal::ActiveModal;
    use crate::views::settings_modal::SettingsModalState;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use xai_grok_shell::agent::config::UiConfig;

    fn left() -> Event {
        Event::Key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE))
    }

    #[test]
    fn settings_and_memory_paste_only_into_focused_filters() {
        let registry = ActionRegistry::defaults();
        let mut agent = make_agent();
        agent.prompt.set_text("hidden prompt");

        let mut settings = SettingsModalState::new(
            Arc::new(SettingsRegistry::defaults()),
            UiConfig::default(),
            PagerLocalSnapshot::default(),
        );
        settings.focus_filter();
        settings.set_query("ab");
        agent.active_modal = Some(ActiveModal::Settings {
            state: Box::new(settings),
        });
        let _ = agent.handle_input(&left(), &registry);
        let outcome = agent.handle_input(&Event::Paste("中\r\n".to_owned()), &registry);
        assert!(matches!(outcome, InputOutcome::Changed));
        let Some(ActiveModal::Settings { state }) = agent.active_modal.as_ref() else {
            panic!("settings modal remains open");
        };
        assert_eq!(state.query(), "a中b");
        assert_eq!(agent.prompt.text(), "hidden prompt");

        let mut memory = MemoryModalState::new(Vec::new());
        memory.mode = MemoryModalMode::FilterFocused;
        agent.active_modal = Some(ActiveModal::MemoryBrowser {
            state: Box::new(memory),
        });
        let _ = agent.handle_input(&Event::Paste("ab".to_owned()), &registry);
        let _ = agent.handle_input(&left(), &registry);
        let outcome = agent.handle_input(&Event::Paste("中\r\n".to_owned()), &registry);
        assert!(matches!(outcome, InputOutcome::Changed));
        let Some(ActiveModal::MemoryBrowser { state }) = agent.active_modal.as_ref() else {
            panic!("memory modal remains open");
        };
        assert_eq!(state.query(), "a中b");
        assert_eq!(agent.prompt.text(), "hidden prompt");
    }
}

#[cfg(test)]
mod notifications_modal_input_tests {
    use crate::app::agent_view::test_fixtures::make_agent;
    use crate::app::app_view::ExternalNotification;
    use crate::views::modal::{ActiveModal, NotificationListState};
    use crate::views::modal_window::ModalWindowState;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn open_notifications(agent: &mut crate::app::agent_view::AgentView) {
        agent.active_modal = Some(ActiveModal::Notifications {
            state: NotificationListState::new(vec![
                ExternalNotification {
                    message: "short toast".into(),
                    kind: Some("info".into()),
                },
                ExternalNotification {
                    message: "line one\nline two full body".into(),
                    kind: Some("warning".into()),
                },
            ]),
            window: ModalWindowState::new(),
        });
    }

    fn picker_state(
        agent: &crate::app::agent_view::AgentView,
    ) -> &crate::views::picker::PickerState {
        match agent.active_modal.as_ref() {
            Some(ActiveModal::Notifications { state, .. }) => &state.picker,
            _ => panic!("expected notifications modal"),
        }
    }

    #[test]
    fn opens_nav_first_and_e_expands_not_search() {
        let mut agent = make_agent();
        open_notifications(&mut agent);
        assert!(!picker_state(&agent).search_active);

        let out = agent.handle_modal_key(&key('e'));
        assert!(matches!(out, crate::app::app_view::InputOutcome::Changed));
        let st = picker_state(&agent);
        assert!(st.expanded.contains(&0), "e must expand selected row");
        assert!(!st.search_active, "e must not enter search");
        assert!(st.query().is_empty(), "e must not type into query");
    }

    #[test]
    fn right_arrow_expands_and_left_collapses() {
        let mut agent = make_agent();
        open_notifications(&mut agent);

        let right = KeyEvent::new(KeyCode::Right, KeyModifiers::NONE);
        agent.handle_modal_key(&right);
        assert!(picker_state(&agent).expanded.contains(&0));

        let left = KeyEvent::new(KeyCode::Left, KeyModifiers::NONE);
        agent.handle_modal_key(&left);
        assert!(!picker_state(&agent).expanded.contains(&0));
    }

    #[test]
    fn y_copies_while_nav_and_type_enters_search() {
        let mut agent = make_agent();
        open_notifications(&mut agent);

        agent.handle_modal_key(&key('y'));
        assert!(
            !picker_state(&agent).search_active,
            "y is copy in nav, not search"
        );
        assert!(picker_state(&agent).query().is_empty());
        assert_eq!(
            agent.toast.as_ref().map(|(m, _)| m.as_str()),
            Some("Copied notification")
        );

        // Non-reserved printable starts search (type-to-search).
        agent.handle_modal_key(&key('w'));
        let st = picker_state(&agent);
        assert!(st.search_active);
        assert_eq!(st.query(), "w");
    }

    #[test]
    fn slash_focuses_search_without_query_char() {
        let mut agent = make_agent();
        open_notifications(&mut agent);
        agent.handle_modal_key(&key('/'));
        let st = picker_state(&agent);
        assert!(st.search_active);
        assert!(st.query().is_empty());
    }
}
