use crate::{
    background_bash_bridge::{
        BackgroundBashProjection, BackgroundBashTask, background_bash_notification,
        background_bash_output_update, drain_running_background_bash,
        parse_background_bash_message, parse_background_bash_status,
        parse_background_bash_tool_result, record_background_bash,
    },
    btw_bridge::parse_btw_message,
    context_projection::{
        build_session_info_response, context_tokens_from_stats, context_tokens_from_usage,
        entries_to_messages_value, parse_context_breakdown,
    },
    goal_host::{GoalControl, GoalHost},
    loop_host,
    model::{
        PiCommand, PiEntryReplayCache, PiHistoryItem, PiModel, PiReplayEntry, PiSessionSwitch,
        PiSessionTree, PiState, PiToolContent, extract_delta, json_text, parse_commands,
        parse_messages, parse_models, parse_session_switch, parse_session_tree, parse_state,
        scan_local_sessions, scan_local_sessions_for_cwd, string, tree_entry_editor_text,
    },
    pi_rpc::PiRpc,
    pi_workflow_backend::{
        BridgeCommandRequest, BridgeCommandTx, WORKFLOW_CANCEL_COMMAND, WORKFLOW_SPAWN_COMMAND,
    },
    prompt_bridge::{
        direct_bash_command, format_bash_result, prompt_response, prompt_streaming_behavior,
        prompt_to_pi, queue_lane_for_behavior,
    },
    queue_bridge::{
        QueueEntry, QueueLane, QueueMirror, QueueOrigin, queue_changed_params, string_list,
    },
    recap_bridge::{parse_recap_message, session_recap_notification},
    subagent_projection::{BridgeOperation, bridge_parent_session_id, parse_bridge_message},
    subagent_transport::SubagentEventTransport,
    todo_bridge::plan_update_for_tool,
    tool_projection::{
        bash_tool_output, edit_diff_content, history_tool_content, normalize_tool_raw_input,
        normalize_tool_raw_output, pi_result_text, tool_content, tool_kind,
    },
    workflow_host::{
        WorkflowHost, WorkflowRequest, format_outcome_for_tool, outcome_to_json,
        parse_workflow_request,
    },
};
use agent_client_protocol as acp;
use anyhow::{Result, anyhow, bail};
use indexmap::IndexMap;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{mpsc, oneshot};
use xai_acp_lib::{AcpClientMessage, acp_send};

mod agent;
mod events;
mod notifications;
mod queue_runtime;
mod recovery;
mod replay;
mod session;
mod tools;

#[derive(Debug, Clone)]
pub struct PiBootstrap {
    state: PiState,
    models: Vec<PiModel>,
    commands: Vec<PiCommand>,
}

impl PiBootstrap {
    pub async fn load(rpc: &PiRpc) -> Result<Self> {
        let state = parse_state(&rpc.request(json!({ "type": "get_state" })).await?);
        let mut models = parse_models(
            &rpc.request(json!({ "type": "get_available_models" }))
                .await?,
        );
        if let Some(current) = state.model.clone()
            && !models
                .iter()
                .any(|model| model.provider == current.provider && model.id == current.id)
        {
            models.push(current);
        }
        let commands = parse_commands(&rpc.request(json!({ "type": "get_commands" })).await?);
        Ok(Self {
            state,
            models,
            commands,
        })
    }

    pub fn acp_models(&self) -> Option<acp::SessionModelState> {
        let (available, current) = build_model_catalog(
            &self.models,
            self.state.model.as_ref(),
            &self.state.thinking_level,
        );
        let current = current.or_else(|| available.first().map(|(id, _)| id.clone()))?;
        Some(acp::SessionModelState::new(
            current,
            available.into_values().collect(),
        ))
    }

    pub fn acp_commands(&self, workflows_enabled: bool) -> Vec<acp::AvailableCommand> {
        command_catalog(&self.commands, workflows_enabled)
    }

    /// Pi session identifier used to seed the native Grok session surface.
    pub fn session_id(&self) -> &str {
        &self.state.session_id
    }

    /// Optional Pi session title used for Grok's terminal title and header.
    pub fn session_title(&self) -> Option<&str> {
        self.state.session_name.as_deref()
    }
}

struct ActivePrompt {
    id: u64,
    /// Pager-minted id (`_meta.promptId`); echoed on PromptResponse so non-running
    /// mid-turn completions are discarded instead of emitting phantom turns.
    client_prompt_id: Option<String>,
    completion: oneshot::Sender<PromptCompletion>,
    agent_started: bool,
    cancelled: bool,
}

struct PromptCompletion {
    reason: acp::StopReason,
    client_prompt_id: Option<String>,
}

const EXTENSION_QUEUE_STATUS_KEY: &str = "__pi_grok_queue_enqueue__";
/// Out-of-band terminal state for Pi-owned background Bash tasks. The private
/// Bash extension publishes here because `setStatus` is fire-and-forget in Pi's
/// RPC mode, while its bridge message shares the agent's queue lifetime.
const EXTENSION_BASH_TASK_STATUS_KEY: &str = "__pi_grok_bash_task__";

