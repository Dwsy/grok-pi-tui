//! The network side of a [`WorkspaceSandbox`]: one egress proxy per served folder, started and
//! stopped by [`WorkspaceSandbox::sync_network`] as the mode says; the decider that turns an origin
//! nobody allowed into a held connection and a card (asked mid-command through the
//! [`ViolationSink`], settled with `settle_hold`); and the per-call credential the launch hook
//! puts in `HTTP_PROXY`. The decider reads the folder's rows, the call's mode and its epoch through
//! the sandbox at every decision; nothing here copies a row. The credential is attribution, not
//! isolation (see `xai_grok_egress_proxy::credential`).

use std::ffi::OsString;
use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock, Weak};

use async_trait::async_trait;
use tokio::sync::broadcast;
#[cfg(test)]
use xai_grok_egress_proxy::Decider;
use xai_grok_egress_proxy::{
    BlockedRequest, DeciderOutcome, DenySource, EgressProxy, EgressProxyHandle, EgressProxyOptions,
    ProxyCredential, ProxyError, WouldBe,
};
use xai_grok_paths::AbsPathBuf;
use xai_grok_sandbox::command::backend::{CommandTag, OriginalArgv};
use xai_grok_sandbox::command::policy::ProxyEndpoint;
use xai_grok_sandbox::command::{
    Blocked, CallId, Grant, Replay, SandboxMode, Violation, WouldVerdict,
};
use xai_grok_sandbox::{WebsiteAction, WebsitePolicy};

use super::calls::Released;
use super::{OwnedTasks, ProxyEndpointOwned, WorkspaceSandbox, WorkspaceSandboxError};
use crate::permission::{
    GrantView, HoldAnswer, PermissionHookTransport, SandboxNetworkDecider,
    SandboxNetworkDeciderConfig, SettleContext, ViolationSettlement, ViolationSink,
    WebFetchDomainFile, WebFetchDomains, settle_violation,
};

/// Where the folder's proxy listens (`sandbox.status`, the tests).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetworkInfo {
    pub address: SocketAddr,
}

#[derive(Debug, thiserror::Error)]
pub enum NetworkStartError {
    #[error("the egress proxy is already running for this folder")]
    AlreadyStarted,
    #[error("workspace root {} is not absolute", .0.display())]
    RelativeRoot(PathBuf),
    #[error(transparent)]
    Proxy(#[from] ProxyError),
    #[error("the folder is no longer served")]
    Closed,
}

/// The folder's running proxy; it owns the decider it was started with, so the pair goes
/// together. The handle sits behind a lock so `stop_network` can always take it and await the
/// shutdown, whether or not a reader still holds the side mid-use (a reader that finds the slot
/// empty is racing the stop and gets no credential — the proxy is going).
pub(super) struct NetworkSide {
    address: SocketAddr,
    /// The folder served: what a call's end, which has `&self` only, upgrades for the sync it
    /// schedules ([`WorkspaceSandbox::resync_unless_enforced`]).
    sandbox: Weak<WorkspaceSandbox>,
    proxy: parking_lot::Mutex<Option<EgressProxyHandle>>,
    /// The blocked-request feed: stopped with the folder's network, and on drop. A hold's
    /// settlement is the sandbox's ([`NetworkViolationSink`]): it ends with its hold.
    tasks: Arc<OwnedTasks>,
    decider: Arc<SandboxNetworkDecider>,
    /// The folder's `permission.toml` (the web-fetch domain sets the decider shares with the
    /// web-fetch tool), opened with the proxy and gone with it.
    web_fetch: tokio::sync::Mutex<WebFetchDomainFile>,
}

impl NetworkSide {
    fn new(
        sandbox: Weak<WorkspaceSandbox>,
        proxy: EgressProxyHandle,
        tasks: Arc<OwnedTasks>,
        decider: Arc<SandboxNetworkDecider>,
        web_fetch: WebFetchDomainFile,
    ) -> NetworkSide {
        NetworkSide {
            address: proxy.address(),
            sandbox,
            proxy: parking_lot::Mutex::new(Some(proxy)),
            tasks,
            decider,
            web_fetch: tokio::sync::Mutex::new(web_fetch),
        }
    }

    /// Whether the proxy's accept loop still runs (it gives up on an accept error).
    fn is_running(&self) -> bool {
        self.with_proxy(EgressProxyHandle::is_running)
            .unwrap_or(false)
    }

    fn endpoint(&self) -> ProxyEndpointOwned {
        ProxyEndpointOwned {
            port: self.address.port(),
        }
    }

