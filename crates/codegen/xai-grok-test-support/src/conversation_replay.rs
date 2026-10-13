//! Plays each scripted [`Conversation`] forward one request at a time. Only a reply advances the
//! script, so a resend repeats it and a pin takes over and supersedes an in-progress turn; a failure
//! answers in place without advancing, so a retry meets the same position. A failure that closes the
//! turn is that reply: once its count is served, the entry is accounted. A departure is a [`ScriptViolation`].

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::btree_map::Entry;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;

use crate::conversation::ConversationId;
use crate::conversation_script::{
    CallDelivery, Conversation, MockToolCall, ScriptEntries, ScriptTarget, ScriptViolation,
    TextCheck, mock_call_id_prefix,
};
use crate::failure::{DOOM_LOOP_CHECK_HEADER, Failure, ObservedFailure};
use crate::inference_request::{
    BodyHash, InferenceRequest, OfferedTools, first_system_message, offered_tools, tool_results,
};
use crate::model_reply::ModelReply;
use crate::sse::UsageReport;
use crate::tools::PickedToolCall;

fn substitute_task_id(
    arguments: &serde_json::Value,
    results: &BTreeMap<String, String>,
) -> serde_json::Value {
    let raw = arguments.to_string();
    if !raw.contains("$TASK_ID") {
        return arguments.clone();
    }
    let Some(id) = results.values().find_map(|text| task_id_in(text)) else {
        return arguments.clone();
    };
    serde_json::from_str(&raw.replace("$TASK_ID", id)).unwrap_or_else(|_| arguments.clone())
}

fn task_id_in(text: &str) -> Option<&str> {
    let start = text.find("<task-id>")? + "<task-id>".len();
    let rest = text.get(start..)?;
    let end = rest.find("</task-id>")?;
    Some(&rest[..end])
}

/// Only the results for this conversation's calls count.
struct ScriptRequest {
    conversation: ConversationId,
    body_hash: BodyHash,
    /// Serialized body, so a turn pinned to a substring can recognize its request.
    body: String,
    first_system_message: Option<String>,
    results: BTreeMap<String, String>,
    offered: OfferedTools,
    asks_for_loop_report: bool,
}

impl ScriptRequest {
    fn new(conversation: ConversationId, request: &InferenceRequest<'_>) -> Self {
        let body = request.body();
        let prefix = mock_call_id_prefix(conversation);
        ScriptRequest {
            conversation,
            body_hash: request.body_hash(),
            body: serde_json::to_string(body).unwrap_or_default(),
            first_system_message: first_system_message(body),
            results: tool_results(body)
                .into_iter()
                .filter(|(call_id, _)| call_id.starts_with(&prefix))
                .collect(),
            offered: offered_tools(body),
            asks_for_loop_report: request.headers().contains_key(DOOM_LOOP_CHECK_HEADER),
        }
    }
}

/// A scripted conversation's answer to one request, recorded on its log entry.
pub(crate) struct ServedReply {
    pub(crate) reply: ModelReply,
    pub(crate) hold: Option<Duration>,
    /// Wait until this conversation has logged this many requests, counted from 1, before answering.
    pub(crate) hold_until: Option<(usize, usize)>,
    /// Park the answer until the mock releases parked replies.
    pub(crate) park: bool,
    /// Own release for this park. Absent parks wait on the shutdown latch.
    pub(crate) park_token: Option<u64>,
    /// Turn in the script that served this request.
    pub(crate) reply_index: Option<usize>,
    /// The mock LSP request log this answer waits on before it is sent.
    pub(crate) hold_until_lsp_log: Option<PathBuf>,
    pub(crate) observed: Option<ObservedFailure>,
    pub(crate) park_until: Option<String>,
}

struct ServedRequest {
    served: ServedReply,
    violations: Vec<ScriptViolation>,
}

/// Served calls keep their checks until a request carries their results.
/// A reply keeps the request that took it.
enum Progress {
    Unserved,
    Pinned {
        entry_index: usize,
    },
    Calling {
        entry_index: usize,
        awaited: Vec<AwaitedResult>,
        taken_by: BodyHash,
    },
    Failing {
        entry_index: usize,
    },
    Replied {
        entry_index: usize,
        taken_by: BodyHash,
        /// The unpinned entry after it; `None` when its reply is the one that repeats.
        next: Option<usize>,
    },
}

