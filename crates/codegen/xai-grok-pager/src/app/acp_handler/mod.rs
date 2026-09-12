use std::collections::hash_map::Entry;
use std::path::PathBuf;
use std::sync::Arc;

use agent_client_protocol as acp;
use xai_acp_lib::AcpClientMessage;

use super::actions::Effect;
use xai_grok_shell::extensions::notification::{
    SessionNotification, SessionUpdate as XaiSessionUpdate, is_reauthable_failure,
};
use xai_grok_shell::tools::todo::todo_item_from_plan_entry;
use xai_grok_tools::notification::ScheduledTaskRemovedReason;
use xai_grok_workspace::permission::bash_command_splitting::BashCommandHighlights;

use crate::acp::meta::NotificationMeta;
use crate::acp::tracker::AcpUpdateTracker;
use crate::acp::tracker::TurnActivity;
use crate::app::agent::{
    AgentId, AgentSession, AgentState, BgTaskState, BgTaskStatus, GoalDisplayPhase,
    GoalDisplayState, GoalDisplayStatus,
};
use crate::app::command_catalog::CommandCatalogSource;
use crate::notifications::{NotificationEvent, NotificationEventKind};
use crate::scrollback::block::RenderBlock;
use crate::scrollback::blocks::SessionEvent;
use crate::views::permission_view::{
    McpScope, McpScopeState, PermissionFocus, PermissionViewState, SubagentInfo,
};
use crate::views::plan_approval_view::PlanReviewSource;

use super::agent_view::{AgentPane, AgentView, InputMode};
use super::app_view::{ActiveView, AppView};

mod background;
mod follow_ups;
mod interactions;
mod mcp;
mod permissions;
mod prompt_origin;
mod queue;
mod routing;
mod session_notification;
mod settings;
mod subagent_activity;
mod subagent_lifecycle;
mod workflow_ingest;

#[cfg(test)]
use permissions::{
    MCP_ARGS_MAX_LINE_CHARS, MCP_ARGS_MAX_LINES, build_permission_display, mcp_args_lines,
};
use permissions::{
    apply_recap_block, handle_permission_request, should_drop_duplicate_auto_recap,
    should_drop_late_auto_recap,
};

use routing::{
    SessionMatch, find_session_match, interaction_target_agent, is_matched_agent_active,
    mcp_target_agent, resolve_notif_agent, resolve_target_view,
};

use prompt_origin::{finish_wake_turn, viewer_turn_anchor};
pub(crate) use prompt_origin::{
    is_scheduler_fired_prompt, is_server_initiated_prompt, is_wake_prompt,
    should_adopt_running_prompt,
};

pub(crate) use subagent_activity::finalize_killed_subagent;
use subagent_activity::{subagent_activity_label, sync_subagent_activity};
use subagent_lifecycle::{
    LifecycleOrigin, classify_subagent_lifecycle, prepare_tui_subagent_lifecycle,
};

use workflow_ingest::ingest_workflow_update;

pub(crate) use session_notification::apply_child_view_session_event;
#[cfg(test)]
pub(crate) use session_notification::apply_session_event_for_test;
pub(crate) use session_notification::drop_unexpected_replay;
use session_notification::{
    PlanModeTransition, advance_reconnect_cursor, confirm_context_used, detect_plan_mode_change,
    handle_session_notification, handle_session_notification_with_origin,
};

pub(crate) use queue::PendingRunningAdoption;
use queue::{handle_prompt_complete, handle_queue_changed};

use background::{
    derive_child_cwd, handle_git_head_changed, handle_monitor_event, handle_scheduled_task_created,
    handle_scheduled_task_deleted, handle_scheduled_task_fired, handle_task_backgrounded,
    handle_task_completed, route_bg_task_stdout,
};
use follow_ups::handle_follow_ups;
pub(crate) use interactions::handle_ask_user_question;
use interactions::{handle_exit_plan_mode, handle_mcp_elicit};
use mcp::{
    handle_mcp_elicit_complete, handle_mcp_init_progress, handle_mcp_server_status,
    handle_mcp_servers_updated, handle_mcp_tools_changed, push_server_status_enabled,
};
use settings::{
    handle_announcements_update, handle_models_update, handle_sessions_changed,
    handle_settings_update,
};

#[cfg(test)]
#[allow(unused_imports)]
use background::*;
#[cfg(test)]
#[allow(unused_imports)]
use follow_ups::*;
#[cfg(test)]
#[allow(unused_imports)]
use interactions::*;
#[cfg(test)]
#[allow(unused_imports)]
use mcp::*;
#[cfg(test)]
#[allow(unused_imports)]
use prompt_origin::*;
#[cfg(test)]
#[allow(unused_imports)]
use queue::*;
#[cfg(test)]
#[allow(unused_imports)]
use routing::*;
#[cfg(test)]
#[allow(unused_imports)]
use session_notification::*;
#[cfg(test)]
#[allow(unused_imports)]
use settings::*;
#[cfg(test)]
#[allow(unused_imports)]
use subagent_activity::*;
#[cfg(test)]
#[allow(unused_imports)]
use workflow_ingest::*;