    /// The listener the kernel policy lets a command reach.
    pub(super) fn policy_endpoint(&self) -> ProxyEndpoint {
        ProxyEndpoint {
            port: self.address.port(),
        }
    }

    fn with_proxy<T>(&self, read: impl FnOnce(&EgressProxyHandle) -> T) -> Option<T> {
        self.proxy.lock().as_ref().map(read)
    }

    /// The web-fetch domain sets as `permission.toml` has them now (re-read when it changed).
    async fn web_fetch_domains(&self) -> WebFetchDomains {
        self.web_fetch.lock().await.current().await
    }

    /// A credential minted for `tag` on this side, armed: dropped before the spawn's record
    /// holds it, it is revoked. `None` with the handle taken out (the stop is under way).
    ///
    /// # Errors
    /// The proxy runs but minted nothing (the token source failed).
    fn mint(self: &Arc<Self>, tag: &CommandTag) -> io::Result<Option<MintedCredential>> {
        let Some(minted) = self.with_proxy(|proxy| proxy.mint_call_credential(tag)) else {
            return Ok(None);
        };
        Ok(Some(MintedCredential {
            side: self.clone(),
            credential: minted?,
            armed: true,
        }))
    }
}

/// A credential minted for one spawn, revoked on the side that minted it when dropped still
/// armed: every exit from `prepare` but the one that kept the spawn's record lets go of it, so no
/// token outlives a spawn that never ran. Revoked as the one credential it is, so a guard dropped
/// late never takes a replay's newer token with it.
#[must_use = "a credential nobody keeps is revoked on drop"]
pub(super) struct MintedCredential {
    side: Arc<NetworkSide>,
    credential: ProxyCredential,
    armed: bool,
}

impl MintedCredential {
    /// `http://grok:<token>@<address>`, for the spawn's `HTTP_PROXY`.
    pub(super) fn proxy_url(&self) -> String {
        self.credential.proxy_url(self.side.address)
    }

    /// The spawn's record holds the credential now: `finish` or the call's release revokes it.
    pub(super) fn keep(mut self) {
        self.armed = false;
    }
}

impl Drop for MintedCredential {
    fn drop(&mut self) {
        if self.armed {
            self.side
                .with_proxy(|proxy| proxy.revoke_credential(&self.credential));
        }
    }
}

impl WorkspaceSandbox {
    /// The daemon's serve step for one folder, as one call:
    /// open the sandbox and bring its proxy in line with the folder's mode. Never
    /// fails — a proxy that does not bind is logged and the folder is served without one
    /// (`enforce` then runs with the network off, `observe` unproxied). The daemon's exposure
    /// and the daemon-level pins call this, so what the tests prove about "serving a folder" is
    /// what the daemon does.
    pub async fn serve(config: super::WorkspaceSandboxConfig) -> Arc<WorkspaceSandbox> {
        let sandbox = Arc::new(WorkspaceSandbox::open(config).await);
        match sandbox.sync_network().await {
            Ok(Some(network)) => tracing::info!(
                root = %sandbox.root.display(),
                address = %network.address,
                "egress proxy started for the folder"
            ),
            Ok(None) => {}
            Err(error) => tracing::warn!(
                root = %sandbox.root.display(),
                %error,
                "the folder's egress proxy did not start; sandboxed commands have no network"
            ),
        }
        sandbox
    }

