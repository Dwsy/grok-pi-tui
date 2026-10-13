//! The one handler behind the three inference endpoints. A request is admitted, logged, and offered
//! to the overrides; one they decline is answered by the next queued agent turn or the response
//! mode, in the format of the endpoint it arrived on.

use std::borrow::Cow;
use std::collections::VecDeque;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{MethodRouter, post};
use serde_json::Value;

use crate::conversation::{ConversationTracker, ReadConversation};
use crate::inference_override::InferenceOverrides;
use crate::inference_request::{
    InferenceEndpoint, InferenceRequest, InferenceRequestKind, last_user_message, model_name,
};
use crate::request_log::RequestLog;
use crate::scripted::ScriptedResponse;
use crate::sse;

#[derive(Clone)]
enum ResponseMode {
    /// `Echo: <last user message>`, with whitespace collapsed.
    Echo,
    /// Deltas reconstruct the text byte for byte, newlines included.
    Fixed(String),
}

impl ResponseMode {
    fn text(&self, body: &Value) -> Cow<'_, str> {
        match self {
            ResponseMode::Echo => {
                let user_message = last_user_message(body).unwrap_or_else(|| "hello".to_owned());
                Cow::Owned(format!("Echo: {user_message}"))
            }
            ResponseMode::Fixed(text) => Cow::Borrowed(text),
        }
    }
}

/// Read once per request so one request sees one setting of each.
#[derive(Clone)]
struct FallbackSettings {
    mode: ResponseMode,
    /// `stop_reason` on the `/v1/messages` terminal `message_delta`.
    messages_stop_reason: String,
    chunk_delay: Option<Duration>,
    park_auxiliary: bool,
    /// One-shot hold for the next foreground request, including a tool-call reply.
    /// Script `.hold()` leaves tool calls unparked so a job can start; this holds the turn's first
    /// answer itself.
    park_foreground: bool,
    /// One-shot hold consumed only by an auxiliary whose `x-grok-req-id` starts with this prefix.
    park_request_prefix: Option<String>,
}

#[derive(Clone)]
pub(crate) struct InferenceRoute {
    log: Arc<RequestLog>,
    conversations: ConversationTracker,
    overrides: InferenceOverrides,
    fallback: Arc<std::sync::RwLock<FallbackSettings>>,
    /// One assistant text per foreground turn, consumed in order.
    agent_turns: Arc<std::sync::Mutex<VecDeque<String>>>,
}

impl InferenceRoute {
    pub(crate) fn new(log: Arc<RequestLog>, overrides: InferenceOverrides) -> Self {
        InferenceRoute {
            log,
            conversations: ConversationTracker::default(),
            overrides,
            fallback: Arc::new(std::sync::RwLock::new(FallbackSettings {
                mode: ResponseMode::Echo,
                messages_stop_reason: "end_turn".to_owned(),
                chunk_delay: None,
                park_auxiliary: false,
                park_foreground: false,
                park_request_prefix: None,
            })),
            agent_turns: Arc::new(std::sync::Mutex::new(VecDeque::new())),
        }
    }

    pub(crate) fn conversations(&self) -> Vec<ReadConversation> {
        self.conversations.snapshot(&self.log.entries())
    }

    pub(crate) fn set_response(&self, text: String) {
        self.write_fallback().mode = ResponseMode::Fixed(text);
    }

    pub(crate) fn set_auxiliary_hold(&self) {
        self.write_fallback().park_auxiliary = true;
    }

    /// Park the next foreground reply, tool call included, until [`InferenceOverrides::release_parked_replies`].
    pub(crate) fn set_foreground_hold(&self) {
        self.write_fallback().park_foreground = true;
    }

    /// Park the next auxiliary whose request id starts with `prefix`. A different auxiliary leaves it armed.
    pub(crate) fn set_auxiliary_hold_matching(&self, prefix: impl Into<String>) {
        self.write_fallback().park_request_prefix = Some(prefix.into());
    }

    pub(crate) fn set_agent_turns(&self, turns: VecDeque<String>) {
        *self.agent_turns.lock().unwrap() = turns;
    }

    pub(crate) fn set_messages_stop_reason(&self, stop_reason: String) {
        self.write_fallback().messages_stop_reason = stop_reason;
    }

    pub(crate) fn set_chunk_delay(&self, delay: Option<Duration>) {
        self.write_fallback().chunk_delay = delay;
    }

