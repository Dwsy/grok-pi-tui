//! Pi `/fork` message picker: prompt-area list overlay (same shell as `/jump`).

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::app::actions::PiForkMessage;
use crate::render::line_utils::truncate_str;
use crate::theme::Theme;
use crate::views::overlay_list::ListOverlay;

#[derive(Debug, Clone)]
pub struct ForkPickerState {
    pub messages: Vec<PiForkMessage>,
    pub selected: usize,
    pub to_dashboard: bool,
}

impl ForkPickerState {
    pub fn new(messages: Vec<PiForkMessage>) -> Self {
        Self::with_dashboard_target(messages, false)
    }

    pub fn with_dashboard_target(messages: Vec<PiForkMessage>, to_dashboard: bool) -> Self {
        let selected = messages.len().saturating_sub(1);
        Self {
            messages,
            selected,
            to_dashboard,
        }
    }

    fn list(&self) -> ListOverlay {
        ListOverlay {
            len: self.messages.len(),
            selected: self.selected,
        }
    }
}

pub enum ForkPickerInput {
    Select(String),
    Dismissed,
    MoveUp,
    MoveDown,
    Consumed,
}

pub fn handle_fork_picker_key(state: &ForkPickerState, key: &KeyEvent) -> ForkPickerInput {
    if key.kind == crossterm::event::KeyEventKind::Release {
        return ForkPickerInput::Consumed;
    }
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => ForkPickerInput::MoveDown,
        KeyCode::Char('k') | KeyCode::Up => ForkPickerInput::MoveUp,
        KeyCode::Enter => fork_picker_activate(state),
        KeyCode::Esc => ForkPickerInput::Dismissed,
        _ => ForkPickerInput::Consumed,
    }
}

pub fn move_cursor(state: &mut ForkPickerState, delta: i32) {
    if state.messages.is_empty() {
        return;
    }
    let max = state.messages.len() as i32 - 1;
    state.selected = (state.selected as i32 + delta).clamp(0, max) as usize;
}

pub fn set_fork_picker_cursor(state: &mut ForkPickerState, idx: usize) -> bool {
    if state.messages.is_empty() {
        return false;
    }
    let new = idx.min(state.messages.len() - 1);
    if state.selected != new {
        state.selected = new;
        true
    } else {
        false
    }
}

pub fn fork_picker_activate(state: &ForkPickerState) -> ForkPickerInput {
    state
        .messages
        .get(state.selected)
        .map(|message| ForkPickerInput::Select(message.entry_id.clone()))
        .unwrap_or(ForkPickerInput::Consumed)
}

pub fn fork_picker_row_at(
    state: &ForkPickerState,
    area: Rect,
    col: u16,
    row: u16,
) -> Option<usize> {
    state.list().row_at(area, col, row)
}

pub fn fork_picker_overlay_height(state: &ForkPickerState, screen_h: u16) -> u16 {
    state.list().height(screen_h)
}

pub fn render_fork_picker_overlay(
    buf: &mut Buffer,
    area: Rect,
    state: &ForkPickerState,
    focused: bool,
) {
    let theme = Theme::current();
    let ord_width = state.messages.len().max(1).to_string().len();

    state.list().render(
        buf,
        area,
        "Fork from which message?",
        focused,
        |index, ctx| {
            let message = &state.messages[index];
            let ordinal = format!("{:>ord_width$} ", index + 1);
            let ord_style = Style::default().fg(theme.gray).bg(ctx.row_bg);
            let preview = if message.text.trim().is_empty() {
                "(empty message)".to_string()
            } else {
                truncate_str(
                    &message
                        .text
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" "),
                    ctx.content_width.saturating_sub(ord_width as u16 + 3) as usize,
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
                Span::styled(preview, text_style),
            ])
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEventKind, KeyModifiers};

    fn state(n: usize) -> ForkPickerState {
        ForkPickerState {
            messages: (0..n)
                .map(|i| PiForkMessage {
                    entry_id: format!("e{i}"),
                    text: format!("message {i}"),
                })
                .collect(),
            selected: n.saturating_sub(1),
            to_dashboard: false,
        }
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
    fn defaults_to_most_recent_message() {
        let state = state(3);
        assert_eq!(state.selected, 2);
    }

    #[test]
    fn keyboard_maps_navigation_and_selection() {
        let state = state(3);
        assert!(matches!(
            handle_fork_picker_key(&state, &key(KeyCode::Down)),
            ForkPickerInput::MoveDown
        ));
        assert!(matches!(
            handle_fork_picker_key(&state, &key(KeyCode::Char('k'))),
            ForkPickerInput::MoveUp
        ));
        assert!(matches!(
            handle_fork_picker_key(&state, &key(KeyCode::Enter)),
            ForkPickerInput::Select(id) if id == "e2"
        ));
        assert!(matches!(
            handle_fork_picker_key(&state, &key(KeyCode::Esc)),
            ForkPickerInput::Dismissed
        ));
    }
}
