//! The seam between the exact-origin policy and the connection: after the request is
//! authenticated and its origin parsed, and before any DNS or upstream connect, the [`Decider`]
//! says whether this origin may be reached for this call. `Ask` parks the request until the
//! answer arrives, the hold deadline passes, or the client hangs up; nothing is relayed while
//! held. Every decision that a card or a log needs to know about is published as a content-free
//! [`BlockedRequest`].
//!
//! Holds are budgeted twice: [`DEFAULT_MAX_HELD`] across the proxy and [`DEFAULT_MAX_HELD_PER_CALL`]
//! per credential, so one command whose asks nobody answers cannot pin every slot the other
//! commands need.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::{broadcast, oneshot, watch};
use xai_grok_sandbox::WebsiteOrigin;
use xai_grok_sandbox::command::CommandTag;

use crate::ProxyState;
use crate::error::ProxyError;

/// How long an `Ask` may stay parked before the proxy answers 403 `denied` itself. The same
/// ten minutes as the hub's permission backstop, so the card's countdown and the held
/// connection end together.
pub const DEFAULT_HOLD_TIMEOUT: Duration = Duration::from_secs(600);
/// Requests parked at once across every call; the next `Ask` gets 503 `overloaded` so a prompt
/// storm cannot pin every connection slot.
pub const DEFAULT_MAX_HELD: usize = 16;
/// Requests one credential (one command's `CommandTag`, or the session token) may park at once
/// inside [`DEFAULT_MAX_HELD`]; its next `Ask` gets 503 while the rest of the budget stays
/// available to other commands.
pub const DEFAULT_MAX_HELD_PER_CALL: usize = 8;
/// Slow subscribers of [`EgressProxyHandle::blocked_requests`](crate::EgressProxyHandle::blocked_requests)
/// lose the oldest records past this many.
pub const BLOCKED_REQUEST_CAPACITY: usize = 256;
/// Bytes a client may send while held (TLS early records after CONNECT) before the hold is
/// treated as malformed rather than buffered further.
const MAX_HELD_CLIENT_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    /// Observe mode's outcome: allowed, and published with what
    /// enforce mode would have done so the observe summary shows the would-be verdict per host.
    AllowAndRecord {
        would: WouldBe,
    },
    Deny,
    /// Nobody allowed the origin: the proxy checks its hold budget, then asks the decider to
    /// [`hold`](Decider::hold) the connection.
    Ask,
}

/// What enforce mode would have done with a connection observe mode let through.
/// Observe never blocks, deny lists included: the verdict is recorded, not applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WouldBe {
    Allowed,
    Denied(DenySource),
    Asked,
}

/// Which remembered decision would have denied the connection under enforce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DenySource {
    /// A `NetHost` deny row (the card's "keep blocked").
    DenyRow,
    /// The organisation's `disallowed_web_fetch_domains`.
    WebFetchDenylist,
}

/// Implementations run on the connection task, so they must return promptly: a long decision
/// is an `Ask` whose answer is awaited on the channel [`hold`](Decider::hold) returns. `call` is
/// the credential the client presented, `None` for the session-wide token.
#[async_trait]
pub trait Decider: Send + Sync {
    async fn decide(&self, origin: &WebsiteOrigin, call: Option<&CommandTag>) -> Decision;

    /// Park the connection [`decide`](Decider::decide) answered [`Decision::Ask`] for — raise
    /// whatever asks the user — and return the channel the answer arrives on; a dropped sender
    /// is a deny. The default parks nothing and denies.
    async fn hold(
        &self,
        origin: &WebsiteOrigin,
        call: Option<&CommandTag>,
    ) -> oneshot::Receiver<bool> {
        let _ = (origin, call);
        oneshot::channel().1
    }

    /// Whether a request with a missing, unknown or revoked credential proceeds as the session's
    /// (no call) instead of a `407`: a decider that only records must not block a process whose
    /// call has ended. The default refuses.
    fn admits_unauthenticated(&self) -> bool {
        false
    }

    /// The proxy is stopping, drained or aborted: every connection it still parks is being
    /// answered with a deny by the proxy itself, so whatever the decider raised for them (a card
    /// waiting on the user) is to be withdrawn now, not at its deadline. Called once, before the
    /// connection tasks are waited for. The default has nothing to withdraw.
    fn stopping(&self) {}
}

