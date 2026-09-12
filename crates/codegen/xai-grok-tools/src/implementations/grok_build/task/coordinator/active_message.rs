//! Active-child message admission and finalization linearization.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tokio::sync::{OwnedSemaphorePermit, oneshot};
use tokio_util::sync::WaitForCancellationFutureOwned;

use super::SubagentCoordinator;
use super::wake::ResolvedSend;
use crate::implementations::grok_build::task::active_message::{
    ActiveMessageAdmissionLease, ActiveMessageIngress,
};
use crate::implementations::grok_build::task::coordinator_state::{
    ACTIVE_MESSAGE_ADMISSION_TIMEOUT, ACTIVE_MESSAGE_SPAWN_READY_TIMEOUT, ActiveMessageAdmission,
    ChildControl, ChildRunner, MAX_ACTIVE_MESSAGE_ADMISSIONS_PER_CHILD,
};
use crate::implementations::grok_build::task::types::{
    ActiveAgentMessage, ActiveAgentMessageDelivery, ActiveAgentMessageOutcome,
    ActiveAgentMessageRequest, ActiveAgentMessageSource, ActiveMessageTarget,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ActiveChildGeneration(uuid::Uuid);

impl ActiveChildGeneration {
    pub(super) fn new() -> Self {
        Self(uuid::Uuid::now_v7())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BeginAdmission {
    Started,
    Finalizing,
    Saturated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::implementations::grok_build::task) enum TerminalDrainDisposition {
    Clean,
    Uncertain,
}

impl TerminalDrainDisposition {
    fn record(&mut self, is_settled: bool) {
        if !is_settled {
            *self = Self::Uncertain;
        }
    }

    fn is_clean(self) -> bool {
        self == Self::Clean
    }
}

pub(in crate::implementations::grok_build::task) enum ActiveMessageLifecycle {
    Open {
        in_flight: usize,
        disposition: TerminalDrainDisposition,
    },
    Finalizing {
        in_flight: usize,
        disposition: TerminalDrainDisposition,
        waiters: Vec<oneshot::Sender<bool>>,
    },
}

impl Default for ActiveMessageLifecycle {
    fn default() -> Self {
        Self::Open {
            in_flight: 0,
            disposition: TerminalDrainDisposition::Clean,
        }
    }
}

impl ActiveMessageLifecycle {
    fn begin_admission(&mut self) -> BeginAdmission {
        let Self::Open { in_flight, .. } = self else {
            return BeginAdmission::Finalizing;
        };
        if *in_flight >= MAX_ACTIVE_MESSAGE_ADMISSIONS_PER_CHILD {
            return BeginAdmission::Saturated;
        }
        *in_flight += 1;
        BeginAdmission::Started
    }

    pub(super) fn begin_finalizing(&mut self, respond_to: oneshot::Sender<bool>) {
        if let Some(is_clean) = self.start_terminalizing() {
            let _ = respond_to.send(is_clean);
        } else if let Self::Finalizing { waiters, .. } = self {
            waiters.push(respond_to);
        }
    }

    pub(super) fn start_terminalizing(&mut self) -> Option<bool> {
        match self {
            Self::Open {
                in_flight,
                disposition,
            } => {
                let in_flight = *in_flight;
                let disposition = *disposition;
                *self = Self::Finalizing {
                    in_flight,
                    disposition,
                    waiters: Vec::new(),
                };
                (in_flight == 0).then(|| disposition.is_clean())
            }
            Self::Finalizing {
                in_flight,
                disposition,
                ..
            } if *in_flight == 0 => Some(disposition.is_clean()),
            Self::Finalizing { .. } => None,
        }
    }

    fn finish_admission(&mut self, is_settled: bool) -> Option<bool> {
        let (in_flight, disposition, waiters) = match self {
            Self::Open {
                in_flight,
                disposition,
            } => (in_flight, disposition, None),
            Self::Finalizing {
                in_flight,
                disposition,
                waiters,
            } => (in_flight, disposition, Some(waiters)),
        };
        disposition.record(is_settled);
        *in_flight = in_flight
            .checked_sub(1)
            .unwrap_or_else(|| unreachable!("active-message completion without admission"));
        if *in_flight == 0
            && let Some(waiters) = waiters
        {
            let is_clean = disposition.is_clean();
            resolve_waiters(waiters, is_clean);
            return Some(is_clean);
        }
        None
    }
}

impl Drop for ActiveMessageLifecycle {
    fn drop(&mut self) {
        if let Self::Finalizing { waiters, .. } = self {
            resolve_waiters(waiters, false);
        }
    }
}
fn resolve_waiters(waiters: &mut Vec<oneshot::Sender<bool>>, outcome: bool) {
    waiters.drain(..).for_each(|waiter| {
        let _ = waiter.send(outcome);
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActiveMessageCompletionOutcome {
    Admission(ActiveMessageAdmission),
    Cancelled,
    DeadlineElapsed,
}

pub(super) struct ActiveMessageCompletion {
    subagent_id: String,
    generation: ActiveChildGeneration,
    message_id: String,
    respond_to: Option<oneshot::Sender<ActiveAgentMessageOutcome>>,
    outcome: ActiveMessageCompletionOutcome,
    is_settled: bool,
    _ingress_permit: OwnedSemaphorePermit,
}

fn protocol_outcome_from_completion(
    outcome: ActiveMessageCompletionOutcome,
    is_settled: bool,
    message_id: &str,
) -> ActiveAgentMessageOutcome {
    if !is_settled {
        return ActiveAgentMessageOutcome::AdmissionUncertain;
    }
    match outcome {
        ActiveMessageCompletionOutcome::Admission(ActiveMessageAdmission::Admitted) => {
            ActiveAgentMessageOutcome::Accepted {
                message_id: message_id.to_owned(),
            }
        }
        ActiveMessageCompletionOutcome::Admission(ActiveMessageAdmission::Unsupported) => {
            ActiveAgentMessageOutcome::Unsupported
        }
        ActiveMessageCompletionOutcome::Admission(ActiveMessageAdmission::ChannelClosed) => {
            ActiveAgentMessageOutcome::ChannelClosed
        }
        ActiveMessageCompletionOutcome::Admission(ActiveMessageAdmission::Rejected) => {
            ActiveAgentMessageOutcome::NotActiveOrFinalizing
        }
        ActiveMessageCompletionOutcome::Cancelled
        | ActiveMessageCompletionOutcome::DeadlineElapsed => {
            ActiveAgentMessageOutcome::NotAcceptedBeforeDeadline
        }
    }
}

/// Reply when the completion is dropped after poll: a committed/claimed
/// admission cannot become a definite rejection.
fn lost_completion_outcome(
    outcome: ActiveMessageCompletionOutcome,
    is_settled: bool,
) -> ActiveAgentMessageOutcome {
    match (outcome, is_settled) {
        (ActiveMessageCompletionOutcome::Admission(ActiveMessageAdmission::Admitted), _)
        | (_, false) => ActiveAgentMessageOutcome::AdmissionUncertain,
        // Admitted is classified above; the dummy id is never used for Accepted.
        (outcome, true) => protocol_outcome_from_completion(outcome, true, ""),
    }
}

impl Drop for ActiveMessageCompletion {
    fn drop(&mut self) {
        if let Some(respond_to) = self.respond_to.take() {
            let _ = respond_to.send(lost_completion_outcome(self.outcome, self.is_settled));
        }
    }
}

pub(super) struct ActiveMessageFuture {
    subagent_id: String,
    generation: ActiveChildGeneration,
    message_id: String,
    future: Pin<Box<dyn Future<Output = ActiveMessageAdmission> + Send + 'static>>,
    cancellation: Pin<Box<WaitForCancellationFutureOwned>>,
    deadline: Pin<Box<tokio::time::Sleep>>,
    lease: Arc<ActiveMessageAdmissionLease>,
    ingress_permit: Option<OwnedSemaphorePermit>,
    respond_to: Option<oneshot::Sender<ActiveAgentMessageOutcome>>,
}

impl Future for ActiveMessageFuture {
    type Output = ActiveMessageCompletion;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let outcome = match this.future.as_mut().poll(cx) {
            Poll::Ready(admission) => ActiveMessageCompletionOutcome::Admission(admission),
            Poll::Pending if this.cancellation.as_mut().poll(cx).is_ready() => {
                ActiveMessageCompletionOutcome::Cancelled
            }
            Poll::Pending if this.deadline.as_mut().poll(cx).is_ready() => {
                ActiveMessageCompletionOutcome::DeadlineElapsed
            }
            Poll::Pending => return Poll::Pending,
        };
        let is_settled = match outcome {
            ActiveMessageCompletionOutcome::Admission(admission) => this.lease.settle(admission),
            ActiveMessageCompletionOutcome::Cancelled
            | ActiveMessageCompletionOutcome::DeadlineElapsed => this.lease.revoke(),
        };
        Poll::Ready(ActiveMessageCompletion {
            subagent_id: this.subagent_id.clone(),
            generation: this.generation,
            message_id: this.message_id.clone(),
            respond_to: Some(
                this.respond_to
                    .take()
                    .unwrap_or_else(|| unreachable!("active-message future polled twice")),
            ),
            outcome,
            is_settled,
            _ingress_permit: this
                .ingress_permit
                .take()
                .unwrap_or_else(|| unreachable!("active-message ingress permit taken twice")),
        })
    }
}

/// Agent cancel/workflow stays retryable. Human hard-rejects those.
fn refused_active_outcome(source: ActiveAgentMessageSource) -> ActiveAgentMessageOutcome {
    match source {
        ActiveAgentMessageSource::Human => ActiveAgentMessageOutcome::NotFoundOrNotOwned,
        ActiveAgentMessageSource::Agent => ActiveAgentMessageOutcome::NotActiveOrFinalizing,
    }
}

impl Drop for ActiveMessageFuture {
    fn drop(&mut self) {
        let outcome = if self.lease.revoke() {
            ActiveAgentMessageOutcome::ChannelClosed
        } else {
            ActiveAgentMessageOutcome::AdmissionUncertain
        };
        if let Some(respond_to) = self.respond_to.take() {
            let _ = respond_to.send(outcome);
        }
    }
}

struct SpawningChild {
    workflow: bool,
    cancelled: bool,
}

pub(super) struct ParkedSpawnReadyMessage {
    pub(super) subagent_id: String,
    pub(super) parent_session_id: String,
    pub(super) request: ActiveAgentMessageRequest,
    pub(super) respond_to: Option<oneshot::Sender<ActiveAgentMessageOutcome>>,
    pub(super) deadline: Option<tokio::time::Instant>,
    pub(super) initial_message_id: Option<String>,
    /// Held for the park window so the ingress cap cannot be recycled.
    pub(super) permit: Option<OwnedSemaphorePermit>,
}

impl Drop for ParkedSpawnReadyMessage {
    fn drop(&mut self) {
        if let Some(respond_to) = self.respond_to.take() {
            let _ = respond_to.send(ActiveAgentMessageOutcome::ChannelClosed);
        }
    }
}

/// Parked sends plus the admission semaphore they re-acquire on start.
pub(super) struct SpawnReadyMessages {
    parked: Vec<ParkedSpawnReadyMessage>,
    permits: Option<Arc<tokio::sync::Semaphore>>,
    capacity: usize,
}

impl SpawnReadyMessages {
    pub(super) fn new(permits: Option<Arc<tokio::sync::Semaphore>>, capacity: usize) -> Self {
        Self {
            parked: Vec::new(),
            permits,
            capacity,
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.parked.is_empty()
    }

    pub(super) fn clear(&mut self) {
        self.parked.clear();
    }

    pub(super) fn deadlines(&self) -> impl Iterator<Item = tokio::time::Instant> + '_ {
        self.parked.iter().filter_map(|parked| parked.deadline)
    }

    pub(super) fn push(&mut self, parked: ParkedSpawnReadyMessage) {
        self.parked.push(parked);
    }

    pub(super) fn take(&mut self, subagent_id: &str) -> Vec<ParkedSpawnReadyMessage> {
        let (matched, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.parked)
            .into_iter()
            .partition(|parked| parked.subagent_id == subagent_id);
        self.parked = rest;
        matched
    }

    /// Move parked sends onto the active path using the permit they already hold.
    pub(super) fn admit(&mut self, subagent_id: &str) -> Vec<ActiveMessageIngress> {
        let mut admitted = Vec::new();
        for mut parked in self.take(subagent_id) {
            if parked.acknowledge_initial() {
                continue;
            }
            let Some((request, parent_session_id, respond_to, permit)) = parked.into_request()
            else {
                continue;
            };
            let Some(permit) = permit.or_else(|| self.try_acquire().ok()) else {
                let _ = respond_to.send(ActiveAgentMessageOutcome::Saturated {
                    max_in_flight: self.capacity,
                });
                continue;
            };
            admitted.push(ActiveMessageIngress {
                request:
                    crate::implementations::grok_build::task::types::SubagentActiveMessageRequest {
                        request,
                        parent_session_id,
                        respond_to,
                    },
                permit,
            });
        }
        admitted
    }

    /// Spawn will never become active. Human addresses are terminal; agent
    /// sends stay retryable `not_active`.
    pub(super) fn reject_terminal(&mut self, subagent_id: &str) {
        for parked in self.take(subagent_id) {
            let outcome = if parked.request.source()
                == crate::implementations::grok_build::task::types::ActiveAgentMessageSource::Human
            {
                ActiveAgentMessageOutcome::NotFoundOrNotOwned
            } else {
                ActiveAgentMessageOutcome::NotActiveOrFinalizing
            };
            parked.reply(outcome);
        }
    }

    pub(super) fn expire(&mut self, now: tokio::time::Instant) {
        let (due, live): (Vec<_>, Vec<_>) = std::mem::take(&mut self.parked)
            .into_iter()
            .partition(|parked| parked.deadline.is_some_and(|deadline| deadline <= now));
        self.parked = live;
        for parked in due {
            parked.reply(ActiveAgentMessageOutcome::NotAcceptedBeforeDeadline);
        }
    }

    fn try_acquire(&self) -> Result<OwnedSemaphorePermit, usize> {
        let Some(permits) = &self.permits else {
            return Err(self.capacity);
        };
        Arc::clone(permits)
            .try_acquire_owned()
            .map_err(|_| self.capacity)
    }
}

impl ParkedSpawnReadyMessage {
    fn acknowledge_initial(&mut self) -> bool {
        let Some(message_id) = self.initial_message_id.take() else {
            return false;
        };
        if let Some(respond_to) = self.respond_to.take() {
            let _ = respond_to.send(ActiveAgentMessageOutcome::Accepted { message_id });
        }
        self.permit.take();
        true
    }

    fn into_request(
        mut self,
    ) -> Option<(
        ActiveAgentMessageRequest,
        String,
        oneshot::Sender<ActiveAgentMessageOutcome>,
        Option<OwnedSemaphorePermit>,
    )> {
        let respond_to = self.respond_to.take()?;
        Some((
            self.request.take(),
            std::mem::take(&mut self.parent_session_id),
            respond_to,
            self.permit.take(),
        ))
    }

    pub(super) fn reply(mut self, outcome: ActiveAgentMessageOutcome) {
        if let Some(respond_to) = self.respond_to.take() {
            let _ = respond_to.send(outcome);
        }
    }
}

impl<R: ChildRunner> SubagentCoordinator<R> {
    pub(super) fn handle_send_active_message(&mut self, ingress: ActiveMessageIngress) {
        let decision = self.resolve_send(
            ingress.request.request.target(),
            &ingress.request.parent_session_id,
        );
        match decision {
            ResolvedSend::Admit { subagent_id } => {
                self.admit_active_message(subagent_id, ingress);
            }
            ResolvedSend::Park { subagent_id } => {
                self.park_until_spawn_ready(subagent_id, ingress);
            }
            ResolvedSend::Wake { subagent_id } => {
                self.wake_completed_child(subagent_id, ingress);
            }
            ResolvedSend::Fail(outcome) => {
                let _ = ingress.request.respond_to.send(outcome);
            }
        }
    }

    fn park_until_spawn_ready(&mut self, subagent_id: String, ingress: ActiveMessageIngress) {
        let ActiveMessageIngress { request, permit } = ingress;
        let crate::implementations::grok_build::task::types::SubagentActiveMessageRequest {
            request,
            parent_session_id,
            respond_to,
        } = request;
        self.spawn_ready.push(ParkedSpawnReadyMessage {
            subagent_id,
            parent_session_id,
            request,
            respond_to: Some(respond_to),
            deadline: Some(tokio::time::Instant::now() + ACTIVE_MESSAGE_SPAWN_READY_TIMEOUT),
            initial_message_id: None,
            permit: Some(permit),
        });
    }

    fn resolve_send(&self, target: &ActiveMessageTarget, sender_session_id: &str) -> ResolvedSend {
        match target {
            ActiveMessageTarget::Address(address) => {
                self.resolve_address_send(address, sender_session_id)
            }
            ActiveMessageTarget::ChildId(id) => {
                // Ownership is not checked upstream.
                if !self.graph.is_reachable_from(id, sender_session_id) {
                    return ResolvedSend::Fail(ActiveAgentMessageOutcome::NotFoundOrNotOwned);
                }
                if self.active.contains_key(id) {
                    return ResolvedSend::Admit {
                        subagent_id: id.clone(),
                    };
                }
                self.resolve_inactive_child_id(id)
            }
        }
    }

    fn resolve_address_send(
        &self,
        address: &crate::implementations::grok_build::task::types::AgentAddress,
        sender_session_id: &str,
    ) -> ResolvedSend {
        use xai_message_delivery_core::{AddressCandidate, AddressDecision, AddressPresence};

        let presented = address.as_str();
        let address_hit =
            |stored: &Option<crate::implementations::grok_build::task::types::AgentAddress>| {
                stored
                    .as_ref()
                    .is_some_and(|value| value.as_str() == presented)
            };

        let active = self
            .active
            .iter()
            .find(|(_, child)| address_hit(&child.agent_address));
        let pending = self
            .pending
            .values()
            .find(|child| address_hit(&child.agent_address));
        let queued = self
            .queued
            .iter()
            .find(|child| address_hit(&child.agent_address));
        let active_by_id = self.active.iter().find(|(_, child)| {
            child.request.id == presented || child.child_session_id == presented
        });
        let pending_by_id = self
            .pending
            .values()
            .find(|child| child.request.id == presented);
        let completed = self
            .completed
            .iter()
            .find(|(_, child)| address_hit(&child.agent_address));

        let hit = if let Some((id, child)) = active {
            Some((
                id.as_str(),
                child.agent_address.as_ref().map(|value| value.as_str()),
                child.request.id.as_str(),
                child.request.owner.is_workflow(),
                AddressPresence::Active,
            ))
        } else if let Some(child) = pending {
            Some((
                child.request.id.as_str(),
                child.agent_address.as_ref().map(|value| value.as_str()),
                child.request.id.as_str(),
                child.request.owner.is_workflow(),
                if child.cancellation.is_cancelled() || child.explicitly_killed {
                    AddressPresence::Gone
                } else {
                    AddressPresence::Pending
                },
            ))
        } else if let Some(child) = queued {
            Some((
                child.request.id.as_str(),
                child.agent_address.as_ref().map(|value| value.as_str()),
                child.request.id.as_str(),
                child.request.owner.is_workflow(),
                if child.request.cancel_token.is_cancelled() {
                    AddressPresence::Gone
                } else {
                    AddressPresence::Pending
                },
            ))
        } else if let Some((id, child)) = active_by_id {
            Some((
                id.as_str(),
                child.agent_address.as_ref().map(|value| value.as_str()),
                if child.request.id == presented {
                    child.request.id.as_str()
                } else {
                    child.child_session_id.as_str()
                },
                child.request.owner.is_workflow(),
                AddressPresence::Active,
            ))
        } else if let Some(child) = pending_by_id {
            Some((
                child.request.id.as_str(),
                child.agent_address.as_ref().map(|value| value.as_str()),
                child.request.id.as_str(),
                child.request.owner.is_workflow(),
                AddressPresence::Pending,
            ))
        } else {
            completed.map(|(id, child)| {
                (
                    id.as_str(),
                    child.agent_address.as_ref().map(|value| value.as_str()),
                    child.request.id.as_str(),
                    child.request.owner.is_workflow(),
                    AddressPresence::Gone,
                )
            })
        };

        let candidate = hit
            .as_ref()
            .map(
                |(id, stored_address, session_id, workflow, presence)| AddressCandidate {
                    stored_address: *stored_address,
                    session_id,
                    owner_match: self.graph.is_reachable_from(id, sender_session_id),
                    generation_current: true,
                    workflow: *workflow,
                    presence: *presence,
                },
            );
        let decision = xai_message_delivery_core::resolve_address(presented, candidate.as_ref());
        match (decision, hit) {
            (
                AddressDecision::Owned {
                    presence: AddressPresence::Active,
                },
                Some((subagent_id, ..)),
            ) => ResolvedSend::Admit {
                subagent_id: subagent_id.to_owned(),
            },
            (
                AddressDecision::Owned {
                    presence: AddressPresence::Pending,
                },
                Some((subagent_id, ..)),
            ) => ResolvedSend::Park {
                subagent_id: subagent_id.to_owned(),
            },
            (AddressDecision::Stale, Some((subagent_id, ..)))
                if self.completed.contains_key(subagent_id) =>
            {
                ResolvedSend::Wake {
                    subagent_id: subagent_id.to_owned(),
                }
            }
            _ => ResolvedSend::Fail(ActiveAgentMessageOutcome::NotFoundOrNotOwned),
        }
    }

    /// Ownership was already established by the caller.
    fn resolve_inactive_child_id(&self, id: &str) -> ResolvedSend {
        if let Some(spawning) = self.spawning_child(id) {
            // Cancel and workflow stay fail-fast, matching the active path.
            if spawning.workflow || spawning.cancelled {
                return ResolvedSend::Fail(ActiveAgentMessageOutcome::NotActiveOrFinalizing);
            }
            return ResolvedSend::Park {
                subagent_id: id.to_owned(),
            };
        }
        // Caller already established ownership via the lineage graph.
        if self
            .completed
            .get(id)
            .is_some_and(|child| !child.request.owner.is_workflow())
        {
            ResolvedSend::Wake {
                subagent_id: id.to_owned(),
            }
        } else {
            ResolvedSend::Fail(if self.completed.contains_key(id) {
                ActiveAgentMessageOutcome::NotActiveOrFinalizing
            } else {
                ActiveAgentMessageOutcome::NotFoundOrNotOwned
            })
        }
    }

    fn spawning_child(&self, id: &str) -> Option<SpawningChild> {
        if let Some(child) = self.pending.get(id) {
            return Some(SpawningChild {
                workflow: child.request.owner.is_workflow(),
                cancelled: child.cancellation.is_cancelled(),
            });
        }
        self.queued.iter().find_map(|queued| {
            (queued.request.id == id).then_some(SpawningChild {
                workflow: queued.request.owner.is_workflow(),
                cancelled: queued.request.cancel_token.is_cancelled(),
            })
        })
    }

    pub(super) fn admit_spawn_ready_messages(&mut self, subagent_id: &str) {
        for ingress in self.spawn_ready.admit(subagent_id) {
            self.admit_active_message(subagent_id.to_owned(), ingress);
        }
    }

    pub(super) fn reject_spawn_ready_ids(&mut self, ids: &[String]) {
        for id in ids {
            // Cancelled pending children never become active. Human parked
            // sends are terminal; agent sends stay retryable not_active.
            self.spawn_ready.reject_terminal(id);
        }
    }

    pub(super) fn expire_spawn_ready_messages(&mut self, now: tokio::time::Instant) {
        self.spawn_ready.expire(now);
    }

    fn admit_active_message(
        &mut self,
        canonical_subagent_id: String,
        ingress: ActiveMessageIngress,
    ) {
        let ActiveMessageIngress { request, permit } = ingress;
        let crate::implementations::grok_build::task::types::SubagentActiveMessageRequest {
            request,
            parent_session_id,
            respond_to,
        } = request;
        let source = request.source();
        // Single ownership gate for address and raw-id sends before delivery.
        let owned = self
            .graph
            .is_reachable_from(&canonical_subagent_id, &parent_session_id);
        let Some(child) = self.active.get_mut(&canonical_subagent_id) else {
            let _ = respond_to.send(ActiveAgentMessageOutcome::NotActiveOrFinalizing);
            return;
        };
        if !owned {
            let _ = respond_to.send(ActiveAgentMessageOutcome::NotFoundOrNotOwned);
            return;
        }
        if child.request.owner.is_workflow() || child.cancellation.is_cancelled() {
            let _ = respond_to.send(refused_active_outcome(source));
            return;
        }
        if source == ActiveAgentMessageSource::Human && child.explicitly_killed {
            let _ = respond_to.send(ActiveAgentMessageOutcome::NotFoundOrNotOwned);
            return;
        }
        match child.active_messages.begin_admission() {
            BeginAdmission::Started => {}
            BeginAdmission::Finalizing => {
                let _ = respond_to.send(refused_active_outcome(source));
                return;
            }
            BeginAdmission::Saturated => {
                let _ = respond_to.send(ActiveAgentMessageOutcome::Saturated {
                    max_in_flight: MAX_ACTIVE_MESSAGE_ADMISSIONS_PER_CHILD,
                });
                return;
            }
        }

        let message_id = uuid::Uuid::now_v7().to_string();
        let lease = ActiveMessageAdmissionLease::new();
        let admission = child
            .control
            .send_active_message(ActiveAgentMessageDelivery::new(
                ActiveAgentMessage {
                    message_id: message_id.clone(),
                    sender_session_id: parent_session_id,
                    text: request.text().clone(),
                },
                request.operation(),
                request.source(),
                Arc::clone(&lease),
            ));
        self.active_messages.push(ActiveMessageFuture {
            subagent_id: canonical_subagent_id,
            generation: child.generation,
            message_id,
            future: admission,
            cancellation: Box::pin(child.cancellation.clone().cancelled_owned()),
            deadline: Box::pin(tokio::time::sleep(ACTIVE_MESSAGE_ADMISSION_TIMEOUT)),
            lease,
            ingress_permit: Some(permit),
            respond_to: Some(respond_to),
        });
    }

    pub(super) fn finish_active_message(&mut self, mut completion: ActiveMessageCompletion) {
        let Some(respond_to) = completion.respond_to.take() else {
            return;
        };
        let Some(child) = self
            .active
            .get_mut(&completion.subagent_id)
            .filter(|child| child.generation == completion.generation)
        else {
            let _ = respond_to.send(lost_completion_outcome(
                completion.outcome,
                completion.is_settled,
            ));
            return;
        };
        let protocol_outcome = protocol_outcome_from_completion(
            completion.outcome,
            completion.is_settled,
            &completion.message_id,
        );
        let _ = respond_to.send(protocol_outcome);
        let terminal_disposition = child
            .active_messages
            .finish_admission(completion.is_settled);
        if let Some(is_clean) = terminal_disposition
            && let Some(output) = self.terminal_outputs.remove(&completion.subagent_id)
        {
            self.finish_terminalized_child(&completion.subagent_id, output, is_clean);
        }
    }

    pub(super) fn handle_active_message_finalizing(
        &mut self,
        subagent_id: String,
        respond_to: oneshot::Sender<bool>,
    ) {
        let Some(child) = self.active.get_mut(&subagent_id) else {
            let _ = respond_to.send(false);
            return;
        };
        child.active_messages.begin_finalizing(respond_to);
    }
}

#[cfg(test)]
#[path = "active_message_tests.rs"]
pub(super) mod tests;

#[cfg(test)]
#[path = "active_message_lineage_tests.rs"]
mod lineage_tests;