fn stop_reason_wire(reason: &acp::StopReason) -> &'static str {
    match reason {
        acp::StopReason::Cancelled => "cancelled",
        _ => "end_turn",
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct StreamSeen {
    text: bool,
    thought: bool,
}

#[derive(Default)]
struct PendingSubagentBridge {
    target_session_id: Option<String>,
    events: Vec<Value>,
}

impl PendingSubagentBridge {
    fn begin(&mut self, target_session_id: &str) -> Result<()> {
        if let Some(existing) = &self.target_session_id {
            bail!("Pi session transition to {existing} is already in progress");
        }
        self.target_session_id = Some(target_session_id.to_string());
        self.events.clear();
        Ok(())
    }

    fn defer_if_targeted(&mut self, event: &Value) -> Result<bool> {
        let Some(parent_session_id) = bridge_parent_session_id(event)? else {
            return Ok(false);
        };
        let Some(target_session_id) = &self.target_session_id else {
            return Ok(false);
        };
        if parent_session_id != target_session_id {
            return Ok(false);
        }
        self.events.push(event.clone());
        Ok(true)
    }

    fn commit_if_target(&mut self, target_session_id: &str) -> Result<Vec<Value>> {
        match self.target_session_id.as_deref() {
            None => Ok(Vec::new()),
            Some(current) if current == target_session_id => {
                self.target_session_id = None;
                Ok(std::mem::take(&mut self.events))
            }
            Some(current) => bail!(
                "Pi session transition to {current} is still pending while {target_session_id} loads"
            ),
        }
    }

    fn abandon(&mut self, target_session_id: &str) {
        if self.target_session_id.as_deref() == Some(target_session_id) {
            self.target_session_id = None;
            self.events.clear();
        }
    }
}

struct AdapterState {
    bootstrap: PiBootstrap,
    acp_session_id: String,
    model_map: HashMap<String, PiModel>,
    /// Flat append-log cache for active-branch replay. Pi's `get_entries.since`
    /// updates this incrementally when navigating within the same session.
    entry_replay_cache: PiEntryReplayCache,
    active_prompts: Vec<ActivePrompt>,
    queued_prompt_completions: HashMap<String, oneshot::Sender<PromptCompletion>>,
    next_prompt_id: u64,
    agent_running: bool,
    cancelling: bool,
    bash_running: bool,
    live_assistant: Option<StreamSeen>,
    session_dir: PathBuf,
    session_paths: HashMap<String, PathBuf>,
    /// Pi tool args keyed by toolCallId. End events may omit args; the pager
    /// still needs path/command when projecting native Read/Execute cards.
    tool_args: HashMap<String, Value>,
    /// Latest Pi context-window usage (tokens used). Stamped on ACP session
    /// updates as `_meta.totalTokens` so Grok's native context bar can render.
    last_context_tokens: Option<u64>,
    /// UTC ms when the current Pi agent turn began (`turnStartMs` on live ACP
    /// notifications). Mirrors stock Grok shell so the pager can pre-create
    /// Thinking… and drive turn timers / breathing animation.
    turn_start_ms: Option<i64>,
    /// UTC ms when the current LLM stream segment began (`streamStartMs`).
    /// Bumped on each assistant `message_start` / `turn_start` so stream
    /// boundaries match Grok shell semantics.
    stream_start_ms: Option<i64>,
    /// Client `promptId` of the in-flight primary turn (Pager-minted). Stamped
    /// on every live ACP `SessionNotification._meta.promptId` so the pager's
    /// prompt-id gate and turn chrome match stock Grok shell.
    live_prompt_id: Option<String>,
    /// Last full bash stdout (bytes) per toolCallId — used to emit
    /// `BashOutput.output_delta` on `tool_execution_update` so Execute cards
    /// stream instead of only jumping at tool end.
    bash_stream_output: HashMap<String, Vec<u8>>,
    /// Lifecycle mirror for Pi-owned background Bash tasks, keyed by task id.
    /// Terminal state arrives on two independent channels, and the shells die
    /// silently with the Pi child — this resolves both into one final state.
    background_bash_tasks: HashMap<String, BackgroundBashTask>,
    /// Local timing only; Pi owns compaction itself and reports its token result.
    compaction_started_at: Option<Instant>,
    /// Pi steering / follow-up queue mirrored as Grok `x.ai/queue/changed`.
    queue_mirror: QueueMirror,
    /// Last accepted live bridge sequence per child. The adapter uses this only
    /// to reject duplicate/out-of-order transport events; child lifecycle stays
    /// owned by the Pi extension.
    subagent_bridge_sequences: HashMap<String, u64>,
    /// Pi emits extension `session_start` events before its `switch_session`
    /// response reaches ACP. Buffer target-session subagent replay until ACP
    /// commits the matching session load, rather than validating it against the
    /// still-active Pager session.
    pending_subagent_bridge: PendingSubagentBridge,
    /// In-flight `/btw` side questions keyed by requestId (extension custom message).
    pending_btw: HashMap<String, oneshot::Sender<Result<String, String>>>,
    /// A recap command must finish before another recap can be requested.
    recap_in_flight: bool,
    /// Pi interactive replaces its editor while reloading, so a second reload
    /// cannot overlap the first. Keep the same session-level exclusion here
    /// for RPC callers that do not have that editor lifecycle.
    reload_in_flight: bool,
    /// Plan mode lifecycle tracker. The adapter is the sole owner of plan mode
    /// state — Pi RPC has no mode concept, and the Pager only renders.
    plan_mode: crate::plan_mode::PiPlanTracker,
    /// Process-private control file consumed by the injected Pi plan gate.
    /// It is deliberately not session persistence; the adapter rewrites it
    /// from its authoritative tracker after every transition.
    plan_mode_control: Option<PathBuf>,
    /// Storm guard for automatic Pi RPC crash recovery.
    rpc_recovery: recovery::RpcRecoveryTracker,
}

#[derive(Clone)]
pub struct PiAgent {
    rpc: PiRpc,
    client_tx: mpsc::UnboundedSender<AcpClientMessage>,
    state: Rc<RefCell<AdapterState>>,
    bash_control_meta: Option<PathBuf>,
    /// Process-unique JSON path written by `__pi_context_breakdown`.
    context_breakdown: Option<PathBuf>,
    /// Channel to execute workflow spawn/cancel bridge commands on the LocalSet.
    workflow_bridge_tx: BridgeCommandTx,
    workflow_bridge_rx: Rc<RefCell<Option<mpsc::UnboundedReceiver<BridgeCommandRequest>>>>,
    /// Lazy session-scoped upstream workflow host (xai-workflow + Pi spawn).
    workflow_host: Rc<RefCell<Option<std::sync::Arc<WorkflowHost>>>>,
    /// F2 pi_goal control file + GoalHost (None when feature off).
    goal_host: Rc<RefCell<Option<GoalHost>>>,
    /// Process-private path-based local IPC emitted by the Pi subagent extension.
    /// Child traffic belongs here, never in the parent SessionManager JSONL.
    subagent_transport: Option<Rc<SubagentEventTransport>>,
    /// Startup-resolved host capability; never re-reads disk or parent env.
    workflows_enabled: bool,
}

impl PiAgent {
    pub fn new(
        rpc: PiRpc,
        client_tx: mpsc::UnboundedSender<AcpClientMessage>,
        bootstrap: PiBootstrap,
        session_dir: PathBuf,
        bash_control_meta: Option<PathBuf>,
        context_breakdown: Option<PathBuf>,
        plan_mode_control: Option<PathBuf>,
        goal_control: Option<PathBuf>,
        subagent_transport: Option<SubagentEventTransport>,
        workflows_enabled: bool,
    ) -> Result<Self> {
        let acp_session_id = bootstrap.state.session_id.clone();
        let plan_file = plan_file_path(&bootstrap.state, &session_dir);
        let plan_mode = load_plan_tracker(&plan_file)?;
        let model_map = bootstrap
            .models
            .iter()
            .cloned()
            .map(|model| (model_key(&model), model))
            .collect();
        let (workflow_bridge_tx, workflow_bridge_rx) = mpsc::unbounded_channel();
        Ok(Self {
            rpc,
            client_tx,
            bash_control_meta,
            context_breakdown,
            workflow_bridge_tx,
            workflow_bridge_rx: Rc::new(RefCell::new(Some(workflow_bridge_rx))),
            workflow_host: Rc::new(RefCell::new(None)),
            goal_host: Rc::new(RefCell::new(goal_control.map(GoalHost::new))),
            subagent_transport: subagent_transport.map(Rc::new),
            workflows_enabled,
            state: Rc::new(RefCell::new(AdapterState {
                bootstrap,
                acp_session_id,
                model_map,
                entry_replay_cache: PiEntryReplayCache::default(),
                active_prompts: Vec::new(),
                queued_prompt_completions: HashMap::new(),
                next_prompt_id: 1,
                agent_running: false,
                cancelling: false,
                bash_running: false,
                live_assistant: None,
                session_dir: session_dir.clone(),
                session_paths: HashMap::new(),
                tool_args: HashMap::new(),
                last_context_tokens: None,
                turn_start_ms: None,
                stream_start_ms: None,
                live_prompt_id: None,
                bash_stream_output: HashMap::new(),
                background_bash_tasks: HashMap::new(),
                compaction_started_at: None,
                queue_mirror: QueueMirror::default(),
                subagent_bridge_sequences: HashMap::new(),
                pending_subagent_bridge: PendingSubagentBridge::default(),
                pending_btw: HashMap::new(),
                recap_in_flight: false,
                reload_in_flight: false,
                plan_mode,
                plan_mode_control,
                rpc_recovery: recovery::RpcRecoveryTracker::default(),
            })),
        })
    }

    pub async fn run_events(self: Rc<Self>, mut events: mpsc::UnboundedReceiver<Value>) {
        self.clone().spawn_rpc_watchdog();
        if let Some(mut bridge_rx) = self.workflow_bridge_rx.borrow_mut().take() {
            let agent = self.clone();
            tokio::task::spawn_local(async move {
                while let Some(req) = bridge_rx.recv().await {
                    let result = agent
                        .run_bridge_command(&req.command, &req.args)
                        .await
                        .map_err(|error| error.to_string());
                    let _ = req.reply.send(result);
                }
            });
        }
        let (subagent_tx, mut subagent_rx) = mpsc::unbounded_channel();
        let subagent_task = self.subagent_transport.clone().map(|transport| {
            tokio::task::spawn_local(async move { transport.forward(subagent_tx).await })
        });
        loop {
            tokio::select! {
                maybe_event = events.recv() => {
                    let Some(event) = maybe_event else { break; };
                    if let Err(error) = self.handle_event(event).await {
                        tracing::warn!(%error, "failed to adapt Pi event into Grok ACP");
                        self.send_ui_notification(&format!("Pi adapter: {error}"), Some("warning"))
                            .await;
                    }
                }
                maybe_subagent = subagent_rx.recv(), if self.subagent_transport.is_some() => {
                    let Some(event) = maybe_subagent else { continue; };
                    if let Err(error) = self.handle_subagent_bridge_message(&event).await {
                        tracing::warn!(%error, "failed to adapt transient Pi subagent event");
                    }
                }
            }
        }
        if let Some(task) = subagent_task {
            task.abort();
        }
        self.finish_prompts(acp::StopReason::Cancelled);
    }

    pub async fn refresh(&self) -> Result<PiBootstrap> {
        let bootstrap = PiBootstrap::load(&self.rpc).await?;
        self.replace_bootstrap(bootstrap.clone());
        Ok(bootstrap)
    }
}

fn build_model_catalog(
    models: &[PiModel],
    current: Option<&PiModel>,
    thinking_level: &str,
) -> (IndexMap<acp::ModelId, acp::ModelInfo>, Option<acp::ModelId>) {
    let mut available = IndexMap::new();
    for model in models {
        let id = acp::ModelId::new(model_key(model));
        let mut meta = serde_json::Map::new();
        if !model.provider.is_empty() {
            meta.insert("provider".into(), json!(model.provider));
        }
        meta.insert("modelId".into(), json!(model.id));
        if let Some(tokens) = model.context_window {
            meta.insert("totalContextTokens".into(), json!(tokens));
        }
        if let Some(tokens) = model.max_tokens {
            meta.insert("maxTokens".into(), json!(tokens));
        }
        if let Some(api) = model.api.as_ref() {
            meta.insert("api".into(), json!(api));
        }
        if let Some(base_url) = model.base_url.as_ref() {
            meta.insert("baseUrl".into(), json!(base_url));
        }
        meta.insert("acceptsImages".into(), json!(model.accepts_images));
        meta.insert("reasoning".into(), json!(model.reasoning));
        if !model.input.is_empty() {
            meta.insert(
                "inputModalities".into(),
                Value::Array(model.input.iter().cloned().map(Value::String).collect()),
            );
        }
        if model.cost_input.is_some()
            || model.cost_output.is_some()
            || model.cost_cache_read.is_some()
            || model.cost_cache_write.is_some()
        {
            let mut cost = serde_json::Map::new();
            if let Some(v) = model.cost_input {
                cost.insert("input".into(), json!(v));
            }
            if let Some(v) = model.cost_output {
                cost.insert("output".into(), json!(v));
            }
            if let Some(v) = model.cost_cache_read {
                cost.insert("cacheRead".into(), json!(v));
            }
            if let Some(v) = model.cost_cache_write {
                cost.insert("cacheWrite".into(), json!(v));
            }
            meta.insert("cost".into(), Value::Object(cost));
        }
        let reasoning_efforts = model_reasoning_efforts(model);
        if !reasoning_efforts.is_empty() {
            meta.insert("supportsReasoningEffort".into(), json!(true));
            if let Some(effort) = model.acp_effort_for_pi_level(thinking_level) {
                meta.insert("reasoningEffort".into(), json!(effort));
            }
            meta.insert("reasoningEfforts".into(), Value::Array(reasoning_efforts));
        }
        let description = model_catalog_description(model);
        let mut info = acp::ModelInfo::new(id.clone(), model.label.clone()).meta(Some(meta));
        if !description.is_empty() {
            info = info.description(description);
        }
        available.insert(id, info);
    }
    let current = current.map(|model| acp::ModelId::new(model_key(model)));
    (available, current)
}

/// Compact right-side detail for the native model picker.
/// Provider lives on the left row (`id [provider]`); this side mirrors
/// model-selector-x metadata: context / max-out / protocol / input / cost.
fn model_catalog_description(model: &PiModel) -> String {
    let mut parts = Vec::new();
    if let Some(tokens) = model.context_window {
        parts.push(format!("ctx {}", format_token_count(tokens)));
    }
    if let Some(tokens) = model.max_tokens {
        parts.push(format!("out {}", format_token_count(tokens)));
    }
    if let Some(api) = model.api.as_deref().and_then(format_protocol_short) {
        parts.push(format!("api {api}"));
    }
    let input = format_input_short(&model.input, model.accepts_images);
    if !input.is_empty() {
        parts.push(format!("in {input}"));
    }
    if model.reasoning {
        parts.push("⚡".into());
    }
    if let Some(cost) = format_cost_short(model) {
        parts.push(cost);
    }
    // Fallback when we only know the provider (no numeric metadata yet).
    if parts.is_empty() && !model.provider.is_empty() {
        return format!("[{}]", model.provider);
    }
    parts.join(" · ")
}

fn format_token_count(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        let millions = tokens as f64 / 1_000_000.0;
        if millions.fract() == 0.0 {
            format!("{}M", millions as u64)
        } else {
            format!("{millions:.1}M")
        }
    } else if tokens >= 1_000 {
        let thousands = tokens as f64 / 1_000.0;
        if thousands.fract() == 0.0 {
            format!("{}k", thousands as u64)
        } else {
            format!("{thousands:.0}k")
        }
    } else {
        tokens.to_string()
    }
}

