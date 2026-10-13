use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use rmcp::model::{
    CallToolResult, ContentBlock, ElicitRequestParams, ElicitResult, ElicitationAction, RequestId,
};
use rmcp::service::InboundStreamOrigin;
use tokio::sync::{Notify, oneshot};

/// Caps the notes on one tool result against a server that loops on elicitation.
const MAX_REFUSAL_NOTES: usize = 3;

/// Longest server name a refusal note quotes, in bytes.
const MAX_NOTE_SERVER_NAME_BYTES: usize = 64;

/// How long a finished call waits for its open input requests to end.
/// A server's cancel can arrive just after its tool result.
const ASK_END_GRACE: Duration = Duration::from_millis(250);

#[derive(Debug)]
pub struct ElicitationJob {
    pub server_name: String,
    /// Pre-validated by [`bridge_elicit`] via [`wire_mode_and_fields`], so consumers never see an unsupported mode.
    pub fields: WireElicitFields,
    pub response_tx: oneshot::Sender<ElicitResult>,
}

/// Why an elicitation ended without the user's answer reaching the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ElicitationRefusal {
    Declined,
    /// Dismissed without an answer.
    Cancelled,
    /// The server cancelled the request, or returned its result, before the user answered.
    Withdrawn,
    /// This client could not show the request. The mode was unsupported or no interactive client was attached.
    Unshown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AskOutcome {
    Accepted,
    Refused(ElicitationRefusal),
}

impl AskOutcome {
    fn from_action(action: &ElicitationAction) -> AskOutcome {
        match action {
            ElicitationAction::Accept => AskOutcome::Accepted,
            ElicitationAction::Decline => AskOutcome::Refused(ElicitationRefusal::Declined),
            _ => AskOutcome::Refused(ElicitationRefusal::Cancelled),
        }
    }
}

struct ElicitationInboxInner {
    slot: parking_lot::Mutex<Option<ElicitationJob>>,
    notify: Notify,
    closed: AtomicBool,
}

#[derive(Clone)]
pub struct ElicitationInbox {
    inner: Arc<ElicitationInboxInner>,
}

impl std::fmt::Debug for ElicitationInbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ElicitationInbox")
            .field("closed", &self.inner.closed.load(Ordering::SeqCst))
            .field("occupied", &self.inner.slot.lock().is_some())
            .finish()
    }
}

impl Default for ElicitationInbox {
    fn default() -> Self {
        Self::new()
    }
}

impl ElicitationInbox {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(ElicitationInboxInner {
                slot: parking_lot::Mutex::new(None),
                notify: Notify::new(),
                closed: AtomicBool::new(false),
            }),
        }
    }

    pub fn close(&self) {
        {
            let mut slot = self.inner.slot.lock();
            self.inner.closed.store(true, Ordering::SeqCst);
            if let Some(prev) = slot.take() {
                let _ = prev.response_tx.send(cancel_result());
            }
        }
        self.inner.notify.notify_waiters();
    }

    pub fn push(&self, job: ElicitationJob) -> Result<(), ElicitationJob> {
        {
            let mut slot = self.inner.slot.lock();
            if self.inner.closed.load(Ordering::SeqCst) {
                return Err(job);
            }
            if let Some(prev) = slot.replace(job) {
                let _ = prev.response_tx.send(cancel_result());
            }
        }
        self.inner.notify.notify_one();
        Ok(())
    }

    pub async fn recv(&self) -> Option<ElicitationJob> {
        loop {
            if let Some(job) = self.inner.slot.lock().take() {
                return Some(job);
            }
            if self.inner.closed.load(Ordering::SeqCst) {
                return None;
            }
            self.inner.notify.notified().await;
        }
    }
}

/// The requests running on one MCP client, and the input requests each one raised.
/// A refusal becomes a note on an agent tool call's result.
#[derive(Debug, Default)]
pub(crate) struct RequestTracker {
    running: parking_lot::Mutex<Vec<Arc<CallAsks>>>,
    next_service: AtomicU64,
}

/// One rmcp service of a client. A recovered service numbers its requests from the start again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ServiceId(u64);

#[derive(Debug, Default)]
struct CallAsks {
    state: parking_lot::Mutex<CallAskState>,
    ask_ended: Notify,
}

/// Notes describe the latest `tools/call` send. That covers the answers it carried and the input requests raised during it.
#[derive(Debug, Default)]
struct CallAskState {
    /// Counts the sends. An input request raised during an earlier send is ignored when it ends.
    send_seq: u64,
    /// The service and rmcp id of the latest send. Streamable HTTP tags each input request raised during a send with its id.
    request_id: Option<(ServiceId, RequestId)>,
    /// Input requests raised during the latest send that have not ended.
    open: usize,
    /// Refused MRTR input requests. The latest send carries their answers, or the next send once it goes out.
    answered: Vec<ElicitationRefusal>,
    /// Refused `elicitation/create` requests raised during the latest send.
    raised: Vec<ElicitationRefusal>,
}

