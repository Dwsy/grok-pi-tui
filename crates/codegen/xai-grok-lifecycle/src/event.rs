//! The pre-stop event handlers receive, the outcome they report, and the built-in handler trait.

use std::future::Future;
use std::pin::Pin;

use tokio::time::Instant;

use crate::token::ReasonToken;

/// What happens to the VM's local disk after the stop.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, strum::EnumString, strum::IntoStaticStr, strum::Display,
)]
#[strum(serialize_all = "snake_case")]
pub enum Disk {
    /// A snapshot: the local disk comes back when the VM resumes.
    Kept,
    /// Recycle, terminate, reset, or replace: the local disk is gone.
    Discarded,
}

/// One pre-stop trigger as handlers see it.
#[derive(Debug, Clone)]
pub struct PreStopEvent {
    reason: ReasonToken,
    disk: Disk,
    deadline: Instant,
}

impl PreStopEvent {
    pub fn new(reason: ReasonToken, disk: Disk, deadline: Instant) -> Self {
        PreStopEvent {
            reason,
            disk,
            deadline,
        }
    }

    pub fn reason(&self) -> &ReasonToken {
        &self.reason
    }

    pub fn disk(&self) -> Disk {
        self.disk
    }

    /// The absolute deadline. A handler still running at this instant is cut and reported `timed_out`.
    pub fn deadline(&self) -> Instant {
        self.deadline
    }
}

/// What one handler reports for one trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandlerOutcome {
    Ok,
    Failed,
    TimedOut,
}

/// The boxed future a [`PreStopHandler`] returns; boxed so the trait stays object safe.
pub type HandlerFuture<'a> = Pin<Box<dyn Future<Output = HandlerOutcome> + Send + 'a>>;

/// An in-process handler a component registers with [`crate::LifecycleBroker::register_builtin`] at startup.
///
/// Implementers must honor this contract:
/// - Idempotent: a handler may run zero, one, or many times in one VM lifetime, including twice for one stop.
/// - Non-destructive: a snapshot stop can be abandoned and the VM keeps serving afterwards. The one exception is
///   `reason() == "reset"`, which means the user asked to wipe the VM.
/// - Bounded: finish by [`PreStopEvent::deadline`]. The broker drops the future at the deadline and reports
///   `timed_out`, so never block the thread; offload blocking work and keep the future cancel-safe.
/// - Quiet: put no user data in the outcome.
pub trait PreStopHandler: Send + Sync + 'static {
    fn pre_stop<'a>(&'a self, event: &'a PreStopEvent) -> HandlerFuture<'a>;
}