fn format_protocol_short(api: &str) -> Option<&'static str> {
    match api {
        "openai-responses" | "openai-codex-responses" => Some("resp"),
        "openai-completions" => Some("comp"),
        "anthropic-messages" => Some("anth"),
        "google-generative-ai" => Some("goog"),
        _ => None,
    }
}

fn format_input_short(input: &[String], accepts_images: bool) -> String {
    if input.is_empty() {
        return if accepts_images {
            "txt+img".into()
        } else {
            "txt".into()
        };
    }
    let mut parts = Vec::new();
    if input.iter().any(|m| m.eq_ignore_ascii_case("text")) {
        parts.push("txt");
    }
    if input.iter().any(|m| m.eq_ignore_ascii_case("image")) || accepts_images {
        parts.push("img");
    }
    if input.iter().any(|m| m.eq_ignore_ascii_case("audio")) {
        parts.push("aud");
    }
    if parts.is_empty() {
        "txt".into()
    } else {
        parts.join("+")
    }
}

fn format_cost_short(model: &PiModel) -> Option<String> {
    let input = model.cost_input.unwrap_or(0.0);
    let output = model.cost_output.unwrap_or(0.0);
    if input == 0.0 && output == 0.0 {
        // Only claim free when cost fields were present.
        if model.cost_input.is_some() || model.cost_output.is_some() {
            return Some("free".into());
        }
        return None;
    }
    Some(format!(
        "${} / ${}",
        format_cost_num(input),
        format_cost_num(output)
    ))
}

