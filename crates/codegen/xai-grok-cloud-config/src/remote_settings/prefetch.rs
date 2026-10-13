use indexmap::IndexMap;
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::{ModelsCacheScope, ModelsPrefetch, commit_catalog_fetch};

#[must_use]
pub struct InitialModelsLoad<E>(tokio::sync::oneshot::Receiver<Option<IndexMap<String, E>>>);

impl<E> InitialModelsLoad<E> {
    pub async fn join(
        self,
        cancel: &tokio_util::sync::CancellationToken,
        timeout: std::time::Duration,
    ) -> Option<IndexMap<String, E>> {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => None,
            received = tokio::time::timeout(timeout, self.0) => match received {
                Ok(Ok(models)) => models,
                Ok(Err(_sender_dropped)) => None,
                Err(_elapsed) => {
                    tracing::info!(
                        "initial models prefetch timed out; catalog freeze uses bundled defaults"
                    );
                    None
                }
            },
        }
    }
}

/// Fetch, then commit. `live`'s fetch policy is read after the fetch so a mid-fetch disable abandons the result.
pub fn run_catalog_prefetch<E>(
    cancel: &tokio_util::sync::CancellationToken,
    expected: &ModelsCacheScope,
    fetch: impl FnOnce() -> ModelsPrefetch<E>,
    live: impl FnOnce() -> (ModelsCacheScope, bool),
) -> Option<IndexMap<String, E>>
where
    E: Serialize + DeserializeOwned + Clone,
{
    if cancel.is_cancelled() {
        return None;
    }
    commit_catalog_fetch(expected, fetch(), live)
}

/// `work` must not run on a tokio worker because its fetch builds its own runtime.
/// `work` still writes the cache after the receiver is dropped.
fn spawn_catalog_thread<E>(
    name: &str,
    cancel: tokio_util::sync::CancellationToken,
    work: impl FnOnce(&tokio_util::sync::CancellationToken) -> Option<IndexMap<String, E>>
    + Send
    + 'static,
    deliver: impl FnOnce(Option<IndexMap<String, E>>) + Send + 'static,
) -> Option<()>
where
    E: Send + 'static,
{
    std::thread::Builder::new()
        .name(name.into())
        .spawn(move || deliver(work(&cancel)))
        .ok()
        .map(|_| ())
}

pub fn start_catalog_load<E>(
    name: &str,
    cancel: tokio_util::sync::CancellationToken,
    work: impl FnOnce(&tokio_util::sync::CancellationToken) -> Option<IndexMap<String, E>>
    + Send
    + 'static,
) -> Option<InitialModelsLoad<E>>
where
    E: Send + 'static,
{
    let (tx, rx) = tokio::sync::oneshot::channel();
    spawn_catalog_thread(name, cancel, work, move |models| {
        let _ = tx.send(models);
    })?;
    Some(InitialModelsLoad(rx))
}

pub fn fetch_catalog_blocking<E>(
    name: &str,
    cancel: &tokio_util::sync::CancellationToken,
    timeout: std::time::Duration,
    poll: std::time::Duration,
    work: impl FnOnce(&tokio_util::sync::CancellationToken) -> Option<IndexMap<String, E>>
    + Send
    + 'static,
) -> Option<IndexMap<String, E>>
where
    E: Send + 'static,
{
    let (tx, rx) = std::sync::mpsc::channel();
    spawn_catalog_thread(name, cancel.clone(), work, move |models| {
        let _ = tx.send(models);
    })?;
    wait_catalog_prefetch(cancel, timeout, poll, &rx)
}

fn wait_catalog_prefetch<E>(
    cancel: &tokio_util::sync::CancellationToken,
    timeout: std::time::Duration,
    poll: std::time::Duration,
    recv: &std::sync::mpsc::Receiver<Option<IndexMap<String, E>>>,
) -> Option<IndexMap<String, E>> {
    let started = std::time::Instant::now();
    loop {
        if cancel.is_cancelled() {
            return None;
        }

        let remaining = timeout.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            tracing::info!(
                "initial models prefetch timed out; catalog freeze uses bundled defaults"
            );
            return None;
        }

        match recv.recv_timeout(remaining.min(poll)) {
            Ok(models) => return models,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return None,
        }
    }
}

#[cfg(test)]
#[path = "prefetch_tests.rs"]
mod tests;
