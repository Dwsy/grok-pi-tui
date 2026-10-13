//! The broker handle, its configuration, and trigger orchestration.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use tokio::time::Instant;
use xai_tty_utils::ProcessScope;

use crate::error::{LifecycleError, Result};
use crate::event::{PreStopEvent, PreStopHandler};
use crate::exec::ExecContext;
use crate::exec_log::ExecLog;
use crate::image::load_image_handlers;
use crate::persist::{BootId, RegistrationStore};
use crate::registry::HandlerRegistry;
use crate::token::HandlerName;
use crate::trigger::{
    HandlerKind, TableEntry, TriggerReport, TriggerRequest, duration_ms, fan_out, handler_table,
    verdict,
};

/// Where images bake their handler manifests (`<name>.json`, same body as a PUT).
pub const DEFAULT_IMAGE_HANDLER_DIR: &str = "/usr/share/grok/lifecycle/handlers.d";

/// Cap on in-process handlers; with the image and registered caps it bounds the trigger fan-out.
pub(crate) const MAX_BUILTIN_HANDLERS: usize = 16;

/// Bound on reading the image manifest dir.
pub(crate) const IMAGE_SCAN_TIMEOUT: Duration = Duration::from_secs(2);

/// Where the broker keeps its files.
#[derive(Debug, Clone)]
pub struct LifecycleConfig {
    /// Image handler manifests, read at trigger time. A missing dir means no image handlers.
    pub image_dir: PathBuf,
    /// Durable home of runtime registrations; `None` keeps them in memory only. Ignored off Linux, where there is no
    /// kernel boot id to tell a restart from a restore.
    pub state_dir: Option<PathBuf>,
    /// The capped log file exec handler output goes to.
    pub exec_log: PathBuf,
}

/// The pre-stop broker. Cheap to clone; every clone shares one handler registry and one trigger lock.
#[derive(Clone)]
pub struct LifecycleBroker {
    inner: Arc<Inner>,
}

struct Inner {
    image_dir: PathBuf,
    builtins: parking_lot::RwLock<BTreeMap<HandlerName, Arc<dyn PreStopHandler>>>,
    registry: HandlerRegistry,
    exec_log: Arc<ExecLog>,
    /// Owns every exec handler's process group; dropping the last broker handle kills whatever is still running.
    processes: ProcessScope,
    /// Held for a whole trigger; `try_lock` turns a concurrent trigger away instead of queueing it.
    trigger_lock: tokio::sync::Mutex<()>,
}

/// Another trigger is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TriggerBusy;

impl LifecycleBroker {
    /// Loads the registrations persisted under this kernel boot and deletes those from other boots. The read is
    /// synchronous and small; call this once at startup, before the diagnostics server binds.
    pub fn new(config: LifecycleConfig) -> Self {
        LifecycleBroker::with_boot_id(config, BootId::current())
    }

    /// `boot_id` is `None` when registrations must stay in memory.
    pub(crate) fn with_boot_id(config: LifecycleConfig, boot_id: Option<BootId>) -> Self {
        let store = config
            .state_dir
            .zip(boot_id)
            .map(|(dir, boot_id)| RegistrationStore::new(dir, boot_id));
        LifecycleBroker {
            inner: Arc::new(Inner {
                image_dir: config.image_dir,
                builtins: parking_lot::RwLock::new(BTreeMap::new()),
                registry: HandlerRegistry::load_sync(store),
                exec_log: Arc::new(ExecLog::new(config.exec_log)),
                processes: ProcessScope::new(),
                trigger_lock: tokio::sync::Mutex::new(()),
            }),
        }
    }

    /// Adds an in-process handler for the life of the broker. Its name is then reserved: PUT and DELETE on it get
    /// `409`, and an image or registered handler with the same name is shadowed.
    ///
    /// # Errors
    ///
    /// [`LifecycleError::DuplicateBuiltin`] when `name` is already a built-in, and
    /// [`LifecycleError::TooManyBuiltins`] past the built-in cap.
    pub fn register_builtin(
        &self,
        name: HandlerName,
        handler: Arc<dyn PreStopHandler>,
    ) -> Result<()> {
        let mut builtins = self.inner.builtins.write();
        if builtins.contains_key(&name) {
            return Err(LifecycleError::DuplicateBuiltin { name });
        }
        if builtins.len() >= MAX_BUILTIN_HANDLERS {
            return Err(LifecycleError::TooManyBuiltins);
        }
        builtins.insert(name, handler);
        Ok(())
    }

    /// The four contract routes: `POST /lifecycle/pre-stop`, `GET /lifecycle/handlers`, and
    /// `PUT`/`DELETE /lifecycle/handlers/{name}`.
    pub fn router(&self) -> Router {
        crate::routes::router(self.clone())
    }

    #[tracing::instrument(skip_all, fields(reason = %request.reason, disk = %request.disk))]
    pub(crate) async fn trigger(
        &self,
        request: TriggerRequest,
        received: Instant,
    ) -> std::result::Result<TriggerReport, TriggerBusy> {
        let Ok(_running) = self.inner.trigger_lock.try_lock() else {
            return Err(TriggerBusy);
        };
        let deadline = received + request.budget;
        let event = PreStopEvent::new(request.reason, request.disk, deadline);
        let table = self
            .handler_table((Instant::now() + IMAGE_SCAN_TIMEOUT).min(deadline))
            .await;
        let has_exec = table
            .iter()
            .any(|entry| matches!(entry.kind, HandlerKind::Exec(_)));
        if has_exec {
            self.inner.exec_log.start_trigger(deadline).await;
        }
        let ctx = ExecContext {
            log: Arc::clone(&self.inner.exec_log),
            processes: self.inner.processes.clone(),
        };
        let handlers = fan_out(table, &event, &ctx).await;
        if has_exec {
            self.inner.exec_log.finish_trigger();
        }
        let report = TriggerReport {
            verdict: verdict(&handlers),
            elapsed: received.elapsed(),
            handlers,
        };
        tracing::info!(
            verdict = ?report.verdict,
            handlers = report.handlers.len(),
            elapsed_ms = duration_ms(report.elapsed),
            "pre-stop trigger finished"
        );
        Ok(report)
    }

    /// Every handler a trigger started now would run, giving up on image manifests at `image_scan_until`.
    pub(crate) async fn handler_table(&self, image_scan_until: Instant) -> Vec<TableEntry> {
        let builtins: Vec<_> = self
            .inner
            .builtins
            .read()
            .iter()
            .map(|(name, handler)| (name.clone(), Arc::clone(handler)))
            .collect();
        let images = load_image_handlers(self.inner.image_dir.clone(), image_scan_until).await;
        handler_table(builtins, images, self.inner.registry.snapshot())
    }

    pub(crate) fn is_builtin(&self, name: &HandlerName) -> bool {
        self.inner.builtins.read().contains_key(name)
    }

    pub(crate) fn image_dir(&self) -> &Path {
        &self.inner.image_dir
    }

    pub(crate) fn registry(&self) -> &HandlerRegistry {
        &self.inner.registry
    }
}