    /// Bring the folder's proxy in line with its mode (the owner drives the
    /// proxy's lifecycle from the mode, however the mode moved). Under `enforce` a proxy runs —
    /// the one already listening is kept, else one is started on a loopback port with its
    /// default limits. Under `observe` no command is pointed at a proxy (observe watches the
    /// file system only; observing the network is a follow-up): a running one stays only for
    /// the commands still running under `enforce`, decided as they were, and is stopped by the
    /// sync the last of them schedules as it ends ([`WorkspaceSandbox::resync_unless_enforced`]).
    /// Under `off` a running proxy is stopped at once and its listener closed (a command still
    /// running loses the one it had: its held connections are
    /// refused with the stop). The daemon calls this when it serves the folder, after
    /// `sandbox.mode.set`, and before each shell call it dispatches — so a mode flipped by
    /// editing `workspaced.toml` starts or stops the proxy exactly as the verb does, on the next
    /// command. A mode change releases nothing else: a command already running keeps the mode
    /// it started under. Idempotent and cheap when nothing changed: under `off` with no proxy it
    /// is one uncontended lock and a mode read. A mode that is not `off` engages the folder first
    /// ([`WorkspaceSandbox::engage_unless_off`]): the proxy never runs for a folder that is not
    /// engaged, and its decider's rows are the engaged store's.
    ///
    /// # Errors
    /// The proxy's bind error; the folder is served either way.
    pub async fn sync_network(self: &Arc<Self>) -> Result<Option<NetworkInfo>, NetworkStartError> {
        let lifecycle = self.network_lifecycle.lock().await;
        if lifecycle.closed {
            return Ok(None);
        }
        // Engaged, the sync is the one read of the layers the proxy's mode follows
        let mode = match self.engaged.get() {
            Some(_) => self.mode(),
            None => self.engage_unless_off().await,
        };
        if !mode.is_wrapped() {
            if self.network.load().is_none() {
                return Ok(None);
            }
            if mode == SandboxMode::Observe && self.calls.lock().any_runs_enforced() {
                return Ok(self.network());
            }
            tracing::info!(root = %self.root.display(), mode = <&str>::from(mode), "sandbox mode is not enforce; stopping the folder's egress proxy");
            self.stop_network_locked().await;
            return Ok(None);
        }
        if let Some(side) = self.network.load_full() {
            if side.is_running() {
                return Ok(self.network());
            }
            tracing::warn!(root = %self.root.display(), "the folder's egress proxy stopped on its own; starting another");
            self.stop_network_locked().await;
        }
        let options = EgressProxyOptions {
            // The proxy stamps `deadline_unix` with the clock the grant rows are judged by
            clock: self.clock.clone(),
            ..EgressProxyOptions::default()
        };
        self.start_network_locked(options).await.map(Some)
    }

    /// Start the folder's egress proxy with `options` and install it: the decider reads this
    /// folder's mode, grant rows and `permission.toml` at every decision, its holds become cards
    /// for the call's owner and are settled from the answer, and its observe-mode decisions land
    /// in the observe summary.
    ///
    /// # Errors
    /// [`NetworkStartError::AlreadyStarted`] on a second call, the proxy's bind error otherwise.
    /// The sandbox keeps working
    /// without a proxy either way: commands then run with the network off under `enforce` and
    /// unproxied under `observe`.
    pub async fn start_network(
        self: &Arc<Self>,
        options: EgressProxyOptions,
    ) -> Result<NetworkInfo, NetworkStartError> {
        let lifecycle = self.network_lifecycle.lock().await;
        if lifecycle.closed {
            return Err(NetworkStartError::Closed);
        }
        self.start_network_locked(options).await
    }

    /// [`WorkspaceSandbox::start_network`] with `network_lifecycle` held and not closed.
    async fn start_network_locked(
        self: &Arc<Self>,
        options: EgressProxyOptions,
    ) -> Result<NetworkInfo, NetworkStartError> {
        if self.network.load().is_some() {
            return Err(NetworkStartError::AlreadyStarted);
        }
        let cwd = AbsPathBuf::new(self.root.clone())
            .map_err(|_| NetworkStartError::RelativeRoot(self.root.clone()))?;
        // The web-fetch domain sets come from the `permission.toml` the hub gate keys for a
        // session bound at the folder root
        let web_fetch = WebFetchDomainFile::open(self.grok_home.clone(), &cwd, &self.root).await;
        let tasks = Arc::new(OwnedTasks::default());
        let sink = Arc::new(NetworkViolationSink {
            sandbox: Arc::downgrade(self),
            decider: OnceLock::new(),
        });
        let decider = Arc::new(SandboxNetworkDecider::new(SandboxNetworkDeciderConfig {
            clock: self.clock.clone(),
            // The one value the card's deadline and the proxy's `403` share
            hold_timeout: options.hold_timeout,
            sink: Some(sink.clone()),
            view: Arc::new(SandboxGrantView(Arc::downgrade(self))),
        }));
        // The sink answers the decider that posted to it; weak, so the pair is dropped together
        let _ = sink.decider.set(Arc::downgrade(&decider));
        let proxy = EgressProxy::start_with(
            WebsitePolicy::new(WebsiteAction::Allow, [], []),
            decider.clone(),
            options,
        )
        .await?;
        let info = NetworkInfo {
            address: proxy.address(),
        };
        tasks.spawn(blocked_feed(Arc::downgrade(self), proxy.blocked_requests()));
        self.network.store(Some(Arc::new(NetworkSide::new(
            Arc::downgrade(self),
            proxy,
            tasks,
            decider,
            web_fetch,
        ))));
        tracing::info!(
            root = %self.root.display(),
            address = %info.address,
            "egress proxy listening for the folder's commands"
        );
        Ok(info)
    }

