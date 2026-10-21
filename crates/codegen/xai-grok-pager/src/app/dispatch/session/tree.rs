//! Pi `/tree` dispatch: native SessionTree modal + navigate/label bridge.

use crate::acp::tracker::AcpUpdateTracker;
use crate::app::actions::{Effect, SessionTreeNode};
use crate::app::agent::AgentId;
use crate::app::app_view::AppView;
use crate::app::dispatch::ctx::{get_active_agent, with_active_agent};
use crate::scrollback::block::RenderBlock;
use crate::scrollback::state::ScrollbackState;
use crate::views::modal::ActiveModal;
use crate::views::session_tree::SessionTreeState;

pub(in crate::app::dispatch) fn dispatch_show_session_tree(app: &mut AppView) -> Vec<Effect> {
    if !app.external_agent {
        app.show_toast("Session tree is only available for Pi sessions");
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
    let skip_summary = app.current_ui.pi_tree_skip_summary_prompt;
    with_active_agent(app, |agent| {
        let mut state = SessionTreeState::loading();
        state.status = Some("Fetching Pi session tree…".into());
        state.skip_summary_prompt = skip_summary;
        agent.active_modal = Some(ActiveModal::SessionTree {
            state,
            window: crate::views::modal_window::ModalWindowState::new(),
        });
    });
    vec![Effect::FetchSessionTree {
        agent_id,
        session_id,
    }]
}

pub(in crate::app::dispatch) fn dispatch_navigate_session_tree(
    app: &mut AppView,
    entry_id: String,
    summarize: bool,
    custom_instructions: Option<String>,
) -> Vec<Effect> {
    if !app.external_agent {
        app.show_toast("Session tree is only available for Pi sessions");
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
    if entry_id.trim().is_empty() {
        app.show_toast("Tree entry id is empty");
        return vec![];
    }
    with_active_agent(app, |agent| {
        agent.active_modal = None;
    });
    // Status feedback while the navigate_tree ACP call (which performs the
    // branch summary server-side) is in flight. Distinguish the modes so the
    // user knows what is happening during the wait.
    let toast = if summarize {
        if custom_instructions
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .is_some()
        {
            "Navigating · generating branch summary (custom)…"
        } else {
            "Navigating · generating branch summary…"
        }
    } else {
        "Navigating session tree…"
    };
    app.show_toast(toast);
    vec![Effect::NavigateSessionTree {
        agent_id,
        session_id,
        entry_id,
        summarize,
        custom_instructions,
    }]
}

pub(in crate::app::dispatch) fn dispatch_label_session_tree_entry(
    app: &mut AppView,
    entry_id: String,
    label: Option<String>,
) -> Vec<Effect> {
    if !app.external_agent {
        return vec![];
    }
    let Some(agent) = get_active_agent(app) else {
        return vec![];
    };
    let Some(session_id) = agent.session.session_id.as_ref().map(|s| s.0.to_string()) else {
        return vec![];
    };
    let agent_id = agent.session.id;
    if entry_id.trim().is_empty() {
        return vec![];
    }
    vec![Effect::LabelSessionTreeEntry {
        agent_id,
        session_id,
        entry_id,
        label,
    }]
}

pub(in crate::app::dispatch) fn dispatch_session_tree_closed(app: &mut AppView) -> Vec<Effect> {
    with_active_agent(app, |agent| {
        if matches!(
            agent.active_modal,
            Some(ActiveModal::SessionTree { .. }) | Some(ActiveModal::TreeMap { .. })
        ) {
            agent.active_modal = None;
        }
    });
    vec![]
}

/// Preview a file-only rollback to an ancestor entry. The extension computes
/// the plan and reports via toast; the modal stays open.
pub(in crate::app::dispatch) fn dispatch_rollback_files_preview(
    app: &mut AppView,
    entry_id: String,
) -> Vec<Effect> {
    if !app.external_agent {
        app.show_toast("File rollback is only available for Pi sessions");
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
    if entry_id.trim().is_empty() {
        app.show_toast("Tree entry id is empty");
        return vec![];
    }
    app.show_toast("Previewing file rollback…");
    vec![Effect::RollbackFilesPreview {
        agent_id,
        session_id,
        entry_id,
    }]
}

/// Execute a file-only rollback to an ancestor entry. Active leaf and
/// conversation are NOT changed — only tracked files are restored.
pub(in crate::app::dispatch) fn dispatch_rollback_files_execute(
    app: &mut AppView,
    entry_id: String,
) -> Vec<Effect> {
    if !app.external_agent {
        app.show_toast("File rollback is only available for Pi sessions");
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
    if entry_id.trim().is_empty() {
        app.show_toast("Tree entry id is empty");
        return vec![];
    }
    app.show_toast("Rolling back files…");
    vec![Effect::RollbackFilesExecute {
        agent_id,
        session_id,
        entry_id,
    }]
}

pub(in crate::app::dispatch) fn handle_session_tree_loaded(
    app: &mut AppView,
    agent_id: AgentId,
    _session_id: String,
    leaf_id: Option<String>,
    nodes: Vec<SessionTreeNode>,
) -> Vec<Effect> {
    let Some(agent) = app.agents.get_mut(&agent_id) else {
        return vec![];
    };
    if nodes.is_empty() {
        agent.active_modal = None;
        app.show_toast("Session tree is empty");
        return vec![];
    }
    let count = nodes.len();
    match agent.active_modal.as_mut() {
        Some(ActiveModal::SessionTree { state, .. }) => {
            state.replace_nodes(nodes, leaf_id);
            state.status = None;
        }
        Some(ActiveModal::TreeMap { state, .. }) => {
            state.replace_nodes(nodes, leaf_id);
        }
        // Modal was closed while fetch was in flight — drop the result.
        _ => return vec![],
    }
    if count > 200 {
        app.show_toast(&format!("Tree loaded ({count} nodes)"));
    }
    vec![]
}

pub(in crate::app::dispatch) fn handle_session_tree_failed(
    app: &mut AppView,
    agent_id: AgentId,
    error: String,
) -> Vec<Effect> {
    if let Some(agent) = app.agents.get_mut(&agent_id) {
        agent.active_modal = None;
    }
    app.show_toast(&format!("Couldn't load session tree: {error}"));
    vec![]
}

pub(in crate::app::dispatch) fn handle_session_tree_navigated(
    app: &mut AppView,
    agent_id: AgentId,
    session_id: String,
    leaf_id: Option<String>,
    editor_text: Option<String>,
) -> Vec<Effect> {
    let Some(agent) = app.agents.get_mut(&agent_id) else {
        return vec![];
    };
    if let Some(editor_text) = editor_text.filter(|text| !text.is_empty())
        && agent.prompt.text().trim().is_empty()
    {
        agent.prompt.set_text(&editor_text);
    }
    while agent.scrollback.in_batch() {
        agent.scrollback.end_batch();
    }
    if let Some(pid) = agent.loading_placeholder_id.take() {
        agent.scrollback.remove_entry(pid);
    }
    agent.abort_session_reload();
    agent.active_modal = None;
    agent.session.tracker = AcpUpdateTracker::new();
    agent.todo = crate::views::todo_pane::TodoPane::new();
    let mut scrollback = ScrollbackState::new();
    scrollback.set_appearance(agent.scrollback.appearance().clone());
    let placeholder = scrollback.push_block(RenderBlock::system(format!(
        "Reloading branch{}…",
        leaf_id
            .as_deref()
            .map(|id| format!(" ({id})"))
            .unwrap_or_default()
    )));
    agent.scrollback = scrollback;
    agent.loading_placeholder_id = Some(placeholder);
    agent.begin_replay_window();
    agent.scrollback.begin_batch();
    let session_cwd = Some(agent.session.cwd.clone());
    let chat_kind = agent.chat_kind;
    app.show_toast("Navigated to selected point");
    vec![Effect::LoadSession {
        agent_id,
        session_id,
        session_cwd,
        chat_kind,
    }]
}

pub(in crate::app::dispatch) fn handle_session_tree_navigate_failed(
    app: &mut AppView,
    agent_id: AgentId,
    error: String,
) -> Vec<Effect> {
    if let Some(agent) = app.agents.get_mut(&agent_id) {
        // Keep tree open if present; only clear on hard failure path callers.
        agent.abort_session_reload();
    }
    app.show_toast(&format!("Tree navigation failed: {error}"));
    vec![]
}

pub(in crate::app::dispatch) fn handle_session_tree_labeled(
    app: &mut AppView,
    agent_id: AgentId,
    _session_id: String,
    entry_id: String,
    label: Option<String>,
    leaf_id: Option<String>,
    nodes: Vec<SessionTreeNode>,
) -> Vec<Effect> {
    let Some(agent) = app.agents.get_mut(&agent_id) else {
        return vec![];
    };
    if let Some(ActiveModal::SessionTree { state, .. }) = agent.active_modal.as_mut() {
        state.replace_nodes(nodes, leaf_id);
        state.focus = crate::views::session_tree::SessionTreeFocus::List;
        state.label_draft.clear();
    }
    let msg = match label.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(text) => format!("Labeled {entry_id}: {text}"),
        None => format!("Cleared label on {entry_id}"),
    };
    app.show_toast(&msg);
    vec![]
}

pub(in crate::app::dispatch) fn handle_session_tree_label_failed(
    app: &mut AppView,
    _agent_id: AgentId,
    error: String,
) -> Vec<Effect> {
    app.show_toast(&format!("Couldn't update tree label: {error}"));
    vec![]
}

// =========================================================================
// /tree-map: branch map (user-messages-only fork view)
// =========================================================================

pub(in crate::app::dispatch) fn dispatch_show_tree_map(app: &mut AppView) -> Vec<Effect> {
    if !app.external_agent {
        app.show_toast("Branch map is only available for Pi sessions");
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
        let state = crate::views::tree_map::TreeMapState::loading();
        agent.active_modal = Some(ActiveModal::TreeMap {
            state,
            window: crate::views::modal_window::ModalWindowState::new(),
        });
    });
    vec![Effect::FetchSessionTree {
        agent_id,
        session_id,
    }]
}

pub(in crate::app::dispatch) fn handle_tree_map_loaded(
    app: &mut AppView,
    agent_id: AgentId,
    _session_id: String,
    leaf_id: Option<String>,
    nodes: Vec<SessionTreeNode>,
) -> Vec<Effect> {
    let Some(agent) = app.agents.get_mut(&agent_id) else {
        return vec![];
    };
    if nodes.is_empty() {
        agent.active_modal = None;
        app.show_toast("Session tree is empty");
        return vec![];
    }
    match agent.active_modal.as_mut() {
        Some(ActiveModal::TreeMap { state, .. }) => {
            state.replace_nodes(nodes, leaf_id);
        }
        _ => return vec![],
    }
    vec![]
}

pub(in crate::app::dispatch) fn dispatch_tree_map_closed(app: &mut AppView) -> Vec<Effect> {
    with_active_agent(app, |agent| {
        if matches!(agent.active_modal, Some(ActiveModal::TreeMap { .. })) {
            agent.active_modal = None;
        }
    });
    vec![]
}
