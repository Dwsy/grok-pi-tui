use std::collections::BTreeSet;
use std::fmt;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use anyhow::Context as _;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

use crate::conversation::{ConversationId, ReadConversation};
use crate::conversation_script::{Conversation, ScriptViolation};
use crate::feedback_endpoint::FeedbackEndpointState;
pub use crate::feedback_endpoint::FeedbackPost;
pub use crate::gated_upload_proxy::GatedUploadProxy;
use crate::inference_override::InferenceOverrides;
pub use crate::inference_override::{InferenceExpectation, InferenceRequestMatcher};
use crate::inference_request::DEFAULT_MODEL;
pub use crate::inference_request::InferenceEndpoint;
use crate::inference_route::InferenceRoute;
pub use crate::managed_gateway_endpoint::ManagedGatewayCall;
use crate::managed_gateway_endpoint::ManagedGatewayEndpointState;
use crate::mock_server_tls::ThrowawayCa;
pub use crate::request_log::LogEntry;
use crate::request_log::RequestLog;
pub use crate::scripted::{ScriptedBody, ScriptedResponse, SseEvent};
use crate::storage_endpoint::StorageEndpointState;
pub use crate::storage_endpoint::StorageUpload;
use crate::telemetry_events::TelemetryEventsState;

/// Records that arrived since the previous [`MockInferenceServer::arrived_since_observation`].
pub struct ObservationRecords {
    pub requests: Vec<LogEntry>,
    pub telemetry: Vec<Value>,
    pub storage_uploads: Vec<StorageUpload>,
    pub gateway_calls: Vec<ManagedGatewayCall>,
}

/// A model served by `/v1/models`.
/// Each field is emitted under its camelCase name when set, at the top level except for `agent_type`, which goes in `_meta`.
#[derive(Debug, Clone)]
pub struct MockModelEntry {
    id: String,
    agent_type: Option<String>,
    api_backend: Option<String>,
    supports_backend_search: bool,
    supports_reasoning_effort: bool,
    reasoning_effort: Option<String>,
    /// Each entry is a table carrying a `value` key, or a bare value string.
    /// `parse_remote_model_value` defines the full shape.
    reasoning_efforts: Vec<Value>,
    /// Sets `You are <label>` in the primary system prompt, so a model switch is visible on the wire.
    system_prompt_label: Option<String>,
}

impl MockModelEntry {
    pub fn new(id: impl Into<String>) -> Self {
        MockModelEntry {
            id: id.into(),
            agent_type: None,
            api_backend: None,
            supports_backend_search: false,
            supports_reasoning_effort: false,
            reasoning_effort: None,
            reasoning_efforts: Vec::new(),
            system_prompt_label: None,
        }
    }

    pub fn with_agent_type(id: impl Into<String>, agent_type: impl Into<String>) -> Self {
        MockModelEntry {
            agent_type: Some(agent_type.into()),
            ..MockModelEntry::new(id)
        }
    }

    pub fn with_api_backend(mut self, api_backend: impl Into<String>) -> Self {
        self.api_backend = Some(api_backend.into());
        self
    }

    pub fn with_supports_backend_search(mut self, supports: bool) -> Self {
        self.supports_backend_search = supports;
        self
    }

    pub fn with_supports_reasoning_effort(mut self, supports: bool) -> Self {
        self.supports_reasoning_effort = supports;
        self
    }

    pub fn with_reasoning_effort(mut self, effort: impl Into<String>) -> Self {
        self.reasoning_effort = Some(effort.into());
        self
    }

    pub fn with_reasoning_efforts(mut self, efforts: Vec<Value>) -> Self {
        self.reasoning_efforts = efforts;
        self
    }

    pub fn with_system_prompt_label(mut self, label: impl Into<String>) -> Self {
        self.system_prompt_label = Some(label.into());
        self
    }

    fn to_json(&self) -> Value {
        let mut obj = json!({
            "id": self.id,
            "object": "model",
            "created": 1234567890,
            "owned_by": "test",
            "context_length": 131_072
        });
        if let Some(map) = obj.as_object_mut() {
            if let Some(agent_type) = &self.agent_type {
                map.insert("_meta".into(), json!({ "agentType": agent_type }));
            }
            if let Some(backend) = &self.api_backend {
                map.insert("apiBackend".into(), json!(backend));
            }
            if self.supports_backend_search {
                map.insert("supportsBackendSearch".into(), json!(true));
            }
            if self.supports_reasoning_effort {
                map.insert("supportsReasoningEffort".into(), json!(true));
            }
            if let Some(effort) = &self.reasoning_effort {
                map.insert("reasoningEffort".into(), json!(effort));
            }
            if !self.reasoning_efforts.is_empty() {
                map.insert("reasoningEfforts".into(), json!(self.reasoning_efforts));
            }
            if let Some(label) = &self.system_prompt_label {
                map.insert("systemPromptLabel".into(), json!(label));
            }
        }
        obj
    }
}

