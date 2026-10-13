//! The clip route as the pager sees it: route report, interims, stop, `Transcribing`, the one final, and what
//! submit, Esc, a new press, and navigation do to an outstanding clip.

use super::*;
use xai_grok_voice::{VoiceCommand, VoiceEvent, VoiceRoute};

use super::super::voice::TRANSCRIBING_TOAST;
use crate::voice::{TRANSCRIPTION_TIMED_OUT_KEPT_TOAST, TRANSCRIPTION_TIMED_OUT_TOAST};

fn agent0() -> VoiceTarget {
    VoiceTarget::Agent(AgentId(0))
}

fn partial(interim: Option<&str>) -> Partial {
    interim.map_or(Partial::None, |text| Partial::Shown(text.to_owned()))
}

fn recording(route: Option<VoiceRoute>, interim: Option<&str>) -> VoiceState {
    VoiceState::Recording {
        hold: false,
        target: agent0(),
        partial: partial(interim),
        route,
    }
}

fn stopping(route: Option<VoiceRoute>, interim: Option<&str>) -> VoiceState {
    VoiceState::Stopping {
        target: agent0(),
        partial: partial(interim),
        route,
    }
}

fn transcribing(interim: Option<&str>) -> VoiceState {
    VoiceState::Transcribing {
        target: agent0(),
        partial: partial(interim),
    }
}

fn voice_app() -> (AppView, tokio::sync::mpsc::Receiver<VoiceCommand>) {
    let mut app = test_app_with_agent();
    app.apply_voice_mode_enabled(true);
    app.voice_ui_active = true;
    let (tx, rx) = tokio::sync::mpsc::channel(8);
    app.voice_cmd_tx = Some(tx);
    (app, rx)
}

fn event(app: &mut AppView, event: VoiceEvent) -> bool {
    crate::voice::handle_voice_event(app, event)
}

fn final_(text: &str) -> VoiceEvent {
    VoiceEvent::UtteranceFinal {
        text: text.to_owned(),
    }
}

fn interim(text: &str) -> VoiceEvent {
    VoiceEvent::InterimTranscript {
        text: text.to_owned(),
    }
}

fn started(route: VoiceRoute) -> VoiceEvent {
    VoiceEvent::CaptureStarted { route }
}

fn prompt_text(app: &AppView) -> String {
    app.agents
        .get(&AgentId(0))
        .unwrap()
        .prompt
        .text()
        .to_owned()
}

fn prompt_caret(app: &AppView) -> usize {
    app.agents.get(&AgentId(0)).unwrap().prompt.cursor()
}

fn toast(app: &AppView) -> Option<String> {
    app.agents
        .get(&AgentId(0))
        .unwrap()
        .toast
        .as_ref()
        .map(|(message, _)| message.clone())
}

fn set_prompt(app: &mut AppView, text: &str, caret: usize) {
    let p = &mut app.agents.get_mut(&AgentId(0)).unwrap().prompt;
    p.set_text(text);
    p.set_cursor(caret);
}

/// Paints the bound agent prompt as the app would this frame: (first row's text, whether the cell at `x` is italic,
/// the cell the terminal cursor is placed on).
fn render_prompt(app: &mut AppView, x: u16) -> (String, bool, Option<(u16, u16)>) {
    use crate::views::prompt_widget::{PromptStyle, VoicePromptOverlay};
    use ratatui::style::{Color, Modifier};
    use ratatui::{buffer::Buffer, layout::Rect};

    let overlay_text = app.voice_interim().map(str::to_owned);
    let style = PromptStyle {
        focused: true,
        show_prefix: false,
        vpad_top: 0,
        chrome: false,
        ..Default::default()
    };
    let area = Rect::new(0, 0, 40, 3);
    let mut buf = Buffer::empty(area);
    let overlay = overlay_text.as_deref().map(|interim| VoicePromptOverlay {
        interim: Some(interim),
        color: Color::Cyan,
    });
    let result = app
        .agents
        .get_mut(&AgentId(0))
        .unwrap()
        .prompt
        .draw(&mut buf, area, None, &style, None, overlay);
    let row: String = (0..area.width)
        .filter_map(|x| buf.cell((x, 0)).map(|c| c.symbol().to_string()))
        .collect();
    let italic = buf
        .cell((x, 0))
        .is_some_and(|c| c.style().add_modifier.contains(Modifier::ITALIC));
    (row.trim_end().to_owned(), italic, result.cursor_pos)
}