/// A served call whose result is still to arrive.
struct AwaitedResult {
    call_id: String,
    /// `None` once the check ran, or for a call without one.
    check: Option<TextCheck>,
}

impl From<&ServedCall> for AwaitedResult {
    fn from(call: &ServedCall) -> Self {
        AwaitedResult {
            call_id: call.call_id.clone(),
            check: call.pending_check.clone(),
        }
    }
}

struct FailureBudget {
    failure: Failure,
    remaining: usize,
}

impl FailureBudget {
    fn new(failure: &Failure) -> Self {
        FailureBudget {
            failure: failure.clone(),
            remaining: failure.count(),
        }
    }
}

/// One of an entry's failures chosen for a request; `repetition` counts from 1 and numbers a doom
/// loop's replay.
struct ChosenFailure {
    index: usize,
    failure: Failure,
    repetition: usize,
}

/// The failures of one entry in the order the case declared them; the first with budget left answers.
struct EntryFailures(Vec<FailureBudget>);

impl EntryFailures {
    fn next_failure(&self) -> Option<ChosenFailure> {
        let (index, budget) = self
            .0
            .iter()
            .enumerate()
            .find(|(_, budget)| budget.remaining > 0)?;
        Some(ChosenFailure {
            index,
            failure: budget.failure.clone(),
            repetition: budget.failure.count() - budget.remaining + 1,
        })
    }

    fn spend(&mut self, index: usize) {
        let Some(budget) = self.0.get_mut(index) else {
            unreachable!("failure index came from next_failure");
        };
        budget.remaining -= 1;
    }
}

/// A mock tool call picked from the tools the request offers.
struct ServedCall {
    call_id: String,
    pending_check: Option<TextCheck>,
    picked: PickedToolCall,
}

/// What a request gets, decided without touching `Progress`; `record` then applies it.
enum NextStep {
    RepeatReply {
        entry_index: usize,
        text: String,
        usage: Option<UsageReport>,
    },
    Failure {
        entry_index: usize,
        failure_index: usize,
        failure: Failure,
        /// The call a doom loop repeats, when the entry has one the request offers.
        looping_call: Option<(String, PickedToolCall)>,
        violation: Option<ScriptViolation>,
    },
    Call {
        entry_index: usize,
        call: ServedCall,
    },
    /// Every unanswered call of a parallel turn, in order.
    ParallelCalls {
        entry_index: usize,
        calls: Vec<ServedCall>,
    },
    Reply {
        entry_index: usize,
        text: String,
        usage: Option<UsageReport>,
    },
    NotOffered {
        entry_index: usize,
        violation: ScriptViolation,
        reply: String,
    },
}

impl NextStep {
    fn entry_index(&self) -> usize {
        match self {
            NextStep::RepeatReply { entry_index, .. }
            | NextStep::Failure { entry_index, .. }
            | NextStep::Call { entry_index, .. }
            | NextStep::ParallelCalls { entry_index, .. }
            | NextStep::Reply { entry_index, .. }
            | NextStep::NotOffered { entry_index, .. } => *entry_index,
        }
    }
}

