//! `/jump` picker: an overlay listing every turn in the conversation.
//!
//! Navigation is pure client-side over the scrollback timeline ([`crate::scrollback::state::TimelineEntry`]).
//! Moving the cursor live-scrolls the transcript to the hovered turn, Enter jumps there, and Esc restores the viewport the picker opened from.
//! Unlike `/rewind` nothing is fetched and nothing is mutated.
//!
//! Supports incremental search filtering (type to filter turns by preview
//! text) and `y` to copy the selected turn's preview to the clipboard.
//! Chrome, row geometry, and hit-testing come from [`crate::views::overlay_list::ListOverlay`] (shared with the rewind picker).
//! This module owns only the row content and input mapping.


use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::render::line_utils::truncate_str;
use crate::scrollback::entry::EntryId;
use crate::scrollback::state::{ScrollAnchor, TimelineEntry};
use crate::theme::Theme;
use crate::views::overlay_list::{ListOverlay, SearchLine};

/// Viewport snapshot captured when the picker opens, restored on Esc / failed jump.
/// The viewport is a width-stable [`ScrollAnchor`] bookmark, not a raw scroll offset (which clamps and drifts under a resize).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JumpRestore {
    pub(crate) bookmark: Option<ScrollAnchor>,
    pub selected: Option<usize>,
    pub follow_mode: bool,
}

/// What Enter does when a jump-style message list is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum JumpPurpose {
    /// `/jump`: scroll to the selected turn and close.
    #[default]
    Navigate,
    /// `/review-message`: open turn-scoped code review (live-scroll still applies).
    Review,
}

#[derive(Debug)]
pub struct JumpState {
    /// All timeline entries (unfiltered).
    pub all_entries: Vec<TimelineEntry>,
    /// Filtered entries currently displayed (indices into `all_entries`).
    pub filtered: Vec<usize>,
    pub selected: usize,
    pub restore: JumpRestore,
    /// Current search/filter query.
    pub query: String,
    pub purpose: JumpPurpose,
}

impl JumpState {
    pub fn new(entries: Vec<TimelineEntry>, selected: usize, restore: JumpRestore) -> Self {
        Self::with_purpose(entries, selected, restore, JumpPurpose::Navigate)
    }

    pub fn with_purpose(
        entries: Vec<TimelineEntry>,
        selected: usize,
        restore: JumpRestore,
        purpose: JumpPurpose,
    ) -> Self {
        let filtered: Vec<usize> = (0..entries.len()).collect();
        let selected = selected.min(filtered.len().saturating_sub(1));
        Self {
            all_entries: entries,
            filtered,
            selected,
            restore,
            query: String::new(),
            purpose,
        }
    }

    /// The entry currently under the cursor (from the filtered list).
    pub fn current_entry(&self) -> Option<&TimelineEntry> {
        self.filtered
            .get(self.selected)
            .and_then(|&idx| self.all_entries.get(idx))
    }

    fn list(&self) -> ListOverlay {
        ListOverlay {
            len: self.filtered.len(),
            selected: self.selected,
        }
    }

    /// Re-apply the filter query and reset cursor to top.
    pub fn apply_filter(&mut self) {
        let q = self.query.to_lowercase();
        if q.is_empty() {
            self.filtered = (0..self.all_entries.len()).collect();
        } else {
            self.filtered = self
                .all_entries
                .iter()
                .enumerate()
                .filter(|(_, e)| e.preview.to_lowercase().contains(&q))
                .map(|(i, _)| i)
                .collect();
        }
        self.selected = 0;
    }
}

pub enum JumpInput {
    Select(EntryId),
    Dismissed,
    MoveUp,
    MoveDown,
    /// Copy the selected entry's preview text.
    CopySelected(String),
    /// A character was typed — append to search query.
    SearchChar(char),
    /// Backspace in search query.
    SearchBackspace,
    Consumed,
}

