//! Sampling contract traits and 401-attribution types.
//!
//! These are pure interfaces with no I/O, so they can live beside the sampling
//! data types. `xai-grok-sampler` re-exports them for its runtime, and
//! `xai-grok-login` implements them without depending on the sampler runtime.

use std::pin::Pin;
use std::sync::Arc;

/// A 401-emitting site in the sampler client; its string identifier becomes the
/// `consumer` field so queries can break 401s down by API path.
/// This covers sampler endpoints only; tool clients use their own consumer type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SamplingConsumer {
    /// `chat_completion_stream`: OpenAI-compatible streaming OpenAI Chat Completions API.
    ChatCompletionsStream,
    /// `chat_completion`: OpenAI-compatible non-streaming OpenAI Chat Completions API.
    ChatCompletions,
    /// `create_response_stream`: Responses API streaming.
    ResponsesStream,
    /// `create_response`: Responses API non-streaming.
    Responses,
    /// `messages_stream`: Anthropic Messages API streaming.
    MessagesStream,
    /// `messages`: Anthropic Messages API non-streaming.
    Messages,
}

impl SamplingConsumer {
    /// Stable string identifier for this emit site.
    /// Callbacks typically combine this with a fixed prefix (e.g. the client type)
    /// when building the consumer field of the attribution event.
    pub fn as_endpoint(self) -> &'static str {
        match self {
            Self::ChatCompletionsStream => "chat_completions_stream",
            Self::ChatCompletions => "chat_completions",
            Self::ResponsesStream => "responses_stream",
            Self::Responses => "responses",
            Self::MessagesStream => "messages_stream",
            Self::Messages => "messages",
        }
    }
}

/// Hook invoked by the sampler client at every 401 response site.
/// Implementations must be cheap and non-blocking; this runs on the user-visible
/// 401 error path. The `Debug` bound is required because `SamplerConfig` derives
/// `Debug` and holds an `Option<Arc<dyn Auth401AttributionCallback>>`.
pub trait Auth401AttributionCallback: Send + Sync + std::fmt::Debug {
    /// `sent_bearer_suffix` is the truncated tail of the bearer sent on the wire.
    /// It is truncated before crossing this boundary so the full credential never
    /// leaves the sampler client. `None` means no bearer header was sent at all.
    fn record_401(&self, consumer: SamplingConsumer, sent_bearer_suffix: Option<&str>);
}

/// Shared, cheap-to-clone alias for the attribution callback.
pub type SharedAttributionCallback = Arc<dyn Auth401AttributionCallback>;

/// Cheap sync read of the current bearer for the sampler config's `bearer_resolver`.
pub trait BearerResolver: Send + Sync + std::fmt::Debug {
    fn current_bearer(&self) -> Option<String>;

    /// Awaited by the client right before it stamps a request; [`Self::current_bearer`]
    /// is read afterwards. A resolver that can renew its bearer does so here when the
    /// cached one would not survive the send, so the request never leaves with no
    /// credential. Default: no-op.
    fn prepare_for_send(&self) -> Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        Box::pin(async {})
    }
}

/// Shared, cheap-to-clone alias for the bearer resolver.
pub type SharedBearerResolver = Arc<dyn BearerResolver>;
