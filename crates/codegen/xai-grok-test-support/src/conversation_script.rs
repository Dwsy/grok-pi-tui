//! What a test scripts for the model, one [`Conversation`] at a time: each turn's tool calls in order,
//! then its reply, with the failures that answer in their place while their counts last. Call ids carry
//! the conversation and call number ([`mock_call_id`]) so inherited history does not advance a child's script.

use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use regex::Regex;
use serde_json::Value;

use crate::conversation::ConversationId;
use crate::failure::{Failure, StatusFailure, StreamError};
use crate::model_reply::ModelEvent;
use crate::sse::UsageReport;
use crate::tools::Tool;

const MOCK_CALL_ID_PREFIX: &str = "call_mock_";

/// `call_mock_<conversation>_<call_number>`, both counted from 1.
#[must_use]
pub fn mock_call_id(conversation: usize, call_number: usize) -> String {
    format!("{MOCK_CALL_ID_PREFIX}{conversation}_{call_number}")
}

pub(crate) fn mock_call_id_prefix(conversation: ConversationId) -> String {
    format!("{MOCK_CALL_ID_PREFIX}{}_", conversation.number())
}

/// A substring the result must contain, or a pattern it must match. The label doubles as the text
/// of the violation report.
#[derive(Debug, Clone)]
pub(crate) enum TextCheck {
    Contains(String),
    Matches { pattern: String, regex: Regex },
}

impl TextCheck {
    pub(crate) fn contains(expected: impl Into<String>) -> Self {
        TextCheck::Contains(expected.into())
    }

    pub(crate) fn matches(pattern: impl Into<String>) -> Result<Self, regex::Error> {
        let pattern = pattern.into();
        let regex = Regex::new(&pattern)?;
        Ok(TextCheck::Matches { pattern, regex })
    }

    pub(crate) fn label(&self) -> &str {
        match self {
            TextCheck::Contains(expected) => expected,
            TextCheck::Matches { pattern, .. } => pattern,
        }
    }

    pub(crate) fn is_met_by(&self, text: &str) -> bool {
        match self {
            TextCheck::Contains(expected) => text.contains(expected),
            TextCheck::Matches { regex, .. } => regex.is_match(text),
        }
    }
}

#[derive(Debug, Clone)]
#[must_use]
pub struct MockToolCall {
    pub(crate) tool: Tool,
    pub(crate) arguments: Value,
    pub(crate) result_check: Option<TextCheck>,
}

impl MockToolCall {
    pub fn new(tool: Tool, arguments: Value) -> Self {
        MockToolCall {
            tool,
            arguments,
            result_check: None,
        }
    }

    /// Pin what the model is shown for this call after hooks or permissions acted on it.
    pub fn result_contains(mut self, expected: impl Into<String>) -> Self {
        self.result_check = Some(TextCheck::contains(expected));
        self
    }

    /// Pin the result against a regular expression, for a result whose exact text a case cannot
    /// spell (an absolute path, a rendered timestamp).
    pub fn result_matches(mut self, pattern: impl Into<String>) -> Result<Self, regex::Error> {
        self.result_check = Some(TextCheck::matches(pattern)?);
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) enum CallDelivery {
    #[default]
    Sequential,
    Parallel,
}

#[derive(Debug, Clone)]
pub(crate) struct ScriptEntry {
    pub(crate) tool_calls: Vec<MockToolCall>,
    pub(crate) delivery: CallDelivery,
    pub(crate) reply: String,
    pub(crate) at_request: Option<usize>,
    /// A side request such as a compaction summary must not take the next ordinary reply.
    pub(crate) body_contains: Option<String>,
    pub(crate) release_when_body_contains: Option<String>,
    pub(crate) stall: Option<Duration>,
    /// Answer only once conversation `0` has logged request `1`, counted from 1.
    /// `hold_reply_only` waits on the reply step, so an earlier tool call in the same turn still returns.
    pub(crate) hold_until: Option<(usize, usize)>,
    pub(crate) hold_reply_only: bool,
    /// Park the answer until the mock releases parked replies, so a cancel can fire while it is held.
    pub(crate) park: bool,
    /// Set when this park has its own release. Absent parks wait on the shutdown latch.
    pub(crate) park_token: Option<u64>,
    /// The turn in the installed script, so a later log read does not count requests.
    pub(crate) reply_index: Option<usize>,
    pub(crate) answered: bool,
    /// Hold this answer until the mock LSP request log at this path shows a second process.
    pub(crate) hold_until_lsp_log: Option<PathBuf>,
    pub(crate) reasoning: Option<String>,
    pub(crate) usage: Option<UsageReport>,
    pub(crate) failures: Vec<Failure>,
    /// Once every failure on this entry is spent, the entry is accounted. No later reply is required.
    pub(crate) failure_closes_turn: bool,
    pub(crate) events: Option<Vec<ModelEvent>>,
}

/// The last entry repeats once the script is served through, so there is always one to answer from.
#[derive(Debug, Clone)]
pub(crate) struct ScriptEntries {
    earlier: Vec<ScriptEntry>,
    last: ScriptEntry,
}

impl ScriptEntries {
    pub(crate) fn len(&self) -> usize {
        self.earlier.len() + 1
    }