pub fn handle_jump_key(state: &JumpState, key: &KeyEvent) -> JumpInput {
    if key.kind == crossterm::event::KeyEventKind::Release {
        return JumpInput::Consumed;
    }
    // Ctrl+C always dismisses.
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return JumpInput::Dismissed;
    }
    match key.code {
        KeyCode::Down => JumpInput::MoveDown,
        KeyCode::Up => JumpInput::MoveUp,
        // j/k only navigate when query is empty (otherwise they type into search).
        KeyCode::Char('j') if state.query.is_empty() => JumpInput::MoveDown,
        KeyCode::Char('k') if state.query.is_empty() => JumpInput::MoveUp,
        KeyCode::Enter => jump_activate(state),
        KeyCode::Esc => JumpInput::Dismissed,
        // y copies the selected turn preview.
        KeyCode::Char('y') if state.query.is_empty() => {
            if let Some(entry) = state.current_entry() {
                JumpInput::CopySelected(entry.preview.clone())
            } else {
                JumpInput::Consumed
            }
        }
        KeyCode::Backspace => JumpInput::SearchBackspace,
        KeyCode::Char(c) => JumpInput::SearchChar(c),
        _ => JumpInput::Consumed,
    }
}

pub fn move_cursor(state: &mut JumpState, delta: i32) {
    if state.filtered.is_empty() {
        return;
    }
    let max = state.filtered.len() as i32 - 1;
    state.selected = (state.selected as i32 + delta).clamp(0, max) as usize;
}

pub fn set_jump_cursor(state: &mut JumpState, idx: usize) -> bool {
    if state.filtered.is_empty() {
        return false;
    }
    let new = idx.min(state.filtered.len() - 1);
    if state.selected != new {
        state.selected = new;
        true
    } else {
        false
    }
}

pub fn jump_activate(state: &JumpState) -> JumpInput {
    state
        .current_entry()
        .map(|entry| JumpInput::Select(entry.prompt_entry_id))
        .unwrap_or(JumpInput::Consumed)
}

pub fn jump_row_at(state: &JumpState, area: Rect, col: u16, row: u16) -> Option<usize> {
    state.list().row_at_with_search(area, col, row, true)
}

pub fn jump_overlay_height(state: &JumpState, screen_h: u16) -> u16 {
    state.list().height_with_search(screen_h, true)
}

/// Compact wall-clock time after the turn ordinal (`14:32`).
fn format_turn_time(created_at: Option<chrono::DateTime<chrono::Local>>) -> Option<String> {
    created_at.map(|ts| ts.format("%H:%M").to_string())
}

