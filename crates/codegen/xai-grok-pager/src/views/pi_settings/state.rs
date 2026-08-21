//! State for the grok-pi settings panel.
//!
//! ## Row model
//!
//! [`PiSettingsState::rows`] holds every tab's rows in render order —
//! a [`Row::Tab`] heading, then each section's [`Row::Section`] heading
//! followed by its [`Row::Setting`]s. What is on screen is
//! [`PiSettingsState::visible`], a list of indices into `rows`:
//!
//! - **Browsing** (empty query): the active tab's section headings and
//!   settings. Tab headings are suppressed — the tab bar names the tab.
//! - **Searching**: matching settings from *every* tab, each tab's block
//!   introduced by its heading. Section headings are suppressed.
//!
//! ## Modes
//!
//! `Browse` → `Search` on `/`; `Browse` → `Picking` / `PickingGroup` /
//! `EditingString` / `EditingInt` on Enter over the matching row kind;
//! `Browse` → `ConfirmReset` on `d`. Every sub-mode returns to `Browse`.

use std::sync::Arc;

use ratatui::layout::Rect;

use super::layout;
use crate::app::actions::Action;
use crate::input::line_editor::LineEditor;
use crate::settings::{
    CodingDataSharingLock, EnumChoice, OwnedEnumChoice, PagerLocalSnapshot, SettingCategory,
    SettingKey, SettingKind, SettingMeta, SettingValue, SettingsRegistry, StringValidator,
    current_value_for, default_value_for, dynamic_enum_choices,
};
use crate::views::modal_window::ModalWindowState;

use xai_grok_shell::agent::config::UiConfig;

/// Panel title, shown on the modal's top border.
pub const MODAL_TITLE: &str = "Settings";

/// One entry in the flat row table.
#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    /// Tab heading. Rendered only in search results, where matches from
    /// several tabs share one list.
    Tab { category: SettingCategory },
    /// Sidebar section heading; also drawn inline when the pane is too narrow
    /// for the sidebar. Suppressed in search results.
    Section {
        category: SettingCategory,
        name: &'static str,
    },
    Setting {
        key: SettingKey,
        /// Index into `registry.all()`.
        meta: usize,
    },
}

impl Row {
    pub fn is_setting(&self) -> bool {
        matches!(self, Self::Setting { .. })
    }

    fn category(&self, registry: &SettingsRegistry) -> Option<SettingCategory> {
        match self {
            Self::Tab { category } | Self::Section { category, .. } => Some(*category),
            Self::Setting { meta, .. } => registry.all().get(*meta).map(|m| m.category),
        }
    }
}

/// What the panel is doing right now. Crate-internal because the editing
/// variants carry `LineEditor`; callers outside the panel read [`ModeKind`].
#[derive(Debug)]
pub(crate) enum Mode {
    Browse,
    /// `/` was pressed; the query filters settings across every tab.
    Search,
    /// Enum / dynamic-enum chooser.
    Picking {
        key: SettingKey,
        index: usize,
        /// Value on entry, restored when Esc reverts a live preview.
        original: SettingValue,
        supports_preview: bool,
        scroll: usize,
    },
    /// Sub-sheet listing a `Group` setting's child toggles.
    PickingGroup {
        key: SettingKey,
        child: usize,
    },
    EditingString {
        key: SettingKey,
        editor: LineEditor,
        validator: StringValidator,
        error: Option<String>,
    },
    EditingInt {
        key: SettingKey,
        buffer: String,
        min: i64,
        max: i64,
    },
    /// `d` was pressed; y/n confirms resetting the focused row to its default.
    ConfirmReset {
        key: SettingKey,
    },
}

/// Read-only projection of [`Mode`], for callers outside this module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeKind {
    Browse,
    Search,
    Picking,
    PickingGroup,
    EditingString,
    EditingInt,
    ConfirmReset,
}

impl Mode {
    pub fn kind(&self) -> ModeKind {
        match self {
            Self::Browse => ModeKind::Browse,
            Self::Search => ModeKind::Search,
            Self::Picking { .. } => ModeKind::Picking,
            Self::PickingGroup { .. } => ModeKind::PickingGroup,
            Self::EditingString { .. } => ModeKind::EditingString,
            Self::EditingInt { .. } => ModeKind::EditingInt,
            Self::ConfirmReset { .. } => ModeKind::ConfirmReset,
        }
    }