/// The reply a decided step gives, the failure the log records, and any violation the step carries;
/// `held` is the failure a stalled answer records.
fn render(
    step: NextStep,
    request: &ScriptRequest,
    held: Option<ObservedFailure>,
    reasoning: Option<String>,
) -> (ModelReply, Option<ObservedFailure>, Vec<ScriptViolation>) {
    match step {
        NextStep::RepeatReply { text, usage, .. } | NextStep::Reply { text, usage, .. } => {
            let reply = match reasoning {
                Some(reasoning) => ModelReply::ReasoningReply {
                    reasoning,
                    text,
                    usage,
                },
                None => ModelReply::Text { text, usage },
            };
            (reply, held, Vec::new())
        }
        NextStep::Failure {
            failure,
            looping_call,
            violation,
            ..
        } => {
            let observed = failure.observed();
            let reply = match failure {
                Failure::Status(status) => ModelReply::Refusal(status),
                Failure::StreamError(stream_error) => ModelReply::StreamError(stream_error),
                Failure::Cut { .. } => ModelReply::CutReply,
                Failure::ContentFilter { .. } => ModelReply::ContentFilter,
                Failure::Dropped { .. } => ModelReply::Dropped,
                Failure::MalformedBody { .. } => ModelReply::Malformed,
                Failure::Hang { .. } => ModelReply::Hang,
                Failure::Empty { .. } => ModelReply::Empty,
                Failure::Truncated { .. } => ModelReply::Truncated,
                Failure::DoomLoop { .. } => match looping_call {
                    Some((call_id, call)) => ModelReply::ToolCall { call_id, call },
                    None => ModelReply::LoopingReply {
                        reported: request.asks_for_loop_report,
                    },
                },
            };
            (reply, Some(observed), violation.into_iter().collect())
        }
        NextStep::Call { call, .. } => (
            ModelReply::ToolCall {
                call_id: call.call_id,
                call: call.picked,
            },
            held,
            Vec::new(),
        ),
        NextStep::ParallelCalls { calls, .. } => (
            ModelReply::ToolCalls(
                calls
                    .into_iter()
                    .map(|call| (call.call_id, call.picked))
                    .collect(),
            ),
            held,
            Vec::new(),
        ),
        NextStep::NotOffered {
            violation, reply, ..
        } => (
            ModelReply::Text {
                text: reply,
                usage: None,
            },
            held,
            vec![violation],
        ),
    }
}

struct ReplayingScript {
    entries: ScriptEntries,
    /// Requests that reached the script, the count a pinned entry names.
    requests: usize,
    /// Entries whose reply was served or whose turn a pin took over; each counts once, so a
    /// resumed turn a pin already superseded cannot hide a later turn that never answered.
    accounted: BTreeSet<usize>,
    /// The subset of `accounted` a pin took over mid tool call; the pinned turn stands in for
    /// their reply.
    superseded: BTreeSet<usize>,
    progress: Progress,
    failures: Vec<EntryFailures>,
}