    /// The daemon's unserve: stop the folder's proxy and wait for its listener to close. Held
    /// connections end with it; a command still running keeps a pointer to a port nobody answers
    /// on. Explicit because the folder's `WorkspaceHandle` outlives the exposure, so a drop is no
    /// teardown. The shutdown is awaited whether or not a reader still holds the side (the
    /// handle is taken out from under it), so the port is closed when this returns.
    pub async fn stop_network(&self) {
        let mut lifecycle = self.network_lifecycle.lock().await;
        lifecycle.closed = true;
        self.stop_network_locked().await;
    }

    /// [`WorkspaceSandbox::stop_network`] with `network_lifecycle` already held. The side is
    /// taken out first: a spawn that snapshots the side after this mints nothing and gets no
    /// pointer, and one that snapshotted it before finds the side gone when it checks after its
    /// mint and revokes what it minted (`prepare`), so no command starts pointed at the closing
    /// port; the proxy's listener closes before its drain waits. The drain admits what the
    /// modes admit and no more: a credential-less request that lands before the listener closes
    /// is a `407` while the folder is `enforce` or any live call runs enforced
    /// (`admits_unauthenticated`), recorded as the session's under `observe`, and relayed
    /// unrecorded only with the mode published `off` — which restricts and records nothing
    /// anyway. A request with a live credential is decided under its call's own mode, as ever.
    async fn stop_network_locked(&self) {
        let Some(side) = self.network.swap(None) else {
            return;
        };
        // A connection still parked would hold the shutdown (and this lock) for its whole hold,
        // and one still deciding must park nothing for a proxy that is going
        side.decider.close();
        side.tasks.stop();
        let proxy = side.proxy.lock().take();
        if let Some(proxy) = proxy
            && let Err(error) = proxy.shutdown().await
        {
            tracing::warn!(root = %self.root.display(), %error, "egress proxy shutdown");
        }
        tracing::info!(root = %self.root.display(), "egress proxy stopped for the folder");
    }

    /// Takes the proxy handle out of the network side's slot and leaves the side in place: what
    /// a spawn racing the folder's stop sees when it asks for a credential. Put back with
    /// [`Self::restore_proxy_for_test`].
    #[cfg(test)]
    pub(super) fn take_proxy_for_test(&self) -> Option<EgressProxyHandle> {
        self.network
            .load_full()
            .and_then(|side| side.proxy.lock().take())
    }

    #[cfg(test)]
    pub(super) fn restore_proxy_for_test(&self, proxy: EgressProxyHandle) {
        if let Some(side) = self.network.load_full() {
            *side.proxy.lock() = Some(proxy);
        }
    }

    /// Whether the folder's decider still has the card `hold_id` on its books.
    #[cfg(test)]
    pub(super) fn hold_pending_for_test(&self, hold_id: &str) -> bool {
        self.network
            .load_full()
            .is_some_and(|side| side.decider.is_pending(hold_id))
    }

    /// Whether the folder's decider would decide a request with no call credential as the
    /// session's right now.
    #[cfg(test)]
    pub(super) fn admits_unauthenticated_for_test(&self) -> bool {
        self.network
            .load_full()
            .is_some_and(|side| side.decider.admits_unauthenticated())
    }

    /// Stops the proxy's accept loop without the sandbox knowing, as an accept error does.
    #[cfg(test)]
    pub(super) async fn abort_proxy_for_test(&self) {
        let proxy = self
            .network
            .load_full()
            .and_then(|side| side.proxy.lock().take());
        if let Some(mut proxy) = proxy {
            let _ = proxy.abort().await;
            if let Some(side) = self.network.load_full() {
                *side.proxy.lock() = Some(proxy);
            }
        }
    }

    /// Where the proxy listens, once started.
    pub fn network(&self) -> Option<NetworkInfo> {
        let net = self.network.load();
        let net = net.as_ref()?;
        Some(NetworkInfo {
            address: net.address,
        })
    }

    /// The proxy's decision stream from this subscription on (the tests watch a hold resolve).
    pub fn blocked_requests(&self) -> Option<broadcast::Receiver<BlockedRequest>> {
        self.network
            .load()
            .as_ref()
            .and_then(|net| net.with_proxy(EgressProxyHandle::blocked_requests))
    }