#[derive(Debug, Clone, Copy)]
enum AskKind {
    /// An MRTR input request. Its answer goes in the call's next send.
    Answer,
    /// An `elicitation/create` request raised during send `send_seq`.
    Raised { send_seq: u64 },
}

impl RequestTracker {
    pub(crate) fn next_service_id(&self) -> ServiceId {
        ServiceId(self.next_service.fetch_add(1, Ordering::Relaxed))
    }

    pub(crate) fn begin_request(self: &Arc<Self>) -> RunningRequest {
        let asks = Arc::new(CallAsks::default());
        self.running.lock().push(Arc::clone(&asks));
        RunningRequest {
            tracker: Arc::clone(self),
            asks,
        }
    }

    /// Records a server's `elicitation/create` request on the request it was raised during.
    /// An untagged request, or one tagged with an id no send has recorded yet, belongs only to a sole running request.
    pub(crate) fn begin_server_ask(
        &self,
        service: ServiceId,
        origin: Option<&InboundStreamOrigin>,
    ) -> AskGuard {
        let tagged_id = match origin {
            Some(InboundStreamOrigin::OutboundRequest(id)) => Some(id),
            Some(InboundStreamOrigin::Unassociated) => return AskGuard::unowned(),
            None => None,
        };
        let running = self.running.lock();
        let owner = tagged_id
            .and_then(|id| {
                running
                    .iter()
                    .find(|asks| asks.state.lock().request_id == Some((service, id.clone())))
            })
            .or_else(|| match running.as_slice() {
                [only] => Some(only),
                _ => None,
            });
        let Some(asks) = owner else {
            return AskGuard::unowned();
        };
        let mut state = asks.state.lock();
        state.open += 1;
        AskGuard::owned(
            asks,
            AskKind::Raised {
                send_seq: state.send_seq,
            },
        )
    }
}

/// One running request and the input requests raised during it. Dropping it forgets them.
#[must_use]
pub(crate) struct RunningRequest {
    tracker: Arc<RequestTracker>,
    asks: Arc<CallAsks>,
}

impl RunningRequest {
    pub(crate) fn begin_answer_ask(&self) -> AskGuard {
        AskGuard::owned(&self.asks, AskKind::Answer)
    }

    /// Starts a `tools/call` send, including a re-send after a reconnect, before it goes out.
    /// Input requests raised during an earlier send no longer describe the result.
    pub(crate) fn begin_send(&self) {
        let mut state = self.asks.state.lock();
        state.send_seq += 1;
        state.request_id = None;
        state.open = 0;
        state.raised.clear();
    }

    /// Records the service and rmcp id of the send [`Self::begin_send`] started.
    pub(crate) fn bind_send(&self, service: ServiceId, request_id: &RequestId) {
        self.asks.state.lock().request_id = Some((service, request_id.clone()));
    }

    /// The server answered the latest send with `input_required`. It has seen every earlier answer.
    pub(crate) fn note_input_required(&self) {
        let mut state = self.asks.state.lock();
        state.answered.clear();
        state.raised.clear();
    }

    /// Notes lead the result because output truncation keeps the head.
    /// A request still open after [`ASK_END_GRACE`] counts as withdrawn.
    pub(crate) async fn annotate(self, server_name: &str, result: &mut CallToolResult) {
        let deadline = tokio::time::Instant::now() + ASK_END_GRACE;
        loop {
            let ended = self.asks.ask_ended.notified();
            if self.asks.state.lock().open == 0
                || tokio::time::timeout_at(deadline, ended).await.is_err()
            {
                break;
            }
        }
        let notes: Vec<ContentBlock> = {
            let state = self.asks.state.lock();
            state
                .answered
                .iter()
                .chain(&state.raised)
                .copied()
                .chain(std::iter::repeat_n(
                    ElicitationRefusal::Withdrawn,
                    state.open,
                ))
                .take(MAX_REFUSAL_NOTES)
                .map(|refusal| ContentBlock::text(refusal_note(server_name, refusal)))
                .collect()
        };
        result.content.splice(0..0, notes);
    }
}

impl Drop for RunningRequest {
    fn drop(&mut self) {
        self.tracker
            .running
            .lock()
            .retain(|asks| !Arc::ptr_eq(asks, &self.asks));
    }
}

/// Keeps a request other than an agent tool call on its client's tracker.
/// While it runs, an untagged server input request is not credited to a concurrent tool call.
#[must_use]
pub struct OutboundRequest {
    _request: RunningRequest,
}

impl OutboundRequest {
    pub(crate) fn new(tracker: &Arc<RequestTracker>) -> OutboundRequest {
        OutboundRequest {
            _request: tracker.begin_request(),
        }
    }
}

