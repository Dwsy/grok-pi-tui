//! Settings modal state, types, and filter cache.

use std::sync::Arc;

use ratatui::layout::Rect;

use crate::app::actions::Action;
use crate::input::line_editor::LineEditor;
use crate::settings::{
    CodingDataSharingLock, EnumChoice, OwnedEnumChoice, PagerLocalSnapshot, SettingCategory,
    SettingKey, SettingKind, SettingMeta, SettingValue, SettingsRegistry, StringValidator,
    current_value_for, dynamic_enum_choices,
};
use crate::views::modal_window::ModalWindowState;

use xai_grok_shell::agent::config::UiConfig;

// ---------------------------------------------------------------------------
// Public constants
// ---------------------------------------------------------------------------

/// Public display title of the modal — also used by
/// `views/modal.rs::ActiveModal::message` so renames stay in one place.
pub const MODAL_TITLE: &str = "Settings";

/// Width of the `"─ "` leading decoration before the title in the
/// modal's top border. Used to compute the breadcrumb hit-rect x offset.
pub(super) const TITLE_LEADING_DECORATION_W: u16 = 2; // `─ `: 1 cell box-drawing + 1 cell space.

// Descriptions are now expand-on-demand via Right/Left arrows;
// see `render_expanded_description`.

/// Below this width the row list is skipped (chrome renders empty).
pub(super) const CONTENT_MIN_WIDTH: u16 = 10;

/// Default max width for the modal. Keeps the row list compact on wide terminals.
pub(super) const STANDARD_MAX_WIDTH: u16 = 110;

/// Per-side margin when editing `max_thoughts_width` (modal widens
/// to `terminal_width - 2*margin` so the wrap preview is useful).
pub(super) const MAX_THOUGHTS_WIDTH_WIDENED_MARGIN: u16 = 8;