    /// The `Proxy-Authorization` value for the session-wide token: a request from outside any
    /// command (a plain `curl -x` at the proxy) is decided with no call attached.
    pub fn session_proxy_authorization(&self) -> Option<String> {
        self.network
            .load()
            .as_ref()
            .and_then(|net| net.with_proxy(EgressProxyHandle::proxy_authorization))
    }

    /// The egress proxy the policy renders and `sandbox.status` reports: the folder's own, while
    /// its accept loop runs. A proxy that stopped on its own is not one (the next `sync_network`
    /// replaces it), and `None` under `enforce` means the network is off for this folder (the
    /// card is then deny-only).
    pub fn proxy(&self) -> Option<ProxyEndpointOwned> {
        self.network
            .load()
            .as_ref()
            .filter(|net| net.is_running())
            .map(|net| net.endpoint())
    }

    /// The folder's proxy as one spawn sees it: the side whose accept loop runs, read once, so
    /// the port the kernel policy opens and the credential the pointer carries come from the
    /// same listener. `None` is "no proxy" — none started, or the one there stopped on its own
    /// (the next `sync_network` replaces it) — and the spawn gets no pointer.
    pub(super) fn running_network_side(&self) -> Option<Arc<NetworkSide>> {
        self.network.load_full().filter(|side| side.is_running())
    }

