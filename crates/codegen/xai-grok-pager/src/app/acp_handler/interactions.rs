use super::*;

/// Decode the product-owned selection envelope emitted by the grok-pi
/// subagent extension.  The extension still uses the normal
/// `x.ai/ask_user_question` reverse request; this envelope only selects the
/// existing Pi resource manager as its native presentation.
fn pi_grok_resource_picker_request(
    raw_params: &serde_json::Value,
) -> Result<Option<crate::views::pi_config::PiResourcePickerRequest>, String> {
    use crate::pi_resource_config::PiResourceType;
    use crate::views::pi_config::{PiResourcePickerExtra, PiResourcePickerRequest};

    let Some(value) = raw_params.get("piGrokResourcePicker") else {
        return Ok(None);
    };
    let object = value
        .as_object()
        .ok_or_else(|| "piGrokResourcePicker must be an object".to_owned())?;
    let title = object
        .get("title")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .ok_or_else(|| "piGrokResourcePicker.title is required".to_owned())?
        .to_owned();
    let parse_type = |value: &str| match value.to_ascii_lowercase().as_str() {
        "extensions" | "extension" => Ok(PiResourceType::Extensions),
        "skills" | "skill" => Ok(PiResourceType::Skills),
        _ => Err(format!("unsupported Pi resource type: {value}")),
    };
    let resource_types = object
        .get("resourceTypes")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "piGrokResourcePicker.resourceTypes is required".to_owned())?
        .iter()
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| "piGrokResourcePicker.resourceTypes must contain strings".to_owned())
                .and_then(parse_type)
        })
        .collect::<Result<Vec<_>, _>>()?;
    if resource_types.is_empty() {
        return Err("piGrokResourcePicker.resourceTypes must not be empty".to_owned());
    }
    let initial_paths = object
        .get("initialPaths")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .filter(|path| !path.trim().is_empty())
        .map(std::path::PathBuf::from)
        .collect();
    let extra_resources = object
        .get("extraResources")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let object = entry.as_object()?;
            let path = object.get("path")?.as_str()?.trim();
            let label = object.get("label")?.as_str()?.trim();
            let resource_type = parse_type(object.get("type")?.as_str()?).ok()?;
            (!path.is_empty() && !label.is_empty()).then(|| PiResourcePickerExtra {
                path: std::path::PathBuf::from(path),
                label: label.to_owned(),
                resource_type,
            })
        })
        .collect();
    Ok(Some(PiResourcePickerRequest {
        title,
        resource_types,
        initial_paths,
        extra_resources,
    }))
}