/// Allows whatever the exact-origin policy already allowed.
pub struct PolicyOnly;

#[async_trait]
impl Decider for PolicyOnly {
    async fn decide(&self, _origin: &WebsiteOrigin, _call: Option<&CommandTag>) -> Decision {
        Decision::Allow
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeciderOutcome {
    Allowed,
    Denied,
    /// The hold deadline passed with no answer; the client saw 403.
    Timeout,
    /// Observe mode let it through; `would` is what enforce mode would have done.
    Observed {
        would: WouldBe,
    },
    /// The client closed its connection while held; the ask is moot.
    Abandoned,
}

/// Content-free record of one decision: no path, no headers, no body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockedRequest {
    pub host: String,
    pub port: u16,
    pub call: Option<CommandTag>,
    pub decided: DeciderOutcome,
    pub hold_ms: u64,
    /// When the hold would have been (or was) answered 403 by the proxy itself: the park time
    /// plus the hold timeout, in unix seconds. `None` for a decision that never held.
    pub deadline_unix: Option<i64>,
}

/// The per-credential half of the hold budget; the proxy-wide half is the semaphore beside it.
pub(crate) struct HoldBudget {
    per_call: usize,
    held: Mutex<HashMap<Option<CommandTag>, usize>>,
}

impl HoldBudget {
    pub(crate) fn new(per_call: usize) -> HoldBudget {
        HoldBudget {
            per_call,
            held: Mutex::new(HashMap::new()),
        }
    }

    /// `None` when `call` already parks its share; the permit hands the slot back on drop.
    pub(crate) fn try_take(&self, call: Option<&CommandTag>) -> Option<HoldBudgetPermit<'_>> {
        let key = call.cloned();
        let mut held = self
            .held
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let count = held.entry(key.clone()).or_insert(0);
        if *count >= self.per_call {
            return None;
        }
        *count += 1;
        Some(HoldBudgetPermit { budget: self, key })
    }

    #[cfg(test)]
    pub(crate) fn per_call(&self) -> usize {
        self.per_call
    }

    #[cfg(test)]
    pub(crate) fn held_by(&self, call: Option<&CommandTag>) -> usize {
        self.held
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&call.cloned())
            .copied()
            .unwrap_or(0)
    }
}

#[must_use]
pub(crate) struct HoldBudgetPermit<'a> {
    budget: &'a HoldBudget,
    key: Option<CommandTag>,
}

impl Drop for HoldBudgetPermit<'_> {
    fn drop(&mut self) {
        let mut held = self
            .budget
            .held
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(count) = held.get_mut(&self.key) {
            *count -= 1;
            if *count == 0 {
                held.remove(&self.key);
            }
        }
    }
}