fn is_replay_bash_execute(update: &acp::SessionUpdate) -> bool {
    let acp::SessionUpdate::ToolCall(tc) = update else {
        return false;
    };
    tc.meta
        .as_ref()
        .and_then(|m| m.get("bash_mode"))
        .and_then(|v| v.as_bool())
        == Some(true)
}

/// Any live update stamped with the awaited prompt id proves the shell accepted it, whichever branch applies it.
fn ack_prompt_from_update(view: &mut AgentView, meta: &NotificationMeta) {
    if !meta.is_replay {
        view.ack_prompt_if_named(
            meta.prompt_id.as_deref(),
            crate::app::prompt_ack::AckSignal::SessionUpdate,
            std::time::Instant::now(),
        );
    }
}

pub(crate) fn handle(msg: AcpClientMessage, app: &mut AppView) -> bool {
    match msg {
        AcpClientMessage::SessionNotification(notif) => {
            let mut meta = NotificationMeta::from_json(notif.request.meta.as_ref());

            let affected = match find_session_match(app, &notif.request.session_id) {
                Some(SessionMatch::Root(id)) => {
                    let is_active = is_matched_agent_active(app, id);
                    let stashed_adoption_pid = app
                        .pending_running_adoptions
                        .get(&id)
                        .map(|p| p.prompt_id.clone());
                    let agent = app
                        .agents
                        .get_mut(&id)
                        .expect("find_session_match returned an existing AgentId");

                    let dedup_drop = !meta.is_replay
                        && meta.event_seq.is_some_and(|seq| {
                            agent.last_applied_event_seq.is_some_and(|last| seq <= last)
                        });
                    if let Some(seq) = meta.event_seq
                        && !meta.is_replay
                        && !dedup_drop
                    {
                        agent.last_applied_event_seq = Some(seq);
                    }
                    ack_prompt_from_update(agent, &meta);

                    if drop_unexpected_replay(
                        agent,
                        &meta,
                        notif.request.session_id.0.as_ref(),
                        "session/update",
                    ) {
                        notif.response_tx.send(Ok(())).ok();
                        return false;
                    }

                    if !dedup_drop
                        && !meta.is_replay
                        && let Some(notif_pid) = meta.prompt_id.as_deref()
                        && agent.session.current_prompt_id.as_deref() != Some(notif_pid)
                        && !is_server_initiated_prompt(notif_pid)
                    {
                        agent.attached_as_viewer = !agent.is_self_originated_prompt(notif_pid);
                    }

                    if !dedup_drop {
                        if let Some(tokens) = meta.total_tokens {
                            confirm_context_used(agent, tokens);
                        }
                        if let Some(ts) = meta.turn_start_ms {
                            agent.turn_start_ms = Some(ts);
                            agent.turn_start_ms_prompt = meta.prompt_id.clone();
                        }
                    }

                    let mut plan_mode_modal_refresh_needed = false;
                    let mut workflows_modal_refresh = false;

                    let mutated = if dedup_drop {
                        tracing::debug!(
                            session_id = notif.request.session_id.0.as_ref(),
                            event_seq = meta.event_seq,
                            last_applied = agent.last_applied_event_seq,
                            is_replay = meta.is_replay,
                            "load-race: session/update DROPPED by dedup highwater (event_seq <= last_applied)"
                        );
                        false
                    } else if let acp::SessionUpdate::Plan(plan) = notif.request.update {
                        let items: Vec<_> = plan
                            .entries
                            .into_iter()
                            .map(todo_item_from_plan_entry)
                            .collect();
                        agent.todo.update_todos(items);
                        agent.mark_reload_todo_update();
                        advance_reconnect_cursor(agent, &mut meta);
                        !meta.is_replay && !agent.session.loading_replay
                    } else if let acp::SessionUpdate::ToolCallUpdate(ref tcu) = notif.request.update
                        && route_bg_task_stdout(tcu, &mut agent.session)
                    {
                        advance_reconnect_cursor(agent, &mut meta);
                        !meta.is_replay && !agent.session.loading_replay
                    } else if !meta.is_replay
                        && let Some(notif_pid) = meta.prompt_id.as_ref()
                        && agent.session.current_prompt_id.as_ref() != Some(notif_pid)
                        && !agent.attached_as_viewer
                        && stashed_adoption_pid.as_deref() == Some(notif_pid.as_str())
                    {
                        if agent.pending_adoption_updates.len()
                            < super::agent_view::MAX_PENDING_ADOPTION_UPDATES
                        {
                            tracing::debug!(
                                target: "qtrace",
                                pid = std::process::id(),
                                event = "adoption_update_buffered",
                                prompt_id = %notif_pid,
                                "buffering session/update for the stashed pending adoption",
                            );
                            agent.pending_adoption_updates.push((
                                notif_pid.clone(),
                                notif.request.update,
                                meta.clone(),
                            ));
                        } else {
                            tracing::debug!(
                                prompt_id = %notif_pid,
                                "pending-adoption buffer full; dropping update (kept prefix)",
                            );
                        }
                        false
                    } else if !meta.is_replay
                        && let Some(notif_pid) = meta.prompt_id.as_ref()
                        && agent.session.current_prompt_id.as_ref() != Some(notif_pid)
                        && !((agent.session.current_prompt_id.is_none()
                            || agent
                                .session
                                .current_prompt_id
                                .as_deref()
                                .is_some_and(is_server_initiated_prompt))
                            && is_server_initiated_prompt(notif_pid))
                        && !agent.attached_as_viewer
                    {
                        tracing::debug!(
                            session_id = notif.request.session_id.0.as_ref(),
                            notif_prompt_id = meta.prompt_id.as_deref(),
                            current_prompt_id = agent.session.current_prompt_id.as_deref(),
                            attached_as_viewer = agent.attached_as_viewer,
                            loading_replay = agent.session.loading_replay,
                            "load-race: session/update DROPPED by promptId-mismatch gate on a non-viewer (stale/rewound-turn guard)"
                        );
                        !agent.session.loading_replay
                    } else {
                        if let Some(notif_pid) = meta.prompt_id.as_ref()
                            && agent.session.current_prompt_id.as_ref() != Some(notif_pid)
                            && agent.attached_as_viewer
                        {
                            agent.session.current_prompt_id = Some(notif_pid.clone());
                            agent.clear_follow_ups();
                            agent.flush_pending_follow_ups(notif_pid);
                        }
                        if !meta.is_replay
                            && let Some(notif_pid) = meta.prompt_id.as_deref()
                            && is_wake_prompt(notif_pid)
                        {
                            agent.note_streaming_wake_turn(notif_pid);
                        }

                        let plan_transition = detect_plan_mode_change(&notif.request.update, agent);
                        plan_mode_modal_refresh_needed |= plan_transition.is_some();
                        // User-driven entries already got a banner or toast; a replay must not duplicate the row
                        if plan_transition == Some(PlanModeTransition::EnteredByAgent)
                            && !meta.is_replay
                            && !agent.session.loading_replay
                        {
                            agent.scrollback.push_block(RenderBlock::session_event(
                                SessionEvent::PlanModeEnteredByAgent {
                                    permission: agent.session.permission_label(),
                                },
                            ));
                        }

                        let had_activity_before = agent.session.tracker.activity().is_some();
                        let update = notif.request.update;
                        let (is_visible_kind, is_bash) = if meta.is_replay {
                            (
                                crate::acp::tracker::is_agent_output_update(&update),
                                is_replay_bash_execute(&update),
                            )
                        } else {
                            (false, false)
                        };
                        let user_echo = matches!(update, acp::SessionUpdate::UserMessageChunk(_));
                        let changed =
                            agent
                                .session
                                .handle_update(update, &meta, &mut agent.scrollback);
                        if meta.is_replay
                            && let Some(pid) = meta.prompt_id.as_ref()
                        {
                            if is_visible_kind && changed {
                                agent.replayed_visible_prompts.insert(pid.clone());
                            }
                            if is_bash {
                                agent.replayed_bash_prompts.insert(pid.clone());
                            }
                        }
                        if !user_echo
                            && !meta.is_replay
                            && !agent.session.loading_replay
                            && meta.prompt_id.as_deref()
                                == agent.session.current_prompt_id.as_deref()
                        {
                            agent.front_message_committed = true;
                        }
                        if !had_activity_before && agent.session.tracker.activity().is_some() {
                            note_first_turn_activity(agent);
                        }

                        if let Some(commands) = agent.session.tracker.take_pending_acp_commands() {
                            let workflows_changed = workflow_commands(&commands)
                                != workflow_commands(&agent.session.available_commands);
                            agent.session.replace_available_commands(
                                commands,
                                CommandCatalogSource::SessionUpdate,
                            );
                            refresh_workflow_run_capabilities(agent);
                            workflows_modal_refresh =
                                workflows_changed && agent.extensions_modal.is_some();
                        }
                        if let Some(tools) = agent.session.tracker.take_pending_acp_tools() {
                            agent.session.available_tools = Some(tools.into_iter().collect());
                        }
                        for entry_id in agent.session.tracker.take_pending_edit_hl() {
                            agent.submit_edit_highlight(entry_id);
                        }

                        if agent.attached_as_viewer
                            && !meta.is_replay
                            && !agent.session.loading_replay
                            && agent
                                .session
                                .current_prompt_id
                                .as_deref()
                                .is_some_and(should_adopt_running_prompt)
                            && !matches!(agent.session.state, AgentState::TurnRunning)
                        {
                            agent.session.state = AgentState::TurnRunning;
                            agent.turn_started_at = Some(viewer_turn_anchor(agent.turn_start_ms));
                        }

                        advance_reconnect_cursor(agent, &mut meta);

                        !meta.is_replay && !agent.session.loading_replay
                    };

                    if plan_mode_modal_refresh_needed {
                        crate::app::dispatch::refresh_open_settings_modals(app);
                    }
                    if workflows_modal_refresh {
                        queue_open_workflows_modal_refresh(app, id);
                    }

                    mutated && is_active
                }
                Some(SessionMatch::Child(parent_id)) => {
                    let is_active = is_matched_agent_active(app, parent_id);
                    let parent = app
                        .agents
                        .get_mut(&parent_id)
                        .expect("find_session_match returned an existing AgentId");
                    let child_key: &str = notif.request.session_id.0.as_ref();

                    let activity_label = {
                        let child_view = parent
                            .descendant_view_for_live_update_mut(child_key)
                            .expect("find_session_match returned an existing descendant subagent view");
                        // An overlay prompt arms the watch on this child, so the child's own updates must disarm it
                        ack_prompt_from_update(child_view, &meta);
                        if let Some(tokens) = meta.total_tokens {
                            confirm_context_used(child_view, tokens);
                        }
                        if let Some(ts) = meta.turn_start_ms {
                            child_view.turn_start_ms = Some(ts);
                        }
                        let had_activity_before = child_view.session.tracker.activity().is_some();
                        child_view.session.handle_update(
                            notif.request.update,
                            &meta,
                            &mut child_view.scrollback,
                        );
                        if !had_activity_before && child_view.session.tracker.activity().is_some() {
                            note_first_turn_activity(child_view);
                        }
                        if let Some(commands) =
                            child_view.session.tracker.take_pending_acp_commands()
                        {
                            child_view.session.available_commands = commands;
                            child_view.session.available_commands_generation += 1;
                        }
                        if let Some(tools) = child_view.session.tracker.take_pending_acp_tools() {
                            child_view.session.available_tools = Some(tools.into_iter().collect());
                        }
                        for entry_id in child_view.session.tracker.take_pending_edit_hl() {
                            child_view.submit_edit_highlight(entry_id);
                        }
                        subagent_activity_label(child_view)
                    };

                    if let Some(owner) = parent.descendant_parent_view_mut(child_key) {
                        sync_subagent_activity(owner, child_key, activity_label);
                    }

                    is_active
                }
                None => {
                    tracing::debug!(
                        session_id = notif.request.session_id.0.as_ref(),
                        agent_count = app.agents.len(),
                        "load-race: session/update DROPPED — no agent matches session_id (view not loaded yet?)"
                    );
                    false
                }
            };
            notif.response_tx.send(Ok(())).ok();
            affected
        }
        AcpClientMessage::RequestPermission(perm) => handle_permission_request(perm, app),
        AcpClientMessage::ExtNotification(ext) => {
            let affected = handle_ext_notification(&ext.request, app);
            ext.response_tx.send(Ok(())).ok();
            affected
        }
        AcpClientMessage::ExtMethod(ext) => handle_ext_method(ext, app),
        AcpClientMessage::WaitForTerminalExit(args) => {
            args.response_tx
                .send(Err(crate::acp::wait_for_exit_not_supported("pager")))
                .ok();
            false
        }
        _ => false,
    }
}