#[test]
fn clip_session_previews_interims_and_the_final_replaces_them() {
    let (mut app, mut rx) = voice_app();
    set_prompt(&mut app, "hello world", 5);
    app.voice_begin_recording(agent0(), false);
    let _ = rx.try_recv();
    event(&mut app, started(VoiceRoute::Clip));

    event(&mut app, interim("the"));
    let (row, italic, _) = render_prompt(&mut app, 6);
    assert_eq!(("hello the world", true), (row.as_str(), italic));
    assert!(
        !render_prompt(&mut app, 10).1,
        "the draft after the ghost stays non-italic while the interim shows"
    );
    event(&mut app, interim("there my"));
    let (row, italic, _) = render_prompt(&mut app, 6);
    assert_eq!(("hello there my world", true), (row.as_str(), italic));

    app.voice_stop_keeping_final();
    assert!(
        !app.voice_state.is_transcribing(),
        "a plain stop is not yet uploading"
    );
    assert!(matches!(rx.try_recv(), Ok(VoiceCommand::PttRelease)));
    assert!(event(&mut app, VoiceEvent::Transcribing));
    assert!(app.voice_state.is_transcribing());
    assert!(!app.voice_listening());
    assert_eq!(Some(agent0()), app.voice_recording_target());
    let (row, italic, _) = render_prompt(&mut app, 6);
    assert_eq!(
        ("hello there my world", true),
        (row.as_str(), italic),
        "the last interim stays until the final"
    );
    assert_eq!(
        "hello world",
        prompt_text(&app),
        "interims never touch the draft"
    );

    event(&mut app, final_("there my friend"));
    assert_eq!(
        VoiceState::Idle,
        app.voice_state,
        "the clip's one final ends the session"
    );
    assert_eq!(None, app.voice_interim());
    let (row, italic, _) = render_prompt(&mut app, 6);
    assert_eq!(
        ("hello there my friend world", false),
        (row.as_str(), italic)
    );
    assert_eq!("hello there my friend".len(), prompt_caret(&app));
    assert_eq!(None, toast(&app), "no credential toast on the clip route");
    assert!(
        !app.voice_state.blocks_new_capture(),
        "the next press starts a new recording"
    );
}

/// The cap closes the mic on the pipeline side, so `Transcribing` arrives while the pager still says `Recording`;
/// the session ends through the same final, with no release sent, and the notice is only a toast.
#[test]
fn clip_cap_closes_the_mic_and_the_notice_is_only_a_toast() {
    let (mut app, mut rx) = voice_app();
    app.voice_state = recording(Some(VoiceRoute::Clip), Some("partial"));

    event(&mut app, VoiceEvent::Transcribing);
    assert!(app.voice_state.is_transcribing());
    assert_eq!(Some("partial"), app.voice_interim());
    assert!(
        rx.try_recv().is_err(),
        "the pipeline already closed the mic; no release is sent"
    );

    event(&mut app, final_("the captured part"));
    assert_eq!(VoiceState::Idle, app.voice_state);
    app.voice_state = recording(None, None);
    event(
        &mut app,
        VoiceEvent::Notice {
            message: "Recording stopped at the 5-minute limit; the captured part was transcribed."
                .into(),
        },
    );
    assert_eq!("the captured part", prompt_text(&app));
    assert!(
        toast(&app).is_some_and(|m| m.contains("5-minute limit")),
        "{:?}",
        toast(&app)
    );
    assert!(app.voice_listening(), "a notice ends nothing");
    assert!(rx.try_recv().is_err(), "and releases nothing");
}

