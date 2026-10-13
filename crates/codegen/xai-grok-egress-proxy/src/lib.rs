//! Hidden local HTTP/CONNECT proxy for exact website-origin policies.
//!
//! Starting this library does not confine child networking.
//! Callers must provide an independent platform boundary before treating proxy policy as enforcement.
//!
//! Per-call attribution uses per-call proxy credentials in the proxy URL's userinfo; see
//! [`credential`] for the mechanism and [`decider`] for the hold-and-ask seam.

mod connect;
pub mod credential;
pub mod decider;
mod error;
mod http;
mod ip;
mod metrics;
mod request;
mod server;
mod tls;

use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, broadcast, mpsc, watch};
use tokio::task::JoinHandle;
use xai_grok_sandbox::WebsitePolicy;
use xai_grok_sandbox::command::CommandTag;
use xai_grok_sandbox::command::grants::{Clock, SystemClock};

pub use crate::credential::{PROXY_USERNAME, ProxyCredential, ProxyToken};
pub use crate::decider::{
    BLOCKED_REQUEST_CAPACITY, BlockedRequest, DEFAULT_HOLD_TIMEOUT, DEFAULT_MAX_HELD,
    DEFAULT_MAX_HELD_PER_CALL, Decider, DeciderOutcome, Decision, DenySource, PolicyOnly, WouldBe,
};
pub use crate::error::ProxyError;
pub use crate::metrics::{ProxyMetricsSnapshot, ProxyOutcome};

use crate::credential::CredentialRegistry;
use crate::metrics::ProxyMetrics;

pub trait ProxyIo: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> ProxyIo for T {}
pub type BoxProxyIo = Box<dyn ProxyIo>;

/// Resolves all addresses for one normalized hostname and effective port.
#[async_trait]
pub trait Resolver: Send + Sync + 'static {
    async fn resolve(&self, hostname: &str, port: u16) -> io::Result<Vec<SocketAddr>>;
}

/// Connects only to addresses the proxy has already resolved and checked as public.
#[async_trait]
pub trait Connector: Send + Sync + 'static {
    async fn connect(&self, addresses: &[SocketAddr]) -> io::Result<BoxProxyIo>;
}

#[derive(Default)]
pub struct SystemResolver;

#[async_trait]
impl Resolver for SystemResolver {
    async fn resolve(&self, hostname: &str, port: u16) -> io::Result<Vec<SocketAddr>> {
        Ok(tokio::net::lookup_host((hostname, port)).await?.collect())
    }
}

#[derive(Default)]
pub struct TcpConnector;

#[async_trait]
impl Connector for TcpConnector {
    async fn connect(&self, addresses: &[SocketAddr]) -> io::Result<BoxProxyIo> {
        let mut last_error = None;
        for address in addresses {
            match TcpStream::connect(address).await {
                Ok(stream) => return Ok(Box::new(stream)),
                Err(error) => last_error = Some(error),
            }
        }
        Err(last_error.unwrap_or_else(|| io::Error::other("no resolved addresses")))
    }
}

pub struct EgressProxyOptions {
    pub max_connections: usize,
    pub max_header_bytes: usize,
    pub max_headers: usize,
    pub max_tls_client_hello_bytes: usize,
    pub request_timeout: Duration,
    pub dns_timeout: Duration,
    pub connect_timeout: Duration,
    pub tls_hello_timeout: Duration,
    pub drain_timeout: Duration,
    /// Deadline for a request parked on [`Decision::Ask`]; a distinct state from `request_timeout`.
    pub hold_timeout: Duration,
    /// Requests parked at once before the next `Ask` is answered 503.
    pub max_held: usize,
    /// Requests one credential may park at once; clamped to `max_held`, so lowering only
    /// `max_held` lowers this too.
    pub max_held_per_call: usize,
    /// Stamps [`BlockedRequest::deadline_unix`].
    pub clock: Arc<dyn Clock>,
    pub resolver: Arc<dyn Resolver>,
    pub connector: Arc<dyn Connector>,
}