pub(super) fn note_first_turn_activity(agent: &mut AgentView) {
    agent.session.in_flight_prompt = None;

    if let Some(started) = agent.turn_started_at
        && agent.first_activity_logged_for != Some(started)
    {
        agent.first_activity_logged_for = Some(started);
        let activity_label = agent
            .session
            .tracker
            .activity()
            .map(|a| a.as_label())
            .unwrap_or("unknown");
        let ttfa_ms = started.elapsed().as_millis() as u64;
        let sid = agent.session.session_id.as_ref().map(|s| s.0.as_ref());
        crate::unified_log::info(
            "turn.first_activity",
            sid,
            Some(serde_json::json!({
                "ttfa_ms": ttfa_ms,
                "activity": activity_label,
            })),
        );
    }
}

fn workflow_commands(
    commands: &[acp::AvailableCommand],
) -> Vec<(&str, &str, Option<&str>, Option<&str>)> {
    commands
        .iter()
        .filter_map(|command| {
            let meta = command.meta.as_ref()?;
            let source = meta.get("workflowSource")?.as_str();
            Some((
                command.name.as_str(),
                command.description.as_str(),
                source,
                meta.get("workflowPath").and_then(serde_json::Value::as_str),
            ))
        })
        .collect()
}

pub(super) fn is_builtin_workflow_handle(
    commands: &[acp::AvailableCommand],
    display_name: &str,
) -> bool {
    let is_builtin = |command: &acp::AvailableCommand| {
        command.meta.as_ref().is_some_and(|meta| {
            meta.get("workflowSource")
                .and_then(serde_json::Value::as_str)
                == Some("builtin")
        })
    };
    if let Some(exact) = commands.iter().find(|command| command.name == display_name) {
        return is_builtin(exact);
    }
    commands.iter().any(|command| {
        is_builtin(command)
            && display_name
                .strip_prefix(command.name.as_str())
                .and_then(|suffix| suffix.strip_prefix('-'))
                .is_some_and(|ordinal| ordinal.parse::<u32>().is_ok_and(|n| n >= 2))
    })
}

