//! Per-request sampling configuration and the auth/header knobs it carries.
//!
//! Lives beside the sampling data types (not in the sampler runtime) so the shell
//! can build, read, and persist a `SamplerConfig` without depending on the sampler.

use std::path::PathBuf;
use std::sync::Arc;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::contracts::{SharedAttributionCallback, SharedBearerResolver};
use crate::doom_loop::DoomLoopRecoveryPolicy;
use crate::origin::OriginClientInfo;
use crate::types::{
    ApiBackend, CompactionAtTokens, CompactionsRemaining, ConversationGroupId, ReasoningEffort,
};

/// Which auth header shape a request uses.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AuthScheme {
    #[default]
    Bearer,
    XApiKey,
}

/// Per-request header injection (e.g. OTel `traceparent`).
pub trait HeaderInjector: Send + Sync + std::fmt::Debug {
    fn inject(&self, headers: &mut reqwest::header::HeaderMap);
}

/// Shared, cheap-to-clone alias for the header injector.
pub type SharedHeaderInjector = Arc<dyn HeaderInjector>;

/// All knobs that control a single sampling request.
/// Auth is selected separately via `auth_scheme`, while `api_backend` controls only
/// the request/response protocol shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SamplerConfig {
    pub api_key: Option<String>,
    pub base_url: String,
    /// Resolved local directory for this model's mTLS client identity.
    #[serde(default)]
    pub mtls_cert_dir: Option<PathBuf>,
    pub model: String,
    pub max_completion_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub api_backend: ApiBackend,
    #[serde(default)]
    pub auth_scheme: AuthScheme,
    /// Extra request headers applied verbatim. The sampler never inspects the URL to
    /// derive headers. Callers (the session) inject proxy auth and other access
    /// headers here before constructing the config.
    pub extra_headers: IndexMap<String, String>,
    /// Additional Responses API `include` values not represented by the typed client.
    #[serde(default)]
    pub extra_response_includes: Vec<String>,
    /// Query parameters folded into every request URL (percent-encoded).
    #[serde(default)]
    pub query_params: IndexMap<String, String>,
    /// Header name to environment variable, resolved into request headers at client
    /// build and never persisted.
    #[serde(default)]
    pub env_http_headers: IndexMap<String, String>,
    /// Total context window size in tokens.
    /// The sampler does not enforce it; the session uses it for compaction decisions.
    pub context_window: u64,
    pub force_http1: bool,
    pub max_retries: Option<u32>,
    /// Total-attempt ceiling for rate-limited requests.
    /// `None` keeps the sampler actor's retry policy threshold.
    #[serde(default)]
    pub rate_limit_retry_threshold: Option<u32>,
    pub stream_tool_calls: bool,
    pub idle_timeout_secs: Option<u64>,

    // Reasoning effort
    pub reasoning_effort: Option<ReasoningEffort>,

    // Client identity
    pub origin_client: Option<OriginClientInfo>,
    pub client_identifier: Option<String>,
    pub deployment_id: Option<String>,
    pub user_id: Option<String>,
    /// Stable root conversation identifier emitted as `x-grok-conv-group-id`.
    #[serde(default)]
    pub conversation_group_id: Option<ConversationGroupId>,
    pub client_version: Option<String>,

    /// Hook invoked on every 401 response with the bearer that was actually sent on the wire.
    /// Implementations typically compare it against a live credential source to tell a
    /// stale token from a server-rejected live one.
    /// `None` (default) is a no-op; the 401 arm still surfaces an auth error.
    #[serde(skip)]
    pub attribution_callback: Option<SharedAttributionCallback>,

    /// Resolves a fresh bearer for each request. `None` uses the construction-time `api_key`.
    #[serde(skip)]
    pub bearer_resolver: Option<SharedBearerResolver>,

    #[serde(default)]
    pub supports_backend_search: bool,

    /// Per-model config for the `x-compactions-remaining` header; `None` disables it.
    #[serde(default)]
    pub compactions_remaining: Option<CompactionsRemaining>,

    /// Per-model config for the `x-compaction-at` header; `None` disables it.
    #[serde(default)]
    pub compaction_at_tokens: Option<CompactionAtTokens>,

    /// Server-side doom-loop check policy; `None` disables it.
    /// It also absorbs the reported trigger events (unlike environment headers in
    /// `extra_headers`, this gates the client's decode behavior).
    #[serde(default)]
    pub doom_loop_recovery: Option<DoomLoopRecoveryPolicy>,

    /// Per-request header injector (e.g. OTel traceparent). Called in `post()`.
    #[serde(skip)]
    pub header_injector: Option<SharedHeaderInjector>,
}

impl Default for SamplerConfig {
    /// Empty defaults so callers can use `..Default::default()` and new fields don't
    /// ripple through every literal site.
    fn default() -> Self {
        Self {
            api_key: None,
            base_url: String::new(),
            mtls_cert_dir: None,
            model: String::new(),
            max_completion_tokens: None,
            temperature: None,
            top_p: None,
            api_backend: ApiBackend::default(),
            auth_scheme: AuthScheme::default(),
            extra_headers: IndexMap::new(),
            extra_response_includes: Vec::new(),
            query_params: IndexMap::new(),
            env_http_headers: IndexMap::new(),
            context_window: 0,
            force_http1: false,
            max_retries: None,
            rate_limit_retry_threshold: None,
            stream_tool_calls: false,
            idle_timeout_secs: None,
            reasoning_effort: None,
            origin_client: None,
            client_identifier: None,
            deployment_id: None,
            user_id: None,
            conversation_group_id: None,
            client_version: None,
            attribution_callback: None,
            bearer_resolver: None,
            supports_backend_search: false,
            compactions_remaining: None,
            compaction_at_tokens: None,
            doom_loop_recovery: None,
            header_injector: None,
        }
    }
}
