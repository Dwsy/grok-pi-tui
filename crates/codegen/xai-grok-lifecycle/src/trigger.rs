//! One trigger: parse the request, build the handler table, fan out under the absolute deadline, and report.
//!
//! Verdict precedence: `no_handlers` (the table is empty), then `ok` (every handler ok), then `timed_out` (the
//! trigger deadline cut at least one handler), then `failed` (every handler failed or hit its own timeout), then
//! `partial`. A deadline cut outranks `failed` so the host can tell "the budget was too short" from "handlers broke".

use std::collections::{BTreeSet, HashMap};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinSet;
use tokio::time::Instant;

use crate::event::{Disk, HandlerOutcome, PreStopEvent, PreStopHandler};
use crate::exec::{ExecContext, run_exec};
use crate::exec_spec::{ExecSpec, MAX_TIMEOUT_MS, MIN_TIMEOUT_MS};
use crate::image::ImageHandler;
use crate::token::{HandlerName, ReasonToken};
use crate::wire::{
    ErrorClass, HandlerResultWire, HandlerSource, OutcomeWire, TriggerRequestWire,
    TriggerResponseWire, Verdict, WIRE_VERSION,
};

/// Cap on the trigger response body; trailing handler entries are dropped to fit.
pub(crate) const MAX_RESPONSE_BYTES: usize = 2048;

/// Handlers still running this long past the deadline are abandoned (their tasks aborted, their process groups
/// killed) and reported `timed_out`. Leaves headroom under the contract's 250 ms epsilon for encoding and writing.
const COLLECT_SLACK: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TriggerRequest {
    pub(crate) reason: ReasonToken,
    pub(crate) disk: Disk,
    /// `deadline_ms`, within `MIN_TIMEOUT_MS..=MAX_TIMEOUT_MS`.
    pub(crate) budget: Duration,
}

impl TriggerRequest {
    pub(crate) fn parse(body: &[u8]) -> Result<Self, ErrorClass> {
        let wire: TriggerRequestWire = serde_json::from_slice(body).map_err(|e| {
            tracing::debug!(error = %e, "invalid pre-stop trigger body");
            ErrorClass::InvalidBody
        })?;
        if wire.v != WIRE_VERSION {
            return Err(ErrorClass::UnsupportedVersion);
        }
        let reason =
            ReasonToken::try_from(wire.reason.as_str()).map_err(|_| ErrorClass::InvalidReason)?;
        let disk = Disk::from_str(&wire.disk).map_err(|_| ErrorClass::InvalidDisk)?;
        if !(MIN_TIMEOUT_MS..=MAX_TIMEOUT_MS).contains(&wire.deadline_ms) {
            return Err(ErrorClass::InvalidDeadline);
        }
        Ok(TriggerRequest {
            reason,
            disk,
            budget: Duration::from_millis(wire.deadline_ms),
        })
    }
}

/// How one handler's run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunOutcome {
    /// The handler finished on its own and reported this.
    Finished(HandlerOutcome),
    /// An exec handler's own `timeout_ms` cut it before the trigger deadline.
    HandlerTimeout,
    /// The trigger deadline cut it.
    DeadlineCut,
}

impl RunOutcome {
    pub(crate) fn to_wire(self) -> OutcomeWire {
        match self {
            RunOutcome::Finished(HandlerOutcome::Ok) => OutcomeWire::Ok,
            RunOutcome::Finished(HandlerOutcome::Failed) => OutcomeWire::Failed,
            RunOutcome::Finished(HandlerOutcome::TimedOut)
            | RunOutcome::HandlerTimeout
            | RunOutcome::DeadlineCut => OutcomeWire::TimedOut,
        }
    }
}

pub(crate) enum HandlerKind {
    Builtin(Arc<dyn PreStopHandler>),
    Exec(ExecSpec),
}

pub(crate) struct TableEntry {
    pub(crate) name: HandlerName,
    pub(crate) source: HandlerSource,
    pub(crate) kind: HandlerKind,
}