#[test]
fn transcribing_event_moves_only_a_clip_session() {
    let mut app = test_app_with_agent();
    for live in [
        recording(None, None),
        stopping(None, None),
        recording(Some(VoiceRoute::Streaming), Some("partial")),
        stopping(Some(VoiceRoute::Streaming), None),
        VoiceState::Idle,
        VoiceState::ColdStart {
            hold: false,
            target: agent0(),
        },
    ] {
        app.voice_state = live.clone();
        event(&mut app, VoiceEvent::Transcribing);
        assert_eq!(live, app.voice_state);
    }
}

/// While uploading, an error is the toast verbatim and ends the session; a partial that was on screen is kept in
/// the box as the best transcript there is (see `clip_error_commits_the_shown_partial_before_the_reset`).
#[test]
fn error_while_transcribing_is_the_toast_and_ends_the_session() {
    for (message, interim_text) in [
        ("Upgrade to Pro.", None),
        ("No speech was detected. Voice stopped.", Some("um")),
    ] {
        let mut app = test_app_with_agent();
        app.voice_state = transcribing(interim_text);
        if interim_text.is_some() {
            let (row, italic, _) = render_prompt(&mut app, 0);
            assert_eq!(("um", true), (row.as_str(), italic));
        }
        event(
            &mut app,
            VoiceEvent::Error {
                message: message.into(),
                hint: None,
            },
        );
        assert_eq!(VoiceState::Idle, app.voice_state, "{message}");
        assert_eq!(None, app.voice_interim());
        assert_eq!(Some(format!("Voice: {message}")), toast(&app));
        let (row, italic, _) = render_prompt(&mut app, 0);
        assert_eq!(
            (interim_text.unwrap_or(""), false),
            (row.as_str(), italic),
            "the shown partial stays, as plain text"
        );
    }
}

/// A clip whose final fails (timeout, 429, 5xx) keeps the partial that was on screen: it is committed into the bound
/// prompt beside the error toast instead of vanishing with the reset. Streaming committed on the way, so its partial
/// is dropped as before.
#[test]
fn clip_error_commits_the_shown_partial_before_the_reset() {
    for (state, kept) in [
        (
            recording(Some(VoiceRoute::Clip), Some("sixty seconds")),
            "sixty seconds",
        ),
        (
            stopping(Some(VoiceRoute::Clip), Some("sixty seconds")),
            "sixty seconds",
        ),
        (transcribing(Some("sixty seconds")), "sixty seconds"),
        (
            recording(Some(VoiceRoute::Streaming), Some("provisional")),
            "",
        ),
        (transcribing(None), ""),
    ] {
        let (mut app, mut rx) = voice_app();
        app.voice_state = state.clone();
        event(
            &mut app,
            VoiceEvent::Error {
                message: "Transcription failed. Try again in a moment.".into(),
                hint: None,
            },
        );
        assert_eq!(VoiceState::Idle, app.voice_state, "{state:?}");
        assert_eq!(kept, prompt_text(&app), "{state:?}");
        assert_eq!(None, app.voice_interim());
        assert_eq!(
            Some("Voice: Transcription failed. Try again in a moment.".to_owned()),
            toast(&app)
        );
        assert!(matches!(rx.try_recv(), Ok(VoiceCommand::Abort)));
    }
}