    pub(crate) fn at(&self, index: usize) -> &ScriptEntry {
        self.earlier.get(index).unwrap_or(&self.last)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &ScriptEntry> {
        self.earlier.iter().chain(std::iter::once(&self.last))
    }

    pub(crate) fn pinned_to(&self, request: usize) -> Option<usize> {
        self.iter()
            .position(|entry| entry.at_request == Some(request))
    }

    pub(crate) fn request_pinned_twice(&self) -> Option<usize> {
        let mut pinned: Vec<usize> = self.iter().filter_map(|entry| entry.at_request).collect();
        pinned.sort_unstable();
        pinned.windows(2).find_map(|pair| {
            let [earlier, later] = pair else {
                return None;
            };
            (earlier == later).then_some(*earlier)
        })
    }

    /// Call ids continue the count of the calls the entries before it made.
    pub(crate) fn calls(
        &self,
        index: usize,
        conversation: ConversationId,
    ) -> impl Iterator<Item = (String, &MockToolCall)> {
        let calls_before: usize = self
            .earlier
            .iter()
            .take(index)
            .map(|entry| entry.tool_calls.len())
            .sum();
        self.at(index)
            .tool_calls
            .iter()
            .enumerate()
            .map(move |(position, call)| {
                (
                    mock_call_id(conversation.number(), calls_before + position + 1),
                    call,
                )
            })
    }
}

#[derive(Debug, Clone)]
pub(crate) enum ScriptTarget {
    Conversation(ConversationId),
    Opening(TextCheck),
}

impl fmt::Display for ScriptTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScriptTarget::Conversation(conversation) => write!(f, "{conversation}"),
            ScriptTarget::Opening(opening) => {
                write!(f, "the conversation opened with {:?}", opening.label())
            }
        }
    }
}

#[derive(Debug, Clone, Default)]
struct PendingTurn {
    calls: Vec<MockToolCall>,
    delivery: CallDelivery,
    at_request: Option<usize>,
    body_contains: Option<String>,
    release_when_body_contains: Option<String>,
    stall: Option<Duration>,
    hold_until: Option<(usize, usize)>,
    hold_reply_only: bool,
    park: bool,
    park_token: Option<u64>,
    reply_index: Option<usize>,
    hold_until_lsp_log: Option<PathBuf>,
    reasoning: Option<String>,
    usage: Option<UsageReport>,
    failures: Vec<Failure>,
    failure_closes_turn: bool,
}

impl PendingTurn {
    fn is_open(&self) -> bool {
        !self.calls.is_empty()
            || matches!(self.delivery, CallDelivery::Parallel)
            || self.at_request.is_some()
            || self.body_contains.is_some()
            || self.release_when_body_contains.is_some()
            || self.stall.is_some()
            || self.hold_until.is_some()
            || self.park
            || self.hold_until_lsp_log.is_some()
            || self.reasoning.is_some()
            || self.usage.is_some()
            || !self.failures.is_empty()
    }

