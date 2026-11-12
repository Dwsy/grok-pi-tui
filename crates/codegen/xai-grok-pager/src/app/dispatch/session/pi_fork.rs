//! Pi message-level `/fork`: jump-style prompt list overlay + RPC fork reload.

use crate::acp::tracker::AcpUpdateTracker;
use crate::app::actions::{Effect, PiForkMessage};
use crate::app::agent::AgentId;
use crate::app::app_view::AppView;
use crate::app::dispatch::ctx::{get_active_agent, with_active_agent};
use crate::scrollback::block::RenderBlock;
use crate::scrollback::state::ScrollbackState;
use crate::slash::commands::fork::ForkArgs;
use crate::views::fork_picker::ForkPickerState;
use agent_client_protocol as acp;

/// External-profile entry for slash `/fork`.
///
/// Grok peer-agent fork is intentionally not used here: Pi owns session files
/// and message-level branching via RPC `fork` / `get_fork_messages`.
pub(in crate::app::dispatch) fn dispatch_pi_message_fork(
    app: &mut AppView,
    args: ForkArgs,
    to_dashboard: bool,
) -> Vec<Effect> {
    if !app.external_agent {
        app.show_toast("Pi message fork is only available for Pi sessions");
        return vec![];
    }
    if to_dashboard && !crate::views::dashboard::dashboard_enabled() {
        app.show_toast("Enable the Agent Dashboard before forking to it");
        return vec![];
    }
    if args.worktree_override.is_some() {
        app.show_toast("Pi /fork does not support --worktree");
        return vec![];
    }
    match args
        .directive
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
    {
        Some(entry_id) => dispatch_execute_pi_fork(app, entry_id, to_dashboard),
        None => dispatch_show_pi_fork_picker(app, to_dashboard),
    }
}

pub(in crate::app::dispatch) fn dispatch_pi_fork_dismiss(app: &mut AppView) -> Vec<Effect> {
    with_active_agent(app, |agent| {
        agent.dismiss_fork_picker();
    });
    vec![]
}

fn dispatch_show_pi_fork_picker(app: &mut AppView, to_dashboard: bool) -> Vec<Effect> {
    let Some(agent) = get_active_agent(app) else {
        app.show_toast("No active session");
        return vec![];
    };
    if agent.fork_slot_taken() {
        app.show_toast("Cannot open fork picker right now");
        return vec![];
    }
    let Some(session_id) = agent.session.session_id.as_ref().map(|s| s.0.to_string()) else {
        app.show_toast("No active session");
        return vec![];
    };
    let agent_id = agent.session.id;
    // Close competing prompt-area overlays before fetching.
    with_active_agent(app, |agent| {
        agent.dismiss_jump_picker();
        agent.dismiss_fork_picker();
        agent.active_modal = None;
    });
    app.show_toast("Loading fork messages…");
    vec![Effect::FetchPiForkMessages {
        agent_id,
        session_id,
        to_dashboard,
    }]
}

fn dispatch_execute_pi_fork(
    app: &mut AppView,
    entry_id: String,
    to_dashboard: bool,
) -> Vec<Effect> {
    let Some(agent) = get_active_agent(app) else {
        app.show_toast("No active session");
        return vec![];
    };
    let Some(session_id) = agent.session.session_id.as_ref().map(|s| s.0.to_string()) else {
        app.show_toast("No active session");
        return vec![];
    };
    let agent_id = agent.session.id;
    if entry_id.trim().is_empty() {
        app.show_toast("Fork entry id is empty");
        return vec![];
    }
    with_active_agent(app, |agent| {
        agent.dismiss_fork_picker();
        agent.active_modal = None;
    });
    app.show_toast("Forking session…");
    vec![Effect::ForkPiSession {
        agent_id,
        session_id,
        entry_id,
        to_dashboard,
    }]
}

