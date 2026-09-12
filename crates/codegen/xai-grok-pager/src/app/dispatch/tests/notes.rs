//! Tests for feedback / remember / btw / recap dispatchers.

use super::*;
use crate::app::dispatch::ctx::NO_SESSION_NOTICE;
use crate::app::dispatch::{recap_unavailable_toast, scrollback_has_user_messages};

fn send_minimal_btw(app: &mut AppView, question: &str) -> uuid::Uuid {
    match dispatch(Action::SendBtw(question.into()), app).as_slice() {
        [
            Effect::SendBtw {
                minimal_request_id: Some(id),
                ..
            },
        ] => *id,
        other => panic!("expected correlated minimal /btw effect, got {other:?}"),
    }
}

fn esc() -> crossterm::event::Event {
    crossterm::event::Event::Key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Esc,
        crossterm::event::KeyModifiers::NONE,
    ))
}

#[test]
fn remember_save_carries_the_session_pinned_mode() {
    let mut app = test_app_with_agent();
    let id = AgentId(0);
    dispatch(
        Action::TaskComplete(TaskResult::WithPinnedMemoryMode {
            agent_id: id,
            memory_mode: Some(xai_grok_shell::config::MemoryMode::V2),
            result: Box::new(TaskResult::SessionCreated {
                agent_id: id,
                session_id: acp::SessionId::new("pinned-v2"),
                models: None,
            }),
        }),
        &mut app,
    );

    let rewrite = dispatch(
        Action::SendRememberNote("keep this in v2".to_owned()),
        &mut app,
    );
    assert!(matches!(
        rewrite.as_slice(),
        [Effect::RewriteMemoryNote { .. }]
    ));

    // A later disk-config flip cannot affect the effect: it owns the mode
    // returned by the active session when it was created.
    let save = dispatch(Action::SaveRememberNoteFromModal, &mut app);
    assert!(matches!(
        save.as_slice(),
        [Effect::SaveMemoryNote {
            pinned_mode: Some(xai_grok_shell::config::MemoryMode::V2),
            ..
        }]
    ));
}

#[test]
fn remember_save_without_session_defers_to_disk_mode() {
    let mut app = test_app_with_agent();
    let id = AgentId(0);
    app.agents.get_mut(&id).unwrap().session.session_id = None;

    assert!(
        dispatch(
            Action::SendRememberNote("pre-session note".to_owned()),
            &mut app,
        )
        .is_empty()
    );
    let save = dispatch(Action::SaveRememberNoteFromModal, &mut app);
    assert!(matches!(
        save.as_slice(),
        [Effect::SaveMemoryNote {
            pinned_mode: None,
            ..
        }]
    ));
}

#[test]
fn recap_unavailable_toast_empty_vs_with_messages() {
    assert_eq!(recap_unavailable_toast(false), "No messages yet");
    assert_eq!(recap_unavailable_toast(true), "Couldn't generate recap");
}

#[test]
fn manual_recap_with_no_messages_toasts_empty_state_and_skips_request() {
    let mut app = test_app_with_agent();
    app.session_recap_available = true;
    let id = AgentId(0);
    {
        let agent = app.agents.get_mut(&id).unwrap();
        agent.prompt.set_text("/recap");
        assert!(!scrollback_has_user_messages(&agent.scrollback));
    }

    let effects = dispatch(
        Action::SendRecap {
            auto: false,
            custom_instructions: None,
        },
        &mut app,
    );

    assert!(
        effects.is_empty(),
        "empty session must not fire x.ai/recap: {effects:?}"
    );
    let agent = app.agents.get(&id).unwrap();
    assert!(agent.pending_recap_entry.is_none(), "no loading spinner");
    assert_eq!(
        agent.toast.as_ref().map(|(s, _)| s.as_str()),
        Some("No messages yet"),
        "empty session should say No messages yet, not Couldn't generate recap"
    );
    assert_eq!(agent.prompt.text(), "", "slash command text is cleared");
}

#[test]
fn manual_recap_with_messages_requests_and_shows_spinner() {
    let mut app = test_app_with_agent();
    app.session_recap_available = true;
    app.current_ui.recap_model = "provider/recap-model".into();
    let id = AgentId(0);
    {
        let agent = app.agents.get_mut(&id).unwrap();
        agent
            .scrollback
            .push_block(RenderBlock::user_prompt("hello"));
        assert!(scrollback_has_user_messages(&agent.scrollback));
    }

    let effects = dispatch(
        Action::SendRecap {
            auto: false,
            custom_instructions: Some("focus on the API".into()),
        },
        &mut app,
    );

    assert!(
        matches!(
            effects.as_slice(),
            [Effect::SendRecap {
                auto: false,
                custom_instructions: Some(text),
                ..
            }] if text == "focus on the API"
        ),
        "expected SendRecap effect with focus, got {effects:?}"
    );
    let agent = app.agents.get(&id).unwrap();
    assert!(
        agent.pending_recap_entry.is_some(),
        "manual recap shows a loading spinner when there is something to summarize"
    );
    assert!(agent.toast.is_none());
}

/// Regression: during session/load, scrollback is batched so `turn_count()` stays 0 until `end_batch`, but UserPrompt entries may already be present.
/// Manual `/recap` must still request a recap.
#[test]
fn manual_recap_during_batch_load_with_prompts_still_requests() {
    let mut app = test_app_with_agent();
    app.session_recap_available = true;
    app.current_ui.recap_model = "provider/recap-model".into();
    let id = AgentId(0);
    {
        let agent = app.agents.get_mut(&id).unwrap();
        agent.scrollback.begin_batch();
        agent
            .scrollback
            .push_block(RenderBlock::user_prompt("hello from resume"));
        // Batched push defers rebuild_turns: the turn index is stale, the entries aren't
        assert_eq!(agent.scrollback.turn_count(), 0);
        assert!(scrollback_has_user_messages(&agent.scrollback));
    }

    let effects = dispatch(
        Action::SendRecap {
            auto: false,
            custom_instructions: None,
        },
        &mut app,
    );

    assert!(
        matches!(effects.as_slice(), [Effect::SendRecap { auto: false, .. }]),
        "batched resume with user prompts must still fire x.ai/recap: {effects:?}"
    );
    let agent = app.agents.get(&id).unwrap();
    assert!(agent.pending_recap_entry.is_some());
    assert!(agent.toast.is_none());
    // Clean up batch for the test fixture (not required for the assertion).
    app.agents.get_mut(&id).unwrap().scrollback.end_batch();
}

/// While session replay is still streaming, don't claim "No messages yet" even if scrollback looks empty; history may arrive on the next notification.
#[test]
fn manual_recap_while_loading_replay_still_requests() {
    let mut app = test_app_with_agent();
    app.session_recap_available = true;
    app.current_ui.recap_model = "provider/recap-model".into();
    let id = AgentId(0);
    {
        let agent = app.agents.get_mut(&id).unwrap();
        agent.session.loading_replay = true;
        assert!(!scrollback_has_user_messages(&agent.scrollback));
    }

    let effects = dispatch(
        Action::SendRecap {
            auto: false,
            custom_instructions: None,
        },
        &mut app,
    );

    assert!(
        matches!(effects.as_slice(), [Effect::SendRecap { auto: false, .. }]),
        "loading_replay must not short-circuit to No messages yet: {effects:?}"
    );
    let agent = app.agents.get(&id).unwrap();
    assert!(agent.pending_recap_entry.is_some());
    assert!(agent.toast.is_none());
}

#[test]
fn manual_recap_without_dedicated_model_skips_request_and_explains_configuration() {
    let mut app = test_app_with_agent();
    app.session_recap_available = true;
    let id = AgentId(0);
    app.agents
        .get_mut(&id)
        .unwrap()
        .scrollback
        .push_block(RenderBlock::user_prompt("hello"));

    let effects = dispatch(
        Action::SendRecap {
            auto: false,
            custom_instructions: None,
        },
        &mut app,
    );

    assert!(effects.is_empty());
    let agent = app.agents.get(&id).unwrap();
    assert!(agent.pending_recap_entry.is_none());
    assert_eq!(
        agent.toast.as_ref().map(|(message, _)| message.as_str()),
        Some("No active model selected")
    );
}

#[test]
fn manual_recap_uses_active_model_without_dedicated_override() {
    let mut app = test_app_with_agent();
    app.session_recap_available = true;
    let id = AgentId(0);
    {
        let agent = app.agents.get_mut(&id).unwrap();
        agent.session.models.current = Some(acp::ModelId::new("provider::active-model"));
        agent.session.models.reasoning_effort =
            Some(xai_grok_shell::sampling::types::ReasoningEffort::Medium);
        agent
            .scrollback
            .push_block(RenderBlock::user_prompt("hello"));
    }

    let effects = dispatch(
        Action::SendRecap {
            auto: false,
            custom_instructions: None,
        },
        &mut app,
    );

    assert!(matches!(
        effects.as_slice(),
        [Effect::SendRecap {
            auto: false,
            models,
            thinking_level: Some(level),
            custom_instructions: None,
            ..
        }] if models.as_slice() == ["provider::active-model"] && level == "medium"
    ));
}

#[test]
fn auto_recap_without_dedicated_model_is_silent() {
    let mut app = test_app_with_agent();
    app.session_recap_available = true;
    let id = AgentId(0);
    app.agents
        .get_mut(&id)
        .unwrap()
        .scrollback
        .push_block(RenderBlock::user_prompt("hello"));

    let effects = dispatch(
        Action::SendRecap {
            auto: true,
            custom_instructions: None,
        },
        &mut app,
    );

    assert!(effects.is_empty());
    assert!(app.agents[&id].toast.is_none());
}

#[test]
fn recap_request_transport_failure_with_no_turns_uses_empty_toast() {
    let mut app = test_app_with_agent();
    let id = AgentId(0);
    let session_id = app.agents[&id].session.session_id.clone().unwrap();
    {
        let agent = app.agents.get_mut(&id).unwrap();
        let spinner = agent
            .scrollback
            .push(crate::scrollback::entry::ScrollbackEntry::running(
                RenderBlock::session_event(SessionEvent::Recap {
                    summary: String::new(),
                    auto: false,
                }),
            ));
        agent.pending_recap_entry = Some(spinner);
        assert!(!scrollback_has_user_messages(&agent.scrollback));
    }

    dispatch(
        Action::TaskComplete(TaskResult::RecapRequested {
            session_id,
            auto: false,
            error: Some("transport down".into()),
        }),
        &mut app,
    );

    let agent = app.agents.get(&id).unwrap();
    assert!(agent.pending_recap_entry.is_none());
    assert_eq!(
        agent.toast.as_ref().map(|(s, _)| s.as_str()),
        Some("No messages yet")
    );
}

#[test]
fn recap_request_transport_failure_with_turns_uses_generic_toast() {
    let mut app = test_app_with_agent();
    let id = AgentId(0);
    let session_id = app.agents[&id].session.session_id.clone().unwrap();
    {
        let agent = app.agents.get_mut(&id).unwrap();
        agent
            .scrollback
            .push_block(RenderBlock::user_prompt("hello"));
        let spinner = agent
            .scrollback
            .push(crate::scrollback::entry::ScrollbackEntry::running(
                RenderBlock::session_event(SessionEvent::Recap {
                    summary: String::new(),
                    auto: false,
                }),
            ));
        agent.pending_recap_entry = Some(spinner);
        assert!(scrollback_has_user_messages(&agent.scrollback));
    }

    dispatch(
        Action::TaskComplete(TaskResult::RecapRequested {
            session_id,
            auto: false,
            error: Some("transport down".into()),
        }),
        &mut app,
    );

    let agent = app.agents.get(&id).unwrap();
    assert!(agent.pending_recap_entry.is_none());
    assert_eq!(
        agent.toast.as_ref().map(|(s, _)| s.as_str()),
        Some("Couldn't generate recap")
    );
}