/// Outcome of a key or mouse event. Separate from `InputOutcome`
/// because the modal doesn't own `agent.active_modal` — close is
/// the caller's responsibility.
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum SettingsKeyOutcome {
    /// Close the modal.
    Close,
    /// Forward to dispatch.
    Action(Action),
    /// Forward two actions in order (first must resolve before second).
    /// Used by `d`-reset-in-picker to revert preview before opening
    /// the reset-confirm overlay.
    ActionPair(Action, Action),
    /// Close the modal and dispatch `Action` (deep-link Esc revert or Enter commit).
    ActionThenClose(Action),
    /// Internal state mutation, no action.
    Changed,
    /// No-op.
    Unchanged,
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// One row in the flat row table. `rows` holds every tab's rows; which
/// ones are visible is decided by [`compute_filtered`].
#[derive(Debug, Clone)]
pub enum RowEntry {
    /// Tab heading. Hidden while browsing a tab (the tab bar names it) and
    /// rendered only in global-search results, where matches from several
    /// tabs share one list.
    Header {
        category: SettingCategory,
    },
    /// Sidebar section heading, also drawn inline above the section's first
    /// row in the settings pane. Hidden in global-search results.
    Section {
        category: SettingCategory,
        name: &'static str,
    },
    Setting {
        key: SettingKey,
        meta_index: usize,
    },
}

impl RowEntry {
    /// The tab this row belongs to.
    pub fn category(&self, registry: &SettingsRegistry) -> Option<SettingCategory> {
        match self {
            Self::Header { category } | Self::Section { category, .. } => Some(*category),
            Self::Setting { meta_index, .. } => {
                registry.all().get(*meta_index).map(|meta| meta.category)
            }
        }
    }

    /// Whether the cursor can land on this row.
    pub fn is_selectable(&self) -> bool {
        matches!(self, Self::Setting { .. })
    }
}

/// Read-only projection of the modal's private discriminated state.
#[derive(Debug, Clone)]
pub enum SettingsModalMode {
    Browse,
    /// `/` was pressed; chars filter the visible rows.
    FilterFocused,
    /// Enum chooser sub-pane. `supports_preview` is cached at open
    /// time to avoid per-keystroke registry lookups.
    PickingEnum {
        key: SettingKey,
        choices_idx: usize,
        original_value: SettingValue,
        supports_preview: bool,
    },
    /// Group sub-sheet: a list of the group's child Bool toggles. `child_idx`
    /// is the focused child within the group. Space/Enter toggles in place
    /// (the sheet stays open); Esc returns to Browse. Mirrors `PickingEnum`'s
    /// open/render/commit flow but for independent toggles.
    PickingGroup {
        key: SettingKey,
        child_idx: usize,
    },
    /// Inline string/int editor. No live preview; Esc is a pure cancel.
    EditingValue {
        key: SettingKey,
    },
}

#[derive(Debug)]
pub(super) struct SettingsState {
    pub(super) filter: LineEditor,
    pub(super) mode: SettingsMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SettingsModeKind {
    Browse,
    FilterFocused,
    PickingEnum,
    PickingGroup,
    EditingString,
    EditingInt,
}

impl SettingsState {
    pub(super) fn mode_kind(&self) -> SettingsModeKind {
        match &self.mode {
            SettingsMode::Browse => SettingsModeKind::Browse,
            SettingsMode::FilterFocused => SettingsModeKind::FilterFocused,
            SettingsMode::PickingEnum { .. } => SettingsModeKind::PickingEnum,
            SettingsMode::PickingGroup { .. } => SettingsModeKind::PickingGroup,
            SettingsMode::EditingString { .. } => SettingsModeKind::EditingString,
            SettingsMode::EditingInt { .. } => SettingsModeKind::EditingInt,
        }
    }
}

#[derive(Debug)]
pub(super) enum SettingsMode {
    Browse,
    FilterFocused,
    PickingEnum {
        key: SettingKey,
        choices_idx: usize,
        original_value: SettingValue,
        supports_preview: bool,
    },
    PickingGroup {
        key: SettingKey,
        child_idx: usize,
    },
    EditingString {
        key: SettingKey,
        editor: LineEditor,
        validator: StringValidator,
        validation_error: Option<String>,
    },
    EditingInt {
        key: SettingKey,
        buffer: String,
        min: i64,
        max: i64,
    },
}

/// Is the open sub-pane a [`crate::settings::is_consent_chooser`] pane?
pub(super) fn mode_is_consent_chooser(mode: &SettingsMode) -> bool {
    matches!(
        mode,
        SettingsMode::PickingEnum { key, .. } if crate::settings::is_consent_chooser(key)
    )
}

/// Settings modal state. Boxed inside `ActiveModal::Settings` to
/// avoid clippy `large_enum_variant`.
pub struct SettingsModalState {
    pub window: ModalWindowState,
    pub registry: Arc<SettingsRegistry>,
    /// `UiConfig` snapshot, refreshed by the dispatcher on mutations.
    pub ui_snapshot: UiConfig,
    pub pager_snapshot: PagerLocalSnapshot,
    /// Every tab's rows (tab heading + section headings + settings), in
    /// render order. Which subset is on screen is `filtered_cache`.
    pub rows: Vec<RowEntry>,
    /// Tabs with at least one visible row, in `SettingCategory::ALL` order.
    /// Parallel to the modal chrome's tab bar.
    pub tabs: Vec<SettingCategory>,
    /// Index into `tabs` of the tab being browsed.
    pub active_tab: usize,
    /// True while the keyboard drives the section sidebar instead of the
    /// setting rows: Up/Down then jump whole sections.
    pub section_focus: bool,
    /// Index into `rows` of the focused row.
    pub selected: usize,
    /// Vertical scroll offset (line-granular).
    pub scroll_offset: usize,
    pub(super) state: SettingsState,
    /// Row indices matching `query`, recomputed per mutation (not per frame).
    pub(super) filtered_cache: Vec<usize>,

    // -- Mouse hit-test rects (populated by render) --
    pub list_area: Rect,
    /// Click-hit rect per row, parallel to `rows`.
    pub row_rects: Vec<Rect>,
    /// Click-hit rect for the value column on each row. Bool rows
    /// toggle on click; Enum/String/Int rows open the sub-pane.
    pub value_hit_rects: Vec<Rect>,
    /// `(decrement_rect, increment_rect)` for the Int stepper's
    /// `‹`/`›` glyphs. Zero-sized when not in Int editing mode.
    pub editor_adornment_rects: (Rect, Rect),
    /// Click-hit rect per choice in `PickingEnum`. Each rect spans the
    /// full height of a choice (including wrapped description lines).
    pub picker_choice_rects: Vec<Rect>,
    /// Hit-rect for the breadcrumb title in sub-pane modes
    /// (`PickingEnum`/`EditingValue`). Clicking anywhere on
    /// `Settings › <label>` cancels back to Browse. `None` in
    /// Browse/FilterFocused. Cleared on mode transitions.
    pub settings_breadcrumb_rect: Option<Rect>,
    /// Hover flag for the breadcrumb — adds underline affordance.
    pub breadcrumb_hovered: bool,
    /// Click-hit rect per sidebar section, parallel to [`Self::sections`].
    /// Clicking one jumps to that section's first row.
    pub sidebar_rects: Vec<Rect>,
    /// Row under the mouse cursor for hover highlighting. Indexes
    /// `rows` in Browse, `picker_choice_rects` in PickingEnum,
    /// always `None` in EditingValue.
    pub hover_row: Option<usize>,
    /// When true, Esc/Enter from `PickingEnum` close the modal instead of
    /// returning to Browse. Set by deep-link open (`OpenSettingsFocus`
    /// / `/privacy`); cleared on leave from the picker.
    pub close_on_picker_exit: bool,
}

impl SettingsModalState {
    /// Construct a new modal state from a registry + snapshots.
    pub fn new(
        registry: Arc<SettingsRegistry>,
        ui_snapshot: UiConfig,
        pager_snapshot: PagerLocalSnapshot,
    ) -> Self {
        let rows = build_rows(&registry);
        let tabs = build_tabs(&rows);
        // Start on the first selectable (non-header) row.
        let selected = rows
            .iter()
            .position(|r| matches!(r, RowEntry::Setting { .. }))
            .unwrap_or(0);
        let filtered_cache = compute_filtered(&rows, &registry, "", tabs.first().copied());
        Self {
            window: ModalWindowState::with_tabs(tabs.len()),
            registry,
            ui_snapshot,
            pager_snapshot,
            rows,
            tabs,
            active_tab: 0,
            section_focus: false,
            selected,
            scroll_offset: 0,
            state: SettingsState {
                filter: LineEditor::default(),
                mode: SettingsMode::Browse,
            },
            filtered_cache,
            list_area: Rect::default(),
            row_rects: Vec::new(),
            value_hit_rects: Vec::new(),
            editor_adornment_rects: (Rect::default(), Rect::default()),
            picker_choice_rects: Vec::new(),
            settings_breadcrumb_rect: None,
            breadcrumb_hovered: false,
            sidebar_rects: Vec::new(),
            hover_row: None,
            close_on_picker_exit: false,
        }
    }

    /// Why a Browse row cannot be edited (`None` = editable). Consulted by
    /// both render and input.
    pub fn row_lock(&self, key: SettingKey) -> Option<CodingDataSharingLock> {
        if key == "coding_data_sharing" {
            self.pager_snapshot.coding_data_sharing_lock
        } else {
            None
        }
    }

    /// The currently-focused setting row, if any.
    pub fn focused_setting(&self) -> Option<(SettingKey, &SettingMeta)> {
        match self.rows.get(self.selected)? {
            RowEntry::Setting { key, meta_index } => {
                let meta = self.registry.all().get(*meta_index)?;
                Some((*key, meta))
            }
            RowEntry::Header { .. } | RowEntry::Section { .. } => None,
        }
    }

    /// Focus a setting by registry key (Browse mode), switching to the tab
    /// that owns it. Returns whether the key was found; no-op if missing.
    pub fn focus_key(&mut self, key: &str) -> bool {
        let Some(idx) = self
            .rows
            .iter()
            .position(|r| matches!(r, RowEntry::Setting { key: k, .. } if *k == key))
        else {
            return false;
        };
        self.selected = idx;
        // Deep links can target any tab; bring it forward before clamping,
        // otherwise the row is filtered out and the clamp snaps away from it.
        if let Some(category) = self.rows[idx].category(&self.registry)
            && let Some(tab) = self.tabs.iter().position(|t| *t == category)
            && tab != self.active_tab
        {
            self.active_tab = tab;
            self.window.active_tab = tab;
            self.section_focus = false;
            self.scroll_offset = 0;
            self.invalidate_filter();
        }
        self.clamp_selected_to_visible();
        true
    }

    /// Filtered row indices in render order.
    pub fn filtered_indices(&self) -> &[usize] {
        &self.filtered_cache
    }

    // -- Tabs ---------------------------------------------------------------

    /// The category being browsed, or `None` when the registry is empty.
    pub fn active_tab_category(&self) -> Option<SettingCategory> {
        self.tabs.get(self.active_tab).copied()
    }

    /// Tab-bar labels, parallel to [`Self::tabs`].
    pub fn tab_labels(&self) -> Vec<&'static str> {
        self.tabs.iter().map(|cat| cat.tab_label()).collect()
    }

    /// Focus tab `idx`, snapping the selection to its first setting row.
    /// A no-op (returning `false`) when already there or out of range.
    pub fn set_active_tab(&mut self, idx: usize) -> bool {
        if idx >= self.tabs.len() || idx == self.active_tab {
            return false;
        }
        self.active_tab = idx;
        self.window.active_tab = idx;
        self.section_focus = false;
        self.scroll_offset = 0;
        self.hover_row = None;
        // Leaving search behind: tab switching is a browse-mode gesture.
        if !self.query().is_empty() {
            self.state.filter.reset();
        }
        self.invalidate_filter();
        self.select_first_visible();
        true
    }

    /// Step the active tab by `delta`, wrapping at both ends.
    pub fn cycle_tab(&mut self, delta: isize) -> bool {
        if self.tabs.len() < 2 {
            return false;
        }
        let len = self.tabs.len() as isize;
        let next = (self.active_tab as isize + delta).rem_euclid(len) as usize;
        self.set_active_tab(next)
    }

    /// Point the active tab at whichever tab owns the focused row. Used in
    /// search mode, where results from several tabs share one list.
    pub fn sync_tab_to_selection(&mut self) -> bool {
        let Some(category) = self
            .rows
            .get(self.selected)
            .and_then(|row| row.category(&self.registry))
        else {
            return false;
        };
        let Some(idx) = self.tabs.iter().position(|tab| *tab == category) else {
            return false;
        };
        if idx == self.active_tab {
            return false;
        }
        self.active_tab = idx;
        self.window.active_tab = idx;
        true
    }

    // -- Sections -----------------------------------------------------------

    /// Sidebar sections of the current view as `(name, first row index)`,
    /// in render order. Empty while searching (results are tab-grouped).
    pub fn sections(&self) -> Vec<(&'static str, usize)> {
        if !self.query().is_empty() {
            return Vec::new();
        }
        let mut sections: Vec<(&'static str, usize)> = Vec::new();
        let mut pending: Option<&'static str> = None;
        for &row_idx in &self.filtered_cache {
            match self.rows.get(row_idx) {
                Some(RowEntry::Section { name, .. }) => pending = Some(name),
                Some(RowEntry::Setting { .. }) => {
                    if let Some(name) = pending.take() {
                        sections.push((name, row_idx));
                    }
                }
                _ => {}
            }
        }
        sections
    }

    /// Index into [`Self::sections`] of the section containing the focused
    /// row. Falls back to the first section when the selection precedes all
    /// of them.
    pub fn active_section_index(&self) -> usize {
        let sections = self.sections();
        sections
            .iter()
            .rposition(|(_, first)| *first <= self.selected)
            .unwrap_or(0)
    }

    /// Move the selection to the first row of the section `delta` steps away,
    /// wrapping at both ends.
    pub fn jump_section(&mut self, delta: isize) -> bool {
        let sections = self.sections();
        if sections.len() < 2 {
            return false;
        }
        let len = sections.len() as isize;
        let next = (self.active_section_index() as isize + delta).rem_euclid(len) as usize;
        let target = sections[next].1;
        if target == self.selected {
            return false;
        }
        self.selected = target;
        true
    }

    /// Toggle keyboard focus between the section sidebar and the setting
    /// rows. Engages only when there are ≥2 sections to jump between.
    pub fn toggle_section_focus(&mut self) -> bool {
        let engage = !self.section_focus && self.sections().len() >= 2;
        if engage == self.section_focus {
            return false;
        }
        self.section_focus = engage;
        true
    }

    /// Snap the selection to the first selectable row currently visible.
    pub(super) fn select_first_visible(&mut self) {
        for &row_idx in &self.filtered_cache {
            if self.rows[row_idx].is_selectable() {
                self.selected = row_idx;
                return;
            }
        }
    }

    /// Rebuild rows from current process gates (voice / kitty / minimal).
    /// Keeps focus on the same key when possible; exits sub-panes if the key vanished.
    pub fn rebuild_rows(&mut self) {
        let prev_key = self.focused_setting().map(|(k, _)| k);
        let subpane_key = match &self.state.mode {
            SettingsMode::PickingEnum { key, .. }
            | SettingsMode::PickingGroup { key, .. }
            | SettingsMode::EditingString { key, .. }
            | SettingsMode::EditingInt { key, .. } => Some(*key),
            SettingsMode::Browse | SettingsMode::FilterFocused => None,
        };

        self.rows = build_rows(&self.registry);
        // Gates can retire a tab wholesale (e.g. voice rows in the Editor
        // tab); keep the same category focused when it survives.
        let prev_tab = self.active_tab_category();
        self.tabs = build_tabs(&self.rows);
        self.window.tab_count = self.tabs.len();
        self.window.tab_rects = vec![None; self.tabs.len()];
        self.active_tab = prev_tab
            .and_then(|cat| self.tabs.iter().position(|t| *t == cat))
            .unwrap_or(0)
            .min(self.tabs.len().saturating_sub(1));
        self.window.active_tab = self.active_tab;
        self.invalidate_filter();

        if let Some(key) = subpane_key {
            let still_visible = self
                .rows
                .iter()
                .any(|r| matches!(r, RowEntry::Setting { key: k, .. } if *k == key));
            if !still_visible {
                self.transition_to_browse();
            }
        }

        if let Some(key) = prev_key
            && let Some(idx) = self
                .rows
                .iter()
                .position(|r| matches!(r, RowEntry::Setting { key: k, .. } if *k == key))
        {
            self.selected = idx;
        } else {
            self.select_first_visible();
        }
        self.clamp_selected_to_visible();
        if self.section_focus && self.sections().len() < 2 {
            self.section_focus = false;
        }
    }

    pub fn mode(&self) -> SettingsModalMode {
        match &self.state.mode {
            SettingsMode::Browse => SettingsModalMode::Browse,
            SettingsMode::FilterFocused => SettingsModalMode::FilterFocused,
            SettingsMode::PickingEnum {
                key,
                choices_idx,
                original_value,
                supports_preview,
            } => SettingsModalMode::PickingEnum {
                key,
                choices_idx: *choices_idx,
                original_value: original_value.clone(),
                supports_preview: *supports_preview,
            },
            SettingsMode::PickingGroup { key, child_idx } => SettingsModalMode::PickingGroup {
                key,
                child_idx: *child_idx,
            },
            SettingsMode::EditingString { key, .. } | SettingsMode::EditingInt { key, .. } => {
                SettingsModalMode::EditingValue { key }
            }
        }
    }

    pub fn query(&self) -> &str {
        self.state.filter.text()
    }

    pub fn query_cursor(&self) -> usize {
        self.state.filter.cursor_byte()
    }

    pub fn set_query(&mut self, query: impl Into<String>) {
        self.state.filter.set_text(query);
        self.invalidate_filter();
        self.clamp_selected_to_visible();
    }

    pub fn editing_buffer(&self) -> Option<&str> {
        match &self.state.mode {
            SettingsMode::EditingString { editor, .. } => Some(editor.text()),
            SettingsMode::EditingInt { buffer, .. } => Some(buffer),
            _ => None,
        }
    }

    pub fn editing_cursor_byte(&self) -> Option<usize> {
        match &self.state.mode {
            SettingsMode::EditingString { editor, .. } => Some(editor.cursor_byte()),
            _ => None,
        }
    }

    pub fn editing_validation_error(&self) -> Option<&str> {
        match &self.state.mode {
            SettingsMode::EditingString {
                validation_error, ..
            } => validation_error.as_deref(),
            _ => None,
        }
    }

    /// Recompute `filtered_cache` from the current query and active tab.
    pub(super) fn invalidate_filter(&mut self) {
        self.filtered_cache = compute_filtered(
            &self.rows,
            &self.registry,
            self.state.filter.text(),
            self.tabs.get(self.active_tab).copied(),
        );
    }

    /// Snap `selected` to the first visible setting if filtered out.
    pub(super) fn clamp_selected_to_visible(&mut self) {
        if self.filtered_cache.is_empty() {
            return;
        }
        if self.filtered_cache.contains(&self.selected) {
            return;
        }
        self.select_first_visible();
    }

    /// Read the current value for a setting key.
    pub fn value_for(&self, key: SettingKey) -> Option<SettingValue> {
        current_value_for(key, &self.ui_snapshot, &self.pager_snapshot)
    }

    /// Move `selected` forward, skipping headers and filtered-out rows.
    pub(super) fn advance_next(&mut self) -> bool {
        let cur_pos = self.filtered_cache.iter().position(|&i| i == self.selected);
        let mut next = match cur_pos {
            Some(p) => p + 1,
            // Defensive: resume from top if `selected` is hidden.
            None => 0,
        };
        while next < self.filtered_cache.len() {
            let row_idx = self.filtered_cache[next];
            if matches!(self.rows[row_idx], RowEntry::Setting { .. }) {
                self.selected = row_idx;
                return true;
            }
            next += 1;
        }
        false
    }

    /// Move `selected` backward, skipping headers and filtered-out rows.
    pub(super) fn advance_prev(&mut self) -> bool {
        if self.filtered_cache.is_empty() {
            return false;
        }
        let cur_pos = self.filtered_cache.iter().position(|&i| i == self.selected);
        let mut prev = match cur_pos {
            Some(p) if p > 0 => p - 1,
            Some(_) => return false,
            // Defensive: resume from bottom if `selected` is hidden.
            None => self.filtered_cache.len() - 1,
        };
        loop {
            let row_idx = self.filtered_cache[prev];
            if matches!(self.rows[row_idx], RowEntry::Setting { .. }) {
                self.selected = row_idx;
                return true;
            }
            if prev == 0 {
                break;
            }
            prev -= 1;
        }
        false
    }

    /// Set selection to `idx` if it's a selectable row.
    pub fn select_at(&mut self, idx: usize) -> bool {
        if idx >= self.rows.len() {
            return false;
        }
        if !matches!(self.rows[idx], RowEntry::Setting { .. }) {
            return false;
        }
        if self.selected == idx {
            return false;
        }
        self.selected = idx;
        true
    }

    /// Reset hit-test geometry so mouse handlers degrade gracefully
    /// when render is aborted. Does NOT clear `hover_row` — that's
    /// cleared on mode transitions instead to avoid per-frame flicker.
    pub(crate) fn reset_hit_rects(&mut self) {
        self.list_area = Rect::default();
        self.row_rects.clear();
        self.value_hit_rects.clear();
        self.editor_adornment_rects = (Rect::default(), Rect::default());
        self.picker_choice_rects.clear();
        self.settings_breadcrumb_rect = None;
        self.breadcrumb_hovered = false;
        self.sidebar_rects.clear();
    }

    /// Transition to Browse, clearing sub-pane hover/breadcrumb state
    /// to prevent stale hit-rects across mode changes.
    pub(crate) fn transition_to_browse(&mut self) {
        self.state.mode = SettingsMode::Browse;
        self.hover_row = None;
        self.settings_breadcrumb_rect = None;
        self.breadcrumb_hovered = false;
        self.close_on_picker_exit = false;
    }

    pub fn focus_filter(&mut self) {
        self.state.mode = SettingsMode::FilterFocused;
    }

    pub(super) fn transition_to_picking_enum(
        &mut self,
        key: SettingKey,
        choices_idx: usize,
        original_value: SettingValue,
        supports_preview: bool,
    ) {
        self.state.mode = SettingsMode::PickingEnum {
            key,
            choices_idx,
            original_value,
            supports_preview,
        };
    }

    pub(super) fn transition_to_picking_group(&mut self, key: SettingKey, child_idx: usize) {
        self.state.mode = SettingsMode::PickingGroup { key, child_idx };
    }

    pub(super) fn transition_to_editing_string(
        &mut self,
        key: SettingKey,
        editor: LineEditor,
        validator: StringValidator,
        validation_error: Option<String>,
    ) {
        self.state.mode = SettingsMode::EditingString {
            key,
            editor,
            validator,
            validation_error,
        };
    }

    pub(super) fn transition_to_editing_int(
        &mut self,
        key: SettingKey,
        buffer: String,
        min: i64,
        max: i64,
    ) {
        self.state.mode = SettingsMode::EditingInt {
            key,
            buffer,
            min,
            max,
        };
    }

    /// Transition to `PickingEnum` if the focused row is Enum/DynamicEnum.
    /// Returns `false` if the focused row is another kind.
    pub fn try_enter_picking_enum(&mut self) -> bool {
        let (key, first_canonical, current_value, supports_preview, resolved_choices) = {
            let Some((key, meta)) = self.focused_setting() else {
                return false;
            };
            if self.row_lock(key).is_some() {
                return false;
            }
            // Side-model slots use OpenSideModelPicker (searchable ArgPicker), not this list.
            if matches!(
                key,
                "recap_model"
                    | "recap_model_2"
                    | "recap_model_3"
                    | "btw_model"
                    | "btw_model_2"
                    | "btw_model_3"
            ) {
                return false;
            }
            // Handles both static `Enum` and `DynamicEnum` catalogs.
            let (supports_preview, resolved): (bool, Vec<OwnedEnumChoice>) = match &meta.kind {
                SettingKind::Enum {
                    choices,
                    supports_preview,
                    ..
                } => (
                    *supports_preview,
                    effective_enum_choices(key, choices, &self.pager_snapshot)
                        .into_iter()
                        .map(|c| OwnedEnumChoice {
                            canonical: c.canonical.to_string(),
                            display: c.display.to_string(),
                            description: c.description.to_string(),
                        })
                        .collect(),
                ),
                SettingKind::DynamicEnum {
                    source,
                    supports_preview,
                    ..
                } => (
                    *supports_preview,
                    dynamic_enum_choices(*source, &self.pager_snapshot),
                ),
                _ => return false,
            };
            // Soft-fail if a static catalog exceeds the product cap. DynamicEnum
            // (e.g. models) is exempt — those lists are runtime-sized and always
            // scroll. The chooser itself scrolls static lists too; this assert is
            // a design guard, not a render requirement.
            debug_assert!(
                resolved.len() <= MAX_PICKER_CHOICES
                    || matches!(meta.kind, SettingKind::DynamicEnum { .. }),
                "Static Enum setting `{}` has {} choices, exceeds MAX_PICKER_CHOICES ({}). \
                 Raise the cap deliberately if a larger curated catalog is required.",
                key,
                resolved.len(),
                MAX_PICKER_CHOICES,
            );
            let first = resolved
                .first()
                .map(|c| c.canonical.clone())
                .unwrap_or_default();
            let cur = self.value_for(key);
            (key, first, cur, supports_preview, resolved)
        };

        // Resolve choices_idx from current value. For DynamicEnum,
        // if the current value no longer exists in the catalog,
        // fall back to index 1 (first real entry past sentinel)
        // to avoid accidentally wiping the user's preference.
        let is_dynamic_enum = matches!(
            self.registry.find(key).map(|m| &m.kind),
            Some(SettingKind::DynamicEnum { .. })
        );
        let unknown_dynamic_fallback_idx = if is_dynamic_enum && resolved_choices.len() > 1 {
            1
        } else {
            0
        };
        let choices_idx = match &current_value {
            Some(SettingValue::Enum(cur)) => resolved_choices
                .iter()
                .position(|c| c.canonical == *cur)
                .unwrap_or(0),
            Some(SettingValue::String(cur)) if !cur.is_empty() => resolved_choices
                .iter()
                .position(|c| c.canonical == *cur)
                .unwrap_or(unknown_dynamic_fallback_idx),
            Some(SettingValue::String(_)) => 0,
            _ => 0,
        };
        if is_dynamic_enum
            && choices_idx == unknown_dynamic_fallback_idx
            && unknown_dynamic_fallback_idx != 0
        {
            // Telemetry: log when a DynamicEnum value is stale.
            tracing::warn!(
                target: "settings",
                key = key,
                ?current_value,
                "DynamicEnum picker entered with a current value that no longer resolves \
                 in the live catalog — focusing first real choice instead of the \
                 (no override) sentinel to defend against accidental destructive Enter",
            );
        }
        let original_value = current_value.unwrap_or_else(|| {
            // Fallback to first choice, using the right value carrier.
            match self.registry.find(key).map(|m| &m.kind) {
                Some(SettingKind::DynamicEnum { .. }) => SettingValue::String(first_canonical),
                Some(SettingKind::Enum { choices, .. }) => {
                    let first_static = choices.first().map(|c| c.canonical).unwrap_or("");
                    SettingValue::Enum(first_static)
                }
                _ => SettingValue::Enum(""),
            }
        });
        self.transition_to_picking_enum(key, choices_idx, original_value, supports_preview);
        self.hover_row = None;
        true
    }

    /// Transition to `PickingGroup` if the focused row is a `Group`. Returns
    /// `false` for any other kind so the caller can fall through to the
    /// enum/editor entry points.
    pub fn try_enter_picking_group(&mut self) -> bool {
        let Some((key, meta)) = self.focused_setting() else {
            return false;
        };
        if !matches!(meta.kind, SettingKind::Group { .. }) {
            return false;
        }
        self.transition_to_picking_group(key, 0);
        self.hover_row = None;
        true
    }

    /// Transition to `EditingValue` if the focused row is String or Int.
    pub fn try_enter_editing_value(&mut self) -> bool {
        let Some((key, meta)) = self.focused_setting() else {
            return false;
        };
        let kind = meta.kind.clone();
        let value = self.value_for(key);
        match kind {
            SettingKind::String {
                default, validator, ..
            } => {
                let text = match value {
                    Some(SettingValue::String(text)) => text,
                    _ => default.to_string(),
                };
                let mut editor = LineEditor::default();
                editor.set_text(text);
                let validation_error = validate_string(
                    validator,
                    editor.text(),
                    &self.pager_snapshot.available_models,
                );
                self.transition_to_editing_string(key, editor, validator, validation_error);
            }
            SettingKind::Int {
                default, min, max, ..
            } => {
                let buffer = match value {
                    Some(SettingValue::Int(value)) => value.to_string(),
                    _ => default.to_string(),
                };
                self.transition_to_editing_int(key, buffer, min, max);
            }
            _ => return false,
        }
        self.hover_row = None;
        true
    }

    /// Build the Action that toggles the focused Bool row. Returns
    /// `None` with an error log on registry skew (caught by CI tests).
    pub fn toggle_focused_bool(&self) -> Option<Action> {
        let (key, meta) = self.focused_setting()?;
        if !matches!(meta.kind, SettingKind::Bool { .. }) {
            return None;
        }
        let cur = match self.value_for(key) {
            Some(SettingValue::Bool(b)) => b,
            Some(other) => {
                tracing::error!(
                    target: "settings",
                    ?key,
                    ?other,
                    "Bool-kind setting resolved to non-Bool value — registry skew",
                );
                return None;
            }
            None => {
                tracing::error!(
                    target: "settings",
                    ?key,
                    "Bool-kind setting has no current_value_for arm — registry skew",
                );
                return None;
            }
        };
        let action = action_for_bool(key, !cur);
        if action.is_none() {
            tracing::error!(
                target: "settings",
                ?key,
                "Bool-kind setting has no action_for_bool arm — registry skew",
            );
        }
        action
    }
}

/// Compute the visible row indices.
///
/// Empty query = browsing: the active tab's section headings and settings,
/// tab headings suppressed (the tab bar already names the tab). Non-empty
/// query = global search across every tab: matching settings only, each
/// tab's block introduced by its heading, section headings suppressed.
pub(super) fn compute_filtered(
    rows: &[RowEntry],
    registry: &SettingsRegistry,
    query: &str,
    active_tab: Option<SettingCategory>,
) -> Vec<usize> {
    if query.is_empty() {
        let Some(tab) = active_tab else {
            return Vec::new();
        };
        return rows
            .iter()
            .enumerate()
            .filter(|(_, row)| {
                !matches!(row, RowEntry::Header { .. }) && row.category(registry) == Some(tab)
            })
            .map(|(i, _)| i)
            .collect();
    }
    let matched_keys: Vec<SettingKey> = registry.search(query).iter().map(|m| m.key).collect();
    let mut result = Vec::new();
    let mut pending_header: Option<usize> = None;
    for (i, row) in rows.iter().enumerate() {
        match row {
            // Emit the tab heading lazily — only once its tab has a match.
            RowEntry::Header { .. } => pending_header = Some(i),
            RowEntry::Section { .. } => {}
            RowEntry::Setting { key, .. } => {
                if matched_keys.contains(key) {
                    if let Some(h) = pending_header.take() {
                        result.push(h);
                    }
                    result.push(i);
                }
            }
        }
    }
    result
}

/// Row visibility: voice rows need the voice gate; capture needs key releases;
/// `hidden_in_minimal` rows are dropped in minimal mode; `external_only` rows
/// are hidden unless the process runs the external-agent (grok-pi) profile.
/// Pure for unit tests.
pub(super) fn setting_row_visible(
    meta: &SettingMeta,
    kitty_releases: bool,
    minimal: bool,
    voice_mode: bool,
    external_agent: bool,
) -> bool {
    if !voice_mode
        && matches!(
            meta.key,
            "voice_keybind_enabled" | "voice_capture_mode" | "voice_stt_language"
        )
    {
        return false;
    }
    if meta.key == "voice_capture_mode" && !kitty_releases {
        return false;
    }
    if minimal && meta.hidden_in_minimal {
        return false;
    }
    if meta.external_only && !external_agent {
        return false;
    }
    true
}

fn build_rows(registry: &SettingsRegistry) -> Vec<RowEntry> {
    let kitty_releases = crate::app::kitty_releases_reported();
    let minimal = crate::app::minimal_mode_active();
    let voice_mode = crate::app::voice_mode_enabled();
    let external_agent = crate::app::external_agent_active();
    // Keys that belong to a group sub-sheet are rendered only inside that
    // sheet, never as their own top-level rows.
    let group_children: std::collections::HashSet<SettingKey> = registry
        .all()
        .iter()
        .filter_map(|m| match &m.kind {
            SettingKind::Group { children } => Some(*children),
            _ => None,
        })
        .flatten()
        .copied()
        .collect();
    let mut rows = Vec::new();
    for cat in SettingCategory::ALL {
        let visible: Vec<(usize, &SettingMeta)> = registry
            .all()
            .iter()
            .enumerate()
            .filter(|(_, meta)| meta.category == *cat)
            .filter(|(_, meta)| {
                setting_row_visible(meta, kitty_releases, minimal, voice_mode, external_agent)
            })
            .filter(|(_, meta)| !group_children.contains(meta.key))
            .collect();
        if visible.is_empty() {
            continue;
        }
        rows.push(RowEntry::Header { category: *cat });
        // Sidebar order comes from the layout table; within a section the
        // registry's declaration order is preserved.
        for section in crate::settings::sections_for(*cat) {
            let mut emitted_section = false;
            for (meta_index, meta) in &visible {
                if crate::settings::section_for(meta.key) != *section {
                    continue;
                }
                if !emitted_section {
                    rows.push(RowEntry::Section {
                        category: *cat,
                        name: section,
                    });
                    emitted_section = true;
                }
                rows.push(RowEntry::Setting {
                    key: meta.key,
                    meta_index: *meta_index,
                });
            }
        }
    }
    rows
}

/// Tabs with at least one visible row, in `SettingCategory::ALL` order.
fn build_tabs(rows: &[RowEntry]) -> Vec<SettingCategory> {
    rows.iter()
        .filter_map(|row| match row {
            RowEntry::Header { category } => Some(*category),
            _ => None,
        })
        .collect()
}

/// Construct the typed `Action::Set*` for a Bool setting.
pub(super) fn action_for_bool(key: SettingKey, new: bool) -> Option<Action> {
    match key {
        "compact_mode" => Some(Action::SetCompactMode(new)),
        "show_timestamps" => Some(Action::SetTimestamps(new)),
        "show_timeline" => Some(Action::SetTimeline(new)),
        "pi_builtin_tools.read" => Some(Action::SetPiBuiltinTool {
            tool: crate::app::actions::PiBuiltinTool::Read,
            enabled: new,
        }),
        "pi_builtin_tools.bash" => Some(Action::SetPiBuiltinTool {
            tool: crate::app::actions::PiBuiltinTool::Bash,
            enabled: new,
        }),
        "pi_builtin_tools.edit" => Some(Action::SetPiBuiltinTool {
            tool: crate::app::actions::PiBuiltinTool::Edit,
            enabled: new,
        }),
        "pi_builtin_tools.write" => Some(Action::SetPiBuiltinTool {
            tool: crate::app::actions::PiBuiltinTool::Write,
            enabled: new,
        }),
        "pi_builtin_tools.grep" => Some(Action::SetPiBuiltinTool {
            tool: crate::app::actions::PiBuiltinTool::Grep,
            enabled: new,
        }),
        "pi_builtin_tools.find" => Some(Action::SetPiBuiltinTool {
            tool: crate::app::actions::PiBuiltinTool::Find,
            enabled: new,
        }),
        "pi_builtin_tools.ls" => Some(Action::SetPiBuiltinTool {
            tool: crate::app::actions::PiBuiltinTool::Ls,
            enabled: new,
        }),
        "pi_builtin_tools.eval" => Some(Action::SetPiBuiltinTool {
            tool: crate::app::actions::PiBuiltinTool::Eval,
            enabled: new,
        }),
        "pi_bash" => Some(Action::SetPiBash(new)),
        "pi_eval_v2_only" => Some(Action::SetPiEvalV2Only(new)),
        "psm_resume_index" => Some(Action::SetPsmResumeIndex(new)),
        "pi_tree_file_rollback" => Some(Action::SetPiTreeFileRollback(new)),
        "pi_tree_skip_summary_prompt" => Some(Action::SetPiTreeSkipSummaryPrompt(new)),
        "pi_herdr" => Some(Action::SetPiHerdr(new)),
        "pi_subagents" => Some(Action::SetPiSubagents(new)),
        "pi_workflows" => Some(Action::SetPiWorkflows(new)),
        "pi_todo" => Some(Action::SetPiTodo(new)),
        "pi_goal" => Some(Action::SetPiGoal(new)),
        "pi_loop" => Some(Action::SetPiLoop(new)),
        "pi_ask_user_question" => Some(Action::SetPiAskUserQuestion(new)),
        "pi_ask_user_question_notifications" => {
            Some(Action::SetPiAskUserQuestionNotifications(new))
        }
        "pi_btw" => Some(Action::SetPiBtw(new)),
        "pi_cache_graph" => Some(Action::SetPiCacheGraph(new)),
        "pi_config_skill" => Some(Action::SetPiConfigSkill(new)),
        "pi_user_markdown" => Some(Action::SetPiUserMarkdown(new)),
        "pi_at_search_hidden" => Some(Action::SetPiAtSearchHidden(new)),
        "pi_keep_multi_agent" => Some(Action::SetPiKeepMultiAgent(new)),
        "pi_bash_command_format" => Some(Action::SetPiBashCommandFormat(new)),
        "write_edit_hover_popups" => Some(Action::SetWriteEditHoverPopups(new)),
        "show_other_tool_args" => Some(Action::SetShowOtherToolArgs(new)),
        "review_file_tree" => Some(Action::SetReviewFileTree(new)),
        "review_include_reads" => Some(Action::SetReviewIncludeReads(new)),
        "simple_mode" => Some(Action::SetSimpleMode(new)),
        "contextual_hints.undo" => Some(Action::SetContextualHintUndo(new)),
        "contextual_hints.plan_mode" => Some(Action::SetContextualHintPlanMode(new)),
        "contextual_hints.image_input" => Some(Action::SetContextualHintImageInput(new)),
        "contextual_hints.send_now" => Some(Action::SetContextualHintSendNow(new)),
        "contextual_hints.small_screen" => Some(Action::SetContextualHintSmallScreen(new)),
        "contextual_hints.word_select" => Some(Action::SetContextualHintWordSelect(new)),
        "contextual_hints.ssh_wrap" => Some(Action::SetContextualHintSshWrap(new)),
        "multiline_mode" => Some(Action::SetMultilineMode(new)),
        "vim_mode" => Some(Action::SetVimMode(new)),
        "session_recap" => Some(Action::SetSessionRecap(new)),
        "recap_mermaid" => Some(Action::SetRecapMermaid(new)),
        "progress_bar" => Some(Action::SetProgressBar(new)),
        "remote_tui_footer" => Some(Action::SetRemoteTuiFooter(new)),
        "voice_keybind_enabled" => Some(Action::SetVoiceKeybindEnabled(new)),
        "remember_tool_approvals" => Some(Action::SetRememberToolApprovals(new)),
        "toolset.ask_user_question.timeout_enabled" => {
            Some(Action::SetAskUserQuestionTimeoutEnabled(new))
        }
        "show_thinking_blocks" => Some(Action::SetShowThinkingBlocks(new)),
        "thinking_border_colors" => Some(Action::SetThinkingBorderColors(new)),
        "group_tool_verbs" => Some(Action::SetGroupToolVerbs(new)),
        "collapsed_edit_blocks" => Some(Action::SetCollapsedEditBlocks(new)),
        "side_by_side_edit" => Some(Action::SetSideBySideEdit(new)),
        "prompt_suggestions" => Some(Action::SetPromptSuggestions(new)),
        "respect_manual_folds" => Some(Action::SetRespectManualFolds(new)),
        "page_flip_on_send" => Some(Action::SetPageFlipOnSend(new)),
        "confirm_before_rewind" => Some(Action::SetConfirmBeforeRewind(new)),
        "combine_queued_prompts" => Some(Action::SetCombineQueuedPrompts(new)),
        "invert_scroll" => Some(Action::SetInvertScroll(new)),
        "show_tips" => Some(Action::SetShowTips(new)),
        "auto_update" => Some(Action::SetAutoUpdate(new)),
        "display_refresh_auto_cadence" => Some(Action::SetDisplayRefreshAutoCadence(new)),
        _ => None,
    }
}

/// Construct `Action::Preview*` for an Enum setting — used by the
/// picker's Up/Down (live preview) and Esc (revert). Preview actions
/// never persist; they only mutate the live visual.
pub(super) fn action_for_enum(key: SettingKey, choice: &'static str) -> Option<Action> {
    match key {
        "theme" => Some(Action::PreviewTheme(choice.to_string())),
        "auto_dark_theme" => Some(Action::PreviewAutoDarkTheme(choice.to_string())),
        "auto_light_theme" => Some(Action::PreviewAutoLightTheme(choice.to_string())),
        // No preview for settings with irreversible side effects.
        "permission_mode" => None,
        "coding_data_sharing" => None,
        "plan_mode" => None,
        "render_mermaid" => None,
        "keep_text_selection" => None,
        "scroll_mode" => None,
        _ => None,
    }
}

/// Construct `Action::Set*` commit variant for an Enum setting.
/// Commit actions persist to disk and fire a toast.
pub(super) fn action_for_enum_commit(key: SettingKey, choice: &'static str) -> Option<Action> {
    match key {
        "theme" => Some(Action::SetTheme(choice.to_string())),
        "auto_dark_theme" => Some(Action::SetAutoDarkTheme(choice.to_string())),
        "auto_light_theme" => Some(Action::SetAutoLightTheme(choice.to_string())),
        // Canonical strings from settings/defs.rs are the source of truth.
        "permission_mode" => match choice {
            "always-approve" => Some(Action::SetPermissionMode(
                crate::app::actions::PermissionModeKind::AlwaysApprove,
            )),
            // Auto's feature gate is enforced in `set_permission_mode`
            // (via `app.auto_mode_gate`, the same source the Shift+Tab cycle
            // uses), so the modal and the cycle never disagree. Committing Auto
            // when the gate is off degrades to Ask there.
            "auto" => Some(Action::SetPermissionMode(
                crate::app::actions::PermissionModeKind::Auto,
            )),
            "ask" => Some(Action::SetPermissionMode(
                crate::app::actions::PermissionModeKind::Ask,
            )),
            "default" => Some(Action::SetPermissionMode(
                crate::app::actions::PermissionModeKind::Default,
            )),
            _ => None,
        },
        "coding_data_sharing" => match choice {
            "opt-in" => Some(Action::SetCodingDataSharing { opted_in: true }),
            "opt-out" => Some(Action::SetCodingDataSharing { opted_in: false }),
            _ => None,
        },
        "plan_mode" => match choice {
            "on" => Some(Action::SetPlanMode(crate::app::actions::PlanModeKind::On)),
            "off" => Some(Action::SetPlanMode(crate::app::actions::PlanModeKind::Off)),
            _ => None,
        },
        "ctrl_o_tool_expansion" => Some(Action::SetCtrlOToolExpansion(choice.to_string())),
        "pi_eval" => match choice {
            "v1" | "v2" => Some(Action::SetPiEval(choice.to_string())),
            _ => None,
        },
        "pi_eval_v2_language" => match choice {
            "js" | "py" | "all" => Some(Action::SetPiEvalV2Language(choice.to_string())),
            _ => None,
        },
        "pi_bash_run_display" => crate::appearance::ExecuteHeaderContent::from_canonical(choice)
            .map(Action::SetPiBashRunDisplay),
        "hunk_tracker_mode" => Some(Action::SetHunkTrackerMode(choice.to_string())),
        "screen_mode" => Some(Action::SetScreenMode(choice.to_string())),
        "voice_capture_mode" => Some(Action::SetVoiceCaptureMode(choice.to_string())),
        "voice_stt_language" => Some(Action::SetVoiceSttLanguage(choice.to_string())),
        "render_mermaid" => {
            crate::appearance::RenderMermaid::from_canonical(choice).map(Action::SetRenderMermaid)
        }
        "keep_text_selection" => crate::appearance::TextSelection::from_canonical(choice)
            .map(Action::SetKeepTextSelection),
        // Junk canonicals fold to None — Enter no-ops instead of mis-mapping.
        "scroll_mode" => {
            crate::appearance::ScrollMode::from_canonical(choice).map(Action::SetScrollMode)
        }
        "default_selected_permission" => {
            Some(Action::SetDefaultSelectedPermission(choice.to_string()))
        }
        _ => None,
    }
}

/// Construct `Action::Set*` commit variant for a String setting.
/// Resolves model names via the snapshot before producing the action.
/// Empty buffer maps to `Action::Clear*` for model settings.
pub(super) fn action_for_string(
    key: SettingKey,
    value: String,
    snapshot: &PagerLocalSnapshot,
) -> Option<Action> {
    match key {
        "prompt_cursor" => Some(Action::SetPromptCursor(value)),
        "default_model" => {
            if value.is_empty() {
                Some(Action::ClearDefaultModel)
            } else {
                snapshot
                    .resolve_model_name(&value)
                    .map(Action::SetDefaultModel)
            }
        }
        "fork_secondary_model" => {
            if value.is_empty() {
                Some(Action::ClearForkSecondaryModel)
            } else {
                snapshot
                    .resolve_model_name(&value)
                    .map(Action::SetForkSecondaryModel)
            }
        }
        "recap_model" => {
            if value.is_empty() {
                Some(Action::ClearRecapModel)
            } else {
                snapshot
                    .resolve_model_name(&value)
                    .map(Action::SetRecapModel)
            }
        }
        "recap_model_2" => {
            if value.is_empty() {
                Some(Action::ClearRecapModel2)
            } else {
                Some(Action::SetRecapModel2(value))
            }
        }
        "recap_model_3" => {
            if value.is_empty() {
                Some(Action::ClearRecapModel3)
            } else {
                Some(Action::SetRecapModel3(value))
            }
        }
        "btw_model" => {
            if value.is_empty() {
                Some(Action::ClearBtwModel)
            } else {
                Some(Action::SetBtwModel(value))
            }
        }
        "btw_model_2" => {
            if value.is_empty() {
                Some(Action::ClearBtwModel2)
            } else {
                Some(Action::SetBtwModel2(value))
            }
        }
        "btw_model_3" => {
            if value.is_empty() {
                Some(Action::ClearBtwModel3)
            } else {
                Some(Action::SetBtwModel3(value))
            }
        }

        _ => {
            let _ = value;
            let _ = snapshot;
            None
        }
    }
}

/// Construct `Action::Set*` commit variant for an Int setting.
pub(super) fn action_for_int(key: SettingKey, value: i64) -> Option<Action> {
    match key {
        "max_thoughts_width" => Some(Action::SetMaxThoughtsWidth(value)),
        "scroll_speed" => Some(Action::SetScrollSpeed(value)),
        "scroll_lines" => Some(Action::SetScrollLines(value)),
        _ => None,
    }
}

/// Validate a String buffer against the registered `StringValidator`.
/// Returns `Some(error_message)` on failure, `None` on success.
pub(super) fn validate_string(
    validator: StringValidator,
    buffer: &str,
    available_models: &[(String, agent_client_protocol::ModelId)],
) -> Option<String> {
    match validator {
        StringValidator::Any => None,
        StringValidator::PromptCursor => {
            if crate::appearance::PromptCursor::parse_config(buffer).is_some() {
                None
            } else {
                Some(
                    "Use native, block, underline, bar, or one single-column character".to_string(),
                )
            }
        }
        StringValidator::NonEmptyToken => {
            if buffer.is_empty() {
                Some("Value cannot be empty".to_string())
            } else if buffer.chars().any(|c| c.is_whitespace()) {
                Some("Value cannot contain whitespace".to_string())
            } else {
                None
            }
        }
        StringValidator::KnownModel => {
            // Empty = "clear default" sentinel.
            if buffer.is_empty() {
                return None;
            }
            // Reject if the model catalog hasn't loaded yet.
            if available_models.is_empty() {
                return Some("Model catalog still loading — try again".to_string());
            }
            let matched = available_models
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case(buffer));
            if matched {
                None
            } else {
                Some(format!("Unknown model: \"{buffer}\""))
            }
        }
    }
}

/// Soft product cap on static Enum choices (settings unit tests enforce it).
///
/// The chooser already scrolls within the viewport when the focused choice
/// falls off-screen (`picker_scroll_offset`); this limit exists so catalogs
/// stay intentionally curated rather than unbounded. Sized to fit the full
/// Grok STT language list (25 codes + client-only `auto` = 26) with headroom.
pub(crate) const MAX_PICKER_CHOICES: usize = 32;

/// The children of a group setting, or an empty slice if `key` is not a group.
pub(super) fn group_children(state: &SettingsModalState, key: SettingKey) -> &'static [SettingKey] {
    match state.registry.find(key).map(|m| &m.kind) {
        Some(SettingKind::Group { children }) => children,
        _ => &[],
    }
}

/// Whether `(key, canonical)` is gated off and must not be offered as a choice:
/// `permission_mode`'s "auto" when the auto gate is off, and
/// `voice_capture_mode`'s "hold" without key-release reporting. Pure (gates
/// passed as args) so it's unit-testable without touching process globals.
pub(super) fn enum_choice_gated_off(
    key: SettingKey,
    canonical: &str,
    auto_mode_gate: bool,
    kitty_releases: bool,
) -> bool {
    (key == "permission_mode" && canonical == "auto" && !auto_mode_gate)
        || (key == "voice_capture_mode" && canonical == "hold" && !kitty_releases)
}

/// The effective static Enum choices for a picker, hiding gated-off options so
/// the modal never offers a choice the setter would silently no-op. Every
/// index-based picker path (len / at / render / seed) routes through this.
pub(super) fn effective_enum_choices<'a>(
    key: SettingKey,
    choices: &'a [EnumChoice],
    snapshot: &PagerLocalSnapshot,
) -> Vec<&'a EnumChoice> {
    let kitty_releases = crate::app::kitty_releases_reported();
    choices
        .iter()
        .filter(|c| {
            !enum_choice_gated_off(key, c.canonical, snapshot.auto_mode_gate, kitty_releases)
        })
        .collect()
}