/// An outstanding clip whose final never arrives is given up on shortly after the pipeline's own backstop: the
/// shown partial is kept, the session reset, and the toast says which. The deadline is armed only by a blocking clip
/// state, does not move while it stays blocking, and is cleared when the final lands.
#[test]
fn outstanding_clip_is_given_up_on_after_the_deadline() {
    let limit = xai_grok_voice::FINAL_TIMEOUT + std::time::Duration::from_secs(15);
    for (interim_text, expected_toast) in [
        (Some("kept words"), TRANSCRIPTION_TIMED_OUT_KEPT_TOAST),
        (None, TRANSCRIPTION_TIMED_OUT_TOAST),
    ] {
        let (mut app, mut rx) = voice_app();
        let start = std::time::Instant::now();
        app.voice_begin_recording(agent0(), false);
        event(&mut app, started(VoiceRoute::Clip));
        assert_eq!(None, app.voice_clip_deadline, "recording does not block");
        app.voice_stop_keeping_final();
        let armed = app.voice_clip_deadline.expect("armed by the stop");
        assert!(armed >= start + limit);
        event(&mut app, VoiceEvent::Transcribing);
        assert_eq!(
            Some(armed),
            app.voice_clip_deadline,
            "not moved by the upload starting"
        );
        if let Some(text) = interim_text {
            event(&mut app, interim(text));
        }
        while rx.try_recv().is_ok() {}

        app.voice_expire_outstanding_clip(armed - std::time::Duration::from_secs(1));
        assert!(app.voice_state.is_transcribing(), "before the deadline");
        app.voice_expire_outstanding_clip(armed);
        assert_eq!(VoiceState::Idle, app.voice_state);
        assert_eq!(None, app.voice_clip_deadline);
        assert_eq!(interim_text.unwrap_or(""), prompt_text(&app));
        assert_eq!(Some(expected_toast.to_owned()), toast(&app));
        assert!(matches!(rx.try_recv(), Ok(VoiceCommand::Abort)));
    }

    let (mut app, _rx) = voice_app();
    app.voice_begin_recording(agent0(), false);
    event(&mut app, started(VoiceRoute::Streaming));
    app.voice_stop_keeping_final();
    assert_eq!(
        None, app.voice_clip_deadline,
        "a streaming stop is not waited on"
    );

    let (mut app, _rx) = voice_app();
    app.voice_state = transcribing(None);
    app.voice_mark_transcribing();
    event(&mut app, started(VoiceRoute::Clip));
    app.voice_state = stopping(Some(VoiceRoute::Clip), None);
    app.voice_set_route(VoiceRoute::Clip);
    assert!(
        app.voice_clip_deadline.is_some(),
        "a late route report arms it"
    );
    event(&mut app, VoiceEvent::Transcribing);
    event(&mut app, final_("done"));
    assert_eq!(None, app.voice_clip_deadline, "cleared by the final");
}