pub(crate) fn refresh_workflow_run_capabilities(agent: &mut AgentView) {
    let management_available = agent
        .session
        .available_commands
        .iter()
        .any(|command| command.name == "workflow");
    for run in &mut agent.workflow_runs {
        run.management_available = management_available;
        run.builtin = is_builtin_workflow_handle(&agent.session.available_commands, &run.name);
    }
}

fn queue_open_workflows_modal_refresh(app: &mut AppView, agent_id: AgentId) {
    let Some(session_id) = app
        .agents
        .get(&agent_id)
        .and_then(|agent| agent.session.session_id.clone())
    else {
        return;
    };
    let already_pending = app.pending_effects.iter().any(|effect| {
        matches!(
            effect,
            Effect::FetchWorkflowsList {
                agent_id: pending_id,
                ..
            } if *pending_id == agent_id
        )
    });
    if !already_pending {
        app.pending_effects.push(Effect::FetchWorkflowsList {
            agent_id,
            session_id,
        });
    }
}

fn handle_ext_notification(notif: &acp::ExtNotification, app: &mut AppView) -> bool {
    let method = notif.method.as_ref();
    if crate::acp::is_session_update_ext_method(method) {
        return handle_session_notification(notif, app);
    }
    match method {
        "x.ai/follow_ups" => handle_follow_ups(notif, app),
        "x.ai/review_ask_delta" => handle_review_ask_delta(notif, app),
        "x.ai/task_backgrounded" => handle_task_backgrounded(notif, app),
        "x.ai/task_completed" => handle_task_completed(notif, app),
        "x.ai/models/update" => handle_models_update(notif, app),
        "x.ai/settings/update" => handle_settings_update(notif, app),
        "x.ai/sessions/changed" => handle_sessions_changed(notif, app),
        "x.ai/queue/changed" => handle_queue_changed(notif, app),
        "x.ai/session/prompt_complete" => handle_prompt_complete(notif, app),
        "x.ai/session/interjection" => handle_interjection(notif, app),
        "x.ai/monitor_event" => handle_monitor_event(notif, app),
        "x.ai/scheduled_task_created" => handle_scheduled_task_created(notif, app),
        "x.ai/scheduled_task_fired" => handle_scheduled_task_fired(notif, app),
        "x.ai/scheduled_task_deleted" => handle_scheduled_task_deleted(notif, app),
        "x.ai/announcements/update" => handle_announcements_update(notif, app),
        "x.ai/git_head_changed" => handle_git_head_changed(notif, app),
        "x.ai/leader/version_mismatch" => handle_version_mismatch(notif, app),
        "x.ai/mcp/init_progress" => handle_mcp_init_progress(notif, app),
        "x.ai/mcp/tools_changed" | "x.ai/mcp_initialized" => handle_mcp_tools_changed(notif, app),
        "x.ai/mcp/server_status" if push_server_status_enabled() => {
            handle_mcp_server_status(notif, app)
        }
        "x.ai/mcp/elicit_complete" => handle_mcp_elicit_complete(notif, app),
        "x.ai/mcp/servers_updated" => handle_mcp_servers_updated(notif, app),
        // Pi RPC extension UI is projected onto native Grok pager surfaces.
        // info → SystemMessage scrollback (Pi showStatus / chat append);
        // warning/error stay transient toasts.
        "pi/ui/notify" => handle_pi_ui_notify(notif, app),
        "pi/ui/compaction_summary" => handle_pi_ui_compaction_summary(notif, app),
        "pi/ui/btw_history" => handle_pi_ui_btw_history(notif, app),
        "pi/ui/status" => handle_pi_ui_status(notif, app),
        "pi/ui/widget" => handle_pi_ui_widget(notif, app),
        "pi/ui/title" => handle_pi_ui_title(notif, app),
        "pi/ui/reset_extension_ui" => handle_pi_ui_reset_extension_ui(app),
        "pi/ui/editor_text" => handle_pi_ui_editor_text(notif, app),
        "pi/ui/session_catalog" => handle_pi_ui_session_catalog(notif, app),
        "pi/ui/cancel_interaction" => handle_pi_ui_cancel_interaction(notif, app),
        "pi/ui/plan_file" => handle_pi_ui_plan_file(notif, app),
        // Experimental Remote TUI frame projection (PI_GROK_REMOTE_TUI=1 on Pi).
        "pi/ui/remote_tui" => handle_pi_ui_remote_tui(notif, app),
        // Extension shortcut registry sync from Pi.
        "pi/ui/shortcuts" => handle_pi_ui_shortcuts(notif, app),
        _ => false,
    }
}