impl Default for EgressProxyOptions {
    fn default() -> Self {
        EgressProxyOptions {
            max_connections: 64,
            max_header_bytes: 32 * 1024,
            max_headers: 64,
            max_tls_client_hello_bytes: 64 * 1024,
            request_timeout: Duration::from_secs(10),
            dns_timeout: Duration::from_secs(5),
            connect_timeout: Duration::from_secs(10),
            tls_hello_timeout: Duration::from_secs(5),
            drain_timeout: Duration::from_secs(5),
            hold_timeout: DEFAULT_HOLD_TIMEOUT,
            max_held: DEFAULT_MAX_HELD,
            max_held_per_call: DEFAULT_MAX_HELD_PER_CALL,
            clock: Arc::new(SystemClock),
            resolver: Arc::new(SystemResolver),
            connector: Arc::new(TcpConnector),
        }
    }
}

/// Everything a connection task reads; one per proxy, shared by the handle.
pub(crate) struct ProxyState {
    pub(crate) policy: WebsitePolicy,
    pub(crate) decider: Arc<dyn Decider>,
    pub(crate) options: EgressProxyOptions,
    pub(crate) metrics: ProxyMetrics,
    pub(crate) credentials: CredentialRegistry,
    pub(crate) holds: Semaphore,
    pub(crate) hold_budget: decider::HoldBudget,
    pub(crate) blocked: broadcast::Sender<BlockedRequest>,
    /// `true` once the serve loop is stopping: every parked hold answers itself with a deny, so
    /// a drain never waits out a card.
    pub(crate) stopping: watch::Sender<bool>,
}

pub struct EgressProxy;

impl EgressProxy {
    /// Binds a loopback listener that is ready on return; every origin the supplied policy does not allow is denied.
    pub async fn start(
        policy: WebsitePolicy,
        options: EgressProxyOptions,
    ) -> Result<EgressProxyHandle, ProxyError> {
        EgressProxy::start_with(policy, Arc::new(PolicyOnly), options).await
    }

    /// Like [`EgressProxy::start`], with `decider` consulted for every origin the policy allows.
    pub async fn start_with(
        policy: WebsitePolicy,
        decider: Arc<dyn Decider>,
        options: EgressProxyOptions,
    ) -> Result<EgressProxyHandle, ProxyError> {
        if options.max_connections == 0
            || options.max_headers == 0
            || options.max_header_bytes < 4
            || options.max_tls_client_hello_bytes < 5
            || options.max_held == 0
            || options.max_held_per_call == 0
        {
            return Err(ProxyError::InvalidOptions);
        }
        let tcp = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let address = tcp.local_addr()?;
        let drain_timeout = options.drain_timeout;
        let state = Arc::new(ProxyState {
            policy,
            decider,
            holds: Semaphore::new(options.max_held),
            hold_budget: decider::HoldBudget::new(options.max_held_per_call.min(options.max_held)),
            options,
            metrics: ProxyMetrics::default(),
            credentials: CredentialRegistry::new(credential::ProxyToken::generate()?),
            blocked: decider::blocked_channel(),
            stopping: watch::Sender::new(false),
        });
        #[cfg(test)]
        let task_observer = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (shutdown_tx, shutdown_rx) = mpsc::channel(1);
        let task = tokio::spawn(server::serve(
            tcp,
            state.clone(),
            #[cfg(test)]
            task_observer.clone(),
            shutdown_rx,
        ));
        Ok(EgressProxyHandle {
            address,
            state,
            shutdown: Some(shutdown_tx),
            task: Some(task),
            #[cfg(test)]
            task_observer,
            drain_timeout,
        })
    }
}

#[derive(Clone, Copy)]
pub(crate) enum ShutdownMode {
    Drain,
    Abort,
}

pub struct EgressProxyHandle {
    address: SocketAddr,
    state: Arc<ProxyState>,
    shutdown: Option<mpsc::Sender<ShutdownMode>>,
    task: Option<JoinHandle<()>>,
    #[cfg(test)]
    task_observer: Arc<std::sync::atomic::AtomicUsize>,
    drain_timeout: Duration,
}