fn format_cost_num(value: f64) -> String {
    if value == 0.0 {
        "0".into()
    } else if value < 0.01 {
        format!("{value:.3}")
    } else if value < 1.0 {
        format!("{value:.2}")
    } else if (value - value.round()).abs() < f64::EPSILON {
        format!("{}", value.round() as i64)
    } else if value < 10.0 {
        format!("{value:.1}")
    } else {
        format!("{}", value.round() as i64)
    }
}

/// Internal bridge commands injected by grok-pi; never advertised to slash UI.
const NAVIGATE_TREE_COMMAND: &str = "__pi_navigate_tree";
const LABEL_TREE_COMMAND: &str = "__pi_tree_label";
const RELOAD_COMMAND: &str = "__pi_reload";
const SUBAGENT_CANCEL_COMMAND: &str = "__pi_grok_subagent_cancel";
const RECAP_COMMAND: &str = "__pi_grok_recap";
const BTW_COMMAND: &str = "__pi_grok_btw";
const CONTEXT_BREAKDOWN_COMMAND: &str = "__pi_context_breakdown";
const SHORTCUT_DISPATCH_COMMAND: &str = "__pi_shortcut_dispatch";

fn is_bridge_command(name: &str) -> bool {
    name.eq_ignore_ascii_case(NAVIGATE_TREE_COMMAND)
        || name.eq_ignore_ascii_case(LABEL_TREE_COMMAND)
        || name.eq_ignore_ascii_case(RELOAD_COMMAND)
        || name.eq_ignore_ascii_case(SUBAGENT_CANCEL_COMMAND)
        || name.eq_ignore_ascii_case(RECAP_COMMAND)
        || name.eq_ignore_ascii_case(BTW_COMMAND)
        || name.eq_ignore_ascii_case(CONTEXT_BREAKDOWN_COMMAND)
        || name.eq_ignore_ascii_case(SHORTCUT_DISPATCH_COMMAND)
        || name.eq_ignore_ascii_case(WORKFLOW_SPAWN_COMMAND)
        || name.eq_ignore_ascii_case(WORKFLOW_CANCEL_COMMAND)
}

/// Ordered non-empty model refs from `models` array and/or legacy `model` field.
fn model_chain_from_params(params: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let mut push = |raw: &str| {
        let t = raw.trim();
        if !t.is_empty() && !out.iter().any(|x: &String| x == t) {
            out.push(t.to_owned());
        }
    };
    if let Some(arr) = params.get("models").and_then(Value::as_array) {
        for item in arr {
            if let Some(s) = item.as_str() {
                push(s);
            }
        }
    }
    if let Some(s) = string(params, &["model", "modelId", "recapModel", "btwModel"]) {
        push(s);
    }
    out
}

fn recap_extension_enabled() -> bool {
    env_flag_enabled(std::env::var("PI_GROK_RECAP").ok().as_deref())
}

fn env_flag_enabled(value: Option<&str>) -> bool {
    match value.map(str::trim) {
        Some("1") => true,
        Some(value) => value.eq_ignore_ascii_case("true") || value.eq_ignore_ascii_case("on"),
        None => false,
    }
}

fn btw_extension_enabled() -> bool {
    if let Ok(config) = xai_grok_shell::config::load_effective_config() {
        if config
            .get("ui")
            .and_then(|ui| ui.get("pi_btw"))
            .and_then(|v| v.as_bool())
            == Some(true)
        {
            return true;
        }
    }
    match std::env::var("PI_GROK_BTW") {
        Ok(v) => {
            let v = v.trim();
            v == "1" || v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("on")
        }
        Err(_) => false,
    }
}

fn bridge_command_is_registered(commands: &[PiCommand], command: &str) -> bool {
    let command = command.trim_start_matches('/');
    commands.iter().any(|available| {
        available
            .name
            .trim_start_matches('/')
            .eq_ignore_ascii_case(command)
    })
}