    /// Sub-panes take over the whole window; Browse and Search share the
    /// tabbed list surface.
    pub fn is_sub_pane(&self) -> bool {
        !matches!(self, Self::Browse | Self::Search)
    }

    /// The setting a sub-pane is operating on.
    pub fn subject(&self) -> Option<SettingKey> {
        match self {
            Self::Picking { key, .. }
            | Self::PickingGroup { key, .. }
            | Self::EditingString { key, .. }
            | Self::EditingInt { key, .. }
            | Self::ConfirmReset { key } => Some(*key),
            Self::Browse | Self::Search => None,
        }
    }
}

/// Outcome of a key or mouse event. The panel does not own
/// `agent.active_modal`, so closing is the caller's job.
#[derive(Debug)]
pub enum Outcome {
    /// Close the panel.
    Close,
    /// Forward to dispatch.
    Action(Action),
    /// Forward both, in order.
    ActionPair(Action, Action),
    /// Close the panel and forward the action. Used by deep-link opens, where
    /// the chooser is the whole interaction.
    ActionThenClose(Action),
    /// Internal state change; repaint.
    Changed,
    /// Nothing happened.
    Unchanged,
}

impl Outcome {
    pub(super) fn changed_if(moved: bool) -> Self {
        if moved {
            Self::Changed
        } else {
            Self::Unchanged
        }
    }
}

pub struct PiSettingsState {
    pub window: ModalWindowState,
    pub registry: Arc<SettingsRegistry>,
    /// `UiConfig` snapshot, refreshed by the dispatcher on every mutation.
    pub ui_snapshot: UiConfig,
    pub pager_snapshot: PagerLocalSnapshot,

    /// Every tab's rows, in render order.
    pub rows: Vec<Row>,
    /// Tabs with at least one visible row, in `SettingCategory::ALL` order.
    pub tabs: Vec<SettingCategory>,
    /// Index into [`Self::tabs`].
    pub active_tab: usize,
    /// Indices into [`Self::rows`] that are currently on screen.
    pub visible: Vec<usize>,
    /// Index into [`Self::rows`] of the focused row.
    pub selected: usize,
    /// First visible row, as a position within [`Self::visible`].
    pub scroll: usize,
    /// Keyboard drives the section sidebar rather than the rows.
    pub section_focus: bool,

    pub(crate) mode: Mode,
    pub(super) query: LineEditor,

    // -- Hit-test geometry, rebuilt by every render --
    pub list_area: Rect,
    /// Click rect per row, parallel to [`Self::rows`]; zero-sized when off screen.
    pub row_rects: Vec<Rect>,
    /// Click rect for each row's value column. Bool rows toggle, other kinds
    /// open their sub-pane.
    pub value_rects: Vec<Rect>,
    /// Click rect per sidebar section, parallel to [`Self::sections`].
    pub sidebar_rects: Vec<Rect>,
    /// Click rect per choice in `Picking` / `PickingGroup`.
    pub choice_rects: Vec<Rect>,
    /// `(decrement, increment)` rects for the Int stepper.
    pub stepper_rects: (Rect, Rect),
    /// Row under the pointer. Indexes `rows` while browsing, `choice_rects`
    /// inside a chooser.
    pub hover: Option<usize>,
    /// Esc/Enter out of a chooser closes the panel instead of returning to
    /// Browse. Set by deep-link opens such as `/privacy`.
    pub close_on_picker_exit: bool,
}

impl PiSettingsState {
    pub fn new(
        registry: Arc<SettingsRegistry>,
        ui_snapshot: UiConfig,
        pager_snapshot: PagerLocalSnapshot,
    ) -> Self {
        let rows = build_rows(&registry);
        let tabs = build_tabs(&rows);
        let mut state = Self {
            window: ModalWindowState::with_tabs(tabs.len()),
            registry,
            ui_snapshot,
            pager_snapshot,
            rows,
            tabs,
            active_tab: 0,
            visible: Vec::new(),
            selected: 0,
            scroll: 0,
            section_focus: false,
            mode: Mode::Browse,
            query: LineEditor::default(),
            list_area: Rect::default(),
            row_rects: Vec::new(),
            value_rects: Vec::new(),
            sidebar_rects: Vec::new(),
            choice_rects: Vec::new(),
            stepper_rects: (Rect::default(), Rect::default()),
            hover: None,
            close_on_picker_exit: false,
        };
        state.refresh_visible();
        state.select_first_visible();
        state
    }