fn handle_review_ask_delta(notif: &acp::ExtNotification, app: &mut AppView) -> bool {
    let Some(delta) = serde_json::from_str::<serde_json::Value>(notif.params.get())
        .ok()
        .and_then(|params| {
            params
                .get("delta")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .filter(|delta| !delta.is_empty())
    else {
        return false;
    };
    for agent in app.agents.values_mut() {
        if let Some(review) = agent.review_state.as_mut()
            && review.ask.is_loading()
        {
            review.ask.append_delta(&delta);
            return true;
        }
    }
    false
}

fn handle_pi_ui_shortcuts(notif: &acp::ExtNotification, app: &mut AppView) -> bool {
    let Some(params) = pi_ui_params(notif) else {
        return false;
    };
    let Some(shortcuts_arr) = params
        .get("shortcuts")
        .and_then(serde_json::Value::as_array)
    else {
        return false;
    };
    let shortcuts: Vec<crate::app::extension_shortcuts::ExtensionShortcut> = shortcuts_arr
        .iter()
        .filter_map(|v| {
            Some(crate::app::extension_shortcuts::ExtensionShortcut {
                key: v.get("key")?.as_str()?.to_string(),
                description: v
                    .get("description")
                    .and_then(|d| d.as_str())
                    .unwrap_or("")
                    .to_string(),
                extension: v
                    .get("extension")
                    .and_then(|e| e.as_str())
                    .unwrap_or("unknown")
                    .to_string(),
                enabled: v.get("enabled").and_then(|e| e.as_bool()).unwrap_or(true),
                remapped_to: v
                    .get("remappedTo")
                    .and_then(|r| r.as_str())
                    .map(|s| s.to_string()),
            })
        })
        .collect();
    app.external_ui.extension_shortcuts.set_shortcuts(shortcuts);
    true
}

fn pi_ui_params(notif: &acp::ExtNotification) -> Option<serde_json::Value> {
    serde_json::from_str(notif.params.get())
        .map_err(|error| {
            tracing::warn!(method = %notif.method, %error, "invalid Pi UI notification payload");
        })
        .ok()
}

fn handle_pi_ui_notify(notif: &acp::ExtNotification, app: &mut AppView) -> bool {
    let Some(params) = pi_ui_params(notif) else {
        return false;
    };
    let Some(message) = params.get("message").and_then(serde_json::Value::as_str) else {
        return false;
    };
    let kind = params
        .get("notifyType")
        .or_else(|| params.get("kind"))
        .and_then(serde_json::Value::as_str);
    app.show_external_notification(message, kind);
    true
}

fn handle_pi_ui_compaction_summary(notif: &acp::ExtNotification, app: &mut AppView) -> bool {
    let Some(params) = pi_ui_params(notif) else {
        return false;
    };
    let Some(summary) = params
        .get("summary")
        .and_then(serde_json::Value::as_str)
        .filter(|summary| !summary.trim().is_empty())
    else {
        return false;
    };
    let target_id = params
        .get("sessionId")
        .and_then(serde_json::Value::as_str)
        .and_then(|session_id| {
            app.agents.iter().find_map(|(id, agent)| {
                agent
                    .session
                    .session_id
                    .as_ref()
                    .is_some_and(|current| current.0.as_ref() == session_id)
                    .then_some(*id)
            })
        })
        .or_else(|| match app.active_view {
            ActiveView::Agent(id) => Some(id),
            _ => None,
        });
    let Some(agent) = target_id.and_then(|id| app.agents.get_mut(&id)) else {
        return false;
    };
    agent.scrollback.push_block(RenderBlock::session_event(
        SessionEvent::CompactionSummary {
            summary: summary.to_string(),
        },
    ));
    true
}

fn handle_pi_ui_btw_history(notif: &acp::ExtNotification, app: &mut AppView) -> bool {
    let Some(params) = pi_ui_params(notif) else {
        return false;
    };
    if params.get("version").and_then(serde_json::Value::as_u64) != Some(1) {
        return false;
    }
    let source = params
        .get("source")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("replay");
    let entries = params
        .get("entries")
        .and_then(serde_json::Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| {
                    let question = entry.get("question")?.as_str()?.to_owned();
                    let answer = entry.get("answer")?.as_str()?.to_owned();
                    if question.trim().is_empty() || answer.trim().is_empty() {
                        return None;
                    }
                    Some((question, answer))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let session_id = params
        .get("sessionId")
        .or_else(|| params.get("session_id"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);

    let agent = if let Some(session_id) = session_id.as_deref() {
        let session_id = acp::SessionId::new(session_id);
        let Some((matched, _is_active, agent)) = resolve_notif_agent(app, &session_id) else {
            return false;
        };
        // BTW is a root-session surface; never project a root history into a
        // child/subagent transcript that happens to share the Pager process.
        if matches!(matched, SessionMatch::Child(_)) {
            return false;
        }
        agent
    } else {
        let ActiveView::Agent(id) = app.active_view else {
            return false;
        };
        let Some(agent) = app.agents.get_mut(&id) else {
            return false;
        };
        agent
    };

    let mut added = 0;
    for (question, answer) in &entries {
        let already_visible = (0..agent.scrollback.len()).any(|index| {
            let Some(entry) = agent.scrollback.entry(index) else {
                return false;
            };
            let RenderBlock::Btw(block) = &entry.block else {
                return false;
            };
            block.question.as_str() == question.as_str()
                && block.content().text().as_str() == answer.as_str()
        });
        if already_visible {
            continue;
        }
        agent
            .scrollback
            .push_block(RenderBlock::Btw(crate::scrollback::blocks::BtwBlock::new(
                question.clone(),
                answer.clone(),
            )));
        added += 1;
    }

    if source == "command" && added == 0 {
        let message = if entries.is_empty() {
            "No saved BTW history."
        } else {
            "BTW history is already visible."
        };
        agent.scrollback.push_block(RenderBlock::system(message));
    }
    true
}

fn handle_pi_ui_status(notif: &acp::ExtNotification, app: &mut AppView) -> bool {
    let Some(params) = pi_ui_params(notif) else {
        return false;
    };
    let Some(key) = params
        .get("statusKey")
        .or_else(|| params.get("key"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
    else {
        return false;
    };
    let text = params
        .get("statusText")
        .or_else(|| params.get("text"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    app.set_external_status(key, text)
}

fn handle_pi_ui_widget(notif: &acp::ExtNotification, app: &mut AppView) -> bool {
    let Some(params) = pi_ui_params(notif) else {
        return false;
    };
    let Some(key) = params
        .get("widgetKey")
        .or_else(|| params.get("key"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
    else {
        return false;
    };
    if key == "__pi_grok_remote_tui_layout__" {
        let layout = params
            .get("widgetLines")
            .or_else(|| params.get("lines"))
            .and_then(serde_json::Value::as_array)
            .and_then(|lines| lines.first())
            .and_then(serde_json::Value::as_str)
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
            .and_then(|value| crate::app::app_view::RemoteTuiLayout::from_json(&value));
        app.set_remote_tui_layout(layout);
        return true;
    }
    let lines = match params.get("widgetLines").or_else(|| params.get("lines")) {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::Array(values)) => Some(
            values
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_owned)
                .collect(),
        ),
        Some(_) => return false,
    };
    let placement = match params
        .get("widgetPlacement")
        .or_else(|| params.get("placement"))
        .and_then(serde_json::Value::as_str)
    {
        Some("belowEditor") => crate::app::app_view::ExternalWidgetPlacement::BelowEditor,
        _ => crate::app::app_view::ExternalWidgetPlacement::AboveEditor,
    };
    app.set_external_widget(key, lines, placement)
}

fn handle_pi_ui_remote_tui(notif: &acp::ExtNotification, app: &mut AppView) -> bool {
    let Some(params) = pi_ui_params(notif) else {
        return false;
    };
    let op = params
        .get("op")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let id = params
        .get("id")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let title = params
        .get("title")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let lines = match params.get("lines") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::Array(values)) => Some(
            values
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_owned)
                .collect(),
        ),
        Some(_) => None,
    };
    app.apply_remote_tui(op, id, lines, title)
}

fn handle_pi_ui_title(notif: &acp::ExtNotification, app: &mut AppView) -> bool {
    let Some(params) = pi_ui_params(notif) else {
        return false;
    };
    let Some(title) = params.get("title").and_then(serde_json::Value::as_str) else {
        return false;
    };
    super::set_terminal_title(title);
    // Bootstrap/rename session metadata is durable and owns the prompt-border
    // title. An extension's `ctx.ui.setTitle()` is temporary terminal UI and
    // must be undone by Pi's reload reset without renaming the session.
    if params.get("source").and_then(serde_json::Value::as_str) != Some("extension") {
        app.set_external_session_title(title);
    }
    true
}

/// Pi interactive invokes `resetExtensionUI()` just before replacing its
/// extension runner during reload. The adapter emits this only after Pi's
/// streaming/compaction gates pass; clear the equivalent Pager projections
/// and restore the durable Pi session title.
fn handle_pi_ui_reset_extension_ui(app: &mut AppView) -> bool {
    let Some(session_title) = app.reset_external_extension_ui() else {
        return false;
    };
    super::set_terminal_title(&session_title);
    true
}

/// `pi/ui/plan_file` — adapter publishes the session plan sidecar path so
/// `/view-plan` and the plan preview can locate the Pi-owned plan file.
fn handle_pi_ui_plan_file(notif: &acp::ExtNotification, app: &mut AppView) -> bool {
    let Some(params) = pi_ui_params(notif) else {
        return false;
    };
    let Some(plan_path) = params
        .get("planFilePath")
        .and_then(serde_json::Value::as_str)
    else {
        return false;
    };
    // Route to the active agent (same pattern as other pi/ui/* handlers).
    let ActiveView::Agent(id) = app.active_view else {
        return false;
    };
    if let Some(agent) = app.agents.get_mut(&id) {
        agent.pi_plan_file_path = Some(std::path::PathBuf::from(plan_path));
    }
    true
}

fn handle_pi_ui_session_catalog(notif: &acp::ExtNotification, app: &mut AppView) -> bool {
    let Some(params) = pi_ui_params(notif) else {
        return false;
    };
    let Some(sessions) = params.get("sessions").and_then(serde_json::Value::as_array) else {
        return false;
    };
    let Some(scope) = params.get("scope").and_then(serde_json::Value::as_str) else {
        return false;
    };
    let timestamp = |value: Option<&serde_json::Value>| {
        value
            .and_then(|value| {
                value
                    .as_i64()
                    .and_then(chrono::DateTime::from_timestamp_millis)
            })
            .or_else(|| {
                value
                    .and_then(serde_json::Value::as_str)
                    .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
                    .map(|value| value.with_timezone(&chrono::Utc))
            })
            .unwrap_or_else(chrono::Utc::now)
    };
    let entries = sessions
        .iter()
        .filter_map(|session| {
            let id = session.get("id")?.as_str()?.to_owned();
            let summary = session
                .get("summary")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("(no messages)")
                .to_owned();
            let cwd = session
                .get("cwd")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned();
            Some(crate::app::app_view::SessionPickerEntry {
                id,
                summary,
                name: session
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned),
                first_message: session
                    .get("firstMessage")
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned),
                session_path: session
                    .get("sessionPath")
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned),
                total_tokens: session
                    .get("totalTokens")
                    .and_then(serde_json::Value::as_u64),
                total_cost: session.get("totalCost").and_then(serde_json::Value::as_f64),
                updated_at: timestamp(session.get("updatedAt")),
                created_at: timestamp(session.get("createdAt")),
                cwd: cwd.clone(),
                hostname: None,
                source: "pi".to_owned(),
                model_id: session
                    .get("modelId")
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned),
                num_messages: session
                    .get("messageCount")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0) as usize,
                last_active_at: None,
                branch: None,
                repo_name: crate::views::session_picker::repo_name_from_cwd(&cwd),
                worktree_label: None,
                parent_session_path: session
                    .get("parentSessionPath")
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned),
                last_turn_summary: session
                    .get("lastTurnSummary")
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned),
                last_recap: session
                    .get("lastRecap")
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned),
                card_detail: None,
            })
        })
        .collect();
    app.set_external_session_catalog(entries, scope)
}