/// Records how one input request ended on the request that raised it.
/// A guard dropped before the answer records a withdrawal.
#[must_use]
pub(crate) struct AskGuard {
    /// `None` when no single request owns the input request.
    owner: Option<(Arc<CallAsks>, AskKind)>,
    outcome: AskOutcome,
}

impl AskGuard {
    fn owned(asks: &Arc<CallAsks>, kind: AskKind) -> AskGuard {
        AskGuard {
            owner: Some((Arc::clone(asks), kind)),
            outcome: AskOutcome::Refused(ElicitationRefusal::Withdrawn),
        }
    }

    fn unowned() -> AskGuard {
        AskGuard {
            owner: None,
            outcome: AskOutcome::Refused(ElicitationRefusal::Withdrawn),
        }
    }
}

impl Drop for AskGuard {
    fn drop(&mut self) {
        let Some((asks, kind)) = &self.owner else {
            return;
        };
        {
            let mut state = asks.state.lock();
            let refusals = match *kind {
                AskKind::Answer => &mut state.answered,
                AskKind::Raised { send_seq } if send_seq == state.send_seq => {
                    state.open -= 1;
                    &mut state.raised
                }
                AskKind::Raised { .. } => return,
            };
            if let AskOutcome::Refused(refusal) = self.outcome
                && refusals.len() < MAX_REFUSAL_NOTES
            {
                refusals.push(refusal);
            }
        }
        asks.ask_ended.notify_waiters();
    }
}

pub(crate) fn refusal_note(server_name: &str, refusal: ElicitationRefusal) -> String {
    let outcome = match refusal {
        ElicitationRefusal::Declined => "the user declined",
        ElicitationRefusal::Cancelled => "it was cancelled without an answer",
        ElicitationRefusal::Withdrawn => {
            "the server stopped waiting before the user answered (it may have timed out)"
        }
        ElicitationRefusal::Unshown => "grok could not show the request to the user",
    };
    let server_name =
        xai_grok_tools::util::truncate_str_with_marker(server_name, MAX_NOTE_SERVER_NAME_BYTES);
    format!(
        "[grok] MCP server \"{server_name}\" asked the user for input while this call was running and {outcome}. \
         Do not assume the requested action was performed; tell the user and ask before retrying."
    )
}

pub type SharedElicitationTx = Arc<parking_lot::Mutex<Option<ElicitationInbox>>>;

pub fn decline_result() -> ElicitResult {
    ElicitResult::new(ElicitationAction::Decline)
}

pub fn cancel_result() -> ElicitResult {
    ElicitResult::new(ElicitationAction::Cancel)
}

pub fn accept_result(content: Option<serde_json::Value>) -> ElicitResult {
    let mut result = ElicitResult::new(ElicitationAction::Accept);
    if let Some(c) = content {
        result = result.with_content(c);
    }
    result
}

/// Show one server input request to the user and record how it ended on `ask`.
pub(crate) async fn bridge_elicit(
    bridge: &SharedElicitationTx,
    server_name: &str,
    params: ElicitRequestParams,
    mut ask: AskGuard,
) -> ElicitResult {
    let Some(fields) = wire_mode_and_fields(&params) else {
        tracing::warn!(
            server = %server_name,
            "unsupported elicitation mode; declining"
        );
        ask.outcome = AskOutcome::Refused(ElicitationRefusal::Unshown);
        return decline_result();
    };

    let sender = bridge.lock().clone();
    let Some(tx) = sender else {
        tracing::debug!(
            server = %server_name,
            "elicitation request with no bridge installed; declining"
        );
        ask.outcome = AskOutcome::Refused(ElicitationRefusal::Unshown);
        return decline_result();
    };

    let (response_tx, response_rx) = oneshot::channel();
    let job = ElicitationJob {
        server_name: server_name.to_string(),
        fields,
        response_tx,
    };
    if tx.push(job).is_err() {
        tracing::warn!(
            server = %server_name,
            "elicitation bridge channel closed; cancelling"
        );
        ask.outcome = AskOutcome::Refused(ElicitationRefusal::Unshown);
        return cancel_result();
    }

    let result = match response_rx.await {
        Ok(result) => result,
        Err(_) => {
            tracing::warn!(
                server = %server_name,
                "elicitation response oneshot dropped; cancelling"
            );
            cancel_result()
        }
    };
    ask.outcome = AskOutcome::from_action(&result.action);
    result
}

pub fn elicit_result_from_wire(
    response: &xai_grok_tools::mcp_elicitation::McpElicitExtResponse,
) -> ElicitResult {
    use xai_grok_tools::mcp_elicitation::McpElicitExtResponse;
    match response {
        McpElicitExtResponse::Accept { content } => accept_result(content.clone()),
        McpElicitExtResponse::Decline => decline_result(),
        McpElicitExtResponse::Cancel => cancel_result(),
    }
}