#[test]
fn minimal_btw_response_after_esc_is_ignored() {
    let mut app = test_app_with_agent();
    app.screen_mode = crate::app::ScreenMode::Minimal;
    let id = AgentId(0);
    app.agents.get_mut(&id).unwrap().active_pane = crate::app::agent_view::AgentPane::Prompt;
    let request_id = send_minimal_btw(&mut app, "side question");

    let _ = app.handle_input(&esc());
    assert!(app.agents[&id].btw_state.is_none());

    dispatch(
        Action::TaskComplete(TaskResult::BtwResponse {
            agent_id: id,
            result: Ok("late".into()),
            minimal_request_id: Some(request_id),
        }),
        &mut app,
    );

    assert!(app.agents[&id].btw_state.is_none());
}

#[test]
fn minimal_done_dismisses_to_exactly_one_btw_block() {
    let mut app = test_app_with_agent();
    app.screen_mode = crate::app::ScreenMode::Minimal;
    let id = AgentId(0);
    app.agents.get_mut(&id).unwrap().active_pane = ActivePane::Prompt;
    let request_id = send_minimal_btw(&mut app, "original question");
    dispatch(
        Action::TaskComplete(TaskResult::BtwResponse {
            agent_id: id,
            result: Ok("original answer".into()),
            minimal_request_id: Some(request_id),
        }),
        &mut app,
    );

    let _ = app.handle_input(&esc());

    let btw_blocks: Vec<_> = app.agents[&id]
        .scrollback
        .iter_entries()
        .filter_map(|(_, entry)| match &entry.block {
            RenderBlock::Btw(block) => Some(block),
            _ => None,
        })
        .collect();
    assert_eq!(btw_blocks.len(), 1);
    assert_eq!(btw_blocks[0].question, "original question");
    assert_eq!(btw_blocks[0].content().text(), "original answer");
}

#[test]
fn minimal_btw_requests_stay_independent_across_two_agents() {
    let mut app = test_app_with_agent();
    app.screen_mode = crate::app::ScreenMode::Minimal;
    let first = AgentId(0);
    let second = AgentId(1);
    insert_placeholder_agent(&mut app, second);

    let first_old = send_minimal_btw(&mut app, "first old");
    let first_current = send_minimal_btw(&mut app, "first new");

    switch_to_agent(&mut app, second, SwitchCause::Picker);
    let second_request = send_minimal_btw(&mut app, "second");

    // Deliver the background first-agent responses while the second agent is active.
    dispatch(
        Action::TaskComplete(TaskResult::BtwResponse {
            agent_id: first,
            result: Ok("stale first answer".into()),
            minimal_request_id: Some(first_old),
        }),
        &mut app,
    );
    assert!(matches!(
        app.agents[&first].btw_state,
        Some(crate::views::btw_overlay::BtwOverlayState::Loading { ref question })
            if question == "first new"
    ));
    dispatch(
        Action::TaskComplete(TaskResult::BtwResponse {
            agent_id: first,
            result: Ok("current first answer".into()),
            minimal_request_id: Some(first_current),
        }),
        &mut app,
    );
    assert!(matches!(
        app.agents[&first].btw_state,
        Some(crate::views::btw_overlay::BtwOverlayState::Done { ref question, .. })
            if question == "first new"
    ));
    assert!(matches!(
        app.agents[&second].btw_state,
        Some(crate::views::btw_overlay::BtwOverlayState::Loading { ref question })
            if question == "second"
    ));

    // Dismiss the active second request, then its later response must be ignored.
    app.agents.get_mut(&second).unwrap().active_pane = ActivePane::Prompt;
    let _ = app.handle_input(&esc());
    dispatch(
        Action::TaskComplete(TaskResult::BtwResponse {
            agent_id: second,
            result: Ok("late second answer".into()),
            minimal_request_id: Some(second_request),
        }),
        &mut app,
    );
    assert!(app.agents[&second].btw_state.is_none());
    assert!(app.agents[&second].minimal_btw_lifecycle.is_none());
    assert!(matches!(
        app.agents[&first].btw_state,
        Some(crate::views::btw_overlay::BtwOverlayState::Done { ref question, .. })
            if question == "first new"
    ));

    // Reverse delivery order on fresh requests: active second completes first, then the background first response still resolves only the first panel
    switch_to_agent(&mut app, first, SwitchCause::Picker);
    let first_request = send_minimal_btw(&mut app, "first reverse");
    switch_to_agent(&mut app, second, SwitchCause::Picker);
    let second_request = send_minimal_btw(&mut app, "second reverse");
    dispatch(
        Action::TaskComplete(TaskResult::BtwResponse {
            agent_id: second,
            result: Ok("second reverse answer".into()),
            minimal_request_id: Some(second_request),
        }),
        &mut app,
    );
    dispatch(
        Action::TaskComplete(TaskResult::BtwResponse {
            agent_id: first,
            result: Ok("first reverse answer".into()),
            minimal_request_id: Some(first_request),
        }),
        &mut app,
    );
    assert!(matches!(
        app.agents[&second].btw_state,
        Some(crate::views::btw_overlay::BtwOverlayState::Done { ref question, .. })
            if question == "second reverse"
    ));
    assert!(matches!(
        app.agents[&first].btw_state,
        Some(crate::views::btw_overlay::BtwOverlayState::Done { ref question, .. })
            if question == "first reverse"
    ));
}

#[test]
fn fullscreen_btw_response_after_dismiss_keeps_existing_behavior() {
    let mut app = test_app_with_agent();
    let id = AgentId(0);
    let effects = dispatch(Action::SendBtw("side question".into()), &mut app);
    assert!(matches!(
        effects.as_slice(),
        [Effect::SendBtw {
            minimal_request_id: None,
            ..
        }]
    ));
    app.agents.get_mut(&id).unwrap().btw_state = None;

    dispatch(
        Action::TaskComplete(TaskResult::BtwResponse {
            agent_id: id,
            result: Ok("late".into()),
            minimal_request_id: None,
        }),
        &mut app,
    );

    assert!(matches!(
        app.agents[&id].btw_state,
        Some(crate::views::btw_overlay::BtwOverlayState::Done { ref question, .. })
            if question.is_empty()
    ));
}

#[test]
fn btw_no_session_feedback_is_mode_specific() {
    let id = AgentId(0);

    let mut minimal = test_app_with_agent();
    minimal.screen_mode = crate::app::ScreenMode::Minimal;
    minimal.agents.get_mut(&id).unwrap().session.session_id = None;
    assert!(dispatch(Action::SendBtw("q".into()), &mut minimal).is_empty());
    assert!(minimal.agents[&id].toast.is_none());
    assert!(last_system_text(&minimal, id).contains("No active session"));

    let mut fullscreen = test_app_with_agent();
    fullscreen.agents.get_mut(&id).unwrap().session.session_id = None;
    assert!(dispatch(Action::SendBtw("q".into()), &mut fullscreen).is_empty());
    assert_eq!(
        fullscreen.agents[&id]
            .toast
            .as_ref()
            .map(|(text, _)| text.as_str()),
        Some("No active session")
    );
    assert_eq!(fullscreen.agents[&id].scrollback.len(), 0);
}

/// A fresh install initializes before login, so the connection-time snapshot of the trace offer is `false`.
/// The authenticate meta must refresh it or the first post-login `/feedback` silently skips the consent question.
#[test]
fn auth_meta_refreshes_feedback_trace_offer() {
    let mut app = test_app_with_agent();
    app.shell_feedback_trace_offer = false; // initialize ran logged-out

    let meta = xai_grok_login::AuthMeta {
        feedback_trace_offer: true,
        coding_data_retention_opt_out: false,
        ..Default::default()
    };
    app.apply_auth_meta(&meta);
    // The modal reads the offer live at submit time, so refreshing the app-level flag is enough.
    assert!(app.feedback_trace_offer(), "login must refresh the offer");
}

/// A failed send reports the error and leaves the composer alone.
/// The shell persisted the report locally before the POST, so nothing is lost here.
#[test]
fn unknown_immediate_feedback_outcome_warns_against_duplicate_retry() {
    let id = AgentId(0);
    let mut app = test_app_with_agent();

    let _ = dispatch(
        Action::TaskComplete(crate::app::actions::TaskResult::FeedbackComplete {
            agent_id: id,
            origin: crate::app::actions::FeedbackSendOrigin::Immediate,
            outcome: xai_grok_shell::session::FeedbackOutcome::OutcomeUnknown,
            trace_upload_token: None,
        }),
        &mut app,
    );

    let notice = last_system_text(&app, id);
    assert!(notice.contains("may still complete"), "{notice}");
    assert!(notice.contains("do not resend"), "{notice}");
}

#[test]
fn feedback_failed_reports_the_error_and_spares_the_composer() {
    let id = AgentId(0);
    let mut app = test_app_with_agent();
    app.agents
        .get_mut(&id)
        .unwrap()
        .prompt
        .set_text("unrelated draft");

    let _ = dispatch(
        Action::TaskComplete(crate::app::actions::TaskResult::FeedbackFailed {
            agent_id: id,
            origin: crate::app::actions::FeedbackSendOrigin::Immediate,
            error: "disabled".into(),
        }),
        &mut app,
    );

    assert!(last_system_text(&app, id).contains("Couldn't send feedback"));
    assert_eq!(
        app.agents[&id].prompt.text(),
        "unrelated draft",
        "a failed report must not land in the composer, which sends to the model"
    );
}

/// A direct feedback action with no session says so instead of failing silently.
#[test]
fn send_feedback_without_a_session_says_so() {
    let id = AgentId(0);
    let mut app = test_app_with_agent();
    app.agents.get_mut(&id).unwrap().session.session_id = None;

    assert!(
        dispatch(
            Action::SendFeedback {
                text: "long report".into(),
                images: Default::default(),
                trace: None,
            },
            &mut app
        )
        .is_empty()
    );

    assert!(last_system_text(&app, id).contains("No active session"));
}

fn test_pasted_image(mime_type: &str) -> crate::prompt_images::PastedImage {
    crate::prompt_images::from_clipboard_data(&crate::clipboard::ImageData {
        data: vec![
            0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0, 0, 0, 0, 0,
        ],
        mime_type: mime_type.to_string(),
    })
}

fn test_pasted_png() -> crate::prompt_images::PastedImage {
    test_pasted_image("image/png")
}

fn sent_prompt_blocks(effects: &[Effect]) -> Option<&Vec<acp::ContentBlock>> {
    effects.iter().find_map(|effect| match effect {
        Effect::SendPromptBlocks { blocks, .. } => Some(blocks),
        _ => None,
    })
}

/// Plants the agent's session directory where the drain derives it: under the agent's own cwd, not `app.cwd`.
fn plant_agent_session(app: &mut AppView, id: AgentId, session_id: &str) -> std::path::PathBuf {
    let session_dir = plant_local_build_session(&app.agents[&id].session.cwd, session_id);
    app.agents.get_mut(&id).unwrap().session.session_id = Some(session_id.to_owned().into());
    session_dir
}

/// Types `/feedback <text>` plus one composer chip per mime and presses Enter while a turn runs, so the row queues.
fn queue_inline_feedback_with_images(app: &mut AppView, id: AgentId, mime_types: &[&str]) {
    {
        let agent = app.agents.get_mut(&id).unwrap();
        agent.session.state = crate::app::agent::AgentState::TurnRunning;
        agent.active_pane = crate::app::agent_view::AgentPane::Prompt;
        agent.prompt.set_text("/feedback broken thing ");
        let end = agent.prompt.text().len();
        agent.prompt.set_cursor(end);
        for mime_type in mime_types {
            agent
                .prompt
                .insert_image(test_pasted_image(mime_type))
                .expect("chip");
        }
    }
    let composed = app.agents[&id].prompt.text().to_string();
    let enter_effects = dispatch(Action::SendPrompt(composed), app);
    assert!(
        sent_prompt_blocks(&enter_effects).is_none(),
        "a running turn must not send yet: {enter_effects:?}"
    );
    assert_eq!(app.agents[&id].session.pending_prompts.len(), 1);
}

fn drain_idle(app: &mut AppView, id: AgentId) -> Vec<Effect> {
    app.agents.get_mut(&id).unwrap().session.state = crate::app::agent::AgentState::Idle;
    dispatch(Action::DrainQueue, app)
}