fn handle_pi_ui_editor_text(notif: &acp::ExtNotification, app: &mut AppView) -> bool {
    let Some(params) = pi_ui_params(notif) else {
        return false;
    };
    let Some(text) = params
        .get("text")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
    else {
        return false;
    };
    app.set_external_editor_text(text)
}

/// Retract a native QuestionView after Pi's dialog timeout resolves.
/// Pi includes the timeout on the original request but emits no later cancel
/// event, so the adapter sends this narrow lifecycle notification.
fn handle_pi_ui_cancel_interaction(notif: &acp::ExtNotification, app: &mut AppView) -> bool {
    let Some(params) = pi_ui_params(notif) else {
        return false;
    };
    let Some(tool_call_id) = params
        .get("toolCallId")
        .or_else(|| params.get("tool_call_id"))
        .and_then(serde_json::Value::as_str)
    else {
        return false;
    };
    app.dismiss_external_interaction(tool_call_id)
}

fn handle_version_mismatch(notif: &acp::ExtNotification, app: &mut AppView) -> bool {
    let Some(banner) = crate::acp::version_mismatch_banner(notif.params.get()) else {
        tracing::warn!("ignoring x.ai/leader/version_mismatch without usable versions");
        return false;
    };
    app.show_toast(&banner);
    true
}