    // -- Queries ------------------------------------------------------------

    pub fn query(&self) -> &str {
        self.query.text()
    }

    pub fn query_cursor(&self) -> usize {
        self.query.cursor_byte()
    }

    pub fn searching(&self) -> bool {
        !self.query.text().is_empty()
    }

    /// Whether Esc means something inside the panel rather than "close it".
    /// The modal chrome consults this before claiming Esc, so backing out of a
    /// search, a section focus, or a sub-pane never dismisses the panel.
    ///
    /// Search counts even with an empty query: `/` has already put the cursor
    /// in the search field, and Esc there means "leave the field".
    pub fn owns_escape(&self) -> bool {
        self.mode.is_sub_pane()
            || self.mode.kind() == ModeKind::Search
            || self.searching()
            || self.section_focus
    }

    pub fn active_category(&self) -> Option<SettingCategory> {
        self.tabs.get(self.active_tab).copied()
    }

    pub fn tab_labels(&self) -> Vec<&'static str> {
        self.tabs.iter().copied().map(layout::tab_label).collect()
    }

    /// The focused setting row, if the cursor is on one.
    pub fn focused(&self) -> Option<(SettingKey, &SettingMeta)> {
        match self.rows.get(self.selected)? {
            Row::Setting { key, meta } => Some((*key, self.registry.all().get(*meta)?)),
            _ => None,
        }
    }

    /// Metadata for a key, or `None` on registry skew.
    pub fn meta(&self, key: SettingKey) -> Option<&SettingMeta> {
        self.registry.find(key)
    }

    /// Live value for a key, read from the snapshots.
    pub fn value_of(&self, key: SettingKey) -> Option<SettingValue> {
        current_value_for(key, &self.ui_snapshot, &self.pager_snapshot)
    }

    /// Registered default for a key.
    pub fn default_of(&self, key: SettingKey) -> Option<SettingValue> {
        self.registry.find(key).map(default_value_for)
    }

    /// Why a row cannot be edited (`None` = editable).
    pub fn lock(&self, key: SettingKey) -> Option<CodingDataSharingLock> {
        (key == "coding_data_sharing")
            .then_some(self.pager_snapshot.coding_data_sharing_lock)
            .flatten()
    }

    /// Sidebar sections of the current view as `(name, first row index)`.
    /// Empty while searching — results are grouped by tab instead.
    pub fn sections(&self) -> Vec<(&'static str, usize)> {
        if self.searching() {
            return Vec::new();
        }
        let mut sections = Vec::new();
        let mut pending: Option<&'static str> = None;
        for &row in &self.visible {
            match self.rows.get(row) {
                Some(Row::Section { name, .. }) => pending = Some(name),
                Some(Row::Setting { .. }) => {
                    if let Some(name) = pending.take() {
                        sections.push((name, row));
                    }
                }
                _ => {}
            }
        }
        sections
    }

    /// Index into [`Self::sections`] of the section holding the focus.
    pub fn active_section(&self) -> usize {
        self.sections()
            .iter()
            .rposition(|(_, first)| *first <= self.selected)
            .unwrap_or(0)
    }

    /// Enum choices for a key, with gated-off options removed. Covers both
    /// static `Enum` catalogs and runtime `DynamicEnum` sources.
    pub fn choices_for(&self, key: SettingKey) -> Vec<OwnedEnumChoice> {
        match self.registry.find(key).map(|m| &m.kind) {
            Some(SettingKind::Enum { choices, .. }) => self
                .enabled_choices(key, choices)
                .into_iter()
                .map(|c| OwnedEnumChoice {
                    canonical: c.canonical.to_string(),
                    display: c.display.to_string(),
                    description: c.description.to_string(),
                })
                .collect(),
            Some(SettingKind::DynamicEnum { source, .. }) => {
                dynamic_enum_choices(*source, &self.pager_snapshot)
            }
            _ => Vec::new(),
        }
    }

    /// Static choices minus the ones the setter would silently no-op:
    /// `permission_mode`'s Auto without the gate, and `voice_capture_mode`'s
    /// Hold without key-release reporting.
    fn enabled_choices<'a>(
        &self,
        key: SettingKey,
        choices: &'a [EnumChoice],
    ) -> Vec<&'a EnumChoice> {
        let kitty = crate::app::kitty_releases_reported();
        let auto_gate = self.pager_snapshot.auto_mode_gate;
        choices
            .iter()
            .filter(|c| !choice_gated_off(key, c.canonical, auto_gate, kitty))
            .collect()
    }

    /// Children of a `Group` setting, or empty for other kinds.
    pub fn group_children(&self, key: SettingKey) -> &'static [SettingKey] {
        match self.registry.find(key).map(|m| &m.kind) {
            Some(SettingKind::Group { children }) => children,
            _ => &[],
        }
    }

    // -- Row table ----------------------------------------------------------

    /// Recompute [`Self::visible`] from the query and active tab.
    pub(super) fn refresh_visible(&mut self) {
        let query = self.query.text();
        if query.is_empty() {
            let Some(tab) = self.tabs.get(self.active_tab).copied() else {
                self.visible.clear();
                return;
            };
            self.visible = self
                .rows
                .iter()
                .enumerate()
                .filter(|(_, row)| {
                    !matches!(row, Row::Tab { .. }) && row.category(&self.registry) == Some(tab)
                })
                .map(|(i, _)| i)
                .collect();
            return;
        }
        let matched: Vec<SettingKey> = self
            .registry
            .search(query)
            .iter()
            .map(|meta| meta.key)
            .collect();
        let mut visible = Vec::new();
        let mut pending_tab: Option<usize> = None;
        for (i, row) in self.rows.iter().enumerate() {
            match row {
                // Emit a tab heading lazily, only once its tab has a match.
                Row::Tab { .. } => pending_tab = Some(i),
                Row::Section { .. } => {}
                Row::Setting { key, .. } => {
                    if matched.contains(key) {
                        if let Some(tab) = pending_tab.take() {
                            visible.push(tab);
                        }
                        visible.push(i);
                    }
                }
            }
        }
        self.visible = visible;
    }

    /// Rebuild the row table after a runtime gate flips (voice, minimal mode,
    /// kitty key releases). Keeps the focused key when it survives.
    pub fn rebuild_rows(&mut self) {
        let previous_key = self.focused().map(|(key, _)| key);
        let previous_tab = self.active_category();
        let sub_pane_key = self.mode.subject();

        self.rows = build_rows(&self.registry);
        self.tabs = build_tabs(&self.rows);
        self.window.tab_count = self.tabs.len();
        self.window.tab_rects = vec![None; self.tabs.len()];
        self.active_tab = previous_tab
            .and_then(|cat| self.tabs.iter().position(|t| *t == cat))
            .unwrap_or(0);
        self.window.active_tab = self.active_tab;

        // A sub-pane whose setting vanished has nothing left to edit.
        if let Some(key) = sub_pane_key
            && !self.rows.iter().any(|r| row_key(r) == Some(key))
        {
            self.to_browse();
        }

        self.refresh_visible();
        match previous_key.and_then(|key| self.rows.iter().position(|r| row_key(r) == Some(key))) {
            Some(idx) => self.selected = idx,
            None => self.select_first_visible(),
        }
        self.clamp_selection();
        if self.section_focus && self.sections().len() < 2 {
            self.section_focus = false;
        }
    }

    // -- Navigation ---------------------------------------------------------

    pub(super) fn select_first_visible(&mut self) {
        if let Some(&row) = self.visible.iter().find(|&&r| self.rows[r].is_setting()) {
            self.selected = row;
        }
    }

    /// Snap the focus back into the visible set if filtering pushed it out.
    pub(super) fn clamp_selection(&mut self) {
        if self.visible.is_empty() || self.visible.contains(&self.selected) {
            return;
        }
        self.select_first_visible();
    }

    /// Move the focus by one selectable row. Returns whether it moved.
    pub fn step(&mut self, delta: isize) -> bool {
        let position = self.visible.iter().position(|&r| r == self.selected);
        let mut cursor = match (position, delta) {
            (Some(p), _) => p as isize + delta,
            // Focus is hidden: resume from whichever end we are heading toward.
            (None, d) if d > 0 => 0,
            (None, _) => self.visible.len() as isize - 1,
        };
        while cursor >= 0 && (cursor as usize) < self.visible.len() {
            let row = self.visible[cursor as usize];
            if self.rows[row].is_setting() {
                let moved = self.selected != row;
                self.selected = row;
                return moved;
            }
            cursor += delta;
        }
        false
    }

    /// Focus the first (`delta < 0`) or last (`delta > 0`) selectable row.
    pub fn jump_end(&mut self, delta: isize) -> bool {
        let target = if delta > 0 {
            self.visible
                .iter()
                .rev()
                .find(|&&r| self.rows[r].is_setting())
        } else {
            self.visible.iter().find(|&&r| self.rows[r].is_setting())
        };
        match target {
            Some(&row) if row != self.selected => {
                self.selected = row;
                true
            }
            _ => false,
        }
    }

    /// Focus row index `row` when it is selectable.
    pub fn select_row(&mut self, row: usize) -> bool {
        match self.rows.get(row) {
            Some(r) if r.is_setting() && self.selected != row => {
                self.selected = row;
                true
            }
            _ => false,
        }
    }

    /// Focus a setting by key, switching to the tab that owns it.
    pub fn focus_key(&mut self, key: &str) -> bool {
        let Some(idx) = self.rows.iter().position(|r| row_key(r) == Some(key)) else {
            return false;
        };
        self.selected = idx;
        // Deep links can target any tab; bring it forward before clamping, or
        // the row is filtered out and the clamp snaps away from it.
        if let Some(category) = self.rows[idx].category(&self.registry)
            && let Some(tab) = self.tabs.iter().position(|t| *t == category)
        {
            self.active_tab = tab;
            self.window.active_tab = tab;
            self.section_focus = false;
            self.refresh_visible();
        }
        self.clamp_selection();
        true
    }

    pub fn set_active_tab(&mut self, tab: usize) -> bool {
        if tab >= self.tabs.len() || tab == self.active_tab {
            return false;
        }
        self.active_tab = tab;
        self.window.active_tab = tab;
        self.section_focus = false;
        self.scroll = 0;
        self.hover = None;
        // Tab switching is a browse gesture; it leaves any search behind.
        self.query.reset();
        self.mode = Mode::Browse;
        self.refresh_visible();
        self.select_first_visible();
        true
    }

    pub fn cycle_tab(&mut self, delta: isize) -> bool {
        if self.tabs.len() < 2 {
            return false;
        }
        let len = self.tabs.len() as isize;
        let next = (self.active_tab as isize + delta).rem_euclid(len) as usize;
        self.set_active_tab(next)
    }

    /// Point the tab bar at whichever tab owns the focused row. Used while
    /// searching, where results span tabs.
    pub fn sync_tab_to_selection(&mut self) -> bool {
        let Some(category) = self
            .rows
            .get(self.selected)
            .and_then(|r| r.category(&self.registry))
        else {
            return false;
        };
        let Some(tab) = self.tabs.iter().position(|t| *t == category) else {
            return false;
        };
        if tab == self.active_tab {
            return false;
        }
        self.active_tab = tab;
        self.window.active_tab = tab;
        true
    }

    /// Move the focus to the first row of the section `delta` steps away.
    pub fn jump_section(&mut self, delta: isize) -> bool {
        let sections = self.sections();
        if sections.len() < 2 {
            return false;
        }
        let len = sections.len() as isize;
        let next = (self.active_section() as isize + delta).rem_euclid(len) as usize;
        let target = sections[next].1;
        if target == self.selected {
            return false;
        }
        self.selected = target;
        true
    }

    /// Hand the keyboard to the section sidebar, if there is more than one
    /// section to hop between.
    pub fn toggle_section_focus(&mut self) -> bool {
        let engage = !self.section_focus && self.sections().len() >= 2;
        if engage == self.section_focus {
            return false;
        }
        self.section_focus = engage;
        true
    }

    // -- Mode transitions ---------------------------------------------------

    pub fn to_browse(&mut self) {
        self.mode = Mode::Browse;
        self.hover = None;
        self.choice_rects.clear();
        self.stepper_rects = (Rect::default(), Rect::default());
        self.close_on_picker_exit = false;
    }

    pub fn to_search(&mut self) {
        self.mode = Mode::Search;
    }

    /// Seed the query directly. Interactive editing goes through the line
    /// editor in `input`; this exists so tests can jump straight to a result
    /// set.
    #[cfg(test)]
    pub(super) fn set_query(&mut self, text: impl Into<String>) {
        self.query.set_text(text);
        self.refresh_visible();
        self.clamp_selection();
    }

    /// Open the chooser for an Enum / DynamicEnum row. Returns `false` for any
    /// other kind so callers can fall through.
    pub fn open_chooser(&mut self) -> bool {
        let Some((key, meta)) = self.focused() else {
            return false;
        };
        if self.lock(key).is_some() {
            return false;
        }
        // Side-model slots use the searchable /model picker, not this list.
        if super::actions::side_model_picker_action(key).is_some() {
            return false;
        }
        let supports_preview = match &meta.kind {
            SettingKind::Enum {
                supports_preview, ..
            }
            | SettingKind::DynamicEnum {
                supports_preview, ..
            } => *supports_preview,
            _ => return false,
        };
        let is_dynamic = matches!(meta.kind, SettingKind::DynamicEnum { .. });
        let choices = self.choices_for(key);
        let current = self.value_of(key);

        // A DynamicEnum value that no longer resolves (a renamed model, say)
        // must not land on the "(no override)" sentinel, or Enter would
        // silently wipe the user's preference.
        let unknown_fallback = usize::from(is_dynamic && choices.len() > 1);
        let index = match &current {
            Some(SettingValue::Enum(cur)) => choices
                .iter()
                .position(|c| c.canonical == *cur)
                .unwrap_or(0),
            Some(SettingValue::String(cur)) if !cur.is_empty() => choices
                .iter()
                .position(|c| c.canonical == *cur)
                .unwrap_or(unknown_fallback),
            _ => 0,
        };
        if is_dynamic && unknown_fallback != 0 && index == unknown_fallback {
            tracing::warn!(
                target: "settings",
                key,
                ?current,
                "DynamicEnum value no longer resolves in the live catalog — \
                 focusing the first real choice instead of the (no override) sentinel",
            );
        }
        let original = current.unwrap_or(match is_dynamic {
            true => SettingValue::String(
                choices
                    .first()
                    .map(|c| c.canonical.clone())
                    .unwrap_or_default(),
            ),
            false => SettingValue::Enum(match &meta.kind {
                SettingKind::Enum { choices, .. } => {
                    choices.first().map(|c| c.canonical).unwrap_or("")
                }
                _ => "",
            }),
        });
        self.mode = Mode::Picking {
            key,
            index,
            original,
            supports_preview,
            scroll: 0,
        };
        self.hover = None;
        true
    }

    /// Open the sub-sheet for a `Group` row.
    pub fn open_group(&mut self) -> bool {
        let Some((key, meta)) = self.focused() else {
            return false;
        };
        if !matches!(meta.kind, SettingKind::Group { .. }) {
            return false;
        }
        self.mode = Mode::PickingGroup { key, child: 0 };
        self.hover = None;
        true
    }

    /// Open the inline editor for a String / Int row.
    pub fn open_editor(&mut self) -> bool {
        let Some((key, meta)) = self.focused() else {
            return false;
        };
        let kind = meta.kind.clone();
        let value = self.value_of(key);
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
                let error = super::actions::validate_string(
                    validator,
                    editor.text(),
                    &self.pager_snapshot.available_models,
                );
                self.mode = Mode::EditingString {
                    key,
                    editor,
                    validator,
                    error,
                };
            }
            SettingKind::Int {
                default, min, max, ..
            } => {
                let buffer = match value {
                    Some(SettingValue::Int(v)) => v.to_string(),
                    _ => default.to_string(),
                };
                self.mode = Mode::EditingInt {
                    key,
                    buffer,
                    min,
                    max,
                };
            }
            _ => return false,
        }
        self.hover = None;
        true
    }

    /// Build the action that flips the focused Bool row. Logs and returns
    /// `None` on registry skew (caught by the dispatch-arm test).
    pub fn toggle_focused_bool(&self) -> Option<Action> {
        let (key, meta) = self.focused()?;
        if !matches!(meta.kind, SettingKind::Bool { .. }) {
            return None;
        }
        let current = match self.value_of(key) {
            Some(SettingValue::Bool(b)) => b,
            other => {
                tracing::error!(
                    target: "settings",
                    key,
                    ?other,
                    "Bool-kind setting did not resolve to a Bool — registry skew",
                );
                return None;
            }
        };
        super::actions::action_for_bool(key, !current)
    }

    /// Reset the focused row to its registered default.
    pub fn reset_action(&self, key: SettingKey) -> Option<Action> {
        match self.default_of(key)? {
            SettingValue::Bool(b) => super::actions::action_for_bool(key, b),
            SettingValue::Enum(canonical) => super::actions::action_for_enum_commit(key, canonical),
            SettingValue::String(text) => {
                super::actions::action_for_string(key, text, &self.pager_snapshot)
            }
            SettingValue::Int(v) => super::actions::action_for_int(key, v),
            SettingValue::PiBuiltinTools(_) => None,
        }
    }

    /// Reset hit-test geometry so mouse handlers degrade gracefully when a
    /// render is skipped. Hover survives — it is cleared on mode changes
    /// instead, to avoid per-frame flicker.
    pub fn reset_hit_rects(&mut self) {
        self.list_area = Rect::default();
        self.row_rects.clear();
        self.value_rects.clear();
        self.sidebar_rects.clear();
        self.choice_rects.clear();
        self.stepper_rects = (Rect::default(), Rect::default());
    }
}