/// Handle `x.ai/ask_user_question` ext-method.
///
/// Parses the typed request, creates a `QuestionViewState` with the
/// `response_tx` stashed, and opens the question overlay. The pager does
/// NOT respond immediately — the response is sent later when the user
/// submits, cancels, or is replaced by another question.
///
/// If a question is already active, the old one is cancelled first
/// (`Cancelled` is sent on its stashed `response_tx`).
pub(crate) fn handle_ask_user_question(
    ext: xai_acp_lib::AcpArgs<acp::ExtRequest>,
    app: &mut AppView,
) -> bool {
    use crate::views::question_view::QuestionViewState;
    use xai_grok_tools::implementations::grok_build::ask_user_question::{
        AskUserQuestionExtRequest, AskUserQuestionExtResponse,
    };

    // Parse both the typed request and the narrow Pi adapter extensions.
    // `initialText` and `noFreeform` are deliberately client-side hints: the
    // canonical ask-user-question wire type remains unchanged.
    let raw_params: serde_json::Value = match serde_json::from_str(ext.request.params.get()) {
        Ok(value) => value,
        Err(e) => {
            tracing::error!(error = %e, "Failed to parse ask-user-question params");
            ext.response_tx
                .send(Err(acp::Error::new(-32602, format!("Invalid params: {e}"))))
                .ok();
            return false;
        }
    };
    let initial_text = raw_params
        .get("initialText")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let no_freeform = raw_params
        .get("noFreeform")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let resource_picker_request = match pi_grok_resource_picker_request(&raw_params) {
        Ok(request) => request,
        Err(error) => {
            ext.response_tx
                .send(Err(acp::Error::new(-32602, error)))
                .ok();
            return false;
        }
    };

    let ext_req: AskUserQuestionExtRequest = match serde_json::from_value(raw_params.clone()) {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "Failed to parse AskUserQuestionExtRequest");
            ext.response_tx
                .send(Err(acp::Error::new(-32602, format!("Invalid params: {e}"))))
                .ok();
            return false;
        }
    };

    // Route by the request's session id (like `session/update`), so a question
    // raised by a BACKGROUND session lands on its own view even when the user is
    // on the dashboard or another session — rather than failing because the
    // user hasn't entered the session yet.
    let Some(id) = interaction_target_agent(app, &ext_req.session_id) else {
        // No local view for this session. Do NOT send an error — that would FAIL
        // the tool (rendered red). Leave the reverse-request unanswered: the
        // agent keeps awaiting and the leader replays it when a client attaches
        // via `session/load`.
        tracing::info!(
            session_id = %ext_req.session_id,
            "ask_user_question for a session with no local view; parked for leader replay-on-attach"
        );
        drop(ext.response_tx);
        return false;
    };
    let is_active = is_matched_agent_active(app, id);
    let Some(agent) = app.agents.get_mut(&id) else {
        // `interaction_target_agent` only returns ids that exist; defensive.
        tracing::warn!("ask_user_question: agent {id:?} not found");
        drop(ext.response_tx);
        return false;
    };

    if let Some(request) = resource_picker_request {
        let cwd = agent.session.cwd.clone();
        match crate::views::pi_config::PiConfigModalState::open_picker(
            cwd,
            request,
            ext.response_tx,
        ) {
            Ok(state) => {
                if let Some(crate::views::modal::ActiveModal::PiConfig { state }) =
                    agent.active_modal.as_mut()
                {
                    state.complete_picker(false);
                }
                agent.active_modal = Some(crate::views::modal::ActiveModal::PiConfig {
                    state: Box::new(state),
                });
                agent.last_active_at = Some(std::time::Instant::now());
                return is_active;
            }
            Err(error) => {
                tracing::warn!(error = %error, "Failed to open Pi resource picker");
                return false;
            }
        }
    }

    // If a question is already active, cancel it before replacing.
    if let Some(mut old_qv) = agent.question_view.take() {
        agent.record_question_pause(&old_qv);
        tracing::warn!(
            old_tool_call_id = %old_qv.tool_call_id,
            new_tool_call_id = %ext_req.tool_call_id,
            "Replacing active question - cancelling previous"
        );
        if let Some(old_tx) = old_qv.response_tx.take() {
            let cancelled = AskUserQuestionExtResponse::Cancelled;
            let raw = serde_json::value::to_raw_value(&cancelled)
                .expect("Cancelled serialization should not fail");
            old_tx.send(Ok(acp::ExtResponse::new(raw.into()))).ok();
        }
        agent.restore_card_prompt(old_qv.stashed_prompt);

        // Local question displaced by an ACP ask, so surface why it vanished.
        // Any directive it carried is dropped; the user re-issues the command after answering.
        if let Some(ref kind) = old_qv.local_kind {
            use crate::views::question_view::LocalQuestionKind;
            let cmd = match kind {
                LocalQuestionKind::Fork { .. } => "/fork",
                LocalQuestionKind::NewSession => "/new",
                LocalQuestionKind::CreditLimitUpsell { .. } => "credit-limit upsell",
                LocalQuestionKind::FreeUsageUpsell { .. } => "SuperGrok upsell",
                LocalQuestionKind::AgentTypeMismatch { .. } => "model switch",
                LocalQuestionKind::DoctorFix { .. } => "/doctor fix",
                LocalQuestionKind::DeleteCurrentSession => "/delete",
                LocalQuestionKind::Feedback => "/feedback",
            };
            let message = if matches!(kind, LocalQuestionKind::DoctorFix { .. }) {
                "/doctor fix was cancelled because another question opened.".to_owned()
            } else {
                format!("{cmd} cancelled because another question opened.")
            };
            agent.scrollback.push_block(RenderBlock::system(message));
        }
    }

    // Stash the current prompt so the composer comes back when this question closes.
    // Pi's select and confirm requests set `noFreeform`; input/editor requests use
    // the native freeform row and can seed it through `initialText`.
    let mut question_view = QuestionViewState::with_response_tx(
        ext_req.tool_call_id,
        ext_req.questions,
        agent.prompt.stash(),
        Some(ext.response_tx),
        ext_req.mode,
    );
    if no_freeform {
        question_view = question_view.with_no_freeform();
    }

    let mut editor_seed = String::new();
    if !question_view.no_freeform
        && let Some(text) = initial_text
        && !question_view.questions.is_empty()
    {
        question_view.per_question_freeform[0] = text;
        question_view.per_question_cursor[0] = question_view.questions[0].options.len();
        editor_seed = question_view.activate_freeform_input();
    }
    agent.question_view = Some(question_view);

    // QuestionView reuses the production PromptWidget as its freeform editor.
    agent.prompt.set_text(&editor_seed);

    // Stamp the "last activity" anchor so the
    // dashboard's NeedsInput row reflects "time since this question
    // arrived" rather than the previous turn's end time.
    agent.last_active_at = Some(std::time::Instant::now());

    tracing::info!(
        mode = ?ext_req.mode,
        question_count = agent.question_view.as_ref().map(|q| q.questions.len()).unwrap_or(0),
        target_active = is_active,
        "Opened question view from ext_method"
    );

    if app.current_ui.pi_ask_user_question_notifications
        && !app.notification_service.focus_tracker.is_focused()
    {
        crate::notifications::system::notify("Grok", "A question is waiting for your response.");
    }

    // Only the currently-displayed view needs an immediate redraw; a question
    // parked on a background agent surfaces via the roster `NeedsInput` delta
    // and renders when the user switches to that session.
    is_active
}

