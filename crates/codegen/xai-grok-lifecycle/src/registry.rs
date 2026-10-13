//! Runtime-registered exec handlers: an in-memory table, optionally backed by a [`RegistrationStore`].
//!
//! A write persists first and publishes second, so `200` means the registration survives a restart (when a store
//! exists) and a failed write changes nothing. A write whose record outlives [`PERSIST_TIMEOUT`] answers with an error
//! but still finishes in order: it lands in both the store and the table or in neither, before the next write starts.

use std::collections::BTreeMap;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::OwnedMutexGuard;
use tokio::time::Instant;

use crate::exec_spec::ExecSpec;
use crate::persist::RegistrationStore;
use crate::token::HandlerName;

/// Cap on runtime-registered handlers; replacing an existing name never counts against it.
pub(crate) const MAX_REGISTERED_HANDLERS: usize = 16;

/// How long a PUT or DELETE waits for the write lock and its record write before answering with an error.
const PERSIST_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug)]
pub(crate) enum RegistryError {
    Full,
    Persist(io::Error),
}

#[derive(Debug)]
pub(crate) struct HandlerRegistry {
    handlers: Arc<parking_lot::Mutex<BTreeMap<HandlerName, ExecSpec>>>,
    /// Serializes PUT and DELETE. A persisting write moves the guard into its blocking task, so it is held until the
    /// store and the table have both changed, even after the caller stopped waiting.
    writes: Arc<tokio::sync::Mutex<()>>,
    store: Option<Arc<RegistrationStore>>,
    persist_timeout: Duration,
}

impl HandlerRegistry {
    /// Reads the store synchronously; `None` keeps registrations in memory only.
    pub(crate) fn load_sync(store: Option<RegistrationStore>) -> Self {
        let handlers = store
            .as_ref()
            .map(RegistrationStore::load_sync)
            .unwrap_or_default();
        HandlerRegistry {
            handlers: Arc::new(parking_lot::Mutex::new(handlers)),
            writes: Arc::new(tokio::sync::Mutex::new(())),
            store: store.map(Arc::new),
            persist_timeout: PERSIST_TIMEOUT,
        }
    }

    /// Point-in-time copy, sorted by name.
    pub(crate) fn snapshot(&self) -> Vec<(HandlerName, ExecSpec)> {
        self.handlers
            .lock()
            .iter()
            .map(|(name, spec)| (name.clone(), spec.clone()))
            .collect()
    }

    /// Inserts or replaces `name`.
    pub(crate) async fn put(&self, name: HandlerName, spec: ExecSpec) -> Result<(), RegistryError> {
        let deadline = Instant::now() + self.persist_timeout;
        let writes = self
            .lock_writes(deadline)
            .await
            .map_err(RegistryError::Persist)?;
        {
            let handlers = self.handlers.lock();
            if !handlers.contains_key(&name) && handlers.len() >= MAX_REGISTERED_HANDLERS {
                return Err(RegistryError::Full);
            }
        }
        let Some(store) = self.store.clone() else {
            self.handlers.lock().insert(name, spec);
            return Ok(());
        };
        let handlers = Arc::clone(&self.handlers);
        run_blocking(deadline, move || {
            let _writes = writes;
            store.save_sync(&name, &spec)?;
            handlers.lock().insert(name, spec);
            Ok(())
        })
        .await
        .map_err(RegistryError::Persist)
    }

    /// Succeeds whether or not `name` was registered.
    pub(crate) async fn delete(&self, name: &HandlerName) -> io::Result<()> {
        let deadline = Instant::now() + self.persist_timeout;
        let writes = self.lock_writes(deadline).await?;
        let Some(store) = self.store.clone() else {
            self.handlers.lock().remove(name);
            return Ok(());
        };
        let handlers = Arc::clone(&self.handlers);
        let name = name.clone();
        run_blocking(deadline, move || {
            let _writes = writes;
            store.remove_sync(&name)?;
            handlers.lock().remove(&name);
            Ok(())
        })
        .await
    }

    async fn lock_writes(&self, deadline: Instant) -> io::Result<OwnedMutexGuard<()>> {
        tokio::time::timeout_at(deadline, Arc::clone(&self.writes).lock_owned())
            .await
            .map_err(|_elapsed| persist_timed_out())
    }
}

/// Waits for `work` on the blocking pool until `deadline`; past it `work` still runs to completion.
async fn run_blocking(
    deadline: Instant,
    work: impl FnOnce() -> io::Result<()> + Send + 'static,
) -> io::Result<()> {
    match tokio::time::timeout_at(deadline, tokio::task::spawn_blocking(work)).await {
        Ok(Ok(result)) => result,
        Ok(Err(join)) => Err(io::Error::other(join)),
        Err(_elapsed) => Err(persist_timed_out()),
    }
}

fn persist_timed_out() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "lifecycle state write timed out")
}

#[cfg(test)]
#[path = "registry_tests.rs"]
mod tests;