/// Submit by state. A shown partial (either route, recording or after the stop) is promoted into the sent text and
/// the session reset with `Abort`: a trailing final would repeat it. A clip that has shown nothing releases the mic
/// but keeps its target so its one final lands as the next draft, and an upload in progress is left to finish. A
/// session whose route is not reported yet is aborted, as every submit was before the clip route existed.
#[test]
fn submit_promotes_a_shown_partial_and_otherwise_keeps_a_clip_target() {
    type Setup = fn(&mut AppView);
    // (case, setup, text promoted into the send, state kept for the final)
    let cases: [(&str, Setup, Option<&str>, Option<VoiceState>); 9] = [
        (
            "streaming with interim",
            |app| app.voice_state = recording(Some(VoiceRoute::Streaming), Some("spoken")),
            Some("spoken"),
            None,
        ),
        (
            "route not reported yet",
            |app| app.voice_begin_recording(agent0(), false),
            None,
            None,
        ),
        (
            "stopped before the route report",
            |app| {
                app.voice_begin_recording(agent0(), false);
                app.voice_stop_keeping_final();
            },
            None,
            None,
        ),
        (
            "clip, no partial yet",
            |app| {
                app.voice_begin_recording(agent0(), false);
                event(app, started(VoiceRoute::Clip));
            },
            None,
            Some(stopping(Some(VoiceRoute::Clip), None)),
        ),
        (
            "uploading, no partial",
            |app| app.voice_state = transcribing(None),
            None,
            Some(transcribing(None)),
        ),
        (
            "clip with a shown partial",
            |app| {
                app.voice_begin_recording(agent0(), false);
                event(app, started(VoiceRoute::Clip));
                event(app, interim("ship it"));
            },
            Some("ship it"),
            None,
        ),
        (
            "clip stopped, partial shown",
            |app| app.voice_state = stopping(Some(VoiceRoute::Clip), Some("ship it")),
            Some("ship it"),
            None,
        ),
        (
            "uploading, partial shown",
            |app| app.voice_state = transcribing(Some("ship it")),
            Some("ship it"),
            None,
        ),
        (
            "clip partial already promoted by the Enter key path",
            |app| {
                app.voice_begin_recording(agent0(), false);
                event(app, started(VoiceRoute::Clip));
                event(app, interim("ship it"));
                assert!(crate::voice::commit_interim_into_prompt(app).is_some());
                assert_eq!(None, app.voice_interim());
            },
            None,
            None,
        ),
    ];
    for (case, setup, promoted, kept) in cases {
        let (mut app, mut rx) = voice_app();
        setup(&mut app);
        while rx.try_recv().is_ok() {}
        assert_eq!(kept.is_some(), app.voice_state.owes_final(), "{case}");

        let effects = dispatch(Action::SendPrompt(prompt_text(&app)), &mut app);
        let Some(Effect::SendPrompt { text, .. }) = effects.first() else {
            panic!("{case}: expected SendPrompt, got {effects:?}");
        };
        if let Some(promoted) = promoted {
            assert_eq!(promoted, text, "{case}");
        }
        match &kept {
            Some(state) => {
                assert_eq!(*state, app.voice_state, "{case}");
                if matches!(state, VoiceState::Stopping { .. }) {
                    assert!(
                        matches!(rx.try_recv(), Ok(VoiceCommand::PttRelease)),
                        "{case}"
                    );
                    event(&mut app, VoiceEvent::Transcribing);
                }
                event(&mut app, final_("late final"));
                assert_eq!(
                    "late final",
                    prompt_text(&app),
                    "{case}: the final is the next draft"
                );
            }
            None => {
                assert_eq!(VoiceState::Idle, app.voice_state, "{case}");
                assert!(matches!(rx.try_recv(), Ok(VoiceCommand::Abort)), "{case}");
                event(&mut app, final_("late final"));
                assert_eq!("", prompt_text(&app), "{case}: nothing is owed");
            }
        }
    }
}

/// A press while a clip is outstanding (stopped on the clip route, or uploading) would abort the reader and lose the
/// recording, so toggle, enable, and stop change nothing and say so once in a toast.
#[test]
fn press_while_a_clip_is_outstanding_is_refused_with_a_toast() {
    for waiting in [
        stopping(Some(VoiceRoute::Clip), None),
        transcribing(None),
        transcribing(Some("words")),
    ] {
        let (mut app, mut rx) = voice_app();
        app.voice_state = waiting.clone();
        for action in [
            Action::EnableVoiceMode,
            Action::VoiceStop,
            Action::VoiceToggle,
        ] {
            assert!(dispatch(action, &mut app).is_empty(), "{waiting:?}");
        }
        assert_eq!(waiting, app.voice_state);
        assert_eq!(
            Some(TRANSCRIBING_TOAST.to_owned()),
            toast(&app),
            "{waiting:?}"
        );
        assert!(
            rx.try_recv().is_err(),
            "no command reaches the pipeline: {waiting:?}"
        );
    }
}

/// A streaming stop, or a stop the pipeline never classified (a quick tap), is not outstanding: the same press
/// starts a new recording.
#[test]
fn press_after_a_streaming_or_unclassified_stop_starts_a_new_recording() {
    if !xai_grok_voice::AUDIO_SUPPORTED {
        return;
    }
    for waiting in [
        stopping(Some(VoiceRoute::Streaming), None),
        stopping(None, None),
    ] {
        let (mut app, mut rx) = voice_app();
        if waiting == stopping(None, None) {
            // The real shape of a tap: hold-press, release before the route report
            app.voice_begin_recording(agent0(), true);
            app.voice_hold_release();
            assert_eq!(waiting, app.voice_state);
            while rx.try_recv().is_ok() {}
        } else {
            app.voice_state = waiting.clone();
        }
        dispatch(Action::VoiceToggle, &mut app);
        assert!(app.voice_listening(), "{waiting:?}");
        assert!(matches!(rx.try_recv(), Ok(VoiceCommand::PttPress { .. })));
        assert_eq!(None, toast(&app));
    }
}