fn row_key(row: &Row) -> Option<SettingKey> {
    match row {
        Row::Setting { key, .. } => Some(*key),
        _ => None,
    }
}

/// Whether `(key, canonical)` is gated off and must not be offered as a
/// choice. Pure (gates passed in) so it is unit-testable.
pub(super) fn choice_gated_off(
    key: SettingKey,
    canonical: &str,
    auto_mode_gate: bool,
    kitty_releases: bool,
) -> bool {
    (key == "permission_mode" && canonical == "auto" && !auto_mode_gate)
        || (key == "voice_capture_mode" && canonical == "hold" && !kitty_releases)
}

/// Whether a row is visible under the current runtime gates: voice rows need
/// voice mode, Hold-to-talk needs key releases, `hidden_in_minimal` rows drop
/// out of minimal mode, and `external_only` rows only exist under grok-pi.
pub(super) fn row_visible(
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

/// Build the full row table: per category, a tab heading followed by each
/// non-empty section's heading and its settings.
fn build_rows(registry: &SettingsRegistry) -> Vec<Row> {
    let kitty = crate::app::kitty_releases_reported();
    let minimal = crate::app::minimal_mode_active();
    let voice = crate::app::voice_mode_enabled();
    let external = crate::app::external_agent_active();

    // Group children render only inside their parent's sub-sheet.
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
    for category in SettingCategory::ALL {
        let members: Vec<(usize, &SettingMeta)> = registry
            .all()
            .iter()
            .enumerate()
            .filter(|(_, m)| m.category == *category)
            .filter(|(_, m)| row_visible(m, kitty, minimal, voice, external))
            .filter(|(_, m)| !group_children.contains(m.key))
            .collect();
        if members.is_empty() {
            continue;
        }
        rows.push(Row::Tab {
            category: *category,
        });
        // Sidebar order comes from the layout table; inside a section the
        // registry's declaration order is preserved.
        for section in layout::sections_for(*category) {
            let mut emitted = false;
            for (meta, m) in &members {
                if layout::section_for(m.key) != *section {
                    continue;
                }
                if !emitted {
                    rows.push(Row::Section {
                        category: *category,
                        name: section,
                    });
                    emitted = true;
                }
                rows.push(Row::Setting {
                    key: m.key,
                    meta: *meta,
                });
            }
        }
    }
    rows
}

fn build_tabs(rows: &[Row]) -> Vec<SettingCategory> {
    rows.iter()
        .filter_map(|row| match row {
            Row::Tab { category } => Some(*category),
            _ => None,
        })
        .collect()
}