impl EgressProxyHandle {
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// Returns the exact header value required on every HTTP and CONNECT request.
    pub fn proxy_authorization(&self) -> String {
        self.state.credentials.session_authorization()
    }

    /// A fresh credential for one command; a second mint for the same tag replaces the first.
    ///
    /// # Errors
    /// The host gave no entropy for the token: an `io::Error` as the RNG reported it, not a
    /// proxy-side classification (nothing about the client's connection is wrong).
    pub fn mint_call_credential(&self, tag: &CommandTag) -> io::Result<ProxyCredential> {
        self.state.credentials.mint(tag)
    }

    /// Whether a credential existed; any later request presenting it is answered 407.
    pub fn revoke_call_credential(&self, tag: &CommandTag) -> bool {
        self.state.credentials.revoke(tag)
    }

    /// [`Self::revoke_call_credential`] for one credential: whether it was still its tag's live
    /// one. A later mint for the same tag is left as it is.
    pub fn revoke_credential(&self, credential: &ProxyCredential) -> bool {
        self.state.credentials.revoke_exact(credential)
    }

    /// Every decider result worth a card or a log line, from this subscription onward.
    pub fn blocked_requests(&self) -> broadcast::Receiver<BlockedRequest> {
        self.state.blocked.subscribe()
    }

    pub fn metrics(&self) -> ProxyMetricsSnapshot {
        self.state.metrics.snapshot()
    }

    #[cfg(test)]
    pub(crate) fn max_held_per_call(&self) -> usize {
        self.state.hold_budget.per_call()
    }

    pub fn is_running(&self) -> bool {
        self.task.as_ref().is_some_and(|task| !task.is_finished())
    }

    pub async fn abort(&mut self) -> Result<(), ProxyError> {
        if let Some(shutdown) = self.shutdown.take()
            && shutdown.send(ShutdownMode::Abort).await.is_err()
        {
            return self.await_server().await;
        }
        self.await_server().await
    }

    async fn await_server(&mut self) -> Result<(), ProxyError> {
        if let Some(task) = self.task.take() {
            match task.await {
                Ok(()) => {}
                Err(error) if error.is_cancelled() => {}
                Err(error) => return Err(io::Error::other(error).into()),
            }
        }
        Ok(())
    }

    #[cfg(test)]
    fn tracked_tasks(&self) -> usize {
        self.task_observer
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    pub async fn shutdown(mut self) -> Result<(), ProxyError> {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(ShutdownMode::Drain).await;
        }
        if let Some(mut task) = self.task.take()
            && tokio::time::timeout(self.drain_timeout, &mut task)
                .await
                .is_err()
        {
            task.abort();
            let _ = task.await;
            return Err(ProxyError::ShutdownTimeout);
        }
        Ok(())
    }
}

impl Drop for EgressProxyHandle {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.try_send(ShutdownMode::Abort);
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

pub(crate) async fn resolve_public(
    origin: &xai_grok_sandbox::WebsiteOrigin,
    options: &EgressProxyOptions,
) -> Result<Vec<SocketAddr>, ProxyError> {
    let addresses = tokio::time::timeout(
        options.dns_timeout,
        options.resolver.resolve(origin.hostname(), origin.port()),
    )
    .await
    .map_err(|_| ProxyError::Timeout)?
    .map_err(|_| ProxyError::Dns)?;
    if addresses.is_empty() {
        return Err(ProxyError::Dns);
    }
    if addresses
        .iter()
        .any(|address| !ip::is_public_ip(address.ip()))
    {
        return Err(ProxyError::AddressDenied);
    }
    Ok(addresses)
}

pub(crate) async fn resolve_and_connect(
    origin: &xai_grok_sandbox::WebsiteOrigin,
    options: &EgressProxyOptions,
) -> Result<BoxProxyIo, ProxyError> {
    let addresses = resolve_public(origin, options).await?;
    tokio::time::timeout(
        options.connect_timeout,
        options.connector.connect(&addresses),
    )
    .await
    .map_err(|_| ProxyError::Timeout)?
    .map_err(|_| ProxyError::Connect)
}

#[cfg(test)]
mod tests;