    fn close(self, reply: String) -> ScriptEntry {
        ScriptEntry {
            tool_calls: self.calls,
            delivery: self.delivery,
            reply,
            at_request: self.at_request,
            body_contains: self.body_contains,
            release_when_body_contains: self.release_when_body_contains,
            stall: self.stall,
            hold_until: self.hold_until,
            hold_reply_only: self.hold_reply_only,
            park: self.park,
            park_token: self.park_token,
            reply_index: self.reply_index,
            answered: false,
            hold_until_lsp_log: self.hold_until_lsp_log,
            reasoning: self.reasoning,
            usage: self.usage,
            failures: self.failures,
            failure_closes_turn: self.failure_closes_turn,
            events: None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct CompiledTurn {
    pub calls: Vec<MockToolCall>,
    pub parallel: bool,
    pub failures: Vec<CompiledFailure>,
    pub reply: String,
    pub events: Option<Vec<ModelEvent>>,
    /// The stream error is the whole turn. Serving its count accounts the entry.
    pub closing_stream_error: Option<StreamError>,
    pub at_request: Option<usize>,
    pub body_contains: Option<String>,
    pub release_when_body_contains: Option<String>,
    pub stall: Option<Duration>,
    pub hold_until: Option<(usize, usize)>,
    pub hold_reply_only: bool,
    pub park: bool,
    pub park_token: Option<u64>,
    pub reply_index: Option<usize>,
    pub hold_until_lsp_log: Option<PathBuf>,
    pub reasoning: Option<String>,
    pub usage: Option<UsageReport>,
}

#[derive(Debug, Clone)]
pub enum CompiledFailure {
    Status(StatusFailure),
    Stream(StreamError),
    Dropped,
    Malformed,
    Hang,
    CutOnce,
    DoomLoop,
    Empty { count: usize },
    Truncated,
    Cut { count: usize },
    ContentFilter { count: usize },
}

/// `.reply` closes the turn being built. A pin is applied before that close: `.calls` after it
/// starts the next turn, a second `.reply` in a row is a turn with no calls, and every conversation
/// ends with a `.reply`.
#[derive(Debug, Clone)]
#[must_use]
pub struct Conversation {
    target: ScriptTarget,
    turns: Vec<ScriptEntry>,
    pending: PendingTurn,
}

impl Conversation {
    /// Conversations are numbered from 1 in the order the mock opens them.
    pub fn nth(number: usize) -> Self {
        Conversation::new(ScriptTarget::Conversation(ConversationId::nth(number)))
    }

    /// Subagents started together arrive in either order, so each is found by its prompt.
    pub fn for_system_prompt(contains: impl Into<String>) -> Self {
        Conversation::new(ScriptTarget::Opening(TextCheck::contains(contains)))
    }

    fn new(target: ScriptTarget) -> Self {
        Conversation {
            target,
            turns: Vec::new(),
            pending: PendingTurn::default(),
        }
    }

    pub fn calls(mut self, calls: impl IntoIterator<Item = MockToolCall>) -> Self {
        self.pending.calls.extend(calls);
        self
    }

    /// The reply follows once every result is back.
    pub fn parallel_tool_calls(mut self) -> Self {
        self.pending.delivery = CallDelivery::Parallel;
        self
    }

    /// Pin this turn before [`Self::reply`] closes it. Two turns pinned to one request panic.
    pub fn at_request(mut self, nth: usize) -> Self {
        assert!(nth >= 1, "requests are counted from 1");
        self.pending.at_request = Some(nth);
        self
    }

    /// Ordinary requests skip it, and settle still waits until a matching request is answered.
    pub fn when_body_contains(mut self, text: impl Into<String>) -> Self {
        self.pending.body_contains = Some(text.into());
        self
    }

    pub fn release_when_body_contains(mut self, text: impl Into<String>) -> Self {
        self.pending.release_when_body_contains = Some(text.into());
        self
    }

    /// Hold every answer for this turn, a failure's included, for `hold` before opening the stream.
    pub fn stall(mut self, hold: Duration) -> Self {
        self.pending.stall = Some(hold);
        self
    }

    pub fn hold_until_conversation_request(mut self, conversation: usize, request: usize) -> Self {
        assert!(
            conversation >= 1 && request >= 1,
            "requests are counted from 1"
        );
        self.pending.hold_until = Some((conversation, request));
        self.pending.hold_reply_only = false;
        self
    }

    /// The tool-call answers return; only the reply waits.
    pub fn hold_reply_until_conversation_request(
        mut self,
        conversation: usize,
        request: usize,
    ) -> Self {
        assert!(
            conversation >= 1 && request >= 1,
            "requests are counted from 1"
        );
        self.pending.hold_until = Some((conversation, request));
        self.pending.hold_reply_only = true;
        self
    }

    pub fn hold(mut self) -> Self {
        self.pending.park = true;
        self
    }

    pub fn hold_for(mut self, token: u64) -> Self {
        self.pending.park = true;
        self.pending.park_token = Some(token);
        self
    }

    pub fn turn_index(mut self, index: usize) -> Self {
        self.pending.reply_index = Some(index);
        self
    }

    pub fn hold_until_request_log(mut self, log: impl Into<PathBuf>) -> Self {
        self.pending.hold_until_lsp_log = Some(log.into());
        self
    }

    pub fn reasoning(mut self, reasoning: impl Into<String>) -> Self {
        self.pending.reasoning = Some(reasoning.into());
        self
    }

    pub fn usage(mut self, usage: UsageReport) -> Self {
        self.pending.usage = Some(usage);
        self
    }

    pub fn refuse(mut self, failure: StatusFailure) -> Self {
        self.pending.failures.push(Failure::Status(failure));
        self
    }

    /// Answer the next `count` requests with [`crate::CUT_REPLY`] stopped at the output token limit.
    pub fn cut(mut self, count: usize) -> Self {
        self.pending.failures.push(Failure::Cut { count });
        self
    }

    pub fn content_filter(mut self, count: usize) -> Self {
        self.pending.failures.push(Failure::ContentFilter { count });
        self
    }

    pub fn fail_stream(mut self, stream_error: StreamError) -> Self {
        self.pending
            .failures
            .push(Failure::StreamError(stream_error));
        self
    }

    /// The stream error is the whole turn. Serving its count accounts the entry, so a client that
    /// does not retry still leaves the script finished.
    pub fn end_with_stream_error(mut self, stream_error: StreamError) -> Self {
        self.pending
            .failures
            .push(Failure::StreamError(stream_error));
        self.pending.failure_closes_turn = true;
        self.reply(String::new())
    }

    /// Answer the next `count` requests with this turn's first tool call again, each under a fresh
    /// call id the way a looping model issues them, or with [`crate::LOOPING_REPLY`] when the turn
    /// has no tool calls.
    pub fn doom_loop(mut self, count: usize) -> Self {
        self.pending.failures.push(Failure::DoomLoop { count });
        self
    }

    /// Close the connection with no body on the next `count` requests, before the response head.
    pub fn drop_connection(mut self, count: usize) -> Self {
        self.pending.failures.push(Failure::Dropped { count });
        self
    }

    pub fn malformed_body(mut self, count: usize) -> Self {
        self.pending.failures.push(Failure::MalformedBody { count });
        self
    }

    /// Open the stream on the next `count` requests then never send a chunk, so the client's
    /// inference idle timeout fires.
    pub fn hang(mut self, count: usize) -> Self {
        self.pending.failures.push(Failure::Hang { count });
        self
    }

    pub fn empty_reply(mut self, count: usize) -> Self {
        self.pending.failures.push(Failure::Empty { count });
        self
    }

    /// Stream part of a reply on the next `count` requests, then end the body.
    /// The body has no finish reason, no terminal event, and no `[DONE]`.
    pub fn truncated_reply(mut self, count: usize) -> Self {
        self.pending.failures.push(Failure::Truncated { count });
        self
    }

    pub fn reply(mut self, text: impl Into<String>) -> Self {
        let turn = std::mem::take(&mut self.pending).close(text.into());
        self.turns.push(turn);
        self
    }

    pub fn scripted_events(mut self, events: Vec<ModelEvent>) -> Self {
        let mut turn = std::mem::take(&mut self.pending).close(String::new());
        turn.events = Some(events);
        self.turns.push(turn);
        self
    }

    /// `served` marks that turn answered.
    pub fn compile(
        number: usize,
        turns: impl IntoIterator<Item = CompiledTurn>,
        mut served: impl FnMut(usize) -> bool,
    ) -> Self {
        let mut conversation = Conversation::nth(number);
        for (index, turn) in turns.into_iter().enumerate() {
            let answered = served(index);
            conversation = conversation.apply_turn(turn);
            conversation
                .turns
                .last_mut()
                .expect("a compiled turn ends in a reply")
                .answered = answered;
        }
        conversation
    }

    fn apply_turn(self, turn: CompiledTurn) -> Self {
        let mut conversation = self.calls(turn.calls);
        if turn.parallel {
            conversation = conversation.parallel_tool_calls();
        }
        for failure in turn.failures {
            conversation = match failure {
                CompiledFailure::Status(status) => conversation.refuse(status),
                CompiledFailure::Stream(error) => conversation.fail_stream(error),
                CompiledFailure::Dropped => conversation.drop_connection(1),
                CompiledFailure::Malformed => conversation.malformed_body(1),
                CompiledFailure::Hang => conversation.hang(1),
                CompiledFailure::CutOnce => conversation.cut(1),
                CompiledFailure::DoomLoop => conversation.doom_loop(1),
                CompiledFailure::Empty { count } => conversation.empty_reply(count),
                CompiledFailure::Truncated => conversation.truncated_reply(1),
                CompiledFailure::Cut { count } => conversation.cut(count),
                CompiledFailure::ContentFilter { count } => conversation.content_filter(count),
            };
        }
        if let Some(hold) = turn.stall {
            conversation = conversation.stall(hold);
        }
        if let Some((conversation_number, request)) = turn.hold_until {
            conversation = if turn.hold_reply_only {
                conversation.hold_reply_until_conversation_request(conversation_number, request)
            } else {
                conversation.hold_until_conversation_request(conversation_number, request)
            };
        }
        if let Some(token) = turn.park_token {
            conversation = conversation.hold_for(token);
        } else if turn.park {
            conversation = conversation.hold();
        }
        if let Some(log) = turn.hold_until_lsp_log {
            conversation = conversation.hold_until_request_log(log);
        }
        if let Some(reasoning) = turn.reasoning {
            conversation = conversation.reasoning(reasoning);
        }
        if let Some(usage) = turn.usage {
            conversation = conversation.usage(usage);
        }
        if let Some(nth) = turn.at_request {
            conversation = conversation.at_request(nth);
        }
        if let Some(text) = turn.body_contains {
            conversation = conversation.when_body_contains(text);
        }
        if let Some(text) = turn.release_when_body_contains {
            conversation = conversation.release_when_body_contains(text);
        }
        if let Some(index) = turn.reply_index {
            conversation = conversation.turn_index(index);
        }
        if let Some(error) = turn.closing_stream_error {
            conversation.end_with_stream_error(error)
        } else if let Some(events) = turn.events {
            conversation.scripted_events(events)
        } else {
            conversation.reply(turn.reply)
        }
    }

    pub(crate) fn into_script(mut self) -> (ScriptTarget, ScriptEntries) {
        assert!(
            !self.pending.is_open(),
            "{}: an open turn after the last reply; close it with .reply()",
            self.target
        );
        let Some(last) = self.turns.pop() else {
            panic!("{}: no turns; add a .reply()", self.target);
        };
        (
            self.target,
            ScriptEntries {
                earlier: self.turns,
                last,
            },
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptViolation {
    ResultMismatch {
        conversation: usize,
        call_id: String,
        expected: String,
        result: String,
    },
    ToolNotOffered {
        conversation: usize,
        call_id: String,
        tool: Tool,
        offered: Vec<String>,
    },
    Unfinished {
        conversation: usize,
        replies_served: usize,
        /// Turns a pin took over mid tool call; counted toward the entries accounted for.
        superseded: usize,
        entry_count: usize,
    },
    Unopened {
        expected: String,
    },
}

impl fmt::Display for ScriptViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScriptViolation::ResultMismatch {
                conversation,
                call_id,
                expected,
                result,
            } => write!(
                f,
                "conversation {conversation} call {call_id}: result did not meet {expected:?}; got {result:?}"
            ),
            ScriptViolation::ToolNotOffered {
                conversation,
                call_id,
                tool,
                offered,
            } => write!(
                f,
                "conversation {conversation} call {call_id}: the request offered no name {tool} is known by; offered {offered:?}"
            ),
            ScriptViolation::Unfinished {
                conversation,
                replies_served,
                superseded,
                entry_count,
            } => {
                write!(
                    f,
                    "conversation {conversation}: {replies_served} of {entry_count} entries served"
                )?;
                if *superseded > 0 {
                    write!(f, ", {superseded} superseded by a pin")?;
                }
                Ok(())
            }
            ScriptViolation::Unopened { expected } => {
                write!(f, "no conversation opened with {expected:?}")
            }
        }
    }
}