pub(in crate::app::dispatch) fn handle_pi_fork_messages_loaded(
    app: &mut AppView,
    agent_id: AgentId,
    _session_id: String,
    messages: Vec<PiForkMessage>,
    to_dashboard: bool,
) -> Vec<Effect> {
    let Some(agent) = app.agents.get_mut(&agent_id) else {
        return vec![];
    };
    if messages.is_empty() {
        agent.dismiss_fork_picker();
        app.show_toast("No messages to fork from");
        return vec![];
    }
    if agent.fork_slot_taken() {
        app.show_toast("Cannot open fork picker right now");
        return vec![];
    }
    agent.dismiss_jump_picker();
    agent.fork_state = Some(ForkPickerState::with_dashboard_target(
        messages,
        to_dashboard,
    ));
    vec![]
}

pub(in crate::app::dispatch) fn handle_pi_fork_messages_failed(
    app: &mut AppView,
    agent_id: AgentId,
    error: String,
) -> Vec<Effect> {
    if let Some(agent) = app.agents.get_mut(&agent_id) {
        agent.dismiss_fork_picker();
    }
    app.show_toast(&format!("Couldn't load fork messages: {error}"));
    vec![]
}

pub(in crate::app::dispatch) fn handle_pi_session_forked(
    app: &mut AppView,
    agent_id: AgentId,
    previous_session_id: String,
    session_id: String,
    editor_text: Option<String>,
    to_dashboard: bool,
) -> Vec<Effect> {
    let mut effects = handle_pi_session_replaced(
        app,
        agent_id,
        previous_session_id,
        session_id,
        editor_text,
        "Reloading forked session",
        "Forked to new session",
    );
    if to_dashboard
        && effects.iter().any(
            |effect| matches!(effect, Effect::LoadSession { agent_id: id, .. } if *id == agent_id),
        )
    {
        // One Pi RPC host can run only one live session. The old session
        // remains persisted and appears as a dormant dashboard roster row.
        effects.extend(super::super::dashboard::dispatch_open_dashboard(app));
        if matches!(
            app.active_view,
            crate::app::app_view::ActiveView::AgentDashboard
        ) {
            if let Some(dashboard) = app.dashboard.as_mut() {
                dashboard.focus_row(crate::views::dashboard::DashboardRowId::TopLevel(agent_id));
            }
        }
    }
    effects
}

pub(in crate::app::dispatch) fn handle_pi_session_fork_failed(
    app: &mut AppView,
    agent_id: AgentId,
    error: String,
) -> Vec<Effect> {
    if let Some(agent) = app.agents.get_mut(&agent_id) {
        agent.abort_session_reload();
        agent.dismiss_fork_picker();
    }
    app.show_toast(&format!("Fork failed: {error}"));
    vec![]
}

pub(in crate::app::dispatch) fn dispatch_pi_clone(app: &mut AppView) -> Vec<Effect> {
    if !app.external_agent {
        app.show_toast("Pi /clone is only available for Pi sessions");
        return vec![];
    }
    let Some(agent) = get_active_agent(app) else {
        app.show_toast("No active session");
        return vec![];
    };
    let Some(session_id) = agent.session.session_id.as_ref().map(|s| s.0.to_string()) else {
        app.show_toast("No active session");
        return vec![];
    };
    let agent_id = agent.session.id;
    with_active_agent(app, |agent| {
        agent.dismiss_fork_picker();
        agent.active_modal = None;
    });
    app.show_toast("Cloning session…");
    vec![Effect::ClonePiSession {
        agent_id,
        session_id,
    }]
}

pub(in crate::app::dispatch) fn handle_pi_session_cloned(
    app: &mut AppView,
    agent_id: AgentId,
    previous_session_id: String,
    session_id: String,
) -> Vec<Effect> {
    // Pi clears the editor after clone (no selected user message).
    handle_pi_session_replaced(
        app,
        agent_id,
        previous_session_id,
        session_id,
        Some(String::new()),
        "Reloading cloned session",
        "Cloned to new session",
    )
}

