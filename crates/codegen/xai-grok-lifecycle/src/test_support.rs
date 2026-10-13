//! Fixtures shared by the sibling `*_tests.rs` files.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Notify, mpsc};

use crate::broker::{LifecycleBroker, LifecycleConfig};
use crate::event::{Disk, HandlerFuture, HandlerOutcome, PreStopEvent, PreStopHandler};
use crate::exec_spec::ExecSpec;
use crate::persist::BootId;
use crate::token::{HandlerName, ReasonToken};
use crate::trigger::TriggerRequest;
use crate::wire::HandlerSpecWire;

pub(crate) const BOOT_A: &str = "boot-a";

/// A broker over a private temp dir holding `image/`, `state/`, and `exec.log`, persisting under [`BOOT_A`].
pub(crate) struct TestBroker {
    pub(crate) broker: LifecycleBroker,
    dir: tempfile::TempDir,
}

impl TestBroker {
    pub(crate) fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let broker = LifecycleBroker::with_boot_id(config(&dir), Some(BootId::from(BOOT_A)));
        TestBroker { broker, dir }
    }

    /// A fresh broker over the same dirs, as after a workspace-server restart under `boot_id`.
    pub(crate) fn restart(&self, boot_id: &str) -> LifecycleBroker {
        LifecycleBroker::with_boot_id(config(&self.dir), Some(BootId::from(boot_id)))
    }

    pub(crate) fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    pub(crate) fn state_dir(&self) -> PathBuf {
        self.path("state")
    }

    pub(crate) fn write_image_manifest(&self, file_name: &str, contents: &str) {
        let image_dir = self.path("image");
        std::fs::create_dir_all(&image_dir).expect("image dir");
        std::fs::write(image_dir.join(file_name), contents).expect("write manifest");
    }

    pub(crate) fn add_builtin(&self, name: &str, handler: impl PreStopHandler) {
        self.broker
            .register_builtin(handler_name(name), Arc::new(handler))
            .expect("register built-in");
    }

    pub(crate) async fn register(&self, name: &str, argv: &[&str]) {
        self.broker
            .registry()
            .put(handler_name(name), spec(argv, None))
            .await
            .expect("register");
    }
}

fn config(dir: &tempfile::TempDir) -> LifecycleConfig {
    LifecycleConfig {
        image_dir: dir.path().join("image"),
        state_dir: Some(dir.path().join("state")),
        exec_log: dir.path().join("exec.log"),
    }
}

/// A deadline no test reaches.
pub(crate) fn far_future() -> tokio::time::Instant {
    tokio::time::Instant::now() + Duration::from_secs(3600)
}

pub(crate) fn handler_name(name: &str) -> HandlerName {
    HandlerName::try_from(name).expect("handler name")
}

pub(crate) fn spec(argv: &[&str], timeout_ms: Option<u64>) -> ExecSpec {
    ExecSpec::from_wire(HandlerSpecWire {
        v: 1,
        argv: argv.iter().map(|arg| (*arg).to_owned()).collect(),
        timeout_ms,
    })
    .expect("exec spec")
}

pub(crate) fn request(reason: &str, disk: Disk, deadline_ms: u64) -> TriggerRequest {
    TriggerRequest {
        reason: ReasonToken::try_from(reason).expect("reason"),
        disk,
        budget: Duration::from_millis(deadline_ms),
    }
}

/// A built-in that reports `outcome` after `delay`.
pub(crate) struct FixedHandler {
    pub(crate) outcome: HandlerOutcome,
    pub(crate) delay: Duration,
}

impl FixedHandler {
    pub(crate) fn ok() -> Self {
        FixedHandler {
            outcome: HandlerOutcome::Ok,
            delay: Duration::ZERO,
        }
    }

    pub(crate) fn failed() -> Self {
        FixedHandler {
            outcome: HandlerOutcome::Failed,
            delay: Duration::ZERO,
        }
    }

    pub(crate) fn slow(delay: Duration) -> Self {
        FixedHandler {
            outcome: HandlerOutcome::Ok,
            delay,
        }
    }
}

impl PreStopHandler for FixedHandler {
    fn pre_stop<'a>(&'a self, _event: &'a PreStopEvent) -> HandlerFuture<'a> {
        Box::pin(async move {
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            self.outcome
        })
    }
}

/// A built-in that signals when it starts, then holds its trigger until released.
pub(crate) struct GateHandler {
    pub(crate) started: mpsc::Sender<()>,
    pub(crate) release: Arc<Notify>,
}

impl PreStopHandler for GateHandler {
    fn pre_stop<'a>(&'a self, _event: &'a PreStopEvent) -> HandlerFuture<'a> {
        Box::pin(async move {
            self.started.send(()).await.expect("test alive");
            self.release.notified().await;
            HandlerOutcome::Ok
        })
    }
}