/// Enter only queues `/feedback <text>` as a skill row with the `pending` placeholder and its
/// composer image; the draft (text and image) is written when the row drains, and the wire text
/// then carries the real id.
#[test]
fn inline_feedback_writes_the_draft_when_the_row_drains() {
    let mut app = test_app_with_agent();
    let id = AgentId(0);
    let session_id = format!("feedback-drain-{}", uuid::Uuid::new_v4());
    let session_dir = plant_agent_session(&mut app, id, &session_id);
    let drafts_file = session_dir.join(xai_grok_feedback::FEEDBACK_DRAFTS_FILENAME);
    queue_inline_feedback_with_images(&mut app, id, &["image/png"]);
    let queued_wire = app.agents[&id]
        .session
        .pending_prompts
        .front()
        .and_then(|row| row.wire_blocks.clone());
    let drafts_written_on_enter = drafts_file.exists();

    let drain_effects = drain_idle(&mut app, id);
    let drafts = xai_grok_feedback::FeedbackDraftStore::new(&session_dir).list();
    let saved_images = drafts.as_ref().ok().and_then(|drafts| {
        drafts.first().map(|draft| {
            crate::app::dispatch::inline_feedback::read_feedback_draft_images(
                &session_dir,
                &draft.id,
            )
        })
    });
    let _ = std::fs::remove_dir_all(&session_dir);

    let Some(
        [
            acp::ContentBlock::Text(queued_text),
            acp::ContentBlock::Image(_),
        ],
    ) = queued_wire.as_deref()
    else {
        panic!("Enter must queue the instruction plus the composer image, got {queued_wire:?}");
    };
    assert!(queued_text.text.contains("pending"));
    assert!(
        !drafts_written_on_enter,
        "Enter must not touch the drafts file"
    );
    assert_eq!(app.agents[&id].prompt.text(), "");

    let drafts = drafts.expect("drafts file readable after the drain");
    let [draft] = drafts.as_slice() else {
        panic!("the drain writes exactly one predraft, got {drafts:?}");
    };
    assert_eq!(draft.details, "broken thing");
    assert_eq!(saved_images.map(|images| images.len()), Some(1));
    let sent = sent_prompt_blocks(&drain_effects)
        .unwrap_or_else(|| panic!("the drain sends the skill turn, got {drain_effects:?}"));
    let [
        acp::ContentBlock::Text(sent_text),
        acp::ContentBlock::Image(_),
    ] = sent.as_slice()
    else {
        panic!("the wire carries the instruction plus the image, got {sent:?}");
    };
    assert!(sent_text.text.contains(draft.id.as_str()));
    assert!(!sent_text.text.contains("pending"));
    assert!(app.agents[&id].session.pending_prompts.is_empty());
}

/// A draft that cannot be saved never reaches the model: the row is dropped, the error is shown,
/// and the typed command goes back into the composer.
#[test]
fn inline_feedback_save_failure_returns_the_command_to_the_composer() {
    let mut app = test_app_with_agent();
    let id = AgentId(0);
    let missing_session = format!("feedback-missing-{}", uuid::Uuid::new_v4());
    app.agents.get_mut(&id).unwrap().session.session_id = Some(missing_session.into());

    let effects = dispatch(
        Action::SendPrompt("/feedback todo is chopped".into()),
        &mut app,
    );

    assert!(
        effects.iter().all(|effect| !matches!(
            effect,
            Effect::SendPromptBlocks { .. } | Effect::SendPrompt { .. }
        )),
        "a failed save must not send: {effects:?}"
    );
    let agent = &app.agents[&id];
    assert!(agent.session.pending_prompts.is_empty());
    assert!(agent.session.state.is_idle());
    assert!(agent.session.in_flight_prompt.is_none());
    assert_eq!(agent.prompt.text(), "/feedback todo is chopped");
    assert!(last_system_text(&app, id).contains("No active session"));
}

/// Parity with the modal path: a composer chip the feedback API cannot carry (the composer accepts
/// webp/bmp/tiff) is dropped with a notice; the text is still saved and the skill turn still goes out
/// with the row's original wire blocks.
#[test]
fn inline_feedback_unsupported_image_drops_the_image_not_the_report() {
    let mut app = test_app_with_agent();
    let id = AgentId(0);
    let session_id = format!("feedback-webp-{}", uuid::Uuid::new_v4());
    let session_dir = plant_agent_session(&mut app, id, &session_id);
    queue_inline_feedback_with_images(&mut app, id, &["image/webp"]);

    let drain_effects = drain_idle(&mut app, id);
    let drafts = xai_grok_feedback::FeedbackDraftStore::new(&session_dir).list();
    let images_dir_written = drafts.as_ref().ok().and_then(|drafts| {
        drafts.first().map(|draft| {
            xai_grok_feedback::feedback_draft_images_dir(&session_dir, &draft.id).exists()
        })
    });
    let _ = std::fs::remove_dir_all(&session_dir);

    let drafts = drafts.expect("drafts file readable after the drain");
    let [draft] = drafts.as_slice() else {
        panic!("the text must still be saved when only the image is unsupported: {drafts:?}");
    };
    assert_eq!(draft.details, "broken thing");
    assert_eq!(images_dir_written, Some(false));
    let sent = sent_prompt_blocks(&drain_effects)
        .unwrap_or_else(|| panic!("the skill turn must still go out: {drain_effects:?}"));
    assert!(
        matches!(
            sent.as_slice(),
            [acp::ContentBlock::Text(_), acp::ContentBlock::Image(_)]
        ),
        "the wire blocks are never edited: {sent:?}"
    );
    let notice = last_system_text(&app, id);
    assert!(notice.contains("PNG, JPEG, or GIF only"), "{notice}");
    let scrollback = &app.agents[&id].scrollback;
    let bubble = &scrollback.get(scrollback.len() - 2).expect("bubble").block;
    assert!(
        matches!(bubble, RenderBlock::UserPrompt(prompt) if prompt.text == "/feedback broken thing"),
        "the notice follows the skill bubble, got {bubble:?}"
    );
}

/// The user keeps typing while the `/feedback` row waits behind a running turn; when its save then
/// fails the report goes back to the composer ahead of the draft, chips on both sides intact.
#[test]
fn inline_feedback_save_failure_prepends_the_report_to_a_nonempty_composer() {
    let mut app = test_app_with_agent();
    let id = AgentId(0);
    let missing_session = format!("feedback-missing-{}", uuid::Uuid::new_v4());
    app.agents.get_mut(&id).unwrap().session.session_id = Some(missing_session.into());
    queue_inline_feedback_with_images(&mut app, id, &["image/png"]);
    {
        let agent = app.agents.get_mut(&id).unwrap();
        agent.prompt.set_text("next question ");
        let end = agent.prompt.text().len();
        agent.prompt.set_cursor(end);
        agent.prompt.insert_image(test_pasted_png()).expect("chip");
    }
    let draft_chip_identity = app.agents[&id].prompt.images[0].preview.identity();

    let effects = drain_idle(&mut app, id);

    assert!(
        effects.iter().all(|effect| !matches!(
            effect,
            Effect::SendPromptBlocks { .. } | Effect::SendPrompt { .. }
        )),
        "a failed save must not send: {effects:?}"
    );
    let agent = &app.agents[&id];
    assert!(agent.session.pending_prompts.is_empty());
    assert_eq!(
        agent.prompt.text(),
        "/feedback broken thing [Image #2] \nnext question [Image #1] "
    );
    let [draft_chip, report_chip] = agent.prompt.images.as_slice() else {
        panic!(
            "both chips must survive the restore: {:?}",
            agent.prompt.images
        );
    };
    assert_eq!(draft_chip.preview.identity(), draft_chip_identity);
    assert_eq!(report_chip.mime_type, "image/png");
    assert!(last_system_text(&app, id).contains("No active session"));
}

/// A queued row being edited owns the composer, so the report is echoed in the notice instead of
/// being spliced into that row's edit buffer.
#[test]
fn inline_feedback_save_failure_while_editing_a_queued_row_echoes_the_report_in_the_notice() {
    let mut app = test_app_with_agent();
    let id = AgentId(0);
    let missing_session = format!("feedback-missing-{}", uuid::Uuid::new_v4());
    {
        let agent = app.agents.get_mut(&id).unwrap();
        agent.session.session_id = Some(missing_session.into());
        agent.session.state = crate::app::agent::AgentState::TurnRunning;
    }
    dispatch(
        Action::SendPrompt("/feedback todo is chopped".into()),
        &mut app,
    );
    let follower_id = app
        .agents
        .get_mut(&id)
        .unwrap()
        .session
        .enqueue_prompt("queued-2".into());
    {
        let agent = app.agents.get_mut(&id).unwrap();
        agent.prompt_mode = PromptMode::EditingQueued {
            id: follower_id,
            original: "queued-2".into(),
            server_id: None,
            kind: crate::app::agent::QueueEntryKind::Prompt,
        };
        agent.prompt.set_text("queued-2 edited");
    }

    let effects = drain_idle(&mut app, id);

    assert!(
        effects.iter().all(|effect| !matches!(
            effect,
            Effect::SendPromptBlocks { .. } | Effect::SendPrompt { .. }
        )),
        "a failed save must not send: {effects:?}"
    );
    let agent = &app.agents[&id];
    assert_eq!(agent.prompt.text(), "queued-2 edited");
    assert_eq!(
        agent
            .session
            .pending_prompts
            .iter()
            .map(|row| row.text.as_str())
            .collect::<Vec<_>>(),
        ["queued-2"]
    );
    let notice = last_system_text(&app, id);
    assert!(notice.contains("No active session"), "{notice}");
    assert!(notice.contains("/feedback todo is chopped"), "{notice}");
}

/// A `#` memory note in progress owns the composer too: splicing the report ahead of it would file
/// the report as a memory note on the next Enter.
#[test]
fn inline_feedback_save_failure_in_remember_mode_echoes_the_report_in_the_notice() {
    let mut app = test_app_with_agent();
    let id = AgentId(0);
    let missing_session = format!("feedback-missing-{}", uuid::Uuid::new_v4());
    {
        let agent = app.agents.get_mut(&id).unwrap();
        agent.session.session_id = Some(missing_session.into());
        agent.session.state = crate::app::agent::AgentState::TurnRunning;
    }
    dispatch(
        Action::SendPrompt("/feedback todo is chopped".into()),
        &mut app,
    );
    {
        let agent = app.agents.get_mut(&id).unwrap();
        agent.prompt_input_mode = crate::app::agent_view::PromptInputMode::Remember;
        agent.prompt.set_text("prefers tabs");
    }

    let effects = drain_idle(&mut app, id);

    assert!(
        effects.iter().all(|effect| !matches!(
            effect,
            Effect::SendPromptBlocks { .. } | Effect::SendPrompt { .. }
        )),
        "a failed save must not send: {effects:?}"
    );
    let agent = &app.agents[&id];
    assert_eq!(agent.prompt.text(), "prefers tabs");
    assert!(agent.session.pending_prompts.is_empty());
    let notice = last_system_text(&app, id);
    assert!(notice.contains("No active session"), "{notice}");
    assert!(notice.contains("/feedback todo is chopped"), "{notice}");
}