impl ReplayingScript {
    fn new(entries: ScriptEntries) -> Self {
        let accounted: BTreeSet<usize> = entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.answered)
            .map(|(index, _)| index)
            .collect();
        let failures = entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                EntryFailures(
                    entry
                        .failures
                        .iter()
                        .map(|failure| {
                            let mut budget = FailureBudget::new(failure);
                            if accounted.contains(&index) {
                                budget.remaining = 0;
                            }
                            budget
                        })
                        .collect(),
                )
            })
            .collect();
        let progress = if accounted.len() == entries.len() {
            Progress::Replied {
                entry_index: entries.len() - 1,
                taken_by: BodyHash::zero(),
                next: None,
            }
        } else {
            Progress::Unserved
        };
        ReplayingScript {
            entries,
            requests: 0,
            accounted,
            superseded: BTreeSet::new(),
            progress,
            failures,
        }
    }

    /// `None` while every entry is pinned and none has been reached.
    fn serve(&mut self, request: &ScriptRequest) -> Option<ServedRequest> {
        let mut violations = self.run_pending_checks(request);
        if !self.repeats_last_reply(request) {
            self.admit(&request.body);
        }
        let step = self.next_step(request)?;
        self.record(&step, request);
        let entry = self.entries.at(step.entry_index());
        let hold = entry.stall;
        let hold_until = entry.hold_until.filter(|_| {
            !entry.hold_reply_only
                || matches!(step, NextStep::Reply { .. } | NextStep::RepeatReply { .. })
        });
        // A tool call has to be delivered so the job can start. The hold is the answer the pager waits on.
        let park =
            entry.park && !matches!(step, NextStep::Call { .. } | NextStep::ParallelCalls { .. });
        let hold_until_lsp_log = (request.results.len() == 1)
            .then(|| entry.hold_until_lsp_log.clone())
            .flatten();
        let reasoning = entry.reasoning.clone();
        let events = entry.events.clone();
        let held = hold.map(|_| ObservedFailure::Stalled);
        let text_step = matches!(step, NextStep::Reply { .. } | NextStep::RepeatReply { .. });
        let (mut reply, observed, rendered) = render(step, request, held, reasoning);
        if text_step && let Some(events) = events {
            reply = ModelReply::Events(events);
        }
        violations.extend(rendered);
        Some(ServedRequest {
            served: ServedReply {
                reply,
                hold,
                hold_until,
                park,
                park_token: entry.park_token,
                reply_index: entry.reply_index,
                hold_until_lsp_log,
                observed,
                park_until: entry.release_when_body_contains.clone(),
            },
            violations,
        })
    }

    fn run_pending_checks(&mut self, request: &ScriptRequest) -> Vec<ScriptViolation> {
        let Progress::Calling { awaited, .. } = &mut self.progress else {
            return Vec::new();
        };
        awaited
            .iter_mut()
            .filter_map(|awaited| {
                let result = request.results.get(awaited.call_id.as_str())?;
                let check = awaited.check.take()?;
                if check.is_met_by(result) {
                    return None;
                }
                Some(ScriptViolation::ResultMismatch {
                    conversation: request.conversation.number(),
                    call_id: awaited.call_id.clone(),
                    expected: check.label().to_owned(),
                    result: result.clone(),
                })
            })
            .collect()
    }

    /// A resend of the request that already took the last answer, a reply or a tool call, which
    /// repeats it without counting as a new request or firing a pin.
    fn repeats_last_reply(&self, request: &ScriptRequest) -> bool {
        matches!(
            &self.progress,
            Progress::Replied { taken_by, .. } | Progress::Calling { taken_by, .. }
                if *taken_by == request.body_hash
        )
    }

    /// Count the request; an entry pinned to it takes over, whatever was serving. A turn interrupted
    /// mid call or mid failure is superseded, so the drop check does not require the reply it never reached.
    fn unaccounted_body_match(&self, body: &str) -> Option<usize> {
        self.entries.iter().enumerate().find_map(|(index, entry)| {
            let needle = entry.body_contains.as_ref()?;
            (!self.accounted.contains(&index) && body.contains(needle)).then_some(index)
        })
    }

    fn admit(&mut self, body: &str) {
        self.requests += 1;
        let Some(pinned) = self
            .entries
            .pinned_to(self.requests)
            .filter(|index| !self.accounted.contains(index))
            .or_else(|| self.unaccounted_body_match(body))
        else {
            return;
        };
        match &self.progress {
            Progress::Calling { entry_index, .. }
            | Progress::Failing { entry_index }
            | Progress::Pinned { entry_index } => {
                let taken_over = *entry_index;
                self.accounted.insert(taken_over);
                self.superseded.insert(taken_over);
            }
            Progress::Unserved | Progress::Replied { .. } => {}
        }
        self.progress = Progress::Pinned {
            entry_index: pinned,
        };
    }

    /// The next ordinary turn. A reply that already happened is skipped. A turn a pin superseded
    /// stays, so its reply is still delivered.
    fn next_open(&self, after: Option<usize>) -> Option<usize> {
        self.entries
            .iter()
            .enumerate()
            .skip(after.map_or(0, |after| after + 1))
            .find(|(index, entry)| {
                let reply_done = self.accounted.contains(index) && !self.superseded.contains(index);
                !reply_done && entry.at_request.is_none() && entry.body_contains.is_none()
            })
            .map(|(index, _)| index)
    }

    fn entry_to_serve(&self) -> Option<usize> {
        match &self.progress {
            Progress::Unserved => self.next_open(None),
            Progress::Pinned { entry_index }
            | Progress::Calling { entry_index, .. }
            | Progress::Failing { entry_index } => Some(*entry_index),
            Progress::Replied {
                entry_index, next, ..
            } => Some(next.unwrap_or(*entry_index)),
        }
    }

    fn next_step(&self, request: &ScriptRequest) -> Option<NextStep> {
        if let Progress::Replied {
            entry_index,
            taken_by,
            ..
        } = &self.progress
            && *taken_by == request.body_hash
        {
            return Some(NextStep::RepeatReply {
                entry_index: *entry_index,
                text: self.entries.at(*entry_index).reply.clone(),
                usage: self.entries.at(*entry_index).usage,
            });
        }
        let index = self.entry_to_serve()?;
        let entry = self.entries.at(index);
        if let Some(chosen) = self
            .failures
            .get(index)
            .and_then(EntryFailures::next_failure)
        {
            return Some(self.failure_step(index, chosen, request));
        }
        if let Progress::Replied { next: None, .. } = &self.progress {
            return Some(NextStep::Reply {
                entry_index: index,
                text: entry.reply.clone(),
                usage: entry.usage,
            });
        }
        let reply = NextStep::Reply {
            entry_index: index,
            text: entry.reply.clone(),
            usage: entry.usage,
        };
        let serve = |(call_id, call): (String, &MockToolCall)| {
            let arguments = substitute_task_id(&call.arguments, &request.results);
            match call.tool.pick(&request.offered, &arguments) {
                Some(picked) => Ok(ServedCall {
                    call_id,
                    pending_check: call.result_check.clone(),
                    picked,
                }),
                None => Err(ScriptViolation::ToolNotOffered {
                    conversation: request.conversation.number(),
                    call_id,
                    tool: call.tool.clone(),
                    offered: request.offered.names(),
                }),
            }
        };
        let mut unanswered = self
            .entries
            .calls(index, request.conversation)
            .filter(|(call_id, _)| !request.results.contains_key(call_id));
        let step = match entry.delivery {
            CallDelivery::Sequential => match unanswered.next() {
                None => Ok(reply),
                Some(next) => serve(next).map(|call| NextStep::Call {
                    entry_index: index,
                    call,
                }),
            },
            CallDelivery::Parallel => {
                unanswered
                    .map(serve)
                    .collect::<Result<Vec<_>, _>>()
                    .map(|calls| {
                        if calls.is_empty() {
                            reply
                        } else {
                            NextStep::ParallelCalls {
                                entry_index: index,
                                calls,
                            }
                        }
                    })
            }
        };
        Some(step.unwrap_or_else(|violation| NextStep::NotOffered {
            entry_index: index,
            violation,
            reply: entry.reply.clone(),
        }))
    }

    /// Decides which failure answers and, for a doom loop, the call to repeat under a fresh call id
    /// per repetition; `serve` renders the reply. The looping reply stands in when the entry has no
    /// call or the request does not offer that tool.
    fn failure_step(
        &self,
        entry_index: usize,
        chosen: ChosenFailure,
        request: &ScriptRequest,
    ) -> NextStep {
        let ChosenFailure {
            index: failure_index,
            failure,
            repetition,
        } = chosen;
        let (looping_call, violation) = match &failure {
            Failure::Status(_)
            | Failure::StreamError(_)
            | Failure::Cut { .. }
            | Failure::ContentFilter { .. }
            | Failure::Dropped { .. }
            | Failure::MalformedBody { .. }
            | Failure::Hang { .. }
            | Failure::Empty { .. }
            | Failure::Truncated { .. } => (None, None),
            Failure::DoomLoop { .. } => {
                match self.entries.calls(entry_index, request.conversation).next() {
                    None => (None, None),
                    Some((id, call)) => {
                        let call_id = format!("{id}_loop{repetition}");
                        match call.tool.pick(&request.offered, &call.arguments) {
                            Some(picked) => (Some((call_id, picked)), None),
                            None => (
                                None,
                                Some(ScriptViolation::ToolNotOffered {
                                    conversation: request.conversation.number(),
                                    call_id,
                                    tool: call.tool.clone(),
                                    offered: request.offered.names(),
                                }),
                            ),
                        }
                    }
                }
            }
        };
        NextStep::Failure {
            entry_index,
            failure_index,
            failure,
            looping_call,
            violation,
        }
    }

    fn record(&mut self, step: &NextStep, request: &ScriptRequest) {
        match step {
            NextStep::RepeatReply { .. } => {}
            NextStep::Failure {
                entry_index,
                failure_index,
                ..
            } => {
                let entry_index = *entry_index;
                let failure_index = *failure_index;
                let spent = {
                    let Some(budget) = self.failures.get_mut(entry_index) else {
                        unreachable!("failure entry index came from next_step");
                    };
                    budget.spend(failure_index);
                    budget.next_failure().is_none()
                };
                if spent && self.entries.at(entry_index).failure_closes_turn {
                    self.accounted.insert(entry_index);
                    self.progress = Progress::Replied {
                        entry_index,
                        taken_by: request.body_hash,
                        next: self.next_open(Some(entry_index)),
                    };
                } else {
                    self.progress = Progress::Failing { entry_index };
                }
            }
            NextStep::Call { entry_index, call } => {
                self.progress = Progress::Calling {
                    entry_index: *entry_index,
                    awaited: vec![AwaitedResult::from(call)],
                    taken_by: request.body_hash,
                };
            }
            NextStep::ParallelCalls { entry_index, calls } => {
                self.progress = Progress::Calling {
                    entry_index: *entry_index,
                    awaited: calls.iter().map(AwaitedResult::from).collect(),
                    taken_by: request.body_hash,
                };
            }
            NextStep::Reply { entry_index, .. } | NextStep::NotOffered { entry_index, .. } => {
                self.accounted.insert(*entry_index);
                self.progress = Progress::Replied {
                    entry_index: *entry_index,
                    taken_by: request.body_hash,
                    next: self.next_open(Some(*entry_index)),
                };
            }
        }
    }

    fn replies_served(&self) -> usize {
        self.accounted.len()
    }

    fn unfinished(&self, conversation: ConversationId) -> Option<ScriptViolation> {
        (self.accounted.len() < self.entries.len()).then_some(ScriptViolation::Unfinished {
            conversation: conversation.number(),
            replies_served: self.accounted.len() - self.superseded.len(),
            superseded: self.superseded.len(),
            entry_count: self.entries.len(),
        })
    }
}