    fn write_fallback(&self) -> FallbackWrite<'_> {
        FallbackWrite {
            guard: Some(self.fallback.write().unwrap()),
        }
    }

    pub(crate) fn handler(&self, endpoint: InferenceEndpoint) -> MethodRouter {
        let route = self.clone();
        post(move |uri: Uri, headers: HeaderMap, raw: Bytes| {
            let query = uri.query().map(str::to_owned);
            route.clone().serve(endpoint, query, headers, raw)
        })
    }

    async fn serve(
        self,
        endpoint: InferenceEndpoint,
        query: Option<String>,
        headers: HeaderMap,
        raw: Bytes,
    ) -> Response {
        let Ok(body) = serde_json::from_slice::<Value>(&raw) else {
            return (
                StatusCode::BAD_REQUEST,
                "inference request body was not JSON",
            )
                .into_response();
        };

        let request = InferenceRequest::new(&self.conversations, endpoint, &headers, &body);
        let sequence = self.log.record_inference(&request, &raw, query.as_deref());
        let settings = self.fallback.read().unwrap().clone();
        let park_foreground = request.kind() == InferenceRequestKind::Foreground && {
            let mut settings = self.write_fallback();
            std::mem::take(&mut settings.park_foreground)
        };
        if park_foreground {
            self.overrides
                .wait_until_park_released(
                    request
                        .conversation()
                        .map(crate::conversation::ConversationId::number),
                )
                .await;
        }
        if let Some(response) = self
            .overrides
            .response_override(
                &request,
                settings.chunk_delay,
                |failure| self.log.note_failure(sequence, failure),
                |conversation| self.log.conversation_requests(conversation),
                |reply| self.log.note_scripted_reply(sequence, reply),
            )
            .await
        {
            return response;
        }

        let agent_turn = (request.kind() == InferenceRequestKind::Foreground)
            .then(|| self.agent_turns.lock().unwrap().pop_front())
            .flatten();
        let mode = match agent_turn {
            Some(text) => ResponseMode::Fixed(text),
            None => settings.mode,
        };
        let park_matching = request.kind() == InferenceRequestKind::Auxiliary
            && request.request_id().is_some_and(|id| {
                let mut settings = self.write_fallback();
                let matches = settings
                    .park_request_prefix
                    .as_deref()
                    .is_some_and(|prefix| id.starts_with(prefix));
                if matches {
                    settings.park_request_prefix = None;
                }
                matches
            });
        let park_auxiliary = request.kind() == InferenceRequestKind::Auxiliary && {
            let mut settings = self.write_fallback();
            std::mem::take(&mut settings.park_auxiliary)
        };
        if park_matching {
            self.overrides.note_matching_park();
        }
        if park_auxiliary || park_matching {
            self.overrides
                .wait_until_park_released(
                    request
                        .conversation()
                        .map(crate::conversation::ConversationId::number),
                )
                .await;
        }
        let text = mode.text(&body);
        let model = model_name(&body);
        let events = match (endpoint, &mode) {
            (InferenceEndpoint::ChatCompletions, ResponseMode::Echo) => {
                sse::chat_completion_script(&text, model)
            }
            (InferenceEndpoint::ChatCompletions, ResponseMode::Fixed(_)) => {
                sse::chat_completion_script_exact(&text, model)
            }
            (InferenceEndpoint::Responses, ResponseMode::Echo) => {
                sse::responses_api_script(&text, model)
            }
            (InferenceEndpoint::Responses, ResponseMode::Fixed(_)) => {
                sse::responses_api_script_exact(&text, model)
            }
            (InferenceEndpoint::Messages, ResponseMode::Echo | ResponseMode::Fixed(_)) => {
                sse::messages_api_script(&text, model, &settings.messages_stop_reason)
            }
        };
        let wait = self.overrides.fallback_terminal_wait(&request);
        let response = ScriptedResponse::sse(events)
            .into_response_paced(settings.chunk_delay, wait)
            .await;
        self.log.note_finished(sequence);
        response
    }
}

#[cfg(test)]
static OPEN_HOLD_GAP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

struct FallbackWrite<'a> {
    guard: Option<std::sync::RwLockWriteGuard<'a, FallbackSettings>>,
}

impl Deref for FallbackWrite<'_> {
    type Target = FallbackSettings;

    fn deref(&self) -> &FallbackSettings {
        self.guard.as_ref().expect("fallback write guard")
    }
}

impl DerefMut for FallbackWrite<'_> {
    fn deref_mut(&mut self) -> &mut FallbackSettings {
        self.guard.as_mut().expect("fallback write guard")
    }
}

impl Drop for FallbackWrite<'_> {
    fn drop(&mut self) {
        self.guard.take();
        #[cfg(test)]
        if OPEN_HOLD_GAP.load(std::sync::atomic::Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(40));
        }
    }
}

#[cfg(test)]
#[path = "inference_route_tests.rs"]
mod tests;