#[test]
fn capture_cancelled_ends_only_an_unclassified_stop() {
    for (live, ends) in [
        (stopping(None, None), true),
        (recording(None, None), false),
        (recording(Some(VoiceRoute::Clip), None), false),
        (stopping(Some(VoiceRoute::Streaming), None), false),
        (transcribing(None), false),
    ] {
        let (mut app, mut rx) = voice_app();
        app.voice_state = live.clone();
        event(&mut app, VoiceEvent::CaptureCancelled);
        assert_eq!(
            if ends { VoiceState::Idle } else { live.clone() },
            app.voice_state,
            "{live:?}"
        );
        assert!(rx.try_recv().is_err(), "{live:?}: nothing to release");
        assert_eq!(None, toast(&app), "a tap is not an error");
    }
}

#[test]
fn session_ids_gate_events_and_reset_aborts() {
    use xai_grok_voice::TaggedVoiceEvent;
    let (mut app, mut rx) = voice_app();

    app.voice_begin_recording(agent0(), false);
    let first = app.voice_session;
    assert!(matches!(rx.try_recv(), Ok(VoiceCommand::PttPress { session }) if session == first));
    for waiting in [
        recording(None, None),
        stopping(Some(VoiceRoute::Clip), None),
        transcribing(None),
    ] {
        app.voice_state = waiting;
        app.voice_reset();
        assert!(matches!(rx.try_recv(), Ok(VoiceCommand::Abort)));
    }

    app.voice_begin_recording(agent0(), false);
    let current = app.voice_session;
    assert_ne!(first, current);
    let live = app.voice_state.clone();
    for stale_event in [
        started(VoiceRoute::Clip),
        VoiceEvent::Transcribing,
        final_("old clip"),
        interim("old partial"),
        VoiceEvent::CaptureCancelled,
        VoiceEvent::Error {
            message: "old failure".into(),
            hint: None,
        },
    ] {
        let tagged = TaggedVoiceEvent {
            session: first,
            event: stale_event.clone(),
        };
        assert!(
            !crate::voice::handle_tagged_voice_event(&mut app, tagged),
            "{stale_event:?}"
        );
        assert_eq!(live, app.voice_state, "{stale_event:?}");
    }
    assert_eq!(("".to_owned(), None), (prompt_text(&app), toast(&app)));
    assert!(crate::voice::handle_tagged_voice_event(
        &mut app,
        TaggedVoiceEvent {
            session: first,
            event: VoiceEvent::Notice {
                message: "limit".into()
            },
        },
    ));
    assert!(toast(&app).is_some());
    assert!(crate::voice::handle_tagged_voice_event(
        &mut app,
        TaggedVoiceEvent {
            session: current,
            event: interim("current"),
        },
    ));
    assert_eq!(Some("current"), app.voice_interim());
}