fn reserve_recap_request(in_flight: &mut bool) -> bool {
    if *in_flight {
        return false;
    }
    *in_flight = true;
    true
}

fn reserve_reload_request(in_flight: &mut bool) -> bool {
    if *in_flight {
        return false;
    }
    *in_flight = true;
    true
}

fn bridge_command_message(command: &str, args: &str) -> String {
    if args.trim().is_empty() {
        format!("/{command}")
    } else {
        format!("/{command} {args}")
    }
}

/// Operating-system language for recap output.
///
/// On macOS, `AppleLanguages` is the authoritative Language & Region order;
/// terminal locale variables can remain `C` or differ from the UI language.
/// Other platforms (and macOS fallback) use the standard locale variables.
fn system_language_tag() -> Option<String> {
    #[cfg(target_os = "macos")]
    if let Ok(output) = std::process::Command::new("defaults")
        .args(["read", "-g", "AppleLanguages"])
        .output()
        && output.status.success()
        && let Some(language) = first_apple_language(&String::from_utf8_lossy(&output.stdout))
    {
        return Some(language);
    }

    for key in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(value) = std::env::var(key)
            && let Some(language) = normalize_language_tag(&value)
        {
            return Some(language);
        }
    }
    None
}

fn first_apple_language(value: &str) -> Option<String> {
    value
        .split(|character: char| {
            character == '(' || character == ')' || character == ',' || character.is_whitespace()
        })
        .find_map(normalize_language_tag)
}

fn normalize_language_tag(value: &str) -> Option<String> {
    let tag = value
        .trim()
        .trim_matches(|character| character == '"' || character == '\'')
        .split('.')
        .next()
        .unwrap_or_default()
        .replace('_', "-");
    if tag.is_empty() || tag.eq_ignore_ascii_case("C") || tag.eq_ignore_ascii_case("POSIX") {
        None
    } else {
        Some(tag)
    }
}

fn command_catalog(commands: &[PiCommand], workflows_enabled: bool) -> Vec<acp::AvailableCommand> {
    // The adapter reports Pi's command catalog (normalized + deduped), minus
    // private bridge commands. When Pi workflows are enabled, inject the
    // upstream-aligned workflow slash surface so Pager autocomplete matches
    // stock Grok: /workflow, /create-workflow, and named workflow scripts.
    let mut seen = HashSet::new();
    let mut out: Vec<acp::AvailableCommand> = commands
        .iter()
        .filter_map(|command| {
            let name = command.name.trim().trim_start_matches('/');
            if name.is_empty() || is_bridge_command(name) || !seen.insert(name.to_ascii_lowercase())
            {
                return None;
            }
            let description = if command.description.trim().is_empty() {
                if command.source.trim().is_empty() {
                    "Pi command".to_string()
                } else {
                    format!("Pi {} command", command.source)
                }
            } else {
                command.description.clone()
            };
            let mut meta = serde_json::Map::new();
            if !command.source.trim().is_empty() {
                meta.insert(
                    "piCommandSource".to_string(),
                    Value::String(command.source.clone()),
                );
            }
            if !command.argument_completions.is_empty() {
                meta.insert(
                    "piArgumentCompletions".to_string(),
                    Value::Array(
                        command
                            .argument_completions
                            .iter()
                            .map(|item| {
                                json!({
                                    "value": item.value,
                                    "label": item.label,
                                    "description": item.description,
                                })
                            })
                            .collect(),
                    ),
                );
            }
            let mut available = acp::AvailableCommand::new(name.to_string(), description);
            if let Some(hint) = command
                .argument_hint
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                available = available.input(Some(acp::AvailableCommandInput::Unstructured(
                    acp::UnstructuredCommandInput::new(hint.to_string()),
                )));
            }
            if !meta.is_empty() {
                available = available.meta(meta);
            }
            Some(available)
        })
        .collect();

    if workflows_enabled {
        inject_workflow_slash_commands(&mut out, &mut seen);
    }
    out
}

fn inject_workflow_slash_commands(
    out: &mut Vec<acp::AvailableCommand>,
    seen: &mut HashSet<String>,
) {
    let push = |out: &mut Vec<acp::AvailableCommand>,
                seen: &mut HashSet<String>,
                name: &str,
                description: &str,
                hint: Option<&str>| {
        let key = name.to_ascii_lowercase();
        if !seen.insert(key) {
            return;
        }
        let mut cmd = acp::AvailableCommand::new(name.to_string(), description.to_string());
        if let Some(hint) = hint {
            cmd = cmd.input(Some(acp::AvailableCommandInput::Unstructured(
                acp::UnstructuredCommandInput::new(hint.to_string()),
            )));
        }
        out.push(cmd);
    };

    push(
        out,
        seen,
        "workflow",
        "Launch a saved workflow, or manage a run (pause, resume, stop, save)",
        Some("<name> [args] | pause|resume|stop|save [name]"),
    );
    push(
        out,
        seen,
        "workflows",
        "Show workflow runs (phases, agents, progress)",
        None,
    );
    push(
        out,
        seen,
        "create-workflow",
        "Author a new multi-agent workflow",
        Some("[goal]"),
    );

    // Named project/user/builtin scripts as first-class slash entries.
    let cwd = std::env::current_dir().ok();
    let listings = xai_grok_shell::session::workflow::list_workflows(cwd.as_deref());
    for listing in listings {
        let desc = format!("Workflow: {}", listing.description);
        push(out, seen, &listing.name, &desc, Some("<args>"));
    }
}

fn model_key(model: &PiModel) -> String {
    if model.provider.is_empty() {
        model.id.clone()
    } else {
        format!("{}::{}", model.provider, model.id)
    }
}

fn catalog_session_dir(state: &PiState, configured_dir: &Path) -> PathBuf {
    state
        .session_file
        .as_deref()
        .map(Path::new)
        .and_then(Path::parent)
        .filter(|directory| !directory.starts_with(configured_dir))
        .map(Path::to_path_buf)
        .unwrap_or_else(|| configured_dir.to_path_buf())
}

/// Derive a plan sidecar that belongs to precisely one Pi JSONL session.
///
/// Pi's session store contains files, not Grok-style per-session directories;
/// `<session>.plan.md` avoids sharing a bare `plan.md` across all sessions.
/// The fallback is still session-id namespaced when Pi has not materialized a
/// session file yet.
fn plan_file_path(state: &PiState, configured_dir: &Path) -> PathBuf {
    if let Some(session_file) = state
        .session_file
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        return PathBuf::from(session_file).with_extension("plan.md");
    }
    configured_dir
        .join("grok-pi-plans")
        .join(format!("{}.plan.md", state.session_id))
}

