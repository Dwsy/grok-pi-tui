use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use xai_grok_pager::app::app_view::AppView;

#[test]
fn shifted_letter_presses_remain_compatible_with_pi_literal_actions() {
    for letter in ['a', 's', 'r', 'q'] {
        let upper = letter.to_ascii_uppercase().to_string();
        for code in [letter, letter.to_ascii_uppercase()] {
            assert_eq!(
                AppView::remote_tui_key_sequence(&KeyEvent::new(
                    KeyCode::Char(code),
                    KeyModifiers::SHIFT
                )),
                Some(upper.clone()),
            );
        }
        assert_eq!(
            AppView::remote_tui_key_sequence(&KeyEvent::new(
                KeyCode::Char(letter),
                KeyModifiers::NONE
            )),
            Some(letter.to_string())
        );
    }
}

#[test]
fn modified_presses_are_encoded_instead_of_dropped() {
    for (code, modifiers, expected) in [
        (KeyCode::Char('s'), KeyModifiers::CONTROL, "\x1b[115;5u"),
        (KeyCode::Char('s'), KeyModifiers::ALT, "\x1b[115;3u"),
        (KeyCode::Char('s'), KeyModifiers::SUPER, "\x1b[115;9u"),
        (
            KeyCode::Char('c'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            "\x1b[99;6u",
        ),
        (KeyCode::Tab, KeyModifiers::SHIFT, "\x1b[9;2u"),
        (KeyCode::Left, KeyModifiers::CONTROL, "\x1b[1;5D"),
        (KeyCode::BackTab, KeyModifiers::NONE, "\x1b[Z"),
    ] {
        assert_eq!(
            AppView::remote_tui_key_sequence(&KeyEvent::new(code, modifiers)).as_deref(),
            Some(expected)
        );
    }
}

#[test]
fn repeat_release_and_legacy_control_events_keep_their_semantics() {
    for (kind, expected) in [
        (KeyEventKind::Repeat, "\x1b[115;2:2u"),
        (KeyEventKind::Release, "\x1b[115;2:3u"),
    ] {
        let mut key = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::SHIFT);
        key.kind = kind;
        assert_eq!(
            AppView::remote_tui_key_sequence(&key).as_deref(),
            Some(expected)
        );
    }
    assert_eq!(
        AppView::remote_tui_key_sequence(&KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL))
            .as_deref(),
        Some("\x03")
    );
    assert_eq!(
        AppView::remote_tui_key_sequence(&KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL))
            .as_deref(),
        Some("\x04")
    );
}