pub fn render_jump_overlay(buf: &mut Buffer, area: Rect, state: &JumpState, focused: bool) {
    let theme = Theme::current();
    let total = state.all_entries.len();
    let ord_width = total.to_string().len();
    // Fixed "HH:MM " gutter so preview columns stay aligned across rows.
    const TIME_GUTTER: usize = 6;

    let title = match state.purpose {
        JumpPurpose::Navigate if state.filtered.len() < total => {
            format!("Jump to which turn? ({}/{})", state.filtered.len(), total)
        }
        JumpPurpose::Navigate => "Jump to which turn?".to_string(),
        JumpPurpose::Review if state.filtered.len() < total => {
            format!("Review which turn? ({}/{})", state.filtered.len(), total)
        }
        JumpPurpose::Review => "Review which turn?  Enter=files  move=jump".to_string(),
    };

    let search = SearchLine {
        query: &state.query,
        placeholder: "type to filter…  y=copy  Esc=close",
        focused,
    };

    state
        .list()
        .render_with_search(buf, area, &title, focused, Some(search), |index, ctx| {
            let &real_idx = &state.filtered[index];
            let entry = &state.all_entries[real_idx];
            let ordinal = format!("{:>ord_width$} ", entry.turn_idx + 1);
            let time = format_turn_time(entry.created_at)
                .map(|t| format!("{t:<5} "))
                .unwrap_or_else(|| " ".repeat(TIME_GUTTER));
            let prefix_width = ord_width + 1 + TIME_GUTTER;
            let ord_style = Style::default().fg(theme.gray).bg(ctx.row_bg);
            let time_style = Style::default().fg(theme.gray_dim).bg(ctx.row_bg);
            let preview = if entry.preview.is_empty() {
                "(no preview)".to_string()
            } else {
                truncate_str(
                    &entry.preview,
                    ctx.content_width.saturating_sub(prefix_width as u16 + 2) as usize,
                )
            };
            let text_style = Style::default()
                .fg(theme.text_primary)
                .bg(ctx.row_bg)
                .add_modifier(if ctx.is_cursor {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                });
            Line::from(vec![
                Span::styled(ordinal, ord_style),
                Span::styled(time, time_style),
                Span::styled(preview, text_style),
            ])
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEventKind, KeyModifiers};

    fn state(turns: usize) -> JumpState {
        let entries: Vec<TimelineEntry> = (0..turns)
            .map(|turn_idx| TimelineEntry {
                turn_idx,
                prompt_entry_id: EntryId::new(turn_idx as u64),
                created_at: None,
                preview: format!("turn {turn_idx}"),
            })
            .collect();
        JumpState::new(
            entries,
            0,
            JumpRestore {
                bookmark: None,
                selected: None,
                follow_mode: false,
            },
        )
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::empty(),
            kind: KeyEventKind::Press,
            state: crossterm::event::KeyEventState::empty(),
        }
    }

    #[test]
    fn keyboard_maps_navigation_and_selection() {
        let state = state(3);
        assert!(matches!(
            handle_jump_key(&state, &key(KeyCode::Down)),
            JumpInput::MoveDown
        ));
        assert!(matches!(
            handle_jump_key(&state, &key(KeyCode::Char('k'))),
            JumpInput::MoveUp
        ));
        assert!(matches!(
            handle_jump_key(&state, &key(KeyCode::Enter)),
            JumpInput::Select(id) if id == EntryId::new(0)
        ));
        assert!(matches!(
            handle_jump_key(&state, &key(KeyCode::Esc)),
            JumpInput::Dismissed
        ));
    }

    #[test]
    fn cursor_is_clamped_to_available_turns() {
        let mut state = state(3);
        move_cursor(&mut state, 10);
        assert_eq!(state.selected, 2);
        move_cursor(&mut state, -10);
        assert_eq!(state.selected, 0);
        assert!(set_jump_cursor(&mut state, 2));
        assert!(!set_jump_cursor(&mut state, 99));
    }

    #[test]
    fn search_filters_entries() {
        let mut state = state(5);
        // entries: "turn 0", "turn 1", ..., "turn 4"
        state.query = "turn 3".to_string();
        state.apply_filter();
        assert_eq!(state.filtered.len(), 1);
        assert_eq!(state.all_entries[state.filtered[0]].turn_idx, 3);
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn search_empty_restores_all() {
        let mut state = state(5);
        state.query = "turn 3".to_string();
        state.apply_filter();
        assert_eq!(state.filtered.len(), 1);
        state.query.clear();
        state.apply_filter();
        assert_eq!(state.filtered.len(), 5);
    }

    #[test]
    fn y_copies_selected_preview() {
        let mut state = state(3);
        state.selected = 1;
        let result = handle_jump_key(&state, &key(KeyCode::Char('y')));
        assert!(matches!(result, JumpInput::CopySelected(text) if text == "turn 1"));
    }

    #[test]
    fn typing_char_produces_search_input() {
        let state = state(3);
        let result = handle_jump_key(&state, &key(KeyCode::Char('a')));
        assert!(matches!(result, JumpInput::SearchChar('a')));
    }

    #[test]
    fn j_navigates_when_query_empty_but_types_when_not() {
        let mut state = state(3);
        // Empty query: j navigates
        assert!(matches!(
            handle_jump_key(&state, &key(KeyCode::Char('j'))),
            JumpInput::MoveDown
        ));
        // Non-empty query: j types
        state.query = "x".to_string();
        assert!(matches!(
            handle_jump_key(&state, &key(KeyCode::Char('j'))),
            JumpInput::SearchChar('j')
        ));
    }
}
