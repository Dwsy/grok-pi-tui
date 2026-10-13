use std::sync::Arc;

use tokio::net::TcpListener;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};
use tokio::task::JoinSet;
use xai_grok_sandbox::command::CommandTag;

use crate::error::{ConnectionError, ProxyError, write_error};
use crate::metrics::ProxyOutcome;
use crate::request::{ParsedRequest, read_head};
use crate::{ProxyIo, ProxyState, ShutdownMode};

pub(crate) async fn serve(
    listener: TcpListener,
    state: Arc<ProxyState>,
    #[cfg(test)] task_observer: Arc<std::sync::atomic::AtomicUsize>,
    mut shutdown: mpsc::Receiver<ShutdownMode>,
) {
    let semaphore = Arc::new(Semaphore::new(state.options.max_connections));
    let mut tasks = JoinSet::new();
    #[cfg(test)]
    observe_tasks(&tasks, &task_observer);
    let mode = loop {
        tokio::select! {
            biased;
            mode = shutdown.recv() => break mode.unwrap_or(ShutdownMode::Abort),
            Some(_) = tasks.join_next(), if !tasks.is_empty() => {
                #[cfg(test)]
                observe_tasks(&tasks, &task_observer);
            }
            accepted = listener.accept() => {
                let Ok((mut stream, peer)) = accepted else { break ShutdownMode::Abort };
                if !peer.ip().is_loopback() {
                    continue;
                }
                let Some(permit) = admit_connection(&semaphore, &state, &mut stream).await else {
                    continue;
                };
                let state = state.clone();
                tasks.spawn(async move {
                    let _permit = permit;
                    serve_connection(&mut stream, &state).await;
                });
                #[cfg(test)]
                observe_tasks(&tasks, &task_observer);
            }
        }
    };
    drop(listener);
    // Fail closed before anything is waited for: every parked connection answers itself with a
    // deny, and the decider withdraws what it raised for them, so a drain never waits out a hold
    state.stopping.send_replace(true);
    state.decider.stopping();
    if matches!(mode, ShutdownMode::Abort) {
        tasks.shutdown().await;
        #[cfg(test)]
        observe_tasks(&tasks, &task_observer);
        return;
    }
    while tasks.join_next().await.is_some() {
        #[cfg(test)]
        observe_tasks(&tasks, &task_observer);
    }
}

async fn admit_connection<S: ProxyIo>(
    semaphore: &Arc<Semaphore>,
    state: &ProxyState,
    stream: &mut S,
) -> Option<OwnedSemaphorePermit> {
    match semaphore.clone().try_acquire_owned() {
        Ok(permit) => Some(permit),
        Err(_) => {
            state.metrics.record(ProxyOutcome::Overloaded);
            // The client may already be gone; there is nothing further to do with the failure.
            let _ = write_error(stream, &ProxyError::Overloaded).await;
            None
        }
    }
}

#[cfg(test)]
fn observe_tasks(tasks: &JoinSet<()>, observer: &std::sync::atomic::AtomicUsize) {
    observer.store(tasks.len(), std::sync::atomic::Ordering::Relaxed);
}

async fn serve_connection<S: ProxyIo>(client: &mut S, state: &ProxyState) {
    match handle_connection(client, state).await {
        Ok(()) => {}
        Err(ConnectionError::Precommit(error)) => {
            state.metrics.record(error.outcome());
            let _ = write_error(client, &error).await;
        }
        Err(error @ ConnectionError::Committed(_)) => {
            state.metrics.record(error.outcome());
        }
    }
}

async fn handle_connection<S: ProxyIo>(
    client: &mut S,
    state: &ProxyState,
) -> Result<(), ConnectionError> {
    let (head, buffered) = tokio::time::timeout(
        state.options.request_timeout,
        read_head(client, state.options.max_header_bytes),
    )
    .await
    .map_err(|_| ConnectionError::Precommit(ProxyError::Timeout))?
    .map_err(ConnectionError::Precommit)?;
    let request = ParsedRequest::parse(&head, state.options.max_headers)
        .map_err(ConnectionError::Precommit)?;
    let call = authenticate(&request, state).map_err(ConnectionError::Precommit)?;
    if request.method.eq_ignore_ascii_case("CONNECT") {
        return crate::connect::handle(client, request, buffered, state, call.as_ref()).await;
    }
    if request.method.eq_ignore_ascii_case("TRACE") {
        return Err(ConnectionError::Precommit(ProxyError::Malformed));
    }
    crate::http::handle(client, request, buffered, state, call.as_ref()).await
}

fn authenticate(
    request: &ParsedRequest,
    state: &ProxyState,
) -> Result<Option<CommandTag>, ProxyError> {
    let authenticated = request
        .single_header("proxy-authorization")?
        .ok_or(ProxyError::Authentication)
        .and_then(|header| state.credentials.authenticate(header));
    match authenticated {
        Err(ProxyError::Authentication) if state.decider.admits_unauthenticated() => Ok(None),
        other => other,
    }
}