/// The message and mode-tagged fields of a supported, size-validated elicitation request.
/// This is exactly what [`McpElicitExtRequest`] still needs on top of the session/tool-call identifiers the shell adds.
/// [`McpElicitExtRequest`]: xai_grok_tools::mcp_elicitation::McpElicitExtRequest
#[derive(Debug, Clone)]
pub struct WireElicitFields {
    pub message: String,
    pub mode: xai_grok_tools::mcp_elicitation::McpElicitModeFields,
}

pub fn wire_mode_and_fields(params: &ElicitRequestParams) -> Option<WireElicitFields> {
    use xai_grok_tools::mcp_elicitation::{
        MAX_ELICIT_ID_CHARS, MAX_ELICIT_MESSAGE_CHARS, MAX_ELICIT_SCHEMA_BYTES,
        MAX_ELICIT_URL_CHARS, McpElicitModeFields, chars_within,
    };
    match params {
        ElicitRequestParams::FormElicitationParams {
            message,
            requested_schema,
            ..
        } => {
            if !chars_within(message, MAX_ELICIT_MESSAGE_CHARS) {
                return None;
            }
            let schema = serde_json::to_value(requested_schema).unwrap_or(serde_json::Value::Null);
            let schema_len = serde_json::to_vec(&schema)
                .map(|b| b.len())
                .unwrap_or(usize::MAX);
            if schema_len > MAX_ELICIT_SCHEMA_BYTES {
                return None;
            }
            Some(WireElicitFields {
                message: message.clone(),
                mode: McpElicitModeFields::Form {
                    requested_schema: Some(schema),
                },
            })
        }
        ElicitRequestParams::UrlElicitationParams {
            message,
            url,
            elicitation_id,
            ..
        } => {
            if !chars_within(message, MAX_ELICIT_MESSAGE_CHARS)
                || !chars_within(url, MAX_ELICIT_URL_CHARS)
                || !chars_within(elicitation_id, MAX_ELICIT_ID_CHARS)
            {
                return None;
            }
            Some(WireElicitFields {
                message: message.clone(),
                mode: McpElicitModeFields::Url {
                    url: url.clone(),
                    elicitation_id: elicitation_id.clone(),
                },
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::{ElicitationSchema, PrimitiveSchemaDefinition, StringSchema};

    fn url_fields(message: &str, url: &str, elicitation_id: &str) -> WireElicitFields {
        wire_mode_and_fields(&ElicitRequestParams::UrlElicitationParams {
            meta: None,
            message: message.into(),
            url: url.into(),
            elicitation_id: elicitation_id.into(),
        })
        .expect("url mode is supported")
    }

    #[tokio::test]
    async fn no_bridge_declines() {
        let bridge: SharedElicitationTx = Arc::new(parking_lot::Mutex::new(None));
        let schema = ElicitationSchema::builder()
            .required_property(
                "email",
                PrimitiveSchemaDefinition::String(StringSchema::email()),
            )
            .build()
            .unwrap();
        let params = ElicitRequestParams::FormElicitationParams {
            meta: None,
            message: "hi".into(),
            requested_schema: schema,
        };
        let result = bridge_elicit(&bridge, "srv", params, AskGuard::unowned()).await;
        assert_eq!(result.action, ElicitationAction::Decline);
    }

    #[tokio::test]
    async fn bridge_accept_with_content() {
        let inbox = ElicitationInbox::new();
        let bridge: SharedElicitationTx = Arc::new(parking_lot::Mutex::new(Some(inbox.clone())));

        let schema = ElicitationSchema::builder()
            .required_property(
                "email",
                PrimitiveSchemaDefinition::String(StringSchema::email()),
            )
            .build()
            .unwrap();
        let params = ElicitRequestParams::FormElicitationParams {
            meta: None,
            message: "hi".into(),
            requested_schema: schema,
        };

        let handle = tokio::spawn(async move {
            let job = inbox.recv().await.expect("job");
            assert_eq!(job.server_name, "srv");
            let _ = job.response_tx.send(accept_result(Some(serde_json::json!({
                "email": "a@b.com"
            }))));
        });

        let result = bridge_elicit(&bridge, "srv", params, AskGuard::unowned()).await;
        handle.await.unwrap();
        assert_eq!(result.action, ElicitationAction::Accept);
        assert_eq!(
            result
                .content
                .as_ref()
                .and_then(|c| c.get("email"))
                .and_then(|e| e.as_str()),
            Some("a@b.com")
        );
    }

    /// Dropping the bridge future (server cancelled `elicitation/create`) must close the queued job's response channel.
    /// The coordinator's `response_tx.closed()` race can then dismiss the orphaned HITL card.
    #[tokio::test]
    async fn abandoned_bridge_closes_job_channel() {
        let inbox = ElicitationInbox::new();
        let bridge: SharedElicitationTx = Arc::new(parking_lot::Mutex::new(Some(inbox.clone())));
        let params = ElicitRequestParams::UrlElicitationParams {
            meta: None,
            message: "open".into(),
            url: "https://example.com".into(),
            elicitation_id: "e1".into(),
        };
        let task = tokio::spawn(async move {
            bridge_elicit(&bridge, "srv", params, AskGuard::unowned()).await
        });
        let mut job = inbox.recv().await.expect("job");
        task.abort();
        let _ = task.await;
        tokio::time::timeout(std::time::Duration::from_secs(1), job.response_tx.closed())
            .await
            .expect("sender must observe the receiver drop");
    }

    #[tokio::test]
    async fn closed_channel_cancels() {
        let inbox = ElicitationInbox::new();
        inbox.close();
        let bridge: SharedElicitationTx = Arc::new(parking_lot::Mutex::new(Some(inbox)));
        let params = ElicitRequestParams::UrlElicitationParams {
            meta: None,
            message: "open".into(),
            url: "https://example.com".into(),
            elicitation_id: "e1".into(),
        };
        let result = bridge_elicit(&bridge, "srv", params, AskGuard::unowned()).await;
        assert_eq!(result.action, ElicitationAction::Cancel);
    }

    #[tokio::test]
    async fn push_after_close_does_not_occupy_slot() {
        let inbox = ElicitationInbox::new();
        inbox.close();
        let (response_tx, _response_rx) = oneshot::channel();
        assert!(
            inbox
                .push(ElicitationJob {
                    server_name: "srv".into(),
                    fields: url_fields("late", "https://example.com", "late"),
                    response_tx,
                })
                .is_err()
        );
        let leftover = tokio::time::timeout(std::time::Duration::from_millis(50), inbox.recv())
            .await
            .expect("recv must not hang");
        assert!(leftover.is_none());
    }

    #[tokio::test]
    async fn concurrent_close_does_not_strand_a_push() {
        for _ in 0..200 {
            let inbox = ElicitationInbox::new();
            let pusher = inbox.clone();
            let thread = std::thread::spawn(move || {
                let (response_tx, response_rx) = oneshot::channel();
                let rejected = pusher
                    .push(ElicitationJob {
                        server_name: "srv".into(),
                        fields: url_fields("race", "https://example.com", "race"),
                        response_tx,
                    })
                    .is_err();
                (rejected, response_rx)
            });
            inbox.close();
            let (rejected, response_rx) = thread.join().expect("pusher");
            if !rejected {
                let action =
                    tokio::time::timeout(std::time::Duration::from_millis(50), response_rx)
                        .await
                        .expect("oneshot must complete")
                        .expect("oneshot must not drop")
                        .action;
                assert_eq!(action, ElicitationAction::Cancel);
            }
            let leftover = tokio::time::timeout(std::time::Duration::from_millis(50), inbox.recv())
                .await
                .expect("recv must not hang");
            assert!(leftover.is_none());
        }
    }

    #[tokio::test]
    async fn later_job_cancels_queued_job() {
        let inbox = ElicitationInbox::new();
        let first = {
            let (response_tx, response_rx) = oneshot::channel();
            inbox
                .push(ElicitationJob {
                    server_name: "a".into(),
                    fields: url_fields("first", "https://example.com/1", "1"),
                    response_tx,
                })
                .expect("push first");
            response_rx
        };
        inbox
            .push(ElicitationJob {
                server_name: "b".into(),
                fields: url_fields("second", "https://example.com/2", "2"),
                response_tx: oneshot::channel().0,
            })
            .expect("push second");
        assert_eq!(first.await.unwrap().action, ElicitationAction::Cancel);
        let kept = inbox.recv().await.expect("kept");
        assert_eq!(kept.server_name, "b");
    }

    #[test]
    fn wire_mapping_form_and_url() {
        use xai_grok_tools::mcp_elicitation::McpElicitModeFields;
        let schema = ElicitationSchema::builder()
            .required_property("x", PrimitiveSchemaDefinition::String(StringSchema::new()))
            .build()
            .unwrap();
        let form = ElicitRequestParams::FormElicitationParams {
            meta: None,
            message: "m".into(),
            requested_schema: schema,
        };
        let fields = wire_mode_and_fields(&form).expect("form mode is supported");
        assert_eq!(fields.message, "m");
        assert!(matches!(
            fields.mode,
            McpElicitModeFields::Form {
                requested_schema: Some(_)
            }
        ));

        let url_p = ElicitRequestParams::UrlElicitationParams {
            meta: None,
            message: "u".into(),
            url: "https://x.ai".into(),
            elicitation_id: "id1".into(),
        };
        let fields = wire_mode_and_fields(&url_p).expect("url mode is supported");
        assert_eq!(fields.message, "u");
        let McpElicitModeFields::Url {
            url,
            elicitation_id,
        } = fields.mode
        else {
            panic!("expected url mode");
        };
        assert_eq!(url, "https://x.ai");
        assert_eq!(elicitation_id, "id1");
    }

    #[test]
    fn oversized_message_is_declined() {
        use xai_grok_tools::mcp_elicitation::MAX_ELICIT_MESSAGE_CHARS;
        let schema = ElicitationSchema::builder()
            .required_property("x", PrimitiveSchemaDefinition::String(StringSchema::new()))
            .build()
            .unwrap();
        let params = ElicitRequestParams::FormElicitationParams {
            meta: None,
            message: "m".repeat(MAX_ELICIT_MESSAGE_CHARS + 1),
            requested_schema: schema,
        };
        assert!(wire_mode_and_fields(&params).is_none());
    }

    fn url_params(elicitation_id: &str) -> ElicitRequestParams {
        ElicitRequestParams::UrlElicitationParams {
            meta: None,
            message: "open".into(),
            url: "https://example.com".into(),
            elicitation_id: elicitation_id.into(),
        }
    }

    fn installed_bridge() -> (ElicitationInbox, SharedElicitationTx) {
        let inbox = ElicitationInbox::new();
        let bridge: SharedElicitationTx = Arc::new(parking_lot::Mutex::new(Some(inbox.clone())));
        (inbox, bridge)
    }

    /// Run one elicitation through `bridge`, answering it with `answer`.
    async fn answer_one(
        inbox: &ElicitationInbox,
        bridge: &SharedElicitationTx,
        ask: AskGuard,
        answer: ElicitResult,
    ) {
        let responder = {
            let inbox = inbox.clone();
            tokio::spawn(async move {
                let job = inbox.recv().await.expect("job");
                let _ = job.response_tx.send(answer);
            })
        };
        let _ = bridge_elicit(bridge, "srv", url_params("e"), ask).await;
        responder.await.expect("responder");
    }

    fn send(call: &RunningRequest, request_id: RequestId) {
        call.begin_send();
        call.bind_send(ServiceId(0), &request_id);
    }

    async fn annotated(call: RunningRequest) -> Vec<ContentBlock> {
        let mut result = CallToolResult::success(vec![ContentBlock::text("tool output")]);
        call.annotate("srv", &mut result).await;
        result.content
    }

    fn expected(refusals: &[ElicitationRefusal]) -> Vec<ContentBlock> {
        refusals
            .iter()
            .map(|refusal| ContentBlock::text(refusal_note("srv", *refusal)))
            .chain(std::iter::once(ContentBlock::text("tool output")))
            .collect()
    }

    #[tokio::test]
    async fn declined_and_cancelled_elicitations_annotate_the_call_result() {
        let (inbox, bridge) = installed_bridge();
        let call = Arc::new(RequestTracker::default()).begin_request();
        answer_one(&inbox, &bridge, call.begin_answer_ask(), decline_result()).await;
        answer_one(&inbox, &bridge, call.begin_answer_ask(), cancel_result()).await;
        assert_eq!(
            expected(&[ElicitationRefusal::Declined, ElicitationRefusal::Cancelled]),
            annotated(call).await
        );
    }

    #[tokio::test]
    async fn accepted_elicitation_leaves_the_call_result_alone() {
        let (inbox, bridge) = installed_bridge();
        let call = Arc::new(RequestTracker::default()).begin_request();
        answer_one(
            &inbox,
            &bridge,
            call.begin_answer_ask(),
            accept_result(None),
        )
        .await;
        assert_eq!(expected(&[]), annotated(call).await);
    }

    #[tokio::test]
    async fn abandoned_elicitation_annotates_as_withdrawn() {
        let (inbox, bridge) = installed_bridge();
        let call = Arc::new(RequestTracker::default()).begin_request();
        let task = tokio::spawn({
            let bridge = Arc::clone(&bridge);
            let ask = call.begin_answer_ask();
            async move { bridge_elicit(&bridge, "srv", url_params("e"), ask).await }
        });
        let _job = inbox.recv().await.expect("job");
        task.abort();
        let _ = task.await;
        assert_eq!(
            expected(&[ElicitationRefusal::Withdrawn]),
            annotated(call).await
        );
    }

    #[tokio::test]
    async fn unshown_elicitation_annotates_the_call_result() {
        let no_bridge: SharedElicitationTx = Arc::new(parking_lot::Mutex::new(None));
        let call = Arc::new(RequestTracker::default()).begin_request();
        let result =
            bridge_elicit(&no_bridge, "srv", url_params("e"), call.begin_answer_ask()).await;
        assert_eq!(ElicitationAction::Decline, result.action);
        assert_eq!(
            expected(&[ElicitationRefusal::Unshown]),
            annotated(call).await
        );
    }

    /// A server's decline can arrive just after its tool result.
    #[tokio::test(start_paused = true)]
    async fn refusal_arriving_after_the_result_is_reported() {
        let tracker = Arc::new(RequestTracker::default());
        let call = tracker.begin_request();
        let ask = tracker.begin_server_ask(ServiceId(0), None);
        let decline = tokio::spawn(async move {
            let mut ask = ask;
            tokio::time::sleep(ASK_END_GRACE / 2).await;
            ask.outcome = AskOutcome::Refused(ElicitationRefusal::Declined);
        });
        let content = annotated(call).await;
        decline.await.expect("decline");
        assert_eq!(expected(&[ElicitationRefusal::Declined]), content);
    }

    #[tokio::test(start_paused = true)]
    async fn request_still_open_after_the_grace_is_reported_as_withdrawn() {
        let tracker = Arc::new(RequestTracker::default());
        let call = tracker.begin_request();
        let _still_open = tracker.begin_server_ask(ServiceId(0), None);
        assert_eq!(
            expected(&[ElicitationRefusal::Withdrawn]),
            annotated(call).await
        );
    }

    #[tokio::test]
    async fn elicitation_create_is_recorded_on_the_sole_running_call() {
        let (inbox, bridge) = installed_bridge();
        let tracker = Arc::new(RequestTracker::default());
        let call = tracker.begin_request();
        answer_one(
            &inbox,
            &bridge,
            tracker.begin_server_ask(ServiceId(0), None),
            decline_result(),
        )
        .await;
        assert_eq!(
            expected(&[ElicitationRefusal::Declined]),
            annotated(call).await
        );
    }

    /// Nothing ties an `elicitation/create` request to one of several running calls.
    #[tokio::test]
    async fn elicitation_create_during_overlapping_calls_annotates_neither() {
        let (inbox, bridge) = installed_bridge();
        let tracker = Arc::new(RequestTracker::default());
        let first = tracker.begin_request();
        let second = tracker.begin_request();
        answer_one(
            &inbox,
            &bridge,
            tracker.begin_server_ask(ServiceId(0), None),
            decline_result(),
        )
        .await;
        assert_eq!(expected(&[]), annotated(first).await);
        assert_eq!(expected(&[]), annotated(second).await);
    }

    #[tokio::test]
    async fn retried_attempt_ignores_the_failed_attempt_refusals() {
        let (inbox, bridge) = installed_bridge();
        let tracker = Arc::new(RequestTracker::default());
        let failed = tracker.begin_request();
        answer_one(&inbox, &bridge, failed.begin_answer_ask(), decline_result()).await;
        drop(failed);
        let retry = tracker.begin_request();
        assert_eq!(expected(&[]), annotated(retry).await);
    }

    #[tokio::test]
    async fn tagged_elicitation_create_is_recorded_on_the_send_it_names() {
        let (inbox, bridge) = installed_bridge();
        let tracker = Arc::new(RequestTracker::default());
        let tagged = tracker.begin_request();
        let other = tracker.begin_request();
        send(&tagged, RequestId::Number(7));
        send(&other, RequestId::Number(8));
        let origin = InboundStreamOrigin::OutboundRequest(RequestId::Number(7));
        answer_one(
            &inbox,
            &bridge,
            tracker.begin_server_ask(ServiceId(0), Some(&origin)),
            decline_result(),
        )
        .await;
        assert_eq!(
            expected(&[ElicitationRefusal::Declined]),
            annotated(tagged).await
        );
        assert_eq!(expected(&[]), annotated(other).await);
    }

    /// rmcp returns the send's id only after the request is out.
    #[tokio::test]
    async fn input_request_raised_before_the_send_id_is_recorded_belongs_to_that_send() {
        let (inbox, bridge) = installed_bridge();
        let tracker = Arc::new(RequestTracker::default());
        let call = tracker.begin_request();
        call.begin_send();
        let origin = InboundStreamOrigin::OutboundRequest(RequestId::Number(9));
        answer_one(
            &inbox,
            &bridge,
            tracker.begin_server_ask(ServiceId(0), Some(&origin)),
            decline_result(),
        )
        .await;
        call.bind_send(ServiceId(0), &RequestId::Number(9));
        assert_eq!(
            expected(&[ElicitationRefusal::Declined]),
            annotated(call).await
        );
    }

    /// A recovered rmcp service numbers its requests from the start again.
    #[tokio::test]
    async fn reused_request_id_reaches_the_live_send() {
        let (inbox, bridge) = installed_bridge();
        let tracker = Arc::new(RequestTracker::default());
        let on_dead_service = tracker.begin_request();
        on_dead_service.begin_send();
        on_dead_service.bind_send(ServiceId(1), &RequestId::Number(3));
        let live = tracker.begin_request();
        live.begin_send();
        live.bind_send(ServiceId(2), &RequestId::Number(3));
        let origin = InboundStreamOrigin::OutboundRequest(RequestId::Number(3));
        let ask = tracker.begin_server_ask(ServiceId(2), Some(&origin));
        answer_one(&inbox, &bridge, ask, decline_result()).await;
        assert_eq!(
            expected(&[ElicitationRefusal::Declined]),
            annotated(live).await
        );
        assert_eq!(expected(&[]), annotated(on_dead_service).await);
    }

    #[tokio::test]
    async fn unassociated_elicitation_create_annotates_no_call() {
        let (inbox, bridge) = installed_bridge();
        let tracker = Arc::new(RequestTracker::default());
        let call = tracker.begin_request();
        let origin = InboundStreamOrigin::Unassociated;
        answer_one(
            &inbox,
            &bridge,
            tracker.begin_server_ask(ServiceId(0), Some(&origin)),
            decline_result(),
        )
        .await;
        assert_eq!(expected(&[]), annotated(call).await);
    }

    /// An `x.ai/mcp/call` or resource read on the same client makes an untagged input request ambiguous.
    #[tokio::test]
    async fn outbound_request_keeps_an_untagged_elicitation_off_the_tool_call() {
        let (inbox, bridge) = installed_bridge();
        let tracker = Arc::new(RequestTracker::default());
        let call = tracker.begin_request();
        let _outbound = OutboundRequest::new(&tracker);
        answer_one(
            &inbox,
            &bridge,
            tracker.begin_server_ask(ServiceId(0), None),
            decline_result(),
        )
        .await;
        assert_eq!(expected(&[]), annotated(call).await);
    }

    #[tokio::test]
    async fn resend_forgets_refusals_raised_during_the_failed_send() {
        let (inbox, bridge) = installed_bridge();
        let tracker = Arc::new(RequestTracker::default());
        let call = tracker.begin_request();
        send(&call, RequestId::Number(1));
        answer_one(
            &inbox,
            &bridge,
            tracker.begin_server_ask(ServiceId(0), None),
            decline_result(),
        )
        .await;
        send(&call, RequestId::Number(2));
        assert_eq!(expected(&[]), annotated(call).await);
    }

    #[tokio::test]
    async fn input_request_from_an_earlier_send_is_ignored_when_it_ends() {
        let tracker = Arc::new(RequestTracker::default());
        let call = tracker.begin_request();
        send(&call, RequestId::Number(1));
        let stale = tracker.begin_server_ask(ServiceId(0), None);
        send(&call, RequestId::Number(2));
        drop(stale);
        assert_eq!(expected(&[]), annotated(call).await);
    }

    #[tokio::test]
    async fn mrtr_refusal_is_still_noted_after_a_resend() {
        let (inbox, bridge) = installed_bridge();
        let call = Arc::new(RequestTracker::default()).begin_request();
        answer_one(&inbox, &bridge, call.begin_answer_ask(), decline_result()).await;
        send(&call, RequestId::Number(1));
        send(&call, RequestId::Number(2));
        assert_eq!(
            expected(&[ElicitationRefusal::Declined]),
            annotated(call).await
        );
    }

    #[tokio::test]
    async fn later_mrtr_round_replaces_an_earlier_refusal() {
        let (inbox, bridge) = installed_bridge();
        let call = Arc::new(RequestTracker::default()).begin_request();
        answer_one(&inbox, &bridge, call.begin_answer_ask(), decline_result()).await;
        call.note_input_required();
        answer_one(
            &inbox,
            &bridge,
            call.begin_answer_ask(),
            accept_result(None),
        )
        .await;
        assert_eq!(expected(&[]), annotated(call).await);
    }

    #[test]
    fn refusal_note_quotes_at_most_the_named_server_name_cap() {
        let note = refusal_note(
            &"s".repeat(MAX_NOTE_SERVER_NAME_BYTES * 4),
            ElicitationRefusal::Declined,
        );
        assert!(!note.contains(&"s".repeat(MAX_NOTE_SERVER_NAME_BYTES + 1)));
    }

    #[tokio::test]
    async fn refusal_notes_stop_at_the_named_cap() {
        let (inbox, bridge) = installed_bridge();
        let call = Arc::new(RequestTracker::default()).begin_request();
        for _ in 0..MAX_REFUSAL_NOTES + 2 {
            answer_one(&inbox, &bridge, call.begin_answer_ask(), decline_result()).await;
        }
        assert_eq!(
            expected(&[ElicitationRefusal::Declined; MAX_REFUSAL_NOTES]),
            annotated(call).await
        );
    }
}