    /// A credential minted for `tag` on `side`, the snapshot the spawn's policy was built from;
    /// a second mint for the same tag (the replay) replaces the first. `Ok(None)` when the side
    /// is no longer the folder's running proxy by the time the mint is checked (the stop took
    /// its handle, swapped it out or its accept loop ended): what was minted is revoked with the
    /// guard, and the command gets no pointer rather than one at a port nobody answers on.
    ///
    /// # Errors
    /// The proxy runs but no credential came out of it (the token source failed): the caller
    /// refuses the spawn, since a pointer with no credential would let the command's requests
    /// pass as nobody's.
    pub(super) fn mint_call_credential(
        &self,
        side: &Arc<NetworkSide>,
        tag: &CommandTag,
    ) -> Result<Option<MintedCredential>, WorkspaceSandboxError> {
        let minted = side
            .mint(tag)
            .map_err(|error| WorkspaceSandboxError::NoCallCredential {
                reason: error.to_string(),
            })?;
        let still_current = self
            .network
            .load()
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, side) && side.is_running());
        if !still_current {
            tracing::warn!(
                tag = %tag,
                "the folder's egress proxy went while the command was prepared; it runs with no proxy pointer"
            );
            return Ok(None);
        }
        Ok(minted)
    }

    /// Any later request presenting the call's token is a 407. Called only through
    /// [`WorkspaceSandbox::revoke_released`], for a call the table let go of.
    pub(super) fn revoke_call_credential(&self, call: &CallId) {
        if let Some(net) = self.network.load().as_ref() {
            let tag = CommandTag::for_call(call);
            net.with_proxy(|proxy| proxy.revoke_call_credential(&tag));
        }
    }

    /// Refuse every hold parked for an epoch the table let end, and withdraw its card: the
    /// second half of a release, off the table's lock (the decider's comes before it). `None`
    /// for the epoch — the table did not hold the call — refuses every hold parked for it under
    /// a known epoch; a hold parked with no epoch waits for its answer or the proxy's stop.
    pub(super) fn release_ended_holds(&self, released: Released) {
        let Some(net) = self.network.load_full() else {
            return;
        };
        for (call, epoch) in released.into_epochs() {
            net.decider
                .release_call_holds(&CommandTag::for_call(&call), epoch.unwrap_or(u64::MAX));
        }
    }

    /// A call's end, after its holds are refused: with nothing left running under `enforce` and
    /// the mode published `observe` or `off`, the proxy kept for that call is stopped by a sync
    /// run as an owned task, off every lock. With no runtime current the next sync stops it.
    pub(super) fn resync_unless_enforced(&self, any_runs_enforced: bool) {
        if any_runs_enforced {
            return;
        }
        let Some(side) = self.network.load_full() else {
            return;
        };
        if self.published_mode() == SandboxMode::Enforce {
            return;
        }
        let Some(sandbox) = side.sandbox.upgrade() else {
            return;
        };
        if tokio::runtime::Handle::try_current().is_err() {
            tracing::debug!(
                root = %self.root.display(),
                "no runtime to stop the folder's egress proxy from; the next sync stops it"
            );
            return;
        }
        #[cfg(test)]
        self.proxy_resyncs
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.spawn_owned(async move {
            if let Err(error) = sandbox.sync_network().await {
                tracing::warn!(
                    root = %sandbox.root.display(),
                    %error,
                    "the folder's egress proxy did not sync after its last enforced call ended"
                );
            }
            #[cfg(test)]
            sandbox.proxy_resynced.notify_one();
        });
    }

    /// The rows or `permission.toml` were reloaded from disk: a hold a row or a list entry now
    /// covers is released with that verdict. Awaits the lists off every lock.
    pub(super) async fn release_holds_now_covered(&self) {
        let Some(net) = self.network.load_full() else {
            return;
        };
        let domains = net.web_fetch_domains().await;
        net.decider.release_holds_now_covered(&domains);
    }

    /// Answer sandbox cards over `transport` instead of the hub's permission channel (a host
    /// with no hub, the real-wiring tests).
    pub fn set_card_transport(&self, transport: Arc<dyn PermissionHookTransport>) {
        *self.card_transport.write() = Some(transport);
    }

    pub fn card_transport(&self) -> Option<Arc<dyn PermissionHookTransport>> {
        self.card_transport.read().clone()
    }

    /// What the owner refused at the proxy while `call` ran, for the model; consumed.
    pub fn take_network_denials(&self, call: &CallId) -> Vec<String> {
        self.calls.lock().take_denials(call)
    }

    /// The web-fetch domain sets of the folder's `permission.toml`, read through the network
    /// side that opened it; none while no proxy runs.
    async fn web_fetch_domains(&self) -> WebFetchDomains {
        match self.network.load_full() {
            Some(side) => side.web_fetch_domains().await,
            None => WebFetchDomains::default(),
        }
    }

    /// One proxy decision: an observe-mode row whose verdict under `enforce` would not have been
    /// an allow is a would-block entry carrying that verdict; the rest are
    /// logged.
    fn note_blocked_request(&self, record: &BlockedRequest) {
        let verdict = match record.decided {
            DeciderOutcome::Observed { would } => match would {
                WouldBe::Asked => Some(WouldVerdict::Ask),
                WouldBe::Denied(DenySource::DenyRow) => Some(WouldVerdict::DenyRow),
                WouldBe::Denied(DenySource::WebFetchDenylist) => Some(WouldVerdict::PolicyDenylist),
                WouldBe::Allowed => None,
            },
            DeciderOutcome::Allowed
            | DeciderOutcome::Denied
            | DeciderOutcome::Timeout
            | DeciderOutcome::Abandoned => None,
        };
        match verdict {
            Some(verdict) => self.observe_blocked(
                &Blocked::Net {
                    host: Some(record.host.clone()),
                    port: Some(record.port),
                },
                verdict,
            ),
            None => tracing::info!(
                host = %record.host,
                port = record.port,
                call = ?record.call.as_ref().map(ToString::to_string),
                outcome = ?record.decided,
                hold_ms = record.hold_ms,
                "egress proxy decision"
            ),
        }
    }

    /// Settle a held connection with the session owner through the same gate as a filesystem
    /// denial (`settle_violation`): the card goes to whoever dispatched the call, an allow records
    /// the `NetHost` row (the decider sees it at once) and resumes the hold, anything else keeps
    /// the hold refused and leaves the model text for the call's result. Fails closed: a request
    /// that carries no call (the session-wide token) or a call nobody bound an owner for is
    /// refused. `epoch` is the call's as its card is raised: a call grant the answer records is
    /// kept only while the call is still under it.
    async fn settle_network_hold(
        &self,
        call: Option<&CallId>,
        epoch: Option<u64>,
        violation: Violation,
    ) -> bool {
        let Some(call) = call else {
            tracing::info!(
                blocked = ?violation.blocked,
                "held request carries the session-wide credential; no call to ask for, refusing"
            );
            return false;
        };
        // Owner, spawn record and the call's mode under one lock, copied out and the table let
        // go before the card is raised: every prepare, finish and bind waits on it
        let bound = {
            let calls = self.calls.lock();
            calls.owner_of(call).map(|owner| {
                let prepared = calls
                    .prepared_record(call)
                    .map(|record| (record.backend, command_text(&record.original)));
                (owner, calls.held_mode(call), prepared)
            })
        };
        let Some((owner, held, prepared)) = bound else {
            // SECURITY: without the call's owner there is no session to ask; the hold is refused
            tracing::warn!(
                %call,
                blocked = ?violation.blocked,
                "no session owner is bound for the call; refusing the held request"
            );
            return false;
        };
        // The card shows the call's own mode: pinned at dispatch or fixed by its spawn. A call
        // with neither is a stray decided as a token-less request, so the folder's mode is its own
        let mode = held.unwrap_or_else(|| self.mode());
        let (backend, argv_text) = match prepared {
            Some((backend, text)) => (backend, Some(text)),
            None => (self.backend_name(), None),
        };
        let command = owner.command.clone().or(argv_text).unwrap_or_default();
        let settlement = settle_violation(
            SettleContext {
                sandbox: self,
                policy: owner.policy,
                call,
                epoch,
                session_id: &owner.session_id,
                command: &command,
                mode,
                backend,
                // A resume never re-runs the command, so a hold is never "after the replay"
                replayed_under: None,
            },
            violation,
            owner.transport.clone(),
        )
        .await;
        match settlement {
            ViolationSettlement::Replay { grant } => {
                tracing::info!(%call, grant = %grant.id, "releasing the held connection");
                true
            }
            ViolationSettlement::Denied { model_text } => {
                self.calls.lock().push_denial(call, model_text);
                false
            }
            // A network violation always carries a target and so a card; refuse if it ever did not
            ViolationSettlement::Recorded => false,
        }
    }
}