/// Pins the rendered cursor cell while the interim shows and the caret index after the final, on both routes.
#[test]
fn caret_follows_the_end_of_the_dictated_words_on_both_routes() {
    // (draft, caret, edit while recording, cursor column with "there" shown, with "there my friend" shown,
    //  text after "there my friend" lands, caret index)
    type Case = (
        &'static str,
        usize,
        Option<(&'static str, usize)>,
        u16,
        u16,
        &'static str,
        usize,
    );
    let cases: [Case; 5] = [
        ("", 0, None, 5, 15, "there my friend", 15),
        (
            "hello world",
            6,
            None,
            11,
            21,
            "hello there my friend world",
            21,
        ),
        (
            "hello",
            5,
            Some(("hello!", 6)),
            12,
            22,
            "hello! there my friend",
            22,
        ),
        (
            "world",
            5,
            Some(("world", 0)),
            5,
            15,
            "there my friend world",
            15,
        ),
        // Five double-width glyphs (10 cells, 15 bytes) plus the added space
        (
            "こんにちは",
            15,
            None,
            16,
            26,
            "こんにちは there my friend",
            31,
        ),
    ];
    for route in [VoiceRoute::Streaming, VoiceRoute::Clip] {
        for (draft, caret, edit, col_there, col_all, expected_text, expected_caret) in cases {
            let (mut app, _rx) = voice_app();
            set_prompt(&mut app, draft, caret);
            app.voice_begin_recording(agent0(), false);
            event(&mut app, started(route));
            if let Some((text, caret)) = edit {
                set_prompt(&mut app, text, caret);
            }
            let (draft_now, caret_now) = (prompt_text(&app), prompt_caret(&app));

            for (words, col) in [("there", col_there), ("there my friend", col_all)] {
                event(&mut app, interim(words));
                assert_eq!(
                    Some((col, 0)),
                    render_prompt(&mut app, 0).2,
                    "{route:?} {draft:?}: the cursor cell follows the end of the interim {words:?}"
                );
                assert_eq!(
                    (draft_now.as_str(), caret_now),
                    (prompt_text(&app).as_str(), prompt_caret(&app)),
                    "interims never move the draft or caret"
                );
            }

            event(&mut app, final_("there my friend"));
            assert_eq!(
                (expected_text, expected_caret),
                (prompt_text(&app).as_str(), prompt_caret(&app)),
                "{route:?} {draft:?}"
            );
            assert_eq!(
                Some((col_all, 0)),
                render_prompt(&mut app, 0).2,
                "{route:?} {draft:?}: cursor cell after the final"
            );
        }
    }
}

/// A ghost that reaches the row edge never hides the caret: mid-text it stays on the row's last cell; on a blank
/// draft a full row wraps it onto the next row, as the textarea does for typed text.
#[test]
fn caret_stays_visible_when_the_interim_fills_the_row() {
    let (mut app, _rx) = voice_app();
    app.voice_begin_recording(agent0(), false);
    // The prompt is 40 wide and 3 rows tall
    event(&mut app, interim(&"x".repeat(40)));
    assert_eq!(
        Some((0, 1)),
        render_prompt(&mut app, 0).2,
        "blank draft, full first row: wrapped onto the next row"
    );
    let three_full_rows = format!("{} {} {}", "x".repeat(40), "y".repeat(40), "z".repeat(40));
    event(&mut app, interim(&three_full_rows));
    assert_eq!(
        Some((39, 2)),
        render_prompt(&mut app, 0).2,
        "blank draft, every row full: the last cell"
    );

    set_prompt(&mut app, "hello", 5);
    event(&mut app, interim(&"x".repeat(60)));
    assert_eq!(
        Some((39, 0)),
        render_prompt(&mut app, 0).2,
        "mid-text, truncated at the edge: the last cell"
    );
}

/// Both routes abandon a recording when the user navigates away from its box, and a clip's outstanding upload too,
/// so the new surface is never blocked by a final headed for a hidden box. A streaming stop (its trailing final is
/// sub-second and its words already previewed) is left to land.
#[test]
fn navigate_away_abandons_a_recording_and_an_outstanding_clip() {
    let (mut app, mut rx) = voice_app();
    for (state, abandoned) in [
        (recording(Some(VoiceRoute::Streaming), None), true),
        (recording(Some(VoiceRoute::Clip), None), true),
        (transcribing(None), true),
        (stopping(Some(VoiceRoute::Streaming), None), false),
    ] {
        app.active_view = ActiveView::Agent(AgentId(0));
        app.voice_state = state.clone();
        app.enforce_voice_session_bound();
        assert_eq!(state, app.voice_state, "still on the bound surface");

        app.active_view = ActiveView::AgentDashboard;
        app.enforce_voice_session_bound();
        assert_eq!(
            if abandoned {
                VoiceState::Idle
            } else {
                state.clone()
            },
            app.voice_state,
            "{state:?}"
        );
        assert_eq!(
            abandoned,
            matches!(rx.try_recv(), Ok(VoiceCommand::Abort)),
            "{state:?}"
        );
    }
}