/// Handle an `x.ai/exit_plan_mode` ext_method request.
///
/// Creates a `PlanApprovalViewState` overlay for interactive approval.
///
/// Follows the `handle_ask_user_question` pattern: parse → guard → cancel old
/// → stash prompt → create state → clear prompt → return true.
pub(super) fn handle_exit_plan_mode(
    ext: xai_acp_lib::AcpArgs<acp::ExtRequest>,
    app: &mut AppView,
) -> bool {
    use crate::views::plan_approval_view::{ExitPlanModeExtRequest, PlanApprovalViewState};

    // 1. Parse typed request from raw JSON params.
    let params: ExitPlanModeExtRequest = match serde_json::from_str(ext.request.params.get()) {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("Failed to parse ExitPlanModeExtRequest: {e}");
            ext.response_tx
                .send(Err(acp::Error::new(
                    -32602,
                    format!("Invalid exit_plan_mode params: {e}"),
                )))
                .ok();
            return false;
        }
    };

    // 2. Route by the request's session id (like `session/update`), so a
    // plan-approval raised by a BACKGROUND session lands on its own view even
    // when the user isn't currently focused on it — rather than failing.
    let Some(id) = interaction_target_agent(app, &params.session_id) else {
        // No local view for this session. Do NOT error (that fails the tool):
        // leave the reverse-request unanswered and rely on the leader's
        // replay-on-attach.
        tracing::info!(
            session_id = %params.session_id,
            "exit_plan_mode for a session with no local view; parked for leader replay-on-attach"
        );
        drop(ext.response_tx);
        return false;
    };
    let is_active = is_matched_agent_active(app, id);
    let Some(agent) = app.agents.get_mut(&id) else {
        // `interaction_target_agent` only returns ids that exist; defensive.
        tracing::warn!("exit_plan_mode: agent {id:?} not found");
        drop(ext.response_tx);
        return false;
    };

    if let Some(mut old) = agent.plan_approval_view.take() {
        tracing::warn!(
            old_tool_call_id = %old.tool_call_id,
            new_tool_call_id = %params.tool_call_id,
            "Replacing active plan approval — dismissing previous"
        );
        old.send_stale_cancel();
        agent.plan_next_comment_id = old.next_comment_id;
        agent.prompt.restore(old.stashed_prompt);
        agent.line_viewer = None;
    }

    // Dismiss competing overlays so plan approval owns the screen.
    // - active_modal: draw returns before line_viewer (plan never paints);
    //   keys still route to the invisible plan viewer.
    // - block_viewer: draw returns on line_viewer (plan visible) but
    //   handle_scroll prefers block_viewer, so wheel hits the hidden Edit pane.
    agent.active_modal = None;
    agent.block_viewer = None;

    let source = plan_review_source_for_tool(&params.tool_call_id, agent);

    // If the user was mid-casual-comment when this new plan-approval
    // request arrived, restore the pre-comment prompt first so the
    // upcoming `stash()` captures the user's original text rather
    // than the in-progress comment draft. Also clears the now-stale
    // `casual_stashed_prompt` so it doesn't dangle into the next
    // casual entry.
    if let Some(stashed) = agent.casual_stashed_prompt.take() {
        agent.prompt.restore(stashed);
    }

    let stashed = agent.prompt.stash();
    let state = PlanApprovalViewState::with_source(params, source, stashed, ext.response_tx);

    agent.plan_comments.clear();
    agent.plan_next_comment_id = 0;

    if state.source == PlanReviewSource::Inline {
        agent.latest_inline_plan_content = state.plan_content.clone();
    } else {
        agent.latest_inline_plan_content = None;
    }
    agent.plan_approval_view = Some(state);
    agent.prompt.set_text("");

    agent.casual_commenting_range = None;
    agent.casual_editing_comment_id = None;

    agent.show_plan_preview_if_available();

    if agent.line_viewer.is_some() {
        if let Some(ref mut viewer) = agent.line_viewer {
            viewer.plan_mut().feedback_active = true;
        }
    } else if let Some(ref mut pav) = agent.plan_approval_view {
        pav.focus = crate::views::plan_approval_view::PlanApprovalFocus::Prompt;
    }

    tracing::info!(
        target_active = is_active,
        "Opened plan approval view from ext_method"
    );

    // Background-parked approval renders when the user switches to the session;
    // only the active view needs an immediate redraw.
    is_active
}

pub(super) fn plan_review_source_for_tool(
    tool_call_id: &str,
    agent: &AgentView,
) -> PlanReviewSource {
    agent
        .session
        .tracker
        .tool_title(tool_call_id)
        .filter(|title| *title == "CreatePlan" || *title == "Plan: Submit for approval")
        .map_or(PlanReviewSource::FileBacked, |_| PlanReviewSource::Inline)
}