/// A store lock held elsewhere is transient: the row goes back to the front untouched and drains
/// on the next trigger once the lock is gone. The notice is shown once per blocked row.
#[test]
fn inline_feedback_busy_store_requeues_the_row_at_the_front() {
    let mut app = test_app_with_agent();
    let id = AgentId(0);
    let session_id = format!("feedback-busy-{}", uuid::Uuid::new_v4());
    let session_dir = plant_agent_session(&mut app, id, &session_id);
    let lock =
        std::fs::File::create(session_dir.join(xai_grok_feedback::FEEDBACK_DRAFTS_LOCK_FILENAME))
            .expect("lock file");
    lock.try_lock().expect("hold the store lock");
    queue_inline_feedback_with_images(&mut app, id, &["image/png"]);

    let busy_effects = drain_idle(&mut app, id);
    let busy_notice = last_system_text(&app, id);
    let retried_effects = drain_idle(&mut app, id);
    let notices_after_retry = {
        let scrollback = &app.agents[&id].scrollback;
        scrollback
            .entries_in_range(0..scrollback.len())
            .iter()
            .filter(|entry| matches!(&entry.block, RenderBlock::System(block) if block.text == busy_notice))
            .count()
    };
    let front = app.agents[&id].session.pending_prompts.front().cloned();
    lock.unlock().expect("release the store lock");
    let sent_effects = drain_idle(&mut app, id);
    let drafts = xai_grok_feedback::FeedbackDraftStore::new(&session_dir).list();
    let _ = std::fs::remove_dir_all(&session_dir);
    drop(lock);

    assert!(
        sent_prompt_blocks(&busy_effects).is_none()
            && sent_prompt_blocks(&retried_effects).is_none(),
        "a busy store must not send: {busy_effects:?} {retried_effects:?}"
    );
    assert!(busy_notice.contains("stays queued"), "{busy_notice}");
    assert_eq!(notices_after_retry, 1);
    let front = front.expect("the row waits at the front of the queue");
    assert_eq!(front.text, "/feedback broken thing");
    assert!(
        matches!(
            front.wire_blocks.as_deref(),
            Some([acp::ContentBlock::Text(_), acp::ContentBlock::Image(_)])
        ),
        "the requeued row keeps its original wire blocks: {:?}",
        front.wire_blocks
    );
    assert_eq!(app.agents[&id].prompt.text(), "");
    assert!(
        sent_prompt_blocks(&sent_effects).is_some(),
        "the row drains once the lock is released: {sent_effects:?}"
    );
    assert_eq!(drafts.expect("drafts readable").len(), 1);
    assert!(app.agents[&id].session.pending_prompts.is_empty());
}

/// The predraft carries at most as many images as a send can, so the modal never shows chips that
/// would be dropped on send. Asserted on disk: the reader caps too, so it could not tell 5 written from 4.
#[test]
fn inline_feedback_saves_at_most_four_images() {
    let mut app = test_app_with_agent();
    let id = AgentId(0);
    let session_id = format!("feedback-cap-{}", uuid::Uuid::new_v4());
    let session_dir = plant_agent_session(&mut app, id, &session_id);
    queue_inline_feedback_with_images(
        &mut app,
        id,
        &["image/png"; xai_grok_shell::session::MAX_FEEDBACK_IMAGES + 1],
    );

    let drain_effects = drain_idle(&mut app, id);
    let drafts = xai_grok_feedback::FeedbackDraftStore::new(&session_dir).list();
    let images_dir = drafts.as_ref().ok().and_then(|drafts| {
        drafts
            .first()
            .map(|draft| xai_grok_feedback::feedback_draft_images_dir(&session_dir, &draft.id))
    });
    let on_disk = images_dir.as_ref().map(|dir| {
        let manifest: Vec<serde_json::Value> = serde_json::from_slice(
            &std::fs::read(dir.join("metadata.json")).expect("manifest written"),
        )
        .expect("manifest is a JSON array");
        let files = std::fs::read_dir(dir).expect("images dir").count();
        (manifest.len(), files)
    });
    let _ = std::fs::remove_dir_all(&session_dir);

    assert_eq!(drafts.expect("drafts readable").len(), 1);
    let cap = xai_grok_shell::session::MAX_FEEDBACK_IMAGES;
    assert_eq!(
        on_disk,
        Some((cap, cap + 1)),
        "manifest entries, then files (images plus the manifest)"
    );
    let sent = sent_prompt_blocks(&drain_effects)
        .unwrap_or_else(|| panic!("the skill turn still goes out: {drain_effects:?}"));
    assert_eq!(
        sent.len(),
        cap + 2,
        "the wire keeps every composer image: {sent:?}"
    );
    let notice = last_system_text(&app, id);
    assert!(
        notice.contains(&format!("1 over the {cap}-image limit")),
        "{notice}"
    );
}

/// Typed bare `/feedback` must not drain composer images until the modal opens: every refusal
/// (minimal mode, voice owning the prompt, no session, a blocker) keeps the composer text, its
/// chip, and the staged temp file that FeedbackImages Drop would otherwise unlink, and is visible.
#[test]
fn refused_bare_feedback_keeps_the_composer_image_and_is_visible() {
    use crate::app::app_view::{VoiceState, VoiceTarget};
    use strum::IntoEnumIterator as _;

    #[derive(Debug, Clone, Copy, strum::EnumIter)]
    enum Refusal {
        Minimal,
        Voice,
        NoSession,
        Blocker,
    }

    let id = AgentId(0);
    let recording = VoiceState::Recording {
        hold: false,
        target: VoiceTarget::Agent(id),
        interim: Some("dictated text".to_owned()),
    };
    for refusal in Refusal::iter() {
        let mut app = test_app_with_agent();
        match refusal {
            Refusal::Minimal => app.screen_mode = crate::app::ScreenMode::Minimal,
            Refusal::Voice => app.voice_state = recording.clone(),
            Refusal::NoSession => app.agents.get_mut(&id).unwrap().session.session_id = None,
            Refusal::Blocker => {
                app.agents.get_mut(&id).unwrap().plan_approval_view =
                    Some(crate::app::agent_view::test_fixtures::make_plan_approval_view_state());
            }
        }
        {
            let agent = app.agents.get_mut(&id).unwrap();
            agent.prompt.set_text("/feedback ");
            let end = agent.prompt.text().len();
            agent.prompt.set_cursor(end);
            agent.prompt.insert_image(test_pasted_png()).expect("chip");
        }
        let composed = app.agents[&id].prompt.text().to_string();
        let image_identity = app.agents[&id].prompt.images[0].preview.identity();
        let dir = tempfile::tempdir().unwrap();
        let staged = dir.path().join("feedback.png");
        std::fs::write(&staged, b"staged").unwrap();
        app.agents.get_mut(&id).unwrap().prompt.images[0].staged_temp_path = Some(staged.clone());

        let effects = dispatch(Action::SendPrompt(composed.clone()), &mut app);

        assert!(effects.is_empty(), "{refusal:?}: {effects:?}");
        let agent = &app.agents[&id];
        assert!(agent.feedback_modal.is_none(), "{refusal:?}");
        assert_eq!(agent.prompt.text(), composed, "{refusal:?}");
        assert_eq!(agent.prompt.images.len(), 1, "{refusal:?}");
        assert_eq!(
            agent.prompt.images[0].preview.identity(),
            image_identity,
            "{refusal:?}"
        );
        assert!(
            staged.exists(),
            "{refusal:?}: the refusal must preserve the staged image"
        );
        match refusal {
            Refusal::Voice => {
                assert_eq!(
                    app.voice_state, recording,
                    "{refusal:?}: voice input keeps the prompt"
                );
            }
            Refusal::Blocker => {
                assert!(
                    agent.plan_approval_view.is_some(),
                    "{refusal:?}: the blocker is untouched"
                );
            }
            Refusal::Minimal | Refusal::NoSession => {}
        }
        let visible = match refusal {
            Refusal::Minimal => last_system_text(&app, id).contains("Use `/feedback <text>`"),
            Refusal::Voice | Refusal::NoSession | Refusal::Blocker => agent.toast.is_some(),
        };
        assert!(visible, "{refusal:?}: the refusal must be visible");
    }
}

/// An accepted typed `/feedback` moves composer images into the modal and clears the draft.
#[test]
fn typed_bare_feedback_moves_composer_images_into_the_modal() {
    let mut app = test_app_with_agent();
    let id = AgentId(0);
    {
        let agent = app.agents.get_mut(&id).unwrap();
        agent.prompt.set_text("/feedback ");
        let end = agent.prompt.text().len();
        agent.prompt.set_cursor(end);
        agent.prompt.insert_image(test_pasted_png()).expect("chip");
    }

    let effects = dispatch(
        Action::SendPrompt(app.agents[&id].prompt.text().to_string()),
        &mut app,
    );

    assert!(
        effects
            .iter()
            .all(|effect| !matches!(effect, Effect::RehydrateFeedbackImage { .. })),
        "clipboard images need no rehydration: {effects:?}"
    );
    let agent = &app.agents[&id];
    assert_eq!(agent.prompt.text(), "");
    assert!(agent.prompt.images.is_empty());
    let modal = agent.feedback_modal.as_ref().expect("modal opened");
    assert_eq!(modal.image_count(), 1);
    assert_eq!(
        modal.active_tab(),
        crate::views::feedback_modal::FeedbackTab::Write,
        "composer chips mean a new report, not a Drafts peek"
    );
}

#[test]
fn draft_preserving_feedback_uses_the_submitted_command_without_draft_images() {
    let mut app = test_app_with_agent();
    let id = AgentId(0);
    {
        let agent = app.agents.get_mut(&id).unwrap();
        agent.session.state = crate::app::agent::AgentState::TurnRunning;
        agent.prompt.set_text("/feedback live draft ");
        let end = agent.prompt.text().len();
        agent.prompt.set_cursor(end);
        agent.prompt.insert_image(test_pasted_png()).expect("chip");
    }
    let draft = app.agents[&id].prompt.text().to_owned();

    let _ = dispatch(
        Action::SendSlashCommandPreservingDraft("/feedback submitted report".into()),
        &mut app,
    );

    let queued = &app.agents[&id].session.pending_prompts;
    assert_eq!(queued.len(), 1, "expected one queued skill row: {queued:?}");
    let Some([acp::ContentBlock::Text(prompt)]) = queued[0].wire_blocks.as_deref() else {
        panic!(
            "draft images must not attach to the command, got {:?}",
            queued[0].wire_blocks
        );
    };
    assert!(prompt.text.contains("submitted report"));
    assert_eq!(app.agents[&id].prompt.text(), draft);
    assert_eq!(app.agents[&id].prompt.images.len(), 1);
}

/// `dispatch_send_feedback` bailing before the send (no agent view) still owns the attachments and must delete their staged temp files.
#[test]
fn send_feedback_without_agent_view_cleans_staged_temp_files() {
    let mut app = test_app_with_agent();
    app.active_view = crate::app::app_view::ActiveView::AgentDashboard;

    let dir = tempfile::tempdir().unwrap();
    let staged = dir.path().join("staged.png");
    std::fs::write(&staged, b"staged").unwrap();
    let mut image = test_pasted_png();
    image.staged_temp_path = Some(staged.clone());

    let effects = dispatch(
        Action::SendFeedback {
            text: "it broke".into(),
            images: vec![image].into(),
            trace: None,
        },
        &mut app,
    );
    assert!(effects.is_empty(), "the send must bail: {effects:?}");
    assert!(
        !staged.exists(),
        "a bailed send must release its staged files"
    );
}

/// A direct send must not wipe the composer draft.
#[test]
fn send_feedback_preserves_composer_draft() {
    let mut app = test_app_with_agent();
    let id = AgentId(0);
    {
        let agent = app.agents.get_mut(&id).unwrap();
        agent.prompt.set_text("keep this draft");
    }

    let effects = dispatch(
        Action::SendFeedback {
            text: "report".into(),
            images: Default::default(),
            trace: None,
        },
        &mut app,
    );
    assert!(
        matches!(
            effects.as_slice(),
            [Effect::SendFeedback {
                feedback_text,
                ..
            }] if feedback_text == "report"
        ),
        "expected SendFeedback effect, got {effects:?}"
    );
    assert_eq!(
        app.agents.get(&id).unwrap().prompt.text(),
        "keep this draft",
        "composer draft must survive SendFeedback"
    );
}

// -- Feedback modal (open route, submit, retry, arbitration) --

/// Dispatch a full-TUI modal open and return the open generation's id.
fn open_feedback_modal(
    app: &mut AppView,
    text: Option<&str>,
) -> crate::views::feedback_modal::FeedbackModalId {
    let effects = dispatch(
        Action::OpenFeedbackModal(crate::views::feedback_modal::OpenFeedbackModal {
            text: text.map(str::to_owned),
            ..Default::default()
        }),
        app,
    );
    assert!(
        effects.iter().all(|effect| matches!(
            effect,
            Effect::FeedbackDraftRequest { .. } | Effect::RehydrateFeedbackImage { .. }
        )),
        "modal open may list drafts off-thread, got {effects:?}"
    );
    app.agents[&AgentId(0)]
        .feedback_modal
        .as_ref()
        .expect("the feedback modal must open")
        .id()
}