struct WaitingScript {
    opening: TextCheck,
    script: ReplayingScript,
}

impl WaitingScript {
    fn is_claimed_by(&self, request: &ScriptRequest) -> bool {
        request
            .first_system_message
            .as_deref()
            .is_some_and(|text| self.opening.is_met_by(text))
    }
}

#[derive(Default)]
struct ReplayState {
    scripts: BTreeMap<ConversationId, ReplayingScript>,
    waiting: Vec<WaitingScript>,
    recorded: Vec<ScriptViolation>,
}

impl ReplayState {
    fn claim_script(&mut self, request: &ScriptRequest) -> Option<&mut ReplayingScript> {
        match self.scripts.entry(request.conversation) {
            Entry::Occupied(entry) => Some(entry.into_mut()),
            Entry::Vacant(entry) => {
                let position = self
                    .waiting
                    .iter()
                    .position(|waiting| waiting.is_claimed_by(request))?;
                Some(entry.insert(self.waiting.remove(position).script))
            }
        }
    }

    fn violations(&self) -> Vec<ScriptViolation> {
        self.recorded
            .iter()
            .cloned()
            .chain(
                self.scripts
                    .iter()
                    .filter_map(|(conversation, script)| script.unfinished(*conversation)),
            )
            .chain(
                self.waiting
                    .iter()
                    .map(|waiting| ScriptViolation::Unopened {
                        expected: waiting.opening.label().to_owned(),
                    }),
            )
            .collect()
    }
}