#[derive(Clone, Copy, Default)]
enum StartupFetchStall {
    #[default]
    None,
    Delay(Duration),
    Hang,
}

/// `Boot` is the placeholder `start` seeds. `Installed` is a catalog a test has written.
#[derive(Clone)]
enum ModelCatalog {
    Boot(Vec<Value>),
    Installed(Vec<Value>),
}

#[derive(Clone)]
struct RouterState {
    log: Arc<RequestLog>,
    models: Arc<std::sync::RwLock<ModelCatalog>>,
    settings: Arc<std::sync::RwLock<Option<Value>>>,
    overrides: InferenceOverrides,
    inference: InferenceRoute,
    storage: Arc<StorageEndpointState>,
    feedback: Arc<FeedbackEndpointState>,
    telemetry: Arc<TelemetryEventsState>,
    managed_gateway: Arc<ManagedGatewayEndpointState>,
    startup_fetch_stall: Arc<std::sync::RwLock<StartupFetchStall>>,
    startup_stalls_served: Arc<AtomicU32>,
    user_tier: Arc<std::sync::RwLock<Option<String>>>,
    user_team: Arc<std::sync::RwLock<Option<MockUserTeam>>>,
    user_can_administer_team: Arc<std::sync::RwLock<MockCanAdministerTeam>>,
    user_coding_data_retention_opt_out: Arc<std::sync::RwLock<Option<bool>>>,
    user_info_released: Arc<tokio::sync::watch::Sender<bool>>,
    user_info_arrivals: Arc<tokio::sync::watch::Sender<usize>>,
}

/// `teamId`, `teamName`, and `teamRole` on `GET /v1/user`.
#[derive(Debug, Clone)]
pub struct MockUserTeam {
    pub id: String,
    pub name: String,
    pub role: String,
}

/// `canAdministerTeam` on `GET /v1/user`. `Omitted` leaves the key out; `Unresolved` is `null`,
/// the proxy's answer when it could not resolve the caller's team-administration capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MockCanAdministerTeam {
    Omitted,
    Unresolved,
    Allowed,
    Denied,
}

impl MockCanAdministerTeam {
    pub fn wire_value(self) -> Option<Value> {
        match self {
            Self::Omitted => None,
            Self::Unresolved => Some(Value::Null),
            Self::Allowed => Some(Value::Bool(true)),
            Self::Denied => Some(Value::Bool(false)),
        }
    }
}

impl RouterState {
    /// Counted once it is let through.
    async fn stall_startup_fetch(&self) {
        let stall = *self.startup_fetch_stall.read().unwrap();
        match stall {
            StartupFetchStall::None => return,
            StartupFetchStall::Delay(delay) => tokio::time::sleep(delay).await,
            StartupFetchStall::Hang => std::future::pending().await,
        }
        self.startup_stalls_served.fetch_add(1, Ordering::Relaxed);
    }
}

enum Transport {
    Plain,
    Tls,
}

pub struct MockInferenceServer {
    addr: SocketAddr,
    shutdown_tx: Option<oneshot::Sender<()>>,
    state: RouterState,
    tls_ca: Option<ThrowawayCa>,
}

impl MockInferenceServer {
    pub async fn start() -> anyhow::Result<Self> {
        Self::start_with_models(vec![MockModelEntry::new(DEFAULT_MODEL)]).await
    }

    pub async fn start_with_models(models: Vec<MockModelEntry>) -> anyhow::Result<Self> {
        Self::start_inner(models, None, Transport::Plain).await
    }

    /// Start a mock that returns 401 on inference requests missing `Authorization: Bearer <required_token>`.
    pub async fn start_with_required_auth(
        models: Vec<MockModelEntry>,
        required_token: impl Into<String>,
    ) -> anyhow::Result<Self> {
        Self::start_inner(models, Some(required_token.into()), Transport::Plain).await
    }

    /// Serve the same router over HTTPS with a throwaway CA; there is no plaintext listener,
    /// so a logged request implies a completed TLS handshake.
    /// [`Self::url`] is `https://127.0.0.1:PORT/v1` and [`Self::ca_pem_path`] is the path to the
    /// CA PEM the client must trust. Only [`crate::headless::run_headless`] and
    /// [`crate::headless::run_headless_with_env`] inject it; any other runner must set
    /// `GROK_EXTRA_CA_BUNDLE` to [`Self::ca_pem_path`] on its own `TestSandbox` via `set_env`.
    pub async fn start_tls() -> anyhow::Result<Self> {
        Self::start_inner(
            vec![MockModelEntry::new(DEFAULT_MODEL)],
            None,
            Transport::Tls,
        )
        .await
    }