/// Built-in, then image, then registered handlers, each already sorted by name. A name claimed by an earlier source
/// shadows later ones.
pub(crate) fn handler_table(
    builtins: Vec<(HandlerName, Arc<dyn PreStopHandler>)>,
    images: Vec<ImageHandler>,
    registered: Vec<(HandlerName, ExecSpec)>,
) -> Vec<TableEntry> {
    let builtins = builtins.into_iter().map(|(name, handler)| TableEntry {
        name,
        source: HandlerSource::Builtin,
        kind: HandlerKind::Builtin(handler),
    });
    let images = images.into_iter().map(|image| TableEntry {
        name: image.name,
        source: HandlerSource::Image,
        kind: HandlerKind::Exec(image.spec),
    });
    let registered = registered.into_iter().map(|(name, spec)| TableEntry {
        name,
        source: HandlerSource::Registered,
        kind: HandlerKind::Exec(spec),
    });
    let mut seen = BTreeSet::new();
    builtins
        .chain(images)
        .chain(registered)
        .filter(|entry| {
            let is_new = seen.insert(entry.name.clone());
            if !is_new {
                tracing::warn!(handler = %entry.name, source = <&'static str>::from(entry.source), "lifecycle handler shadowed by an earlier source");
            }
            is_new
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HandlerReport {
    pub(crate) name: HandlerName,
    pub(crate) source: HandlerSource,
    pub(crate) outcome: RunOutcome,
    pub(crate) elapsed: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TriggerReport {
    pub(crate) verdict: Verdict,
    pub(crate) elapsed: Duration,
    pub(crate) handlers: Vec<HandlerReport>,
}

/// Runs every entry concurrently and returns one report per entry, in table order.
pub(crate) async fn fan_out(
    table: Vec<TableEntry>,
    event: &PreStopEvent,
    ctx: &ExecContext,
) -> Vec<HandlerReport> {
    let started = Instant::now();
    let mut tasks = JoinSet::new();
    let mut slots = HashMap::with_capacity(table.len());
    for (index, entry) in table.iter().enumerate() {
        let event = event.clone();
        let task = match &entry.kind {
            HandlerKind::Builtin(handler) => {
                let handler = Arc::clone(handler);
                tasks.spawn(async move {
                    let run_started = Instant::now();
                    let outcome =
                        match tokio::time::timeout_at(event.deadline(), handler.pre_stop(&event))
                            .await
                        {
                            Ok(outcome) => RunOutcome::Finished(outcome),
                            Err(_elapsed) => RunOutcome::DeadlineCut,
                        };
                    (outcome, run_started.elapsed())
                })
            }
            HandlerKind::Exec(spec) => {
                let run = run_exec(entry.name.clone(), spec.clone(), event, ctx.clone());
                tasks.spawn(async move {
                    let run_started = Instant::now();
                    (run.await, run_started.elapsed())
                })
            }
        };
        slots.insert(task.id(), index);
    }

    let mut results: Vec<Option<(RunOutcome, Duration)>> = vec![None; table.len()];
    let collect_until = event.deadline() + COLLECT_SLACK;
    loop {
        let (id, result) =
            match tokio::time::timeout_at(collect_until, tasks.join_next_with_id()).await {
                Ok(Some(Ok((id, result)))) => (id, result),
                Ok(Some(Err(e))) => {
                    tracing::warn!(error = %e, "lifecycle handler task failed");
                    (
                        e.id(),
                        (
                            RunOutcome::Finished(HandlerOutcome::Failed),
                            started.elapsed(),
                        ),
                    )
                }
                Ok(None) => break,
                Err(_elapsed) => {
                    tracing::warn!(
                        running = tasks.len(),
                        "lifecycle handlers still running past the deadline; abandoning them"
                    );
                    tasks.abort_all();
                    break;
                }
            };
        if let Some(slot) = slots.get(&id).and_then(|&index| results.get_mut(index)) {
            *slot = Some(result);
        }
    }

    table
        .into_iter()
        .zip(results)
        .map(|(entry, result)| {
            let (outcome, elapsed) = result.unwrap_or((RunOutcome::DeadlineCut, started.elapsed()));
            HandlerReport {
                name: entry.name,
                source: entry.source,
                outcome,
                elapsed,
            }
        })
        .collect()
}

pub(crate) fn verdict(handlers: &[HandlerReport]) -> Verdict {
    let is_ok = |report: &HandlerReport| report.outcome == RunOutcome::Finished(HandlerOutcome::Ok);
    if handlers.is_empty() {
        Verdict::NoHandlers
    } else if handlers.iter().all(is_ok) {
        Verdict::Ok
    } else if handlers
        .iter()
        .any(|report| report.outcome == RunOutcome::DeadlineCut)
    {
        Verdict::TimedOut
    } else if !handlers.iter().any(is_ok) {
        Verdict::Failed
    } else {
        Verdict::Partial
    }
}

/// Serializes the `200` body, dropping trailing handler entries (and setting `truncated`) until it fits in
/// [`MAX_RESPONSE_BYTES`].
pub(crate) fn encode_trigger_response(report: &TriggerReport) -> serde_json::Result<Vec<u8>> {
    let mut wire = TriggerResponseWire {
        v: WIRE_VERSION,
        verdict: report.verdict,
        elapsed_ms: duration_ms(report.elapsed),
        handlers: report
            .handlers
            .iter()
            .map(|handler| HandlerResultWire {
                name: handler.name.as_ref(),
                source: handler.source,
                outcome: handler.outcome.to_wire(),
                elapsed_ms: duration_ms(handler.elapsed),
            })
            .collect(),
        truncated: false,
    };
    let mut body = serde_json::to_vec(&wire)?;
    while body.len() > MAX_RESPONSE_BYTES && wire.handlers.pop().is_some() {
        wire.truncated = true;
        body = serde_json::to_vec(&wire)?;
    }
    Ok(body)
}

pub(crate) fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
#[path = "trigger_tests.rs"]
mod tests;