#[derive(Clone)]
pub(crate) struct ConversationReplay {
    state: Arc<std::sync::Mutex<ReplayState>>,
    released_bodies: Arc<watch::Sender<Vec<String>>>,
}

impl Default for ConversationReplay {
    fn default() -> Self {
        let (tx, _rx) = watch::channel(Vec::new());
        Self {
            state: Arc::new(std::sync::Mutex::new(ReplayState::default())),
            released_bodies: Arc::new(tx),
        }
    }
}

impl ConversationReplay {
    /// Two scripts for one conversation, or two turns of a script pinned to one request, panic here
    /// rather than at serve time.
    pub(crate) fn set(&self, conversations: impl IntoIterator<Item = Conversation>) {
        let mut named = BTreeMap::new();
        let mut waiting = Vec::new();
        for conversation in conversations {
            let (target, entries) = conversation.into_script();
            if let Some(request) = entries.request_pinned_twice() {
                panic!("two turns of the script for {target} are pinned to request {request}");
            }
            let replaying = ReplayingScript::new(entries);
            match target {
                ScriptTarget::Conversation(conversation) => {
                    if named.insert(conversation, replaying).is_some() {
                        panic!("duplicate script for {conversation}");
                    }
                }
                ScriptTarget::Opening(opening) => waiting.push(WaitingScript {
                    opening,
                    script: replaying,
                }),
            }
        }
        let mut state = self.state.lock().unwrap();
        let old = std::mem::take(&mut *state);
        let mut recorded = old.recorded;
        recorded.extend(
            old.scripts
                .iter()
                .filter(|(conversation, _)| !named.contains_key(conversation))
                .filter_map(|(conversation, script)| script.unfinished(*conversation)),
        );
        recorded.extend(old.waiting.iter().map(|waiting| ScriptViolation::Unopened {
            expected: waiting.opening.label().to_owned(),
        }));
        *state = ReplayState {
            scripts: named,
            waiting,
            recorded,
        };
    }