pub(in crate::app::dispatch) fn handle_pi_session_clone_failed(
    app: &mut AppView,
    agent_id: AgentId,
    error: String,
) -> Vec<Effect> {
    if let Some(agent) = app.agents.get_mut(&agent_id) {
        agent.abort_session_reload();
        agent.dismiss_fork_picker();
    }
    app.show_toast(&format!("Clone failed: {error}"));
    vec![]
}

pub(in crate::app::dispatch) fn dispatch_pi_reload(app: &mut AppView) -> Vec<Effect> {
    if !app.external_agent {
        app.show_toast("Pi /reload is only available for Pi sessions");
        return vec![];
    }
    let Some(agent) = get_active_agent(app) else {
        app.show_toast("No active session");
        return vec![];
    };
    let Some(session_id) = agent.session.session_id.as_ref().map(|s| s.0.to_string()) else {
        app.show_toast("No active session");
        return vec![];
    };
    let agent_id = agent.session.id;
    // Align with Pi interactive loading copy.
    app.show_toast(
        "Reloading keybindings, extensions, skills, prompts, themes, and context files...",
    );
    vec![Effect::ReloadPiSession {
        agent_id,
        session_id,
    }]
}

pub(in crate::app::dispatch) fn handle_pi_session_reloaded(
    app: &mut AppView,
    _agent_id: AgentId,
    _session_id: String,
) -> Vec<Effect> {
    // Pi interactive re-registers themes after reload; rescan so /theme sees
    // newly added or edited JSON without restarting grok-pi.
    let cwd = get_active_agent(app)
        .map(|agent| agent.session.cwd.clone())
        .unwrap_or_else(|| app.cwd.clone());
    let report = crate::theme::pi::rediscover(&cwd);
    if report.errors.is_empty() {
        app.show_toast(
            "Reloaded keybindings, extensions, skills, prompts, themes, and context files",
        );
    } else {
        app.show_toast(&format!(
            "Reloaded resources; theme rediscovery reported {} issue(s)",
            report.errors.len()
        ));
    }
    vec![]
}

pub(in crate::app::dispatch) fn handle_pi_session_reload_failed(
    app: &mut AppView,
    _agent_id: AgentId,
    error: String,
) -> Vec<Effect> {
    app.show_toast(&format!("Reload failed: {error}"));
    vec![]
}

fn handle_pi_session_replaced(
    app: &mut AppView,
    agent_id: AgentId,
    _previous_session_id: String,
    session_id: String,
    editor_text: Option<String>,
    reload_label: &str,
    success_toast: &str,
) -> Vec<Effect> {
    let Some(agent) = app.agents.get_mut(&agent_id) else {
        return vec![];
    };
    if let Some(text) = editor_text {
        agent.prompt.set_text(&text);
    }
    while agent.scrollback.in_batch() {
        agent.scrollback.end_batch();
    }
    if let Some(pid) = agent.loading_placeholder_id.take() {
        agent.scrollback.remove_entry(pid);
    }
    agent.abort_session_reload();
    agent.dismiss_fork_picker();
    agent.active_modal = None;
    agent.session.tracker = AcpUpdateTracker::new();
    agent.todo = crate::views::todo_pane::TodoPane::new();
    let mut scrollback = ScrollbackState::new();
    scrollback.set_appearance(agent.scrollback.appearance().clone());
    let placeholder = scrollback.push_block(RenderBlock::system(format!(
        "{reload_label} ({session_id})…"
    )));
    agent.scrollback = scrollback;
    agent.loading_placeholder_id = Some(placeholder);
    agent.begin_replay_window();
    agent.scrollback.begin_batch();
    agent.bind_session_id(acp::SessionId::new(session_id.clone()));
    let session_cwd = Some(agent.session.cwd.clone());
    let chat_kind = agent.chat_kind;
    app.show_toast(success_toast);
    vec![Effect::LoadSession {
        agent_id,
        session_id,
        session_cwd,
        chat_kind,
    }]
}