/// Ensure activation has a writable, empty plan artifact without truncating a
/// previous plan on re-entry.
fn ensure_plan_file(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("plan file has no parent directory: {}", path.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| anyhow!("create plan directory {}: {error}", parent.display()))?;
    if path.exists() {
        if !path.is_file() {
            bail!("plan path is not a regular file: {}", path.display());
        }
        return Ok(());
    }
    std::fs::File::create(path)
        .map_err(|error| anyhow!("create plan file {}: {error}", path.display()))?;
    Ok(())
}

fn plan_state_path(plan_file: &Path) -> PathBuf {
    let name = plan_file
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("plan.md");
    let base = name.strip_suffix(".plan.md").unwrap_or(name);
    plan_file.with_file_name(format!("{base}.plan-mode.json"))
}

fn load_plan_tracker(plan_file: &Path) -> Result<crate::plan_mode::PiPlanTracker> {
    let state_path = plan_state_path(plan_file);
    match std::fs::read(&state_path) {
        Ok(bytes) => {
            let snapshot: crate::plan_mode::PiPlanSnapshot = serde_json::from_slice(&bytes)
                .map_err(|error| {
                    anyhow!("parse plan-mode state {}: {error}", state_path.display())
                })?;
            Ok(
                crate::plan_mode::PiPlanTracker::from_snapshot_with_plan_file(
                    plan_file.to_path_buf(),
                    snapshot,
                ),
            )
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(
            crate::plan_mode::PiPlanTracker::with_plan_file(plan_file.to_path_buf()),
        ),
        Err(error) => Err(anyhow!(
            "read plan-mode state {}: {error}",
            state_path.display()
        )),
    }
}

fn atomic_write(path: &Path, body: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("state path has no parent directory: {}", path.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| anyhow!("create state directory {}: {error}", parent.display()))?;
    let staged = parent.join(format!(
        ".{}.{}.next",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("plan-mode"),
        std::process::id(),
    ));
    std::fs::write(&staged, body)
        .map_err(|error| anyhow!("write staged state {}: {error}", staged.display()))?;
    std::fs::rename(&staged, path)
        .map_err(|error| anyhow!("replace state {}: {error}", path.display()))?;
    Ok(())
}

fn content_chunk(content: acp::ContentBlock) -> acp::ContentChunk {
    acp::ContentChunk::new(content)
}

fn text_chunk(text: impl Into<String>) -> acp::ContentChunk {
    content_chunk(acp::ContentBlock::Text(acp::TextContent::new(text)))
}

fn message_role(event: &Value) -> Option<&str> {
    event
        .get("message")
        .and_then(|message| string(message, &["role", "type"]))
}

fn model_reasoning_efforts(model: &PiModel) -> Vec<Value> {
    model
        .thinking_levels
        .iter()
        .filter_map(|level| {
            let effort = model.acp_effort_for_pi_level(level)?;
            Some(json!({
                "id": level,
                "value": effort,
                "label": thinking_level_label(level),
            }))
        })
        .collect()
}

fn thinking_level_label(level: &str) -> String {
    let mut label = String::with_capacity(level.len());
    let mut capitalize = true;
    for ch in level.chars() {
        if ch == '_' || ch == '-' || ch.is_whitespace() {
            if !label.is_empty() && !label.ends_with(' ') {
                label.push(' ');
            }
            capitalize = true;
        } else if capitalize {
            label.extend(ch.to_uppercase());
            capitalize = false;
        } else {
            label.push(ch);
        }
    }
    label.trim().to_string()
}

fn compaction_start_notification(
    session_id: &str,
    event: &Value,
    tokens_used: u64,
    context_window: u64,
) -> Value {
    let percentage = tokens_used
        .saturating_mul(100)
        .checked_div(context_window)
        .unwrap_or(100)
        .min(100) as u8;
    json!({
        "sessionId": session_id,
        "update": {
            "sessionUpdate": "auto_compact_started",
            "tokens_used": tokens_used,
            "context_window": context_window,
            "percentage": percentage,
            "reason": string(event, &["reason"]).unwrap_or("unknown"),
        }
    })
}

fn compaction_end_notification(
    session_id: &str,
    event: &Value,
    elapsed_ms: Option<i64>,
) -> Option<Value> {
    let update = if let Some(error) =
        string(event, &["errorMessage", "error"]).filter(|error| !error.is_empty())
    {
        json!({ "sessionUpdate": "auto_compact_failed", "error": error })
    } else if event.get("aborted").and_then(Value::as_bool) == Some(true) {
        json!({
            "sessionUpdate": "auto_compact_cancelled",
            "reason": string(event, &["reason"]).unwrap_or("Compaction cancelled"),
        })
    } else {
        let result = event.get("result")?;
        let tokens_after = result.get("estimatedTokensAfter").and_then(Value::as_u64)?;
        json!({
            "sessionUpdate": "auto_compact_completed",
            "tokens_before": result.get("tokensBefore").and_then(Value::as_u64),
            "tokens_after": tokens_after,
            "elapsed_ms": elapsed_ms,
            "summary_preview": result.get("summary").and_then(Value::as_str),
        })
    };
    Some(json!({ "sessionId": session_id, "update": update }))
}

/// Send a Pager foreground-to-background request to the injected Bash extension.
///
/// The per-process metadata path is minted by the composition binary and passed
/// to both Pi and this adapter. The extension publishes only live foreground
/// tool IDs, so the adapter cannot create a background task for an arbitrary
/// or already completed tool call.
fn append_bash_background_control(meta_path: &Path, tool_call_id: &str) -> Result<()> {
    use std::fs::OpenOptions;
    use std::io::Write;

    let meta: Value = serde_json::from_str(&std::fs::read_to_string(meta_path)?)?;
    let active = meta
        .get("activeToolCallIds")
        .and_then(Value::as_array)
        .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(tool_call_id)));
    if !active {
        bail!("Pi Bash tool is not promotable: {tool_call_id}");
    }
    let control_path = meta
        .get("controlPath")
        .and_then(Value::as_str)
        .filter(|path| !path.trim().is_empty())
        .ok_or_else(|| anyhow!("Pi Bash control metadata missing controlPath"))?;
    let mut file = OpenOptions::new().append(true).open(control_path)?;
    writeln!(
        file,
        "{}",
        serde_json::to_string(&json!({ "op": "background", "toolCallId": tool_call_id }))?
    )?;
    Ok(())
}