/// Bare `/feedback` opens the modal empty, and an `OpenFeedbackModal` payload with text (e.g. a
/// restored draft) still prefills it; the old question pane never opens.
#[test]
fn feedback_modal_opens_empty_and_prefilled() {
    let mut app = test_app_with_agent();
    open_feedback_modal(&mut app, None);
    {
        let agent = &app.agents[&AgentId(0)];
        assert_eq!(agent.feedback_modal.as_ref().unwrap().text(), "");
        assert!(agent.question_view.is_none(), "the old pane must not open");
    }
    app.agents.get_mut(&AgentId(0)).unwrap().feedback_modal = None;
    open_feedback_modal(&mut app, Some("the tool crashed"));
    assert_eq!(
        app.agents[&AgentId(0)]
            .feedback_modal
            .as_ref()
            .unwrap()
            .text(),
        "the tool crashed"
    );
}

/// Dashboard feedback opens refuse in the visible error slot and consume the dispatch input.
#[test]
fn feedback_modal_open_refuses_visibly_on_dashboard() {
    let mut app = test_app_with_agent();
    app.active_view = crate::app::app_view::ActiveView::AgentDashboard;
    app.dashboard = Some(crate::views::dashboard::DashboardState::new());
    let dashboard = app.dashboard.as_mut().unwrap();
    dashboard.dispatch.set_text("/feedback");

    let effects = dispatch(Action::OpenFeedbackModal(Default::default()), &mut app);

    assert!(effects.is_empty());
    let dashboard = app.dashboard.as_ref().unwrap();
    assert_eq!(dashboard.dispatch.text(), "");
    let expected = format!("{} {NO_SESSION_NOTICE}", crate::glyphs::ballot_x());
    assert_eq!(dashboard.error_toast.as_deref(), Some(expected.as_str()));
    assert!(app.agents[&AgentId(0)].feedback_modal.is_none());
}

/// The palette route refuses visibly under every other input owner (minimal mode, voice in any
/// live state, a question card, a line viewer, a plan approval) without touching the owner, the
/// main draft, or modal state minimal cannot render.
#[test]
fn feedback_modal_open_refuses_visibly_under_every_input_owner() {
    use crate::app::app_view::{VoiceState, VoiceTarget};
    use crate::views::question_view::QuestionViewState;
    use strum::IntoEnumIterator as _;

    #[derive(Debug, Clone, Copy, strum::EnumIter)]
    enum Owner {
        Minimal,
        VoiceColdStart,
        VoiceRecording,
        VoiceStopping,
        QuestionCard,
        LineViewer,
        PlanApproval,
    }

    let id = AgentId(0);
    let viewer_dir = tempfile::tempdir().unwrap();
    let viewer_path = viewer_dir.path().join("preview.txt");
    std::fs::write(&viewer_path, "a preview line\n").unwrap();
    for owner in Owner::iter() {
        let mut app = test_app_with_agent();
        app.agents
            .get_mut(&id)
            .unwrap()
            .prompt
            .set_text("main draft");
        match owner {
            Owner::Minimal => app.screen_mode = crate::app::ScreenMode::Minimal,
            Owner::VoiceColdStart => {
                app.voice_state = VoiceState::ColdStart {
                    hold: false,
                    target: VoiceTarget::Agent(id),
                };
            }
            Owner::VoiceRecording => {
                app.voice_state = VoiceState::Recording {
                    hold: false,
                    target: VoiceTarget::Agent(id),
                    interim: Some("partial".to_owned()),
                };
            }
            Owner::VoiceStopping => {
                app.voice_state = VoiceState::Stopping {
                    target: VoiceTarget::Agent(id),
                    interim: Some("partial".to_owned()),
                };
            }
            Owner::QuestionCard => {
                let agent = app.agents.get_mut(&id).unwrap();
                let stashed = agent.prompt.stash();
                agent.question_view = Some(QuestionViewState::new("q-1".into(), vec![], stashed));
            }
            Owner::LineViewer => {
                app.agents
                    .get_mut(&id)
                    .unwrap()
                    .open_line_viewer(&viewer_path, None);
            }
            Owner::PlanApproval => {
                app.agents.get_mut(&id).unwrap().plan_approval_view =
                    Some(crate::app::agent_view::test_fixtures::make_plan_approval_view_state());
            }
        }

        let effects = dispatch(Action::OpenFeedbackModal(Default::default()), &mut app);

        assert!(effects.is_empty(), "{owner:?}: {effects:?}");
        let agent = &app.agents[&id];
        assert!(
            agent.feedback_modal.is_none(),
            "{owner:?}: the modal must not open under another input owner"
        );
        assert_eq!(
            agent.prompt.text(),
            "main draft",
            "{owner:?}: refusing must not stash or blank the composer"
        );
        match owner {
            Owner::QuestionCard => {
                assert!(
                    agent.question_view.is_some(),
                    "{owner:?}: the blocker is untouched"
                );
            }
            Owner::PlanApproval => {
                assert!(
                    agent.plan_approval_view.is_some(),
                    "{owner:?}: the blocker is untouched"
                );
            }
            Owner::Minimal
            | Owner::VoiceColdStart
            | Owner::VoiceRecording
            | Owner::VoiceStopping
            | Owner::LineViewer => {}
        }
        let visible = match owner {
            Owner::Minimal => last_system_text(&app, id).contains("minimal mode"),
            Owner::VoiceColdStart
            | Owner::VoiceRecording
            | Owner::VoiceStopping
            | Owner::QuestionCard
            | Owner::LineViewer
            | Owner::PlanApproval => agent.toast.is_some(),
        };
        assert!(visible, "{owner:?}: the refusal must be visible");
    }
}

/// The palette emits a default typed payload directly; the main composer draft stays byte-for-byte intact.
#[test]
fn feedback_modal_open_preserves_main_composer_draft() {
    let mut app = test_app_with_agent();
    app.agents
        .get_mut(&AgentId(0))
        .unwrap()
        .prompt
        .set_text("keep this draft");

    open_feedback_modal(&mut app, None);

    let agent = &app.agents[&AgentId(0)];
    assert_eq!(
        agent.prompt.text(),
        "keep this draft",
        "opening the modal must not consume or edit the main draft"
    );
    assert_eq!(
        agent.feedback_modal.as_ref().unwrap().text(),
        "",
        "the modal composer is independent of the main draft"
    );
}

/// A live screenshot is sufficient feedback even when the text is blank.
#[test]
fn feedback_modal_image_only_sends() {
    let mut app = test_app_with_agent();
    let effects = dispatch(
        Action::OpenFeedbackModal(crate::views::feedback_modal::OpenFeedbackModal {
            images: vec![test_pasted_png()].into(),
            ..Default::default()
        }),
        &mut app,
    );
    assert!(effects.is_empty());
    let modal_id = {
        let modal = app.agents[&AgentId(0)].feedback_modal.as_ref().unwrap();
        assert_eq!(modal.image_count(), 1, "the image still renders as a chip");
        modal.id()
    };

    let effects = dispatch(Action::SubmitFeedbackModal { modal_id }, &mut app);

    assert!(matches!(
        effects.as_slice(),
        [Effect::SendFeedback {
            feedback_text,
            images,
            ..
        }] if feedback_text.is_empty() && images.len() == 1
    ));
    assert!(app.agents[&AgentId(0)].feedback_modal.is_none());
}

#[test]
fn feedback_modal_rejected_image_only_submit_keeps_the_modal_and_attachment() {
    let mut app = test_app_with_agent();
    let effects = dispatch(
        Action::OpenFeedbackModal(crate::views::feedback_modal::OpenFeedbackModal {
            images: vec![test_pasted_image("image/webp")].into(),
            ..Default::default()
        }),
        &mut app,
    );
    assert!(effects.is_empty());
    let modal_id = app.agents[&AgentId(0)]
        .feedback_modal
        .as_ref()
        .unwrap()
        .id();

    let effects = dispatch(Action::SubmitFeedbackModal { modal_id }, &mut app);

    assert!(effects.is_empty());
    let modal = app.agents[&AgentId(0)].feedback_modal.as_ref().unwrap();
    assert_eq!(modal.image_count(), 1);
    assert!(modal.text().contains("[Image #"));
}

/// Image chips stay in the composer buffer but must not ride the POST body.
#[test]
fn feedback_modal_submit_strips_image_chips_from_post_body() {
    let mut app = test_app_with_agent();
    let effects = dispatch(
        Action::OpenFeedbackModal(crate::views::feedback_modal::OpenFeedbackModal {
            text: Some("clipboard broke".into()),
            images: vec![test_pasted_png()].into(),
            ..Default::default()
        }),
        &mut app,
    );
    assert!(effects.is_empty());
    let modal_id = app.agents[&AgentId(0)]
        .feedback_modal
        .as_ref()
        .unwrap()
        .id();

    let effects = dispatch(Action::SubmitFeedbackModal { modal_id }, &mut app);

    match effects.as_slice() {
        [Effect::SendFeedback { feedback_text, .. }] => {
            assert_eq!(feedback_text, "clipboard broke");
            assert!(!feedback_text.contains("[Image #"));
        }
        other => panic!("expected POST without image chips, got {other:?}"),
    }
}

/// Submit emits only the POST (no trace/upload effect), closes the modal at send time, and thanks
/// immediately; a duplicate submit finds no modal, and a failed POST reports in the transcript.
#[test]
fn feedback_modal_submit_closes_immediately_and_failure_reports_in_transcript() {
    use crate::app::actions::FeedbackSendOrigin;

    let mut app = test_app_with_agent();
    // The fixture advertises no shell offer, so this submit is the direct-send path.
    let modal_id = open_feedback_modal(&mut app, Some("clipboard broke"));

    let effects = dispatch(Action::SubmitFeedbackModal { modal_id }, &mut app);
    let origin = match effects.as_slice() {
        [
            Effect::SendFeedback {
                feedback_text,
                metadata: Some(metadata),
                origin: origin @ FeedbackSendOrigin::Modal { .. },
                ..
            },
        ] if feedback_text == "clipboard broke" => {
            assert_eq!(
                *metadata,
                serde_json::json!({
                    "structured_feedback": { "schema_version": 1, "source": "write" }
                })
            );
            *origin
        }
        other => panic!("expected exactly one modal-origin POST, got {other:?}"),
    };
    assert!(
        app.agents[&AgentId(0)].feedback_modal.is_none(),
        "a committed submit closes the modal without waiting on the POST"
    );
    assert!(
        last_system_text(&app, AgentId(0)).contains("Thanks for the feedback"),
        "the thank-you lands at send time"
    );
    assert!(
        dispatch(Action::SubmitFeedbackModal { modal_id }, &mut app).is_empty(),
        "a duplicate submit after the close must be ignored"
    );

    let _ = dispatch(
        Action::TaskComplete(TaskResult::FeedbackFailed {
            agent_id: AgentId(0),
            origin,
            error: "offline".into(),
        }),
        &mut app,
    );

    assert!(
        app.agents[&AgentId(0)].feedback_modal.is_none(),
        "a failed POST must not resurrect the closed modal"
    );
    assert!(last_system_text(&app, AgentId(0)).contains("Couldn't send feedback"));
}

