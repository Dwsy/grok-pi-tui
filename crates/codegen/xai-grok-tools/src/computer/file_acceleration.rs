//! Host seam for accelerating a session's local file system.
//!
//! A host installs at most one [`FileAcceleratorFactory`] per process. [`local_fs`] hands it the session's
//! [`LocalFs`] only when the host's [`FileAccelerationGate`] is enabled; otherwise, and whenever nothing is
//! installed, the session gets plain [`LocalFs`]. The accelerator reports through the host's
//! [`AccelerationSink`] with content-free [`AccelerationNotice`]s, because this crate emits no telemetry.

use std::{
    collections::BTreeMap,
    sync::{Arc, OnceLock},
};

use crate::computer::{local::LocalFs, types::AsyncFileSystem};

/// Wraps a session's local file system. Called once per session, on the session's spawn path, so it must not block.
pub type FileAcceleratorFactory =
    Box<dyn Fn(Arc<dyn AsyncFileSystem>, AccelerationContext) -> AcceleratedFs + Send + Sync>;

/// Receives every notice an accelerator sends for one session. It may be called from any thread, including one
/// outside the host's async runtime.
pub type AccelerationSink = Arc<dyn Fn(AccelerationNotice) + Send + Sync>;

/// Upper bounds, in milliseconds, of every latency bucket but the last, which counts everything slower.
pub const LATENCY_BUCKET_EDGES_MS: [u64; 6] = [10, 50, 100, 500, 1_000, 5_000];

/// Search counts per latency bucket, bounded by [`LATENCY_BUCKET_EDGES_MS`].
pub type LatencyBuckets = [u64; LATENCY_BUCKET_EDGES_MS.len() + 1];

static FILE_ACCELERATOR: AcceleratorSlot = AcceleratorSlot(OnceLock::new());

/// Registers the process's accelerator. The first registration wins; later ones are ignored.
pub fn install_file_accelerator(factory: FileAcceleratorFactory) {
    FILE_ACCELERATOR.install(factory);
}

/// The session's local file system, wrapped only when `gate` is enabled and an accelerator is installed.
#[must_use]
pub fn local_fs(gate: FileAccelerationGate) -> AcceleratedFs {
    FILE_ACCELERATOR.local_fs(gate)
}

/// The host's decision for one session. The host enables it only after its own policy checks pass.
pub enum FileAccelerationGate {
    Disabled,
    Enabled(AccelerationContext),
}

/// What the host hands an accelerator for one session.
pub struct AccelerationContext {
    /// Opaque route override from the host's configuration; the accelerator parses it. `None` means its defaults.
    pub routes: Option<Arc<str>>,
    pub sink: AccelerationSink,
}

/// A session's file system plus the owner of its accelerator summary, if an accelerator wrapped it.
pub struct AcceleratedFs {
    pub fs: Arc<dyn AsyncFileSystem>,
    pub session: Option<AccelerationSession>,
}

/// Owns a session's accelerator summary. [`AccelerationSession::finish`] runs it once at teardown, and dropping
/// the owner without finishing runs it too, so a session that ends on an error path still reports.
#[must_use = "dropping the session runs its summary immediately"]
pub struct AccelerationSession {
    summary: Option<Box<dyn FnOnce() + Send>>,
}

impl AccelerationSession {
    pub fn new(summary: impl FnOnce() + Send + 'static) -> Self {
        AccelerationSession {
            summary: Some(Box::new(summary)),
        }
    }

    /// Runs the summary now.
    pub fn finish(self) {
        drop(self);
    }
}

impl Drop for AccelerationSession {
    fn drop(&mut self) {
        if let Some(summary) = self.summary.take() {
            summary();
        }
    }
}

/// Which side of the comparison a session is on. A control session never serves from the accelerator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccelerationArm {
    Treatment,
    Control,
}

/// Content-free report from an accelerator: no paths, patterns, or file contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccelerationNotice {
    SessionStarted {
        arm: AccelerationArm,
    },
    SessionEnded {
        arm: AccelerationArm,
        duration_ms: u64,
        /// Searches the accelerator answered.
        served: u64,
        /// Searches the accelerator declined without doing any I/O, by reason.
        declined: BTreeMap<&'static str, u64>,
        /// Searches the accelerator took on and then gave back to the fallback, by reason.
        failed: BTreeMap<&'static str, u64>,
        served_latency: LatencyBuckets,
        /// Searches declined because every in-flight slot was taken.
        saturated: u64,
    },
    /// A search found the accelerator unavailable; it retries no sooner than `retry_in_ms`. The accelerator sends
    /// this at most once per backoff window, not once per search, because the host emits each notice as an event.
    UnavailableHit {
        label: &'static str,
        retry_in_ms: u64,
    },
}

/// A factory slot, so the process static and a test-owned slot share one code path.
struct AcceleratorSlot(OnceLock<FileAcceleratorFactory>);

impl AcceleratorSlot {
    fn install(&self, factory: FileAcceleratorFactory) {
        if self.0.set(factory).is_err() {
            tracing::debug!("a file accelerator is already installed; ignoring the later one");
        }
    }

    fn local_fs(&self, gate: FileAccelerationGate) -> AcceleratedFs {
        let local: Arc<dyn AsyncFileSystem> = Arc::new(LocalFs);
        match (gate, self.0.get()) {
            (FileAccelerationGate::Enabled(context), Some(factory)) => factory(local, context),
            (FileAccelerationGate::Disabled, _) | (FileAccelerationGate::Enabled(_), None) => {
                AcceleratedFs {
                    fs: local,
                    session: None,
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "file_acceleration_tests.rs"]
mod tests;