    async fn start_inner(
        models: Vec<MockModelEntry>,
        required_token: Option<String>,
        transport: Transport,
    ) -> anyhow::Result<Self> {
        let log = Arc::new(RequestLog::new());
        let overrides = InferenceOverrides::new(required_token);
        let state = RouterState {
            inference: InferenceRoute::new(log.clone(), overrides.clone()),
            log,
            models: Arc::new(std::sync::RwLock::new(ModelCatalog::Boot(
                models.iter().map(MockModelEntry::to_json).collect(),
            ))),
            settings: Arc::new(std::sync::RwLock::new(None)),
            overrides,
            storage: Arc::new(StorageEndpointState::default()),
            feedback: Arc::new(FeedbackEndpointState::default()),
            telemetry: Arc::new(TelemetryEventsState::default()),
            managed_gateway: Arc::new(ManagedGatewayEndpointState::default()),
            startup_fetch_stall: Arc::new(std::sync::RwLock::new(StartupFetchStall::None)),
            startup_stalls_served: Arc::new(AtomicU32::new(0)),
            user_tier: Arc::new(std::sync::RwLock::new(None)),
            user_team: Arc::new(std::sync::RwLock::new(None)),
            user_can_administer_team: Arc::new(std::sync::RwLock::new(
                MockCanAdministerTeam::Omitted,
            )),
            user_coding_data_retention_opt_out: Arc::new(std::sync::RwLock::new(None)),
            user_info_released: Arc::new(tokio::sync::watch::Sender::new(true)),
            user_info_arrivals: Arc::new(tokio::sync::watch::Sender::new(0)),
        };
        let app = Self::build_router(state.clone());
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

        let (addr, tls_ca) = match transport {
            Transport::Plain => {
                let listener = TcpListener::bind("127.0.0.1:0")
                    .await
                    .context("bind mock inference server")?;
                let addr = listener.local_addr().context("local_addr")?;
                tokio::spawn(async move {
                    axum::serve(listener, app)
                        .with_graceful_shutdown(async {
                            let _ = shutdown_rx.await;
                        })
                        .await
                        .unwrap();
                });
                (addr, None)
            }
            Transport::Tls => {
                let (addr, ca) = crate::mock_server_tls::serve_tls(app, shutdown_rx).await?;
                (addr, Some(ca))
            }
        };

        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while tokio::net::TcpStream::connect(addr).await.is_err() {
            if tokio::time::Instant::now() >= deadline {
                anyhow::bail!("mock server not ready within 5s");
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        Ok(MockInferenceServer {
            addr,
            shutdown_tx: Some(shutdown_tx),
            state,
            tls_ca,
        })
    }

    pub fn set_models(&self, models: Vec<MockModelEntry>) {
        *self.state.models.write().unwrap() =
            ModelCatalog::Installed(models.iter().map(MockModelEntry::to_json).collect());
    }

    /// A call that names nothing leaves the catalog as it is, including the model `start` seeds.
    /// The first call that names a model replaces that boot catalog. A later call keeps entries it
    /// does not name, with `leading` in front and `trailing` after.
    pub fn install_models(&self, leading: &[MockModelEntry], trailing: &[MockModelEntry]) {
        if leading.is_empty() && trailing.is_empty() {
            return;
        }
        let mut guard = self.state.models.write().unwrap();
        let merged = match &mut *guard {
            ModelCatalog::Installed(existing) => {
                let named: Vec<&str> = leading
                    .iter()
                    .chain(trailing.iter())
                    .map(|entry| entry.id.as_str())
                    .collect();
                let kept: Vec<Value> = existing
                    .iter()
                    .filter(|model| match model.get("id").and_then(Value::as_str) {
                        Some(id) => !named.contains(&id),
                        None => true,
                    })
                    .cloned()
                    .collect();
                leading
                    .iter()
                    .map(MockModelEntry::to_json)
                    .chain(kept)
                    .chain(trailing.iter().map(MockModelEntry::to_json))
                    .collect()
            }
            ModelCatalog::Boot(_) => leading
                .iter()
                .chain(trailing.iter())
                .map(MockModelEntry::to_json)
                .collect(),
        };
        *guard = ModelCatalog::Installed(merged);
    }

    /// Stream this text instead of echoing. Deltas reconstruct it byte for byte.
    pub fn set_response(&self, text: impl Into<String>) {
        self.state.inference.set_response(text.into());
    }

    pub fn set_auxiliary_hold(&self) {
        self.state.inference.set_auxiliary_hold();
    }

    /// Park the next foreground reply, including a tool call, until [`Self::release_parked_replies`].
    pub fn set_foreground_hold(&self) {
        self.state.inference.set_foreground_hold();
    }

    /// Park the next auxiliary whose `x-grok-req-id` starts with `prefix`.
    /// An auxiliary with a different id leaves the hold armed.
    pub fn set_auxiliary_hold_matching(&self, prefix: impl Into<String>) {
        self.state.inference.set_auxiliary_hold_matching(prefix);
    }

    pub async fn wait_until_parked_replies(&self, at_least: usize) {
        self.state
            .overrides
            .wait_until_parked_replies(at_least)
            .await;
    }

    /// Consumed FIFO per `path`, e.g. `"/v1/chat/completions"`.
    /// An empty queue falls back to the response mode.
    pub fn enqueue_response(&self, path: impl Into<String>, response: ScriptedResponse) {
        self.state.overrides.enqueue_response(path, response);
    }

    /// Default-responder concurrency cap: over `cap` in-flight (each held for `hold`), extra requests get 429 with `Retry-After`.
    /// Scripted responses and expectations bypass the cap.
    pub fn set_inference_concurrency_cap(&self, cap: usize, hold: Duration, retry_after_secs: u64) {
        self.state
            .overrides
            .set_concurrency_cap(cap, hold, retry_after_secs);
    }

    /// Register one named response matched atomically by endpoint and request kind.
    #[must_use = "keep the handle to synchronize and assert expectation satisfaction"]
    pub fn expect_response(
        &self,
        name: impl Into<String>,
        matcher: InferenceRequestMatcher,
        response: ScriptedResponse,
    ) -> InferenceExpectation {
        self.state.overrides.register_expectation(
            name, matcher, response, /*block_before_terminal*/ false,
        )
    }

    /// Register a named response that pauses immediately before completion.
    #[must_use = "keep the handle to release and assert expectation satisfaction"]
    pub fn expect_response_blocked(
        &self,
        name: impl Into<String>,
        matcher: InferenceRequestMatcher,
        response: ScriptedResponse,
    ) -> InferenceExpectation {
        self.state
            .overrides
            .register_expectation(name, matcher, response, /*block_before_terminal*/ true)
    }

    /// Queue one byte-exact response per foreground turn.
    pub fn set_agent_turns(&self, turns: impl IntoIterator<Item = String>) {
        self.state
            .inference
            .set_agent_turns(turns.into_iter().collect());
    }

    /// Replaces earlier conversations and restarts each script. One not served through stays reported.
    pub fn set_conversations(&self, conversations: impl IntoIterator<Item = Conversation>) {
        self.state
            .overrides
            .conversation_scripts()
            .set(conversations);
    }

    /// Each violation once: those recorded, then every script not served through or never opened.
    #[must_use]
    pub fn script_violations(&self) -> Vec<ScriptViolation> {
        self.state.overrides.conversation_scripts().violations()
    }

    /// Later requests fall through to the fallback modes; the drop check has nothing left to report.
    #[must_use = "assert on the violations; finishing disarms the drop check"]
    pub fn finish_scripts(&self) -> Vec<ScriptViolation> {
        self.state.overrides.conversation_scripts().finish()
    }

    /// A violation serves a reply instead of failing the request, so this also runs on drop.
    #[track_caller]
    pub fn assert_no_script_violations(&self) {
        let violations = self.script_violations();
        assert!(
            violations.is_empty(),
            "conversation scripts departed from:\n{}\nconversations:\n{}\nrequests:\n{}\n\
             a test that returned early through `?` has its error replaced by this panic; \
             call finish_scripts() before returning",
            lines(&violations),
            lines(&self.conversations()),
            self.request_log_summary()
        );
    }

    #[must_use]
    pub fn conversations(&self) -> Vec<ReadConversation> {
        self.state.inference.conversations()
    }

    #[must_use]
    pub fn conversation(&self, number: usize) -> Option<ReadConversation> {
        self.conversations()
            .into_iter()
            .find(|conversation| conversation.number() == number)
    }

    #[must_use]
    pub fn conversation_for_system_prompt(&self, contains: &str) -> Option<ReadConversation> {
        self.conversations().into_iter().find(|conversation| {
            conversation
                .first_system_prompt()
                .is_some_and(|prompt| prompt.contains(contains))
        })
    }

    /// Until this is called, `GET /v1/settings` returns 404.
    pub fn set_settings(&self, settings: impl serde::Serialize) {
        let value = serde_json::to_value(settings).expect("serialize settings");
        let mut guard = self.state.settings.write().unwrap();
        *guard = Some(value);
    }

    /// The smallest settings payload that opens the subscription gate.
    /// Without it a client sits on the upsell screen.
    pub fn preset_allow_access(&self) {
        self.set_settings(json!({ "allow_access": true }));
    }

    pub fn set_hang(&self, hang: bool) {
        *self.state.startup_fetch_stall.write().unwrap() = if hang {
            StartupFetchStall::Hang
        } else {
            StartupFetchStall::None
        };
    }

    pub fn set_startup_fetch_delay(&self, delay: Duration) {
        *self.state.startup_fetch_stall.write().unwrap() = StartupFetchStall::Delay(delay);
    }

    pub fn clear_startup_fetch_stall(&self) {
        *self.state.startup_fetch_stall.write().unwrap() = StartupFetchStall::None;
    }

    pub fn startup_stalls_served(&self) -> u32 {
        self.state.startup_stalls_served.load(Ordering::Relaxed)
    }

    /// The `subscriptionTier` on `GET /v1/user`.
    /// `None`, the default, omits the field, which the shell reads as the free tier.
    pub fn set_user_subscription_tier(&self, tier: Option<&str>) {
        *self.state.user_tier.write().unwrap() = tier.map(str::to_owned);
    }

    pub fn set_user_team(&self, team: MockUserTeam) {
        *self.state.user_team.write().unwrap() = Some(team);
    }

    pub fn set_user_can_administer_team(&self, can_administer: MockCanAdministerTeam) {
        *self.state.user_can_administer_team.write().unwrap() = can_administer;
    }

    /// `codingDataRetentionOptOut` on `GET /v1/user`. `None`, the default, omits the field.
    pub fn set_user_coding_data_retention_opt_out(&self, opt_out: Option<bool>) {
        *self
            .state
            .user_coding_data_retention_opt_out
            .write()
            .unwrap() = opt_out;
    }

    /// Park every `GET /v1/user` after logging it, until [`Self::release_user_info`].
    pub fn hold_user_info(&self) {
        self.state.user_info_released.send_replace(false);
    }

    pub fn release_user_info(&self) {
        self.state.user_info_released.send_replace(true);
    }

    /// Resolves once `n` `GET /v1/user` have arrived, parked ones included; panics after 5s rather than hang.
    pub async fn user_info_arrived(&self, n: usize) {
        let mut arrivals = self.state.user_info_arrivals.subscribe();
        let waited = tokio::time::timeout(
            Duration::from_secs(5),
            arrivals.wait_for(|count| *count >= n),
        )
        .await;
        assert!(
            waited.is_ok(),
            "expected {n} GET /v1/user, saw {} within 5s",
            *self.state.user_info_arrivals.borrow()
        );
    }

    pub fn set_messages_stop_reason(&self, stop_reason: impl Into<String>) {
        self.state
            .inference
            .set_messages_stop_reason(stop_reason.into());
    }

    /// Emit each SSE event after `delay`, so a test can hold a turn visibly streaming.
    /// `None` restores instant streaming.
    /// Applies to requests started after the call.
    pub fn set_chunk_delay(&self, delay: Option<Duration>) {
        self.state.inference.set_chunk_delay(delay);
    }

    /// Hold foreground terminal SSE events until [`Self::release_agent_completions`].
    /// Prefer per-expectation blocking.
    pub fn hold_agent_completions(&self) {
        self.state.overrides.hold_completions();
    }

    /// Arms the reply hold. Cancel and drop both release it.
    pub fn arm_reply_hold(&self) -> crate::inference_override::ArmedReplyHold {
        self.state.overrides.arm_reply_hold()
    }

    /// Later [`SseEvent::hold`] markers each wait for one [`Self::release_one_chunk`].
    /// The completion gate is left as it was.
    pub fn arm_chunk_release(&self) {
        self.state.overrides.arm_chunk_release();
    }

    pub fn release_one_chunk(&self) {
        self.state.overrides.release_one_chunk();
    }

    /// Let every held SSE chunk still waiting through, and leave later holds on the completion gate.
    pub fn release_remaining_chunks(&self) {
        self.state.overrides.release_remaining_chunks();
    }

    /// Other conversations stream.
    pub fn hold_only_conversation(&self, conversation: usize) {
        self.state.overrides.hold_only_conversation(conversation);
    }

    pub fn release_agent_completions(&self) {
        self.state.overrides.release_completions();
    }

    /// Hold the terminal SSE event of every foreground reply in `conversation`, counted from 1, until
    /// [`Self::release_conversation`], so one session's turn stays in flight while others finish.
    pub fn hold_conversation(&self, conversation: usize) {
        self.state
            .overrides
            .hold_conversation(ConversationId::nth(conversation));
    }

    pub fn release_conversation(&self, conversation: usize) {
        self.state
            .overrides
            .release_conversation(ConversationId::nth(conversation));
    }

    /// Resolves once `n` replies for `conversation` (counted from 1) are parked.
    pub async fn wait_until_parked_replies_for(&self, conversation: usize, at_least: usize) {
        self.state
            .overrides
            .wait_until_parked_replies_for(conversation, at_least)
            .await;
    }

    /// Resolves once `n` auxiliaries have parked under [`Self::set_auxiliary_hold_matching`].
    pub async fn wait_until_matching_parked(&self, at_least: usize) {
        self.state
            .overrides
            .wait_until_matching_parked(at_least)
            .await;
    }

    /// Lets a scripted reply parked with `hold` finish before the mock shuts down.
    pub fn release_parked_replies(&self) {
        self.state.overrides.release_parked_replies();
    }

    /// A park that [`Self::release_scripted_park`] releases. The shutdown latch does not arm it.
    pub fn arm_scripted_park(&self) -> u64 {
        self.state.overrides.arm_scripted_park()
    }

    /// Let the reply parked under `token` finish. Other parks stay held.
    pub fn release_scripted_park(&self, token: u64) {
        self.state.overrides.release_scripted_park(token);
    }

    pub async fn wait_until_scripted_park(&self, token: u64) {
        self.state.overrides.wait_until_scripted_park(token).await;
    }

    /// Lets parked auxiliary replies finish. Foreground holds stay parked.
    pub fn release_auxiliary_replies(&self) {
        self.state.overrides.release_auxiliary_replies();
    }

    /// Resolves once `n` foreground inference requests are logged (1 is the first). Panics after 30s.
    pub async fn wait_for_inference_requests(&self, n: usize) {
        let mut arrivals = self.state.log.subscribe_inference();
        let waited = tokio::time::timeout(
            crate::scaled(Duration::from_secs(30)),
            arrivals.wait_for(|count| *count >= n),
        )
        .await;
        assert!(
            waited.is_ok(),
            "expected {n} foreground inference requests, saw {} within 30s",
            self.state.log.inference_count()
        );
    }

    pub fn agent_completion_parked(&self) -> bool {
        self.state.overrides.agent_completion_parked()
    }

    pub async fn wait_until_a_reply_is_held(&self) {
        self.state.overrides.wait_until_a_reply_is_held().await;
    }

    pub fn url(&self) -> String {
        format!("{}://{}/v1", self.scheme(), self.addr)
    }

    pub fn origin(&self) -> String {
        format!("{}://{}", self.scheme(), self.addr)
    }

    fn scheme(&self) -> &'static str {
        if self.tls_ca.is_some() {
            "https"
        } else {
            "http"
        }
    }

    pub fn ca_pem_path(&self) -> Option<&Path> {
        self.tls_ca.as_ref().map(ThrowawayCa::pem_path)
    }

    pub fn request_count(&self) -> u32 {
        self.state.log.count()
    }

    pub fn request_count_for(&self, path: &str) -> usize {
        self.state.log.count_for(path)
    }

    /// Stop retaining entries. [`Self::request_count`] stays exact.
    pub fn set_keep_requests(&self, enabled: bool) {
        self.state.log.set_keep_entries(enabled);
    }

    /// Without this, inference logs keep only the parsed body.
    pub fn set_capture_request_bytes(&self, enabled: bool) {
        self.state.log.set_capture_request_bytes(enabled);
    }

    #[must_use]
    pub fn replies_served(&self, conversation: usize) -> usize {
        self.state
            .overrides
            .conversation_scripts()
            .replies_served(conversation)
    }

    /// Turns this conversation has already answered, by script index.
    #[must_use]
    pub fn answered_turns(&self, conversation: usize) -> BTreeSet<usize> {
        self.state
            .overrides
            .conversation_scripts()
            .answered_turns(conversation)
    }

    pub fn requests(&self) -> Vec<LogEntry> {
        self.state.log.entries()
    }

    /// What arrived since the previous observation. The full-log getters stay complete.
    pub fn arrived_since_observation(&self) -> ObservationRecords {
        ObservationRecords {
            requests: self.state.log.take_for_observation(),
            telemetry: self.state.telemetry.take_for_observation(),
            storage_uploads: self.state.storage.take_for_observation(),
            gateway_calls: self.state.managed_gateway.take_for_observation(),
        }
    }

    /// Bodies of all received requests, in arrival order (body-less requests such as `GET /v1/models` are skipped).
    pub fn request_bodies(&self) -> Vec<Value> {
        self.state.log.bodies()
    }

    pub fn has_chat_completion_request(&self) -> bool {
        self.state.log.has_path_containing("chat/completions")
    }

    pub fn has_responses_request(&self) -> bool {
        self.state.log.has_path_containing("responses")
    }

    pub fn messages_request_count(&self) -> usize {
        self.state.log.count_for("/v1/messages")
    }

    pub fn request_log_summary(&self) -> String {
        self.state.log.summary()
    }

    pub fn last_system_prompt(&self) -> Option<String> {
        self.state.log.last_system_prompt()
    }

    /// While closed, every `/v1/storage` upload is rejected with 401.
    pub fn set_storage_unauthorized(&self, unauthorized: bool) {
        self.state.storage.set_unauthorized(unauthorized);
    }

    /// Total `/v1/storage` upload attempts seen, including 401-rejected ones.
    pub fn storage_request_count(&self) -> u32 {
        self.state.storage.request_count()
    }

    /// Only the uploads that were accepted.
    pub fn storage_uploads(&self) -> Vec<StorageUpload> {
        self.state.storage.uploads()
    }

    /// While set, every `POST /v1/feedback` answers 500 (the body is still recorded).
    pub fn set_feedback_failure(&self, fail: bool) {
        self.state.feedback.set_failure(fail);
    }

    /// Every `POST /v1/feedback` seen so far, accepted or scripted to fail, in arrival order.
    pub fn feedback_posts(&self) -> Vec<FeedbackPost> {
        self.state.feedback.posts()
    }

    /// Every product-telemetry event posted to `/v1/events` so far, flattened out of its batch, in arrival order.
    pub fn telemetry_events(&self) -> Vec<Value> {
        self.state.telemetry.events()
    }

    /// Serves `catalog` on `GET /v1/mcp/tools/list` and answers every `POST /v1/mcp/tools/call`
    /// with `call_result` as its `result`; both routes 404 until this is called.
    pub fn set_managed_gateway(&self, catalog: Value, call_result: Value) {
        self.state.managed_gateway.set_script(catalog, call_result);
    }

    pub fn managed_gateway_calls(&self) -> Vec<ManagedGatewayCall> {
        self.state.managed_gateway.calls()
    }

    fn build_router(state: RouterState) -> Router {
        Router::new()
            .route(
                InferenceEndpoint::ChatCompletions.path(),
                state.inference.handler(InferenceEndpoint::ChatCompletions),
            )
            .route(
                InferenceEndpoint::Responses.path(),
                state.inference.handler(InferenceEndpoint::Responses),
            )
            .route(
                InferenceEndpoint::Messages.path(),
                state.inference.handler(InferenceEndpoint::Messages),
            )
            .route(
                "/v1/images/generations",
                post({
                    let state = state.clone();
                    move |headers: HeaderMap, Json(body): Json<Value>| {
                        let state = state.clone();
                        async move {
                            state
                                .log
                                .record("POST", "/v1/images/generations", &body, &headers);
                            Json(json!({ "data": [ { "b64_json": "" } ] })).into_response()
                        }
                    }
                }),
            )
            .route(
                "/v1/videos/generations",
                post({
                    let state = state.clone();
                    move |headers: HeaderMap, Json(body): Json<Value>| {
                        let state = state.clone();
                        async move {
                            state
                                .log
                                .record("POST", "/v1/videos/generations", &body, &headers);
                            Json(json!({ "request_id": "media-1" })).into_response()
                        }
                    }
                }),
            )
            .route(
                "/v1/models",
                get({
                    let state = state.clone();
                    move |headers: HeaderMap| {
                        let state = state.clone();
                        async move {
                            state.log.record_get_with_authorization(
                                "/v1/models",
                                headers
                                    .get("authorization")
                                    .and_then(|value| value.to_str().ok())
                                    .map(str::to_owned),
                            );
                            state.stall_startup_fetch().await;
                            let models_json = match &*state.models.read().unwrap() {
                                ModelCatalog::Boot(entries) | ModelCatalog::Installed(entries) => {
                                    entries.clone()
                                }
                            };
                            Json(json!({
                                "object": "list",
                                "data": models_json,
                            }))
                        }
                    }
                }),
            )
            .route(
                "/v1/settings",
                get({
                    let state = state.clone();
                    move || {
                        let state = state.clone();
                        async move {
                            state.log.record_get("/v1/settings");
                            state.stall_startup_fetch().await;
                            // Scripts take precedence, so a test can serve a transient payload before the steady-state value
                            if let Some(s) = state.overrides.pop_scripted("/v1/settings") {
                                return s.into_response_paced(None, None).await;
                            }
                            let maybe = state.settings.read().unwrap().clone();
                            match maybe {
                                Some(s) => Json(s).into_response(),
                                None => StatusCode::NOT_FOUND.into_response(),
                            }
                        }
                    }
                }),
            )
            .route(
                "/v1/privacy/coding-data-retention",
                put({
                    let state = state.clone();
                    move |headers: HeaderMap, Json(body): Json<Value>| {
                        let state = state.clone();
                        async move {
                            let path = "/v1/privacy/coding-data-retention";
                            state.log.record("PUT", path, &body, &headers);
                            if let Some(s) = state.overrides.pop_scripted(path) {
                                return s.into_response_paced(None, None).await;
                            }
                            let opt_out = body
                                .get("codingDataRetentionOptOut")
                                .cloned()
                                .unwrap_or(Value::Bool(false));
                            Json(json!({ "codingDataRetentionOptOut": opt_out })).into_response()
                        }
                    }
                }),
            )
            .route(
                "/v1/consent/accept",
                post({
                    let state = state.clone();
                    move |headers: HeaderMap, Json(body): Json<Value>| {
                        let state = state.clone();
                        async move {
                            let path = "/v1/consent/accept";
                            state.log.record("POST", path, &body, &headers);
                            if let Some(s) = state.overrides.pop_scripted(path) {
                                return s.into_response_paced(None, None).await;
                            }
                            Json(body).into_response()
                        }
                    }
                }),
            )
            .route(
                "/v1/user",
                get({
                    let state = state.clone();
                    move |axum::extract::RawQuery(query): axum::extract::RawQuery| {
                        let state = state.clone();
                        async move {
                            // Log the query string so a test can count `?include=subscription` on its own
                            let path = match query {
                                Some(q) if !q.is_empty() => format!("/v1/user?{q}"),
                                _ => "/v1/user".to_owned(),
                            };
                            state.log.record_get(&path);
                            state.user_info_arrivals.send_modify(|count| *count += 1);
                            let mut released = state.user_info_released.subscribe();
                            if !*released.borrow_and_update() {
                                let _ = released.wait_for(|r| *r).await;
                            }
                            let tier = state.user_tier.read().unwrap().clone();
                            let team = state.user_team.read().unwrap().clone();
                            let can_administer =
                                state.user_can_administer_team.read().unwrap().wire_value();
                            let opt_out = *state.user_coding_data_retention_opt_out.read().unwrap();
                            let mut body = json!({
                                "userId": "mock-user",
                                "email": "mock-user@test.invalid",
                            });
                            if let Some(obj) = body.as_object_mut() {
                                if let Some(opt_out) = opt_out {
                                    obj.insert("codingDataRetentionOptOut".into(), json!(opt_out));
                                }
                                if let Some(t) = tier {
                                    obj.insert("subscriptionTier".into(), json!(t));
                                }
                                if let Some(team) = team {
                                    obj.insert("teamId".into(), json!(team.id));
                                    obj.insert("teamName".into(), json!(team.name));
                                    obj.insert("teamRole".into(), json!(team.role));
                                }
                                if let Some(can_administer) = can_administer {
                                    obj.insert("canAdministerTeam".into(), can_administer);
                                }
                            }
                            Json(body).into_response()
                        }
                    }
                }),
            )
            .route(
                "/sessions/{id}/data",
                post({
                    let state = state.clone();
                    move |axum::extract::Path(id): axum::extract::Path<String>,
                          headers: HeaderMap,
                          Json(body): Json<Value>| {
                        let state = state.clone();
                        async move {
                            if let Some(reject) = state.overrides.auth_rejection(&headers) {
                                return reject;
                            }
                            state.log.record(
                                "POST",
                                &format!("/sessions/{id}/data"),
                                &body,
                                &headers,
                            );
                            StatusCode::OK.into_response()
                        }
                    }
                }),
            )
            .route(
                "/sessions/{id}",
                put({
                    let state = state.clone();
                    move |axum::extract::Path(id): axum::extract::Path<String>,
                          headers: HeaderMap,
                          Json(body): Json<Value>| {
                        let state = state.clone();
                        async move {
                            if let Some(reject) = state.overrides.auth_rejection(&headers) {
                                return reject;
                            }
                            state
                                .log
                                .record("PUT", &format!("/sessions/{id}"), &body, &headers);
                            StatusCode::OK.into_response()
                        }
                    }
                }),
            )
            .route(
                "/v1/storage",
                post({
                    let storage = state.storage.clone();
                    move |headers: HeaderMap, body: axum::body::Bytes| {
                        let storage = storage.clone();
                        async move { storage.handle(&headers, &body) }
                    }
                }),
            )
            .route(
                "/v1/feedback",
                post({
                    let feedback = state.feedback.clone();
                    move |headers: HeaderMap, body: axum::body::Bytes| {
                        let feedback = feedback.clone();
                        async move { feedback.handle(&headers, &body) }
                    }
                }),
            )
            .route(
                "/v1/events",
                post({
                    let telemetry = state.telemetry.clone();
                    move |body: axum::body::Bytes| {
                        let telemetry = telemetry.clone();
                        async move { telemetry.handle(&body) }
                    }
                }),
            )
            .route(
                "/v1/mcp/tools/list",
                get({
                    let state = state.clone();
                    move || {
                        let state = state.clone();
                        async move {
                            state.log.record_get("/v1/mcp/tools/list");
                            state.managed_gateway.list()
                        }
                    }
                }),
            )
            .route(
                "/v1/mcp/tools/call",
                post({
                    let state = state.clone();
                    move |headers: HeaderMap, Json(body): Json<Value>| {
                        let state = state.clone();
                        async move {
                            state
                                .log
                                .record("POST", "/v1/mcp/tools/call", &body, &headers);
                            state.managed_gateway.call(&headers, &body)
                        }
                    }
                }),
            )
            // 404 reads as an old proxy, so the shell falls back to a plain `POST /v1/storage`
            .route(
                "/v1/storage/exists",
                get(|| async { StatusCode::NOT_FOUND }),
            )
            .route(
                "/v1/storage/batch_exists",
                post(|| async { StatusCode::NOT_FOUND }),
            )
            .route(
                "/v1/storage/batch_upload_json",
                post(|| async { StatusCode::NOT_FOUND }),
            )
            .route(
                "/v1/storage/batch_upload",
                post(|| async { StatusCode::NOT_FOUND }),
            )
            .route(
                "/v1/storage/limits",
                get(|| async { StatusCode::NOT_FOUND }),
            )
            .layer(axum::extract::DefaultBodyLimit::max(256 * 1024 * 1024))
    }
}

fn lines<T: fmt::Display>(items: &[T]) -> String {
    items
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

impl Drop for MockInferenceServer {
    fn drop(&mut self) {
        self.state.overrides.release_parked_replies();
        self.state.overrides.release_completions();
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if !std::thread::panicking() {
            self.assert_no_script_violations();
        }
    }
}

#[cfg(test)]
#[path = "mock_server_tests.rs"]
mod tests;