impl ProxyState {
    /// Policy first, then the decider; `Ok` means DNS and connect may proceed. `buffered` grows
    /// with anything the client sent while held so the CONNECT path still sees its ClientHello.
    pub(crate) async fn admit<S: AsyncRead + Unpin>(
        &self,
        origin: &WebsiteOrigin,
        call: Option<&CommandTag>,
        client: &mut S,
        buffered: &mut Vec<u8>,
    ) -> Result<(), ProxyError> {
        if self.policy.evaluate(origin) != xai_grok_sandbox::WebsiteAction::Allow {
            return Err(ProxyError::PolicyDenied);
        }
        // A decision is prompt by contract; one that is not (a stalled read behind it) is refused
        // on the request timeout instead of pinning the connection
        let Ok(decision) = tokio::time::timeout(
            self.options.request_timeout,
            self.decider.decide(origin, call),
        )
        .await
        else {
            return Err(ProxyError::Timeout);
        };
        match decision {
            Decision::Allow => Ok(()),
            Decision::AllowAndRecord { would } => {
                self.publish(origin, call, DeciderOutcome::Observed { would }, 0, None);
                Ok(())
            }
            Decision::Deny => {
                self.publish(origin, call, DeciderOutcome::Denied, 0, None);
                Err(ProxyError::PolicyDenied)
            }
            Decision::Ask => {
                // Budget before parking: a connection refused with 503 never raised a card. The
                // per-call share goes first so a command at its own cap never takes a proxy-wide
                // permit it would hand straight back.
                let Some(_share) = self.hold_budget.try_take(call) else {
                    return Err(ProxyError::Overloaded);
                };
                let Ok(_permit) = self.holds.try_acquire() else {
                    return Err(ProxyError::Overloaded);
                };
                // The timeout runs from here: a decider slow to park still meets the backstop
                let started = Instant::now();
                let deadline_unix = self.options.clock.now_unix().saturating_add(
                    i64::try_from(self.options.hold_timeout.as_secs()).unwrap_or(i64::MAX),
                );
                let held = match tokio::time::timeout(
                    self.options.hold_timeout,
                    self.decider.hold(origin, call),
                )
                .await
                {
                    Ok(answer) => {
                        let left = self.options.hold_timeout.saturating_sub(started.elapsed());
                        hold(client, buffered, answer, self.stopping.subscribe(), left).await
                    }
                    Err(_) => Ok(DeciderOutcome::Timeout),
                };
                let hold_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
                let (outcome, result) = match held {
                    Ok(outcome @ DeciderOutcome::Allowed) => (outcome, Ok(())),
                    Ok(outcome @ DeciderOutcome::Denied) => {
                        (outcome, Err(ProxyError::PolicyDenied))
                    }
                    Ok(outcome @ DeciderOutcome::Timeout) => {
                        (outcome, Err(ProxyError::HoldTimeout))
                    }
                    Ok(outcome @ DeciderOutcome::Abandoned) => {
                        (outcome, Err(ProxyError::ClientGone))
                    }
                    // A hold never observes: the park is enforce mode's
                    Ok(DeciderOutcome::Observed { .. }) => {
                        (DeciderOutcome::Denied, Err(ProxyError::PolicyDenied))
                    }
                    // The client sent too much while held and was refused (`400`), still connected
                    Err(error) => (DeciderOutcome::Denied, Err(error)),
                };
                self.publish(origin, call, outcome, hold_ms, Some(deadline_unix));
                result
            }
        }
    }

    fn publish(
        &self,
        origin: &WebsiteOrigin,
        call: Option<&CommandTag>,
        decided: DeciderOutcome,
        hold_ms: u64,
        deadline_unix: Option<i64>,
    ) {
        let record = BlockedRequest {
            host: origin.hostname().to_owned(),
            port: origin.port(),
            call: call.cloned(),
            decided,
            hold_ms,
            deadline_unix,
        };
        // A send fails only when nobody subscribed, the normal case for a proxy nobody watches.
        let _ = self.blocked.send(record);
    }
}

pub(crate) fn blocked_channel() -> broadcast::Sender<BlockedRequest> {
    broadcast::channel(BLOCKED_REQUEST_CAPACITY).0
}

/// Parks until the answer, the proxy stopping (a deny, so a drain never waits out a card), the
/// deadline, or the client's EOF — the only hang-up signal before the answer, so a client that
/// half-closes its write side while held reads as gone too. Bytes read while held are kept only
/// for the ClientHello case.
async fn hold<S: AsyncRead + Unpin>(
    client: &mut S,
    buffered: &mut Vec<u8>,
    mut answer: oneshot::Receiver<bool>,
    mut stopping: watch::Receiver<bool>,
    timeout: Duration,
) -> Result<DeciderOutcome, ProxyError> {
    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);
    let mut chunk = [0u8; 4096];
    loop {
        tokio::select! {
            biased;
            decided = &mut answer => {
                return Ok(if decided.unwrap_or(false) {
                    DeciderOutcome::Allowed
                } else {
                    DeciderOutcome::Denied
                });
            }
            // A closed sender is the serve loop gone, which is a stop too
            _ = stopping.wait_for(|stopped| *stopped) => return Ok(DeciderOutcome::Denied),
            () = &mut deadline => return Ok(DeciderOutcome::Timeout),
            read = client.read(&mut chunk) => match read {
                Ok(0) | Err(_) => return Ok(DeciderOutcome::Abandoned),
                Ok(count) => {
                    if buffered.len() + count > MAX_HELD_CLIENT_BYTES {
                        return Err(ProxyError::TooLarge);
                    }
                    buffered.extend_from_slice(&chunk[..count]);
                }
            },
        }
    }
}