/// The command as the card shows it when the owner gave none (the tool's `command` argument is
/// what a shell call's owner carries): the spawned argv, program first, exactly as it was
/// given. Nothing is guessed from it — which argument is "the script" is the shell's business,
/// and a wrong guess would put another program's text on the card.
fn command_text(original: &OriginalArgv) -> String {
    std::iter::once(original.program.as_os_str())
        .chain(original.args.iter().map(OsString::as_os_str))
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The decider's window onto the folder's grant store, call table and mode (no second copy of
/// the rows; the folder's mode is the last resolved, from memory — no stat per connection).
/// Weak, so the decider the sandbox owns never keeps the sandbox alive; a sandbox that is gone
/// reads as `Enforce` with no rows and no call mode (fail closed), which only matters to a
/// request racing the teardown.
pub(super) struct SandboxGrantView(pub(super) Weak<WorkspaceSandbox>);

#[async_trait]
impl GrantView for SandboxGrantView {
    fn mode(&self) -> SandboxMode {
        self.0
            .upgrade()
            .map_or(SandboxMode::Enforce, |sandbox| sandbox.published_mode())
    }

    fn net_rows(&self, call: Option<&CommandTag>) -> Vec<Grant> {
        self.0
            .upgrade()
            .map(|sandbox| sandbox.net_rows_for(call))
            .unwrap_or_default()
    }

    fn call_mode(&self, call: Option<&CommandTag>) -> Option<SandboxMode> {
        let call = call.and_then(CommandTag::call_id)?;
        self.0
            .upgrade()
            .and_then(|sandbox| sandbox.calls.lock().held_mode(&call))
    }

    fn call_epoch(&self, call: Option<&CommandTag>) -> Option<u64> {
        let call = call.and_then(CommandTag::call_id)?;
        self.0
            .upgrade()
            .and_then(|sandbox| sandbox.calls.lock().epoch_of(&call))
    }

    fn call_runs(&self, call: &CommandTag) -> bool {
        let Some(call) = call.call_id() else {
            return false;
        };
        self.0
            .upgrade()
            .is_some_and(|sandbox| sandbox.calls.lock().prepared_record(&call).is_some())
    }

    fn any_call_runs_enforced(&self) -> bool {
        self.0
            .upgrade()
            .is_some_and(|sandbox| sandbox.calls.lock().any_runs_enforced())
    }

    async fn web_fetch_domains(&self) -> WebFetchDomains {
        match self.0.upgrade() {
            Some(sandbox) => sandbox.web_fetch_domains().await,
            None => WebFetchDomains::default(),
        }
    }
}

/// The decider's outlet: a new hold becomes a card for the call's session owner. `post` runs on
/// the proxy's connection task, so the round-trip is spawned (under the sandbox's
/// [`OwnedTasks`]: it ends with its hold, which the folder's stop releases before aborting the
/// proxy's own tasks — the owner is told even then) and the hold is settled from there
/// on the decider that posted it, by `hold_id` (a hold is the call's own; a
/// retry within the card's deadline re-parks under the same id, so the id is never forgotten
/// while the card is on screen). An informational card has nothing parked and is not settled.
/// A hold released by anything but its own card's answer (a row another card recorded, the
/// call's final result or background start, the proxy stopping) withdraws that card the moment
/// the decider cancels its token: the settlement waiting on the owner is dropped and the
/// owner's transport is told once, through [`PermissionHookTransport::withdraw_permission`] —
/// deterministic, not the card's deadline. An answer is judged again against the rows and the
/// web-fetch lists as they stand when it lands: an allow a deny row or a denylist entry now
/// refuses is applied as a deny, and the model is told.
pub(super) struct NetworkViolationSink {
    pub(super) sandbox: Weak<WorkspaceSandbox>,
    pub(super) decider: OnceLock<Weak<SandboxNetworkDecider>>,
}

#[async_trait]
impl ViolationSink for NetworkViolationSink {
    async fn post(&self, call: Option<&CommandTag>, violation: Violation) {
        // A sandbox that is gone takes its proxy with it; the hold ends with the connection
        let (Some(sandbox), Some(decider)) = (
            self.sandbox.upgrade(),
            self.decider.get().and_then(Weak::upgrade),
        ) else {
            return;
        };
        let Replay::Resume { hold_id } = &violation.replay else {
            tracing::warn!(blocked = ?violation.blocked, "network violation without a hold; nothing to settle");
            return;
        };
        let hold_id = hold_id.clone();
        let informational = !violation.is_grantable();
        let call = call.and_then(CommandTag::call_id);
        let owner = sandbox.clone();
        owner.spawn_owned(async move {
            // A row another card recorded since the park already released this hold: no card
            let Some(withdrawn) = decider.card_token(&hold_id) else {
                return;
            };
            // The owner the card goes to and the call's epoch, read once under one lock: the
            // withdrawal goes to the same owner, and the answer counts for this epoch only
            let (transport, epoch) = match &call {
                Some(call) => {
                    let calls = sandbox.calls.lock();
                    (
                        calls.owner_of(call).and_then(|owner| owner.transport),
                        calls.epoch_of(call),
                    )
                }
                None => (None, None),
            };
            let released = tokio::select! {
                biased;
                // Released meanwhile by a row, the call's end or the stop: the settlement is
                // dropped (a late answer on the card records nothing) and the owner is told, once
                () = withdrawn.cancelled() => {
                    tracing::info!(hold_id, "the hold was released while its card was up; the card is withdrawn");
                    if let Some(transport) = transport {
                        transport.withdraw_permission(&hold_id).await;
                    }
                    return;
                }
                released = sandbox.settle_network_hold(call.as_ref(), epoch, violation) => released,
            };
            if informational {
                // Nothing is parked under an informational card; the decider keeps its
                // entry until the card's deadline so a burst does not raise it again
                return;
            }
            let answer = if released {
                HoldAnswer::Allow
            } else {
                HoldAnswer::Deny
            };
            // The lists as they stand now, read off every lock: the answer is judged against
            // them and the rows once more before it is applied
            let domains = sandbox.web_fetch_domains().await;
            match decider.settle_hold(&hold_id, answer, &domains) {
                Some(applied) if applied != answer => {
                    tracing::info!(hold_id, "the hold was allowed on its card, but a deny rule added meanwhile refuses it");
                    if let Some(call) = &call {
                        sandbox
                            .calls
                            .lock()
                            .push_denial(call, DENIED_SINCE_ANSWERED_TEXT.to_owned());
                    }
                }
                Some(_) => {}
                None => tracing::info!(
                    hold_id,
                    ?answer,
                    "the hold was already settled (proxy timeout or the client hung up)"
                ),
            }
        });
    }
}

/// What the model is told when a card's allow was overtaken by a deny rule before it was applied.
const DENIED_SINCE_ANSWERED_TEXT: &str = "a connection allowed on its card was refused: a deny rule \
     added while the card was up covers it";

/// Every proxy decision reaches the sandbox while it lives; ends with the proxy's channel, with
/// the sandbox, or with the [`OwnedTasks`] it runs under, whichever goes first.
async fn blocked_feed(
    sandbox: Weak<WorkspaceSandbox>,
    mut requests: broadcast::Receiver<BlockedRequest>,
) {
    loop {
        match requests.recv().await {
            Ok(record) => {
                let Some(sandbox) = sandbox.upgrade() else {
                    return;
                };
                sandbox.note_blocked_request(&record);
            }
            Err(broadcast::error::RecvError::Lagged(missed)) => {
                tracing::warn!(missed, "observe summary missed proxy decisions");
            }
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}