/// Cycled enums are committed to the modal's stored metadata and ride the POST's metadata bag
/// as the versioned `structured_feedback` envelope of wire values, never display labels.
#[test]
fn edited_enums_ride_the_send_feedback_metadata() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut app = test_app_with_agent();
    // The fixture advertises no shell offer, so the submit emits the POST directly.
    let effects = dispatch(
        Action::OpenFeedbackModal(crate::views::feedback_modal::OpenFeedbackModal {
            text: Some("wrong file rewritten".to_owned()),
            r#type: Some(crate::views::feedback_modal::FeedbackType::Bug),
            task_category: Some(crate::views::feedback_modal::FeedbackTaskCategory::Debug),
            failure_mode: Some(crate::views::feedback_modal::FeedbackFailureMode::SloppyCode),
            ..Default::default()
        }),
        &mut app,
    );
    assert!(effects.is_empty());
    let modal_id = {
        let modal = app
            .agents
            .get_mut(&AgentId(0))
            .unwrap()
            .feedback_modal
            .as_mut()
            .unwrap();
        let area = ratatui::layout::Rect::new(0, 0, 100, 30);
        let mut buffer = ratatui::buffer::Buffer::empty(area);
        modal
            .render(&mut buffer, area, &crate::theme::Theme::current(), false)
            .expect("metadata rows should render");
        // Tab focuses Type; Right cycles Bug to Idea.
        modal.handle_key(&KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        modal.handle_key(&KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        modal.id()
    };

    let effects = dispatch(Action::SubmitFeedbackModal { modal_id }, &mut app);

    match effects.as_slice() {
        [
            Effect::SendFeedback {
                metadata: Some(metadata),
                ..
            },
        ] => {
            assert_eq!(
                *metadata,
                serde_json::json!({
                    "structured_feedback": {
                        "schema_version": 1,
                        "source": "write",
                        // The cycled value (Bug -> Idea) must ride the POST.
                        "type": "idea",
                        "task_category": "debug",
                        "failure_mode": "sloppy_code",
                    }
                })
            );
        }
        other => panic!("expected the modal POST carrying metadata, got {other:?}"),
    }
}

#[test]
fn drafts_with_optional_taxonomy_omitted_remain_sendable() {
    for (r#type, expected_metadata) in [
        (
            crate::views::feedback_modal::FeedbackType::Bug,
            Some(serde_json::json!({
                "structured_feedback": {
                    "schema_version": 1,
                    "source": "draft",
                    "type": "bug",
                }
            })),
        ),
        (
            crate::views::feedback_modal::FeedbackType::Idea,
            Some(serde_json::json!({
                "structured_feedback": {
                    "schema_version": 1,
                    "source": "draft",
                    "type": "idea",
                }
            })),
        ),
    ] {
        let mut app = test_app_with_agent();
        let modal_id = open_feedback_modal(&mut app, None);
        let load = {
            let modal = app
                .agents
                .get_mut(&AgentId(0))
                .unwrap()
                .feedback_modal
                .as_mut()
                .unwrap();
            modal.start_external_draft_load("draft-id".to_owned().into());
            let crate::views::feedback_modal::FeedbackDraftRequest::Load(load) =
                modal.take_pending_request().expect("draft load request")
            else {
                panic!("expected draft load request");
            };
            load
        };
        app.agents
            .get_mut(&AgentId(0))
            .unwrap()
            .feedback_modal
            .as_mut()
            .unwrap()
            .apply_draft_load(
                &load,
                xai_grok_feedback::FeedbackDraft {
                    id: "draft-id".to_owned().into(),
                    title: "Stored feedback".to_owned(),
                    details: "stored feedback".to_owned(),
                    area: None,
                    r#type: Some(r#type),
                    task_category: None,
                    failure_mode: None,
                    created_at: 1,
                    revision: 1,
                },
            );

        let effects = dispatch(Action::SubmitFeedbackModal { modal_id }, &mut app);

        match effects.as_slice() {
            [
                Effect::SendFeedback {
                    metadata,
                    draft: Some(draft),
                    origin: crate::app::actions::FeedbackSendOrigin::Modal { is_draft: true, .. },
                    ..
                },
            ] => {
                assert_eq!(metadata, &expected_metadata);
                assert_eq!(draft.title, "Stored feedback");
                assert_eq!(draft.details, "stored feedback");
                assert_eq!(draft.r#type, r#type);
                assert_eq!(draft.task_category, None);
                assert_eq!(draft.failure_mode, None);
            }
            other => panic!("draft taxonomy must not block send: {other:?}"),
        }
    }
}

#[test]
fn deferred_feedback_submit_stays_armed_while_the_agent_is_off_screen() {
    use crate::app::actions::{ClipboardPasteContext, ClipboardPasteSource, ClipboardPasteTarget};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut app = test_app_with_agent();
    let modal_id = open_feedback_modal(&mut app, Some("include the screenshot"));
    let composition_id = app
        .agents
        .get_mut(&AgentId(0))
        .unwrap()
        .feedback_modal
        .as_mut()
        .map(|modal| {
            modal.note_paste_probe_started();
            modal.composition_id()
        })
        .unwrap();
    let outcome = app
        .agents
        .get_mut(&AgentId(0))
        .unwrap()
        .feedback_modal
        .as_mut()
        .unwrap()
        .handle_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(
        outcome,
        crate::views::feedback_modal::FeedbackModalOutcome::Changed
    );
    app.active_view = crate::app::app_view::ActiveView::AgentDashboard;

    let effects = dispatch(
        Action::TaskComplete(TaskResult::ClipboardAttachmentProbed {
            ctx: ClipboardPasteContext {
                target: ClipboardPasteTarget::FeedbackModal {
                    agent_id: AgentId(0),
                    modal_id,
                    composition_id,
                },
                source: ClipboardPasteSource::ClipboardKey {
                    text: crate::app::actions::ClipboardTextRead::Success(None),
                    tip_showing: false,
                },
            },
            image: crate::app::actions::ProbedAttachment::NoRaster,
            file_urls: None,
        }),
        &mut app,
    );

    assert!(effects.is_empty());
    let modal = app.agents[&AgentId(0)].feedback_modal.as_mut().unwrap();
    assert!(
        modal.take_deferred_submit(),
        "off-screen completion must preserve the deferred submit"
    );
}

/// The matching success is a pure no-op with no consent parked: no second thank-you, no upload.
#[test]
fn feedback_modal_success_completion_is_quiet_without_consent() {
    let mut app = test_app_with_agent();
    // The fixture advertises no shell offer, so this submit sends directly.
    let modal_id = open_feedback_modal(&mut app, Some("it worked"));
    let effects = dispatch(Action::SubmitFeedbackModal { modal_id }, &mut app);
    let origin = match effects.as_slice() {
        [Effect::SendFeedback { origin, .. }] => *origin,
        other => panic!("expected the POST, got {other:?}"),
    };
    assert!(
        app.agents[&AgentId(0)].feedback_modal.is_none(),
        "submit closes the modal"
    );
    assert!(last_system_text(&app, AgentId(0)).contains("Thanks for the feedback"));
    let scrollback_len = app.agents[&AgentId(0)].scrollback.len();

    let effects = dispatch(
        Action::TaskComplete(TaskResult::FeedbackComplete {
            agent_id: AgentId(0),
            origin,
            outcome: xai_grok_shell::session::FeedbackOutcome::Submitted,
            trace_upload_token: None,
        }),
        &mut app,
    );

    assert!(effects.is_empty(), "no consent, no upload: {effects:?}");
    assert_eq!(
        app.agents[&AgentId(0)].scrollback.len(),
        scrollback_len,
        "success must not thank a second time"
    );
}

/// A completion belonging to an earlier committed report cannot close or silently touch a later modal.
#[test]
fn stale_feedback_modal_completion_leaves_a_later_modal_alone() {
    let mut app = test_app_with_agent();
    // The fixture advertises no shell offer, so submits send directly.
    let modal_id = open_feedback_modal(&mut app, Some("first report"));
    let effects = dispatch(Action::SubmitFeedbackModal { modal_id }, &mut app);
    let origin = match effects.as_slice() {
        [Effect::SendFeedback { origin, .. }] => *origin,
        other => panic!("expected the POST, got {other:?}"),
    };
    // The first submit already closed its modal; a second one opened mid-flight.
    open_feedback_modal(&mut app, Some("second report"));
    let scrollback_len = app.agents[&AgentId(0)].scrollback.len();

    let effects = dispatch(
        Action::TaskComplete(TaskResult::FeedbackComplete {
            agent_id: AgentId(0),
            origin,
            outcome: xai_grok_shell::session::FeedbackOutcome::Submitted,
            trace_upload_token: None,
        }),
        &mut app,
    );
    assert!(effects.is_empty(), "no consent was parked: {effects:?}");
    assert_eq!(
        app.agents[&AgentId(0)].scrollback.len(),
        scrollback_len,
        "an unconsented success must not thank again or complain"
    );

    let _ = dispatch(
        Action::TaskComplete(TaskResult::FeedbackFailed {
            agent_id: AgentId(0),
            origin,
            error: "late".into(),
        }),
        &mut app,
    );

    let agent = &app.agents[&AgentId(0)];
    let modal = agent
        .feedback_modal
        .as_ref()
        .expect("the later modal must survive the earlier report's completions");
    assert_eq!(modal.text(), "second report");
    assert!(
        last_system_text(&app, AgentId(0)).contains("Couldn't send feedback"),
        "the committed report's failure is surfaced in the transcript, not the later modal"
    );
}

/// A second open while feedback is already up refuses and leaves the first draft untouched.
#[test]
fn feedback_modal_open_refuses_while_one_is_open() {
    let mut app = test_app_with_agent();
    let first = open_feedback_modal(&mut app, Some("first draft"));

    let effects = dispatch(
        Action::OpenFeedbackModal(crate::views::feedback_modal::OpenFeedbackModal {
            text: Some("second".to_owned()),
            ..Default::default()
        }),
        &mut app,
    );

    assert!(effects.is_empty());
    let agent = &app.agents[&AgentId(0)];
    let modal = agent.feedback_modal.as_ref().unwrap();
    assert!(modal.matches_id(first), "the open modal is not replaced");
    assert_eq!(modal.text(), "first draft");
    assert!(agent.toast.is_some(), "the refusal must be visible");
}

/// The draft-update task result reaches the open modal with its outcome; the banner wording is the modal's contract.
#[test]
fn draft_update_completion_routes_its_outcome_to_the_open_modal() {
    use crate::views::feedback_modal::{
        FeedbackDraftRequest, FeedbackModalState, OpenFeedbackModal,
    };

    for (result, expected) in [
        (Ok(()), "was saved"),
        (Err("store unavailable".to_owned()), "could not be saved"),
    ] {
        let mut app = test_app_with_agent();
        let mut modal = FeedbackModalState::new(OpenFeedbackModal {
            draft_id: Some("draft-id".to_owned().into()),
            r#type: Some(crate::views::feedback_modal::FeedbackType::Bug),
            ..Default::default()
        });
        modal.mark_draft_submit_unknown();
        let Some(FeedbackDraftRequest::Update(update)) = modal.take_pending_request() else {
            panic!("an unknown outcome on a draft queues its update");
        };
        app.agents.get_mut(&AgentId(0)).unwrap().feedback_modal = Some(modal);

        let _ = dispatch(
            Action::TaskComplete(TaskResult::FeedbackDraftUpdateComplete {
                agent_id: AgentId(0),
                update,
                result,
            }),
            &mut app,
        );

        let banner = app.agents[&AgentId(0)]
            .feedback_modal
            .as_ref()
            .unwrap()
            .error_text();
        assert!(
            banner.is_some_and(|text| text.contains(expected)),
            "{expected}: {banner:?}"
        );
    }
}

#[test]
fn displaced_draft_send_outcome_is_reported_in_scrollback() {
    let mut app = test_app_with_agent();
    let modal_id = open_feedback_modal(&mut app, None);
    {
        let modal = app
            .agents
            .get_mut(&AgentId(0))
            .unwrap()
            .feedback_modal
            .as_mut()
            .unwrap();
        modal.start_external_draft_load("draft-id".to_owned().into());
        let crate::views::feedback_modal::FeedbackDraftRequest::Load(actual_load) =
            modal.take_pending_request().expect("draft load request")
        else {
            panic!("expected draft load request");
        };
        modal.apply_draft_load(
            &actual_load,
            xai_grok_feedback::FeedbackDraft {
                id: "draft-id".to_owned().into(),
                title: "Stored feedback".to_owned(),
                details: "stored feedback".to_owned(),
                area: None,
                r#type: Some(crate::views::feedback_modal::FeedbackType::Bug),
                task_category: None,
                failure_mode: None,
                created_at: 1,
                revision: 1,
            },
        );
    }
    let effects = dispatch(Action::SubmitFeedbackModal { modal_id }, &mut app);
    let origin = match effects.as_slice() {
        [Effect::SendFeedback { origin, .. }] => *origin,
        other => panic!("expected draft send, got {other:?}"),
    };
    let (args, _rx) = make_ask_user_question_args("mid-flight-question");
    assert!(crate::app::acp_handler::handle_ask_user_question(
        args, &mut app
    ));
    assert!(app.agents[&AgentId(0)].feedback_modal.is_none());

    let _ = dispatch(
        Action::TaskComplete(TaskResult::FeedbackComplete {
            agent_id: AgentId(0),
            origin,
            outcome: xai_grok_shell::session::FeedbackOutcome::SubmittedCleanupFailed,
            trace_upload_token: None,
        }),
        &mut app,
    );

    assert!(last_system_text(&app, AgentId(0)).contains("do not resend"));
}

/// A mandatory ACP question evicts the open modal via the production handler: the draft is dropped
/// with a visible notice, the question installs, and the stashed main draft is untouched.
#[test]
fn acp_question_displaces_feedback_modal_and_keeps_main_draft() {
    let mut app = test_app_with_agent();
    app.agents
        .get_mut(&AgentId(0))
        .unwrap()
        .prompt
        .set_text("main draft");
    open_feedback_modal(&mut app, Some("unsent report"));

    let (args, _rx) = make_ask_user_question_args("acp-question");
    let handled = crate::app::acp_handler::handle_ask_user_question(args, &mut app);
    assert!(handled);

    let agent = &app.agents[&AgentId(0)];
    assert!(
        agent.feedback_modal.is_none(),
        "mandatory ingress evicts feedback"
    );
    let qv = agent.question_view.as_ref().expect("question installed");
    assert_eq!(
        qv.stashed_prompt.text, "main draft",
        "the question stashes the untouched main draft, not the feedback text"
    );
    assert!(
        last_system_text(&app, AgentId(0)).contains("Feedback closed"),
        "displacement must leave a visible notice"
    );
}

/// A permission request evicts the open modal through the production enqueue path.
#[test]
fn permission_ingress_displaces_feedback_modal() {
    use std::sync::Arc;
    use xai_acp_lib::AcpClientMessage;

    let mut app = test_app_with_agent();
    open_feedback_modal(&mut app, Some("unsent report"));

    let (tx, _rx) = tokio::sync::oneshot::channel();
    let request = acp::RequestPermissionRequest::new(
        acp::SessionId::new("test-session"),
        acp::ToolCallUpdate::new(
            acp::ToolCallId::new(Arc::from("call-perm-1")),
            acp::ToolCallUpdateFields::default(),
        ),
        vec![acp::PermissionOption::new(
            acp::PermissionOptionId::new(Arc::from("allow-once")),
            "Allow once",
            acp::PermissionOptionKind::AllowOnce,
        )],
    );
    crate::app::acp_handler::handle(
        AcpClientMessage::RequestPermission(xai_acp_lib::AcpArgs {
            request,
            response_tx: tx,
        }),
        &mut app,
    );

    let agent = &app.agents[&AgentId(0)];
    assert!(agent.feedback_modal.is_none(), "permission evicts feedback");
    assert!(!agent.permission_queue.is_empty(), "permission installed");
    assert_eq!(
        agent.prompt.text(),
        "",
        "the discarded draft must not land in the composer"
    );
    assert!(
        last_system_text(&app, AgentId(0))
            .contains("Feedback closed because a permission request needs an answer")
    );
}

/// The cancel-turn prompt evicts the open modal immediately before it installs.
#[test]
fn cancel_turn_prompt_displaces_feedback_modal() {
    let mut app = test_app_with_agent();
    let id = AgentId(0);
    open_feedback_modal(&mut app, Some("unsent report"));
    {
        let agent = app.agents.get_mut(&id).unwrap();
        agent.session.state = AgentState::TurnRunning;
        agent
            .subagent_sessions
            .insert("child-1".into(), make_test_subagent("child-1", "sa-1"));
    }

    let effects = dispatch(Action::CancelTurn, &mut app);

    assert!(effects.is_empty());
    let agent = &app.agents[&id];
    assert!(agent.cancel_turn_view.is_some(), "cancel prompt installed");
    assert!(
        agent.feedback_modal.is_none(),
        "cancel prompt evicts feedback"
    );
    assert!(last_system_text(&app, id).contains("Feedback closed"));
}

/// A plan approval evicts the open modal through the production ext-method handler.
#[test]
fn plan_approval_ingress_displaces_feedback_modal() {
    use xai_acp_lib::AcpClientMessage;

    let mut app = test_app_with_agent();
    open_feedback_modal(&mut app, Some("unsent report"));

    let (tx, _rx) = tokio::sync::oneshot::channel();
    let ext_req = crate::views::plan_approval_view::ExitPlanModeExtRequest {
        session_id: "test-session".into(),
        tool_call_id: "tc-plan".into(),
        plan_content: Some("# Plan\nStep 1".into()),
    };
    let raw = serde_json::value::to_raw_value(&ext_req).unwrap();
    crate::app::acp_handler::handle(
        AcpClientMessage::ExtMethod(xai_acp_lib::AcpArgs {
            request: acp::ExtRequest::new("x.ai/exit_plan_mode", raw.into()),
            response_tx: tx,
        }),
        &mut app,
    );

    let agent = &app.agents[&AgentId(0)];
    assert!(
        agent.feedback_modal.is_none(),
        "plan approval evicts feedback"
    );
    assert!(agent.plan_approval_view.is_some(), "approval installed");
    assert!(last_system_text(&app, AgentId(0)).contains("Feedback closed"));
}

/// An MCP elicitation evicts the open modal through the production ext-method handler:
/// it is a `BlockingCard` the modal intercept would otherwise key-starve.
#[test]
fn mcp_elicitation_ingress_displaces_feedback_modal() {
    use xai_acp_lib::AcpClientMessage;

    let mut app = test_app_with_agent();
    open_feedback_modal(&mut app, Some("unsent report"));

    let (tx, _rx) = tokio::sync::oneshot::channel();
    let raw = serde_json::value::to_raw_value(&serde_json::json!({
        "sessionId": "test-session",
        "toolCallId": "tc-elicit",
        "serverName": "test-server",
        "message": "Provide a value",
        "mode": "form",
    }))
    .unwrap();
    crate::app::acp_handler::handle(
        AcpClientMessage::ExtMethod(xai_acp_lib::AcpArgs {
            request: acp::ExtRequest::new("x.ai/mcp/elicit", raw.into()),
            response_tx: tx,
        }),
        &mut app,
    );

    let agent = &app.agents[&AgentId(0)];
    assert!(
        agent.feedback_modal.is_none(),
        "elicitation evicts feedback"
    );
    assert!(agent.elicitation_view.is_some(), "elicitation installed");
    assert!(last_system_text(&app, AgentId(0)).contains("Feedback closed"));
}

// -- Feedback modal trace step: strict POST -> upload sequencing --

/// Confirm a trace choice through the production key layer, then dispatch the resulting submit.
fn confirm_trace_choice(
    app: &mut AppView,
    choice: crate::views::feedback_modal::FeedbackTraceChoice,
) -> Vec<Effect> {
    use crate::views::feedback_modal::{
        FeedbackModalOutcome, FeedbackTraceChoice as ModalTraceChoice,
    };
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let digit = match choice {
        ModalTraceChoice::SendThisSession => '1',
        ModalTraceChoice::FeedbackOnly => '2',
        ModalTraceChoice::NeverAsk => '3',
    };
    let modal = app
        .agents
        .get_mut(&AgentId(0))
        .unwrap()
        .feedback_modal
        .as_mut()
        .unwrap();
    assert!(modal.in_trace_step(), "the trace step must be up");
    modal.handle_key(&KeyEvent::new(KeyCode::Char(digit), KeyModifiers::NONE));
    assert_eq!(
        modal.handle_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        FeedbackModalOutcome::Submit
    );
    let modal_id = modal.id();
    dispatch(Action::SubmitFeedbackModal { modal_id }, app)
}

fn expect_single_modal_post(effects: &[Effect]) -> crate::app::actions::FeedbackSendOrigin {
    match effects {
        // Exactly one effect: the POST may never bring the upload (or any persistence) along.
        [
            Effect::SendFeedback {
                origin: origin @ crate::app::actions::FeedbackSendOrigin::Modal { .. },
                ..
            },
        ] => *origin,
        other => panic!("expected only the modal-origin POST, got {other:?}"),
    }
}

/// Open, submit into the trace step, and confirm `SendThisSession`; returns the POST's origin.
/// The empty first-submit effects prove the offer asks in-modal instead of sending, and the
/// single-effect confirm proves POST and upload are never sibling effects.
fn park_send_this_session(
    app: &mut AppView,
    text: &str,
) -> crate::app::actions::FeedbackSendOrigin {
    use crate::views::feedback_modal::FeedbackTraceChoice as ModalTraceChoice;

    app.shell_feedback_trace_offer = true;
    let modal_id = open_feedback_modal(app, Some(text));
    let effects = dispatch(Action::SubmitFeedbackModal { modal_id }, app);
    assert!(
        effects.is_empty(),
        "the offer asks in-modal instead of sending: {effects:?}"
    );
    let effects = confirm_trace_choice(app, ModalTraceChoice::SendThisSession);
    expect_single_modal_post(&effects)
}

/// One labelled arrangement of the app before a modal submit.
type Arrange = (&'static str, fn(&mut AppView));

/// The trace step needs the shell-advertised offer and none of its suppressors: ZDR, retention
/// opt-out (this path does not re-enable sharing), or the session NeverAsk latch. Otherwise a
/// validated Write submit sends directly.
#[test]
fn write_submit_sends_directly_unless_the_trace_offer_applies() {
    let cases: [Arrange; 4] = [
        ("no shell offer", |app| assert!(!app.feedback_trace_offer())),
        ("zdr", |app| {
            app.shell_feedback_trace_offer = true;
            app.is_zdr = true;
        }),
        ("retention opt-out", |app| {
            app.shell_feedback_trace_offer = true;
            app.coding_data_retention_opt_out = true;
        }),
        ("never-ask latch", |app| {
            app.shell_feedback_trace_offer = true;
            app.feedback_trace_choice_latched = true;
        }),
    ];
    for (label, arrange) in cases {
        let mut app = test_app_with_agent();
        arrange(&mut app);
        let modal_id = open_feedback_modal(&mut app, Some("direct send"));

        let effects = dispatch(Action::SubmitFeedbackModal { modal_id }, &mut app);

        assert!(
            matches!(effects.as_slice(), [Effect::SendFeedback { .. }]),
            "{label}: {effects:?}"
        );
        assert!(app.agents[&AgentId(0)].feedback_modal.is_none(), "{label}");
    }
}

/// With the offer on, a validated Write submit swaps to the in-modal trace step; POST and upload
/// are never sibling effects, and only the matching success emits exactly one typed upload.
#[test]
fn trace_offer_sequences_post_then_exactly_one_upload() {
    use crate::views::feedback_modal::FeedbackTraceUploadIntent;

    let mut app = test_app_with_agent();
    let origin = park_send_this_session(&mut app, "send with trace");
    let crate::app::actions::FeedbackSendOrigin::Modal { submission_id, .. } = origin else {
        unreachable!()
    };
    assert!(
        app.agents[&AgentId(0)].feedback_modal.is_none(),
        "the committed trace choice closes the modal at send time"
    );
    assert!(last_system_text(&app, AgentId(0)).contains("Thanks for the feedback"));
    let parked_session = app.agents[&AgentId(0)]
        .session
        .session_id
        .clone()
        .expect("the POST session must exist");
    // A reconnect while the POST is in flight must not retarget the one-shot archive.
    app.agents.get_mut(&AgentId(0)).unwrap().session.session_id =
        Some(agent_client_protocol::SessionId::new("replaced-session"));

    let effects = dispatch(
        Action::TaskComplete(TaskResult::FeedbackComplete {
            agent_id: AgentId(0),
            origin,
            outcome: xai_grok_shell::session::FeedbackOutcome::Submitted,
            trace_upload_token: Some("grant".to_string()),
        }),
        &mut app,
    );
    match effects.as_slice() {
        [
            Effect::UploadFeedbackTrace {
                session_id,
                submission_id: Some(sid),
                intent: Some(FeedbackTraceUploadIntent::SendThisSession),
                ..
            },
        ] => {
            assert_eq!(
                *sid, submission_id,
                "the upload correlates to the exact POST"
            );
            assert_eq!(
                session_id, &parked_session,
                "the upload must use the session id parked at POST time"
            );
        }
        other => panic!("success must emit exactly one upload, got {other:?}"),
    }
}

#[test]
fn terminal_feedback_outcomes_take_parked_consent_and_upload_only_remote_successes() {
    use crate::views::feedback_modal::FeedbackTraceUploadIntent;

    for (outcome, should_upload) in [
        (
            xai_grok_shell::session::FeedbackOutcome::SubmittedCleanupFailed,
            true,
        ),
        (xai_grok_shell::session::FeedbackOutcome::LocalOnly, false),
        (
            xai_grok_shell::session::FeedbackOutcome::OutcomeUnknown,
            false,
        ),
    ] {
        let mut app = test_app_with_agent();
        let origin = park_send_this_session(&mut app, "terminal outcome");
        let crate::app::actions::FeedbackSendOrigin::Modal { submission_id, .. } = origin else {
            unreachable!()
        };

        let effects = dispatch(
            Action::TaskComplete(TaskResult::FeedbackComplete {
                agent_id: AgentId(0),
                origin,
                outcome,
                trace_upload_token: Some("grant".to_string()),
            }),
            &mut app,
        );

        if should_upload {
            assert!(matches!(
                effects.as_slice(),
                [Effect::UploadFeedbackTrace {
                    submission_id: Some(id),
                    intent: Some(FeedbackTraceUploadIntent::SendThisSession),
                    ..
                }] if *id == submission_id
            ));
        } else {
            assert!(
                effects.is_empty(),
                "{outcome:?} must not upload: {effects:?}"
            );
        }
        assert!(
            app.agents[&AgentId(0)]
                .parked_feedback_trace_consents
                .is_empty(),
            "terminal outcome must always consume parked consent"
        );
    }
}

#[test]
fn ninth_trace_submit_is_rejected_without_revoking_confirmed_consent() {
    let mut app = test_app_with_agent();
    let origins: Vec<_> = (0..8)
        .map(|index| park_send_this_session(&mut app, &format!("report {index}")))
        .collect();

    app.shell_feedback_trace_offer = true;
    let modal_id = open_feedback_modal(&mut app, Some("ninth report"));
    assert!(dispatch(Action::SubmitFeedbackModal { modal_id }, &mut app).is_empty());
    let effects = confirm_trace_choice(
        &mut app,
        crate::views::feedback_modal::FeedbackTraceChoice::SendThisSession,
    );
    assert!(effects.is_empty());
    assert!(
        app.agents[&AgentId(0)].feedback_modal.is_some(),
        "the rejected report stays open with a visible error"
    );
    assert_eq!(
        app.agents[&AgentId(0)].parked_feedback_trace_consents.len(),
        origins.len()
    );

    let effects = dispatch(
        Action::TaskComplete(TaskResult::FeedbackComplete {
            agent_id: AgentId(0),
            origin: origins[0],
            outcome: xai_grok_shell::session::FeedbackOutcome::Submitted,
            trace_upload_token: Some("grant-0".to_string()),
        }),
        &mut app,
    );
    let submission_id = match effects.as_slice() {
        [
            Effect::UploadFeedbackTrace {
                submission_id: Some(submission_id),
                ..
            },
        ] => *submission_id,
        _ => panic!("the oldest confirmed consent must survive: {effects:?}"),
    };
    assert!(
        dispatch(
            Action::TaskComplete(TaskResult::FeedbackTraceUploaded {
                agent_id: AgentId(0),
                submission_id: Some(submission_id),
                error: None,
            }),
            &mut app,
        )
        .is_empty()
    );
    let effects = confirm_trace_choice(
        &mut app,
        crate::views::feedback_modal::FeedbackTraceChoice::SendThisSession,
    );
    assert!(matches!(effects.as_slice(), [Effect::SendFeedback { .. }]));
}

/// A successful POST with no upload token must not pop the parked one-shot.
/// A later completion that does carry a token can still start the upload.
#[test]
fn feedback_complete_without_token_keeps_parked_consent() {
    let mut app = test_app_with_agent();
    let origin = park_send_this_session(&mut app, "grant later");
    assert_eq!(
        app.agents[&AgentId(0)].parked_feedback_trace_consents.len(),
        1
    );

    let effects = dispatch(
        Action::TaskComplete(TaskResult::FeedbackComplete {
            agent_id: AgentId(0),
            origin,
            outcome: xai_grok_shell::session::FeedbackOutcome::Submitted,
            trace_upload_token: None,
        }),
        &mut app,
    );
    assert!(effects.is_empty(), "no token, no upload: {effects:?}");
    assert_eq!(
        app.agents[&AgentId(0)].parked_feedback_trace_consents.len(),
        1,
        "consent stays parked until an upload actually starts"
    );

    let effects = dispatch(
        Action::TaskComplete(TaskResult::FeedbackComplete {
            agent_id: AgentId(0),
            origin,
            outcome: xai_grok_shell::session::FeedbackOutcome::Submitted,
            trace_upload_token: Some("grant".to_string()),
        }),
        &mut app,
    );
    assert!(
        matches!(effects.as_slice(), [Effect::UploadFeedbackTrace { .. }]),
        "a later token still starts the upload: {effects:?}"
    );
    assert!(
        app.agents[&AgentId(0)]
            .parked_feedback_trace_consents
            .is_empty()
    );
}

/// A failed POST yields zero uploads and drops the parked consent; a stale success arriving
/// after the failure cannot resurrect the upload.
#[test]
fn trace_post_failure_yields_zero_uploads() {
    let mut app = test_app_with_agent();
    let origin = park_send_this_session(&mut app, "will fail");
    assert!(
        app.agents[&AgentId(0)].feedback_modal.is_none(),
        "the committed trace choice closes the modal at send time"
    );

    let effects = dispatch(
        Action::TaskComplete(TaskResult::FeedbackFailed {
            agent_id: AgentId(0),
            origin,
            error: "offline".into(),
        }),
        &mut app,
    );
    assert!(effects.is_empty(), "failure emits no upload: {effects:?}");
    assert!(
        app.agents[&AgentId(0)].feedback_modal.is_none(),
        "the failure surfaces in the transcript, not a resurrected modal"
    );
    assert!(last_system_text(&app, AgentId(0)).contains("Couldn't send feedback"));

    let effects = dispatch(
        Action::TaskComplete(TaskResult::FeedbackComplete {
            agent_id: AgentId(0),
            origin,
            outcome: xai_grok_shell::session::FeedbackOutcome::Submitted,
            trace_upload_token: None,
        }),
        &mut app,
    );
    assert!(
        effects.is_empty(),
        "a stale success cannot upload: {effects:?}"
    );
}

/// A mandatory question arriving after the committed send cannot revoke the parked consent:
/// the report already went out, and the upload still follows only the matching successful POST.
#[test]
fn mid_flight_question_does_not_drop_committed_trace_consent() {
    let mut app = test_app_with_agent();
    let origin = park_send_this_session(&mut app, "committed");

    let (args, _rx) = make_ask_user_question_args("mid-flight-question");
    assert!(crate::app::acp_handler::handle_ask_user_question(
        args, &mut app
    ));

    let effects = dispatch(
        Action::TaskComplete(TaskResult::FeedbackComplete {
            agent_id: AgentId(0),
            origin,
            outcome: xai_grok_shell::session::FeedbackOutcome::Submitted,
            trace_upload_token: Some("grant".to_string()),
        }),
        &mut app,
    );
    assert!(
        matches!(effects.as_slice(), [Effect::UploadFeedbackTrace { .. }]),
        "the committed consent still uploads exactly once: {effects:?}"
    );
}

/// FeedbackOnly sends the report alone; NeverAsk additionally persists only the card latch.
/// No modal path may ever write `[telemetry] trace_upload = true`.
#[test]
fn feedback_only_and_never_ask_upload_nothing_and_never_persist_trace_upload() {
    use crate::views::feedback_modal::FeedbackTraceChoice as ModalTraceChoice;

    let mut app = test_app_with_agent();
    app.shell_feedback_trace_offer = true;
    let modal_id = open_feedback_modal(&mut app, Some("no trace"));
    let _ = dispatch(Action::SubmitFeedbackModal { modal_id }, &mut app);
    let effects = confirm_trace_choice(&mut app, ModalTraceChoice::FeedbackOnly);
    let origin = expect_single_modal_post(&effects);
    let effects = dispatch(
        Action::TaskComplete(TaskResult::FeedbackComplete {
            agent_id: AgentId(0),
            origin,
            outcome: xai_grok_shell::session::FeedbackOutcome::Submitted,
            trace_upload_token: None,
        }),
        &mut app,
    );
    assert!(effects.is_empty(), "no consent, no upload: {effects:?}");
    assert!(app.agents[&AgentId(0)].feedback_modal.is_none());

    // Same app: the FeedbackOnly answer must not have latched the offer away.
    let modal_id = open_feedback_modal(&mut app, Some("never ask"));
    let _ = dispatch(Action::SubmitFeedbackModal { modal_id }, &mut app);
    let effects = confirm_trace_choice(&mut app, ModalTraceChoice::NeverAsk);
    match effects.as_slice() {
        [
            Effect::SendFeedback { .. },
            Effect::PersistSetting {
                key: "feedback_trace_card",
                value: crate::settings::SettingValue::Bool(false),
                ..
            },
        ] => {}
        other => panic!("expected the POST plus the card latch, got {other:?}"),
    }
    assert!(
        effects.iter().all(|effect| !matches!(
            effect,
            Effect::PersistSetting {
                key: "trace_upload",
                ..
            }
        )),
        "the one-shot flow must never persist trace_upload"
    );
    assert!(
        app.feedback_trace_choice_latched,
        "the answer latches re-offers off for this session"
    );
}

/// A trace completion acts only on its registered pending submission, exactly once; a replay is a
/// no-op and can never warn against (or otherwise touch) a later modal.
#[test]
fn stale_trace_completion_cannot_touch_a_later_modal() {
    let mut app = test_app_with_agent();
    let origin = park_send_this_session(&mut app, "first report");
    let crate::app::actions::FeedbackSendOrigin::Modal { submission_id, .. } = origin else {
        unreachable!()
    };
    let effects = dispatch(
        Action::TaskComplete(TaskResult::FeedbackComplete {
            agent_id: AgentId(0),
            origin,
            outcome: xai_grok_shell::session::FeedbackOutcome::Submitted,
            trace_upload_token: Some("grant".to_string()),
        }),
        &mut app,
    );
    assert!(
        matches!(effects.as_slice(), [Effect::UploadFeedbackTrace { .. }]),
        "{effects:?}"
    );

    open_feedback_modal(&mut app, Some("second report"));

    let effects = dispatch(
        Action::TaskComplete(TaskResult::FeedbackTraceUploaded {
            agent_id: AgentId(0),
            submission_id: Some(submission_id),
            error: Some("proxy refused".into()),
        }),
        &mut app,
    );
    assert!(effects.is_empty());
    assert!(
        last_system_text(&app, AgentId(0)).contains("feedback was still sent"),
        "the matching pending completion warns once"
    );

    let scrollback_len = app.agents[&AgentId(0)].scrollback.len();
    let _ = dispatch(
        Action::TaskComplete(TaskResult::FeedbackTraceUploaded {
            agent_id: AgentId(0),
            submission_id: Some(submission_id),
            error: Some("late replay".into()),
        }),
        &mut app,
    );
    assert_eq!(
        app.agents[&AgentId(0)].scrollback.len(),
        scrollback_len,
        "a replayed completion is a no-op"
    );
    let modal = app.agents[&AgentId(0)].feedback_modal.as_ref().unwrap();
    assert_eq!(
        modal.text(),
        "second report",
        "the later modal is untouched"
    );
}