fn handle_interjection(notif: &acp::ExtNotification, app: &mut AppView) -> bool {
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(notif.params.get()) else {
        tracing::warn!("Failed to parse x.ai/session/interjection");
        return false;
    };
    let Some(session_id) = parsed.get("sessionId").and_then(|v| v.as_str()) else {
        return false;
    };
    let Some(text) = parsed.get("text").and_then(|v| v.as_str()) else {
        return false;
    };
    let interjection_id = parsed.get("interjectionId").and_then(|v| v.as_str());

    let sid = acp::SessionId::new(session_id.to_string());
    let Some(SessionMatch::Root(id)) = find_session_match(app, &sid) else {
        return false;
    };
    let is_active = is_matched_agent_active(app, id);
    let Some(agent) = app.agents.get_mut(&id) else {
        return false;
    };

    if let Some(iid) = interjection_id {
        if agent.self_interjection_ids.remove(iid) {
            return false;
        }
        if agent.is_self_originated_prompt(iid)
            && let Some((entry_id, _)) = agent.send_now_painted_blocks.remove(iid)
        {
            agent.clear_send_now_expectation();
            if let Some(index) = agent.scrollback.index_of_id(entry_id)
                && let Some(RenderBlock::UserPrompt(block)) = agent
                    .scrollback
                    .entry_mut(index)
                    .map(|entry| &mut entry.block)
            {
                block.is_interjection = true;
            }
            return false;
        }
    }

    agent
        .scrollback
        .push_block(RenderBlock::interjection_prompt(text));
    is_active
}

fn handle_ext_method(ext: xai_acp_lib::AcpArgs<acp::ExtRequest>, app: &mut AppView) -> bool {
    match ext.request.method.as_ref() {
        "x.ai/ask_user_question" => handle_ask_user_question(ext, app),
        "x.ai/exit_plan_mode" => handle_exit_plan_mode(ext, app),
        "x.ai/mcp/elicit" => handle_mcp_elicit(ext, app),
        unknown => {
            tracing::warn!("Unknown ext_method: {unknown}");
            ext.response_tx
                .send(Err(acp::Error::new(
                    -32601,
                    format!("Method not found: {unknown}"),
                )))
                .ok();
            false
        }
    }
}

#[cfg(test)]
mod tests;