/// Ask the injected Bash extension to kill a running background task.
///
/// Returns the wire outcome string consumed by Pager (`killed` / `not_found`).
/// The extension is the process owner; this only validates against the published
/// `runningTaskIds` set and appends a one-way control event.
fn append_bash_kill_control(meta_path: &Path, task_id: &str) -> Result<&'static str> {
    use std::fs::OpenOptions;
    use std::io::Write;

    let meta: Value = serde_json::from_str(&std::fs::read_to_string(meta_path)?)?;
    let running = meta
        .get("runningTaskIds")
        .and_then(Value::as_array)
        .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(task_id)));
    if !running {
        return Ok("not_found");
    }
    let control_path = meta
        .get("controlPath")
        .and_then(Value::as_str)
        .filter(|path| !path.trim().is_empty())
        .ok_or_else(|| anyhow!("Pi Bash control metadata missing controlPath"))?;
    let mut file = OpenOptions::new().append(true).open(control_path)?;
    writeln!(
        file,
        "{}",
        serde_json::to_string(&json!({ "op": "kill", "taskId": task_id }))?
    )?;
    Ok("killed")
}

/// Experimental Remote TUI: extension host watches a keyfile under tmp.
/// Meta written by the injected extension: `{id, keysPath}`.
fn append_remote_tui_key_event(event: Value) -> Result<()> {
    use std::fs::OpenOptions;
    use std::io::Write;

    let meta_path = std::env::temp_dir().join("pi-grok-remote-tui-active.json");
    if !meta_path.exists() {
        bail!("remote_tui meta missing ({})", meta_path.display());
    }
    let meta: Value = serde_json::from_str(&std::fs::read_to_string(&meta_path)?)?;
    let keys_path = meta
        .get("keysPath")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("remote_tui meta missing keysPath"))?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(keys_path)?;
    writeln!(file, "{}", serde_json::to_string(&event)?)?;
    Ok(())
}

/// Extension-shortcut dispatch keyfile (pi-grok-shortcut-manager watches it).
/// Prefer instance env `PI_GROK_SHORTCUT_KEYS` (set by grok-pi). Fall back to
/// legacy meta `tmp/pi-grok-shortcut-dispatch-active.json` for older children.
fn append_shortcut_dispatch_event(event: Value) -> Result<()> {
    use std::fs::OpenOptions;
    use std::io::Write;

    let keys_path = if let Ok(path) = std::env::var("PI_GROK_SHORTCUT_KEYS") {
        if path.is_empty() { None } else { Some(path) }
    } else {
        None
    };
    let keys_path = match keys_path {
        Some(path) => path,
        None => {
            let meta_path = std::env::temp_dir().join("pi-grok-shortcut-dispatch-active.json");
            if !meta_path.exists() {
                bail!("shortcut_dispatch keys missing (set PI_GROK_SHORTCUT_KEYS)");
            }
            let meta: Value = serde_json::from_str(&std::fs::read_to_string(&meta_path)?)?;
            meta.get("keysPath")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("shortcut_dispatch meta missing keysPath"))?
                .to_string()
        }
    };
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&keys_path)?;
    writeln!(file, "{}", serde_json::to_string(&event)?)?;
    file.flush()?;
    Ok(())
}

fn extension_tool_call_id(id: &Value) -> String {
    let id = id
        .as_str()
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| id.to_string());
    format!("pi-extension-ui:{id}")
}

fn extension_dialog_timeout(event: &Value) -> Option<Duration> {
    event
        .get("timeout")
        .and_then(Value::as_u64)
        .filter(|milliseconds| *milliseconds > 0)
        .map(Duration::from_millis)
}

const PI_GROK_MULTI_SELECT_TITLE_PREFIX: &str = "__pi_grok_multi_select_v1__:";
const PI_GROK_RESOURCE_PICKER_TITLE_PREFIX: &str = "__pi_grok_resource_picker_v1__:";

/// Decode a narrow, product-owned request to render a normal Pi `ui.select`
/// callback with QuestionView's native checkbox mode. The payload is not a new
/// Pi RPC surface: it is an opt-in title envelope understood only by the
/// injected grok-pi subagent extension.
fn extension_multi_select_title(title: &str) -> Option<String> {
    let encoded = title.strip_prefix(PI_GROK_MULTI_SELECT_TITLE_PREFIX)?;
    serde_json::from_str::<Value>(encoded)
        .ok()?
        .get("title")?
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

/// Decode the product-owned select envelope that opens the existing native Pi
/// resource manager in selection mode. This travels over the ordinary
/// extension UI and ACP question request; it does not extend Pi RPC.
fn extension_resource_picker(title: &str) -> Option<Value> {
    let encoded = title.strip_prefix(PI_GROK_RESOURCE_PICKER_TITLE_PREFIX)?;
    let value = serde_json::from_str::<Value>(encoded).ok()?;
    let object = value.as_object()?;
    object
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|title| !title.is_empty())?;
    let types = object.get("resourceTypes")?.as_array()?;
    (!types.is_empty() && types.iter().all(Value::is_string)).then_some(value)
}

const ASK_USER_CANCEL_TEXT: &str = "User declined to answer the questions. Continue with the task using your best judgment, or ask different questions.";

/// Convert the opaque Pi tool id to a bounded, portable response filename.
/// Provider/model ids may contain Windows-reserved characters or exceed the
/// per-component filename limit, so never use the raw id as a path component.
fn ask_user_response_file_name(tool_call_id: &str) -> String {
    let digest = Sha256::digest(tool_call_id.as_bytes());
    let mut name = String::with_capacity(4 + digest.len() * 2 + 5);
    name.push_str("ask-");
    use std::fmt::Write as _;
    for byte in digest {
        write!(name, "{byte:02x}").expect("writing to String cannot fail");
    }
    name.push_str(".json");
    name
}

fn write_ask_user_response(tool_call_id: &str, payload: Value) {
    let Some(dir) = std::env::var_os("PI_GROK_ASK_USER_DIR") else {
        tracing::warn!(
            tool_call_id,
            "PI_GROK_ASK_USER_DIR unset; cannot write Q&A response"
        );
        return;
    };
    let path = std::path::Path::new(&dir).join(ask_user_response_file_name(tool_call_id));
    if let Err(error) = std::fs::write(&path, payload.to_string()) {
        tracing::warn!(%error, path = %path.display(), "failed to write ask_user_question response");
    }
}