    /// Replies conversation `conversation` has served from the script currently installed.
    /// A catalog GET or another conversation's post does not count.
    #[must_use]
    pub(crate) fn replies_served(&self, conversation: usize) -> usize {
        self.state
            .lock()
            .unwrap()
            .scripts
            .get(&ConversationId::nth(conversation))
            .map(ReplayingScript::replies_served)
            .unwrap_or(0)
    }

    /// Turns whose reply this conversation has already accounted, by the index the script stored.
    pub(crate) fn answered_turns(&self, conversation: usize) -> BTreeSet<usize> {
        let state = self.state.lock().unwrap();
        let Some(script) = state.scripts.get(&ConversationId::nth(conversation)) else {
            return BTreeSet::new();
        };
        script
            .accounted
            .iter()
            .filter_map(|index| script.entries.at(*index).reply_index)
            .collect()
    }

    #[must_use]
    pub(crate) fn violations(&self) -> Vec<ScriptViolation> {
        self.state.lock().unwrap().violations()
    }

    #[must_use = "assert on the violations; finishing disarms the drop check"]
    pub(crate) fn finish(&self) -> Vec<ScriptViolation> {
        std::mem::take(&mut *self.state.lock().unwrap()).violations()
    }

    /// A body-pinned turn also answers an auxiliary request, which has no conversation number.
    pub(crate) fn respond(&self, request: &InferenceRequest<'_>) -> Option<ServedReply> {
        let body = serde_json::to_string(request.body()).unwrap_or_default();
        if request.conversation().is_none() {
            return self.serve_auxiliary_body(&body);
        }
        let conversation = request.conversation()?;
        let script_request = ScriptRequest::new(conversation, request);
        let mut state = self.state.lock().unwrap();
        let served = state
            .claim_script(&script_request)?
            .serve(&script_request)?;
        state.recorded.extend(served.violations);
        Some(served.served)
    }

    /// A compaction summary has no conversation number, so it must not take the child's next reply.
    fn serve_auxiliary_body(&self, body: &str) -> Option<ServedReply> {
        let mut state = self.state.lock().unwrap();
        let served = state.scripts.values_mut().find_map(|script| {
            let index = script.unaccounted_body_match(body)?;
            let entry = script.entries.at(index);
            let text = entry.reply.clone();
            let events = entry.events.clone();
            let usage = entry.usage;
            let hold = entry.stall;
            script.accounted.insert(index);
            let park_until = entry.release_when_body_contains.clone();
            let reply = match events {
                Some(events) => ModelReply::Events(events),
                None => ModelReply::Text { text, usage },
            };
            Some(ServedReply {
                reply,
                hold,
                hold_until: None,
                park: entry.park,
                park_token: entry.park_token,
                reply_index: entry.reply_index,
                hold_until_lsp_log: None,
                observed: hold.map(|_| ObservedFailure::Stalled),
                park_until,
            })
        })?;
        Some(served)
    }

    /// A park that starts later still sees a body recorded now.
    pub(crate) fn release_parks(&self, body: &str) {
        let body = body.to_owned();
        self.released_bodies.send_modify(|bodies| {
            if !bodies.iter().any(|seen| seen == &body) {
                bodies.push(body.clone());
            }
        });
    }

    pub(crate) async fn park_until(&self, needle: String) {
        let mut bodies = self.released_bodies.subscribe();
        if bodies
            .borrow()
            .iter()
            .any(|body| body.contains(needle.as_str()))
        {
            return;
        }
        let _ = bodies
            .wait_for(|bodies| bodies.iter().any(|body| body.contains(needle.as_str())))
            .await;
    }
}

#[cfg(test)]
mod park_tests {
    use super::ConversationReplay;

    #[tokio::test]
    async fn release_before_park_sticks() {
        let replay = ConversationReplay::default();
        replay.release_parks("please SKILL-TOKEN now");
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            replay.park_until("SKILL-TOKEN".to_owned()),
        )
        .await
        .expect("a release before park must already be visible");
    }
}

#[cfg(test)]
#[path = "conversation_replay_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "conversation_replay_failure_tests.rs"]
mod failure_tests;