/// Normalize model/tool args into the ACP Question array for QuestionView.
fn normalize_ask_user_questions(args: Option<&Value>) -> Vec<Value> {
    let Some(args) = args else {
        return Vec::new();
    };
    let Some(items) = args.get("questions").and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let question = item
                .get("question")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())?;
            let header = item
                .get("header")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty());
            let question_text = match header {
                Some(header) => format!("{header}: {question}"),
                None => question.to_string(),
            };
            let options = item
                .get("options")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|option| {
                    let label = option
                        .get("label")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|text| !text.is_empty())?;
                    let description = option
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let preview = option.get("preview").cloned().filter(|v| !v.is_null());
                    Some(json!({
                        "label": label,
                        "description": description,
                        "preview": preview,
                        "id": null,
                    }))
                })
                .collect::<Vec<_>>();
            if options.len() < 2 {
                return None;
            }
            let multi_select = item
                .get("multi_select")
                .or_else(|| item.get("multiSelect"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            Some(json!({
                "question": question_text,
                "options": options,
                "multiSelect": multi_select,
                "id": null,
            }))
        })
        .collect()
}

/// Format Grok QuestionView outcome into the extension control payload.
fn format_ask_user_tool_result(result: &Value) -> Value {
    let outcome = result
        .get("outcome")
        .and_then(Value::as_str)
        .unwrap_or("cancelled");
    match outcome {
        "accepted" => {
            let Some(answers) = result.get("answers").and_then(Value::as_object) else {
                return json!({
                    "outcome": "cancelled",
                    "message": ASK_USER_CANCEL_TEXT,
                });
            };
            let annotations = result.get("annotations").and_then(Value::as_object);
            let entries: Vec<String> = answers
                .iter()
                .map(|(question, selected)| {
                    let labels = match selected {
                        Value::String(s) => s.clone(),
                        Value::Array(items) => items
                            .iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join(", "),
                        _ => selected.to_string(),
                    };
                    let mut parts = vec![format!("\"{question}\"=\"{labels}\"")];
                    if let Some(ann) = annotations.and_then(|map| map.get(question)) {
                        if let Some(preview) = ann.get("preview").and_then(Value::as_str) {
                            parts.push(format!("selected preview:\n{preview}"));
                        }
                        if let Some(notes) = ann.get("notes").and_then(Value::as_str) {
                            parts.push(format!("user notes: {notes}"));
                        }
                    }
                    parts.join(" ")
                })
                .collect();
            let message = format!(
                "User has answered your questions: {}. You can now continue with the user's answers in mind.",
                entries.join(", ")
            );
            json!({ "outcome": "accepted", "message": message })
        }
        "chat_about_this" | "skip_interview" => {
            // Plan-mode partial paths: surface partial labels so the model can continue.
            let partial = result
                .get("partial_answers")
                .and_then(Value::as_object)
                .map(|map| {
                    map.iter()
                        .map(|(q, v)| format!("\"{q}\"=\"{}\"", v.as_str().unwrap_or_default()))
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .filter(|s| !s.is_empty());
            let message = match (outcome, partial) {
                ("chat_about_this", Some(partial)) => format!(
                    "User wants to chat about the plan interview. Partial answers: {partial}"
                ),
                ("chat_about_this", None) => {
                    "User wants to chat about the plan interview without selecting options."
                        .to_string()
                }
                (_, Some(partial)) => {
                    format!("User skipped the plan interview. Partial answers: {partial}")
                }
                _ => "User skipped the plan interview.".to_string(),
            };
            json!({ "outcome": "accepted", "message": message })
        }
        _ => json!({
            "outcome": "cancelled",
            "message": ASK_USER_CANCEL_TEXT,
        }),
    }
}

fn selected_answer(value: &Value) -> Option<String> {
    let answers = value.get("answers").and_then(Value::as_object)?;
    for answer in answers.values() {
        if let Some(text) = answer.as_str() {
            return Some(text.to_string());
        }
        if let Some(text) = answer
            .as_array()
            .and_then(|items| items.first())
            .and_then(Value::as_str)
        {
            return Some(text.to_string());
        }
    }
    None
}

fn annotated_answer(value: &Value) -> Option<String> {
    let annotations = value.get("annotations").and_then(Value::as_object)?;
    for annotation in annotations.values() {
        if let Some(notes) = annotation.get("notes").and_then(Value::as_str) {
            return Some(notes.to_string());
        }
    }
    None
}

/// Translate Grok QuestionView's response into the value Pi expects.
///
/// Freeform rows are represented by the native question component as the
/// selected option `Other`, with the actual editor text under
/// `annotations.<question>.notes`. Pi input/editor must therefore prefer notes;
/// select/confirm must prefer the selected option.
fn extension_answer(method: &str, value: &Value) -> Option<String> {
    let direct = || {
        value
            .get("value")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
    };
    match method {
        "input" | "editor" => annotated_answer(value)
            .or_else(|| selected_answer(value))
            .or_else(direct),
        _ => selected_answer(value)
            .or_else(|| annotated_answer(value))
            .or_else(direct),
    }
}

/// Pi's stock select callback accepts one string, so encode the native
/// QuestionView multi-select labels as a JSON array for the product-owned
/// callback envelope above. The TypeScript caller validates every label before
/// using it as a configuration toggle.
fn extension_multi_select_answer(value: &Value) -> Option<String> {
    let answers = value.get("answers").and_then(Value::as_object)?;
    let labels = answers.values().find_map(|answer| {
        answer.as_array().map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
    })?;
    serde_json::to_string(&labels).ok()
}

fn extension_resource_picker_answer(value: &Value) -> Option<String> {
    let paths = value.get("paths")?.as_array()?;
    let paths = paths
        .iter()
        .filter_map(Value::as_str)
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    serde_json::to_string(&paths).ok()
}

fn ext_response(value: Value) -> Result<acp::ExtResponse> {
    let raw = serde_json::value::to_raw_value(&json!({ "result": value }))?;
    Ok(acp::ExtResponse::new(raw.into()))
}

fn acp_internal(error: impl std::fmt::Display) -> acp::Error {
    acp::Error::internal_error().data(error.to_string())
}

/// Wall-clock UTC ms for ACP `_meta.agentTimestampMs` / stream anchors.
fn utc_now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod ask_user_response_file_tests {
    use super::ask_user_response_file_name;

    #[test]
    fn hashes_opaque_ids_into_portable_bounded_filename() {
        let name = ask_user_response_file_name("ask_user_question:1753958400000:abc/def|+=");
        assert_eq!(
            name,
            "ask-9bbafc01081c04c51639e387a27b0e3b6b3f052b59b87131c04217b9c0817ba2.json"
        );
        assert_eq!(name.len(), 73);
        let digest_name = name
            .strip_prefix("ask-")
            .and_then(|name| name.strip_suffix(".json"))
            .expect("hashed response filename");
        assert_eq!(digest_name.len(), 64);
        assert!(digest_name.chars().all(|ch| ch.is_ascii_hexdigit()));
    }
}
