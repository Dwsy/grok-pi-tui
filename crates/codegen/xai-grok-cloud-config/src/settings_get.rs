//! One owner task per `(origin, identity)` scope loads the startup settings.
//! Callers start it with [`warm_startup_settings`], wait on [`await_startup_settings`] or [`block_on_startup_settings`], then call [`consume_wait`].

use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::AbortHandle;
use tokio_util::sync::CancellationToken;

use xai_grok_config::{EndpointsConfig, RemoteSettings};
use xai_grok_login::{GrokAuth, GrokComConfig};
use xai_grok_telemetry::instrumentation::{
    InstrumentationMode, InstrumentationTimer, TARGET, current_mode,
};

use crate::commit::evaluate_commit;
use crate::settings_cache::{SettingsCacheManager, SettingsCacheMode, SettingsLoad};
use crate::{Commit, PolicyRepairPending, SettingsEndpoint, SettingsFetch};

/// A settings load request, built by [`resolve`](Self::resolve) for a startup warm.
#[derive(Clone)]
pub struct SettingsQuery {
    auth: Option<GrokAuth>,
    endpoint: SettingsEndpoint,
    /// The grok.com config that disk auth was resolved under, for the commit-time identity recheck.
    /// The env-only default config would see a file or managed IdP as a changed credential.
    auth_config: Option<GrokComConfig>,
}

impl SettingsQuery {
    #[cfg(any(test, feature = "test-support"))]
    pub fn from_auth(auth: Option<GrokAuth>) -> Self {
        Self::resolve(auth, None)
    }

    /// A live refresh query for `endpoints`.
    /// `--cli-chat-proxy-base-url` can point them away from the startup endpoints.
    pub fn from_endpoints(
        endpoints: &EndpointsConfig,
        auth: GrokAuth,
        auth_config: GrokComConfig,
    ) -> SettingsQuery {
        SettingsQuery {
            auth: Some(auth),
            endpoint: SettingsEndpoint::from(endpoints),
            auth_config: Some(auth_config),
        }
    }

    /// The auth this query resolved to, reused by the consume-time recheck.
    /// A just-refreshed session may not be on disk yet.
    pub fn auth(&self) -> Option<&GrokAuth> {
        self.auth.as_ref()
    }

    /// `auth` takes priority over the on-disk session for `grok_com_config`.
    pub fn resolve(auth: Option<GrokAuth>, grok_com_config: Option<GrokComConfig>) -> Self {
        let endpoint = SettingsEndpoint::from(&EndpointsConfig::from_effective_config());
        let auth = auth.or_else(|| crate::resolve_disk_auth(grok_com_config.clone()));
        Self {
            auth,
            endpoint,
            auth_config: grok_com_config,
        }
    }
}

/// Missing `settings` is not a lasting empty policy and does not use up a later refresh.
#[derive(Clone, Debug)]
pub struct SettingsOutcome {
    settings: Option<RemoteSettings>,
    attempted: bool,
    origin: String,
    identity: String,
}

impl SettingsOutcome {
    pub fn settings(&self) -> Option<&RemoteSettings> {
        self.settings.as_ref()
    }

    pub fn attempted(&self) -> bool {
        self.attempted
    }

    fn skipped() -> Self {
        Self {
            settings: None,
            attempted: false,
            origin: String::new(),
            identity: String::new(),
        }
    }

    fn failed() -> Self {
        Self {
            settings: None,
            attempted: true,
            origin: String::new(),
            identity: String::new(),
        }
    }

    /// Whether this outcome may still be installed under `grok_com_config`.
    /// A repair between warm and consume can change the origin or identity.
    pub fn install_allowed(
        &self,
        grok_com_config: &GrokComConfig,
        warmed_auth: Option<&GrokAuth>,
        policy_repair_pending: PolicyRepairPending,
    ) -> bool {
        self.scope_matches(
            &SettingsQuery::resolve(warmed_auth.cloned(), Some(grok_com_config.clone())),
            policy_repair_pending,
        )
    }

    fn scope_matches(
        &self,
        query: &SettingsQuery,
        policy_repair_pending: PolicyRepairPending,
    ) -> bool {
        if self.settings.is_none() {
            return true;
        }
        if !xai_grok_config::resolve_remote_fetch_enabled() || policy_repair_pending() {
            return false;
        }
        let Some(auth) = query.auth.as_ref() else {
            return false;
        };
        let account = query.endpoint.cache_account(auth);
        account.origin == self.origin && account.identity == self.identity
    }
}

/// The result of a bounded wait on the startup load.
/// The owner task keeps running after a timeout or cancel.
#[derive(Clone, Debug)]
pub enum SettingsWait {
    /// `SettingsOutcome` is boxed because it is large.
    Ready(Box<SettingsOutcome>),
    /// The caller falls back to defaults after the deadline passes.
    TimedOut,
    /// The caller cancelled the wait.
    Cancelled,
}

/// The scope a load runs under.
/// A warm for a different key aborts and replaces the current owner.
#[derive(Clone, PartialEq, Eq)]
struct OwnerKey {
    origin: String,
    identity: String,
}

impl OwnerKey {
    fn of(query: &SettingsQuery) -> Self {
        let identity = query
            .auth
            .as_ref()
            .map(|auth| query.endpoint.cache_account(auth).identity)
            .unwrap_or_default();
        Self {
            origin: query.endpoint.origin().to_owned(),
            identity,
        }
    }
}

/// The single active owner.
/// Its `watch` cell carries values for its own scope only.
struct Owner {
    key: OwnerKey,
    tx: watch::Sender<Option<SettingsOutcome>>,
    abort: AbortHandle,
}

struct StartupState {
    owner: Option<Owner>,
}

/// A receiver on this scope's owner, or `None` when there is no runtime to spawn on.
/// Two concurrent starts for one scope never both fetch.
fn warm_and_subscribe(
    query: &SettingsQuery,
    policy_repair_pending: PolicyRepairPending,
) -> Option<watch::Receiver<Option<SettingsOutcome>>> {
    let runtime = tokio::runtime::Handle::try_current().ok()?;
    let key = OwnerKey::of(query);
    let mut state = lock_state();
    if let Some(owner) = &state.owner {
        if owner.key == key && !owner_published_skip(owner) {
            return Some(owner.tx.subscribe());
        }
        // The scope changed, or the owner published a skip while the scope was briefly ineligible.
        // Otherwise a scope that is eligible again would reuse that skip for the whole process.
        owner.abort.abort();
    }

    let (tx, rx) = watch::channel(None);
    let task_tx = tx.clone();
    let task_query = query.clone();
    let task = runtime.spawn(async move {
        let outcome = load_settings(task_query, policy_repair_pending).await;
        // `send_replace` stores the value even when no receiver is alive
        task_tx.send_replace(Some(outcome));
    });
    state.owner = Some(Owner {
        key,
        tx,
        abort: task.abort_handle(),
    });
    Some(rx)
}

/// Whether the owner published a skip after its scope went ineligible.
fn owner_published_skip(owner: &Owner) -> bool {
    owner
        .tx
        .borrow()
        .as_ref()
        .is_some_and(|outcome| !outcome.attempted && outcome.settings.is_none())
}

fn lock_state() -> std::sync::MutexGuard<'static, StartupState> {
    startup_state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

static STARTUP_STATE: OnceLock<Mutex<StartupState>> = OnceLock::new();

fn startup_state() -> &'static Mutex<StartupState> {
    STARTUP_STATE.get_or_init(|| Mutex::new(StartupState { owner: None }))
}

/// An ineligible query makes no network call and writes no cache.
pub fn is_eligible(query: &SettingsQuery, policy_repair_pending: PolicyRepairPending) -> bool {
    if query.auth.is_none() {
        return false;
    }
    if !xai_grok_config::resolve_remote_fetch_enabled() {
        return false;
    }
    if policy_repair_pending() {
        return false;
    }
    true
}

#[cfg(any(test, feature = "test-support"))]
pub fn reset_startup_settings_for_tests() {
    let mut state = lock_state();
    if let Some(owner) = state.owner.take() {
        owner.abort.abort();
    }
}

/// A cache-first settings load outside the startup watch.
/// A successful fetch writes the signed cache only after a policy and identity recheck.
#[cfg(any(test, feature = "test-support"))]
pub async fn get_settings(
    query: SettingsQuery,
    policy_repair_pending: PolicyRepairPending,
) -> SettingsOutcome {
    load_settings(query, policy_repair_pending).await
}

/// A live `/v1/settings` read for a mid-session refresh, with no startup cache lookup.
/// A successful fetch is written to the cache for a later startup.
pub async fn fetch_settings_live(
    query: SettingsQuery,
    policy_repair_pending: PolicyRepairPending,
) -> SettingsFetch {
    let Some(auth) = query.auth.clone() else {
        return SettingsFetch::Retry;
    };
    let loaded = tokio::task::spawn_blocking(move || {
        let fetched_at = chrono::Utc::now();
        let fetch = crate::fetch_settings_blocking(
            query.endpoint.origin(),
            &auth,
            query.endpoint.alpha_test_key(),
        );
        if let SettingsFetch::Fetched(settings) = &fetch {
            let scope = query
                .endpoint
                .cache_scope(&auth, xai_grok_http::process_client_identifier());
            // The cache is written only when the whole scope is still current.
            // The caller still serves the fetch from memory when the session is not on disk yet.
            if matches!(
                commit_for_fetch(
                    &query,
                    &scope.account.origin,
                    &scope.account.identity,
                    policy_repair_pending,
                ),
                Commit::CacheAndServe
            ) {
                warn_if_next_start_refetches(
                    settings_cache().write_through(&scope, settings, fetched_at),
                );
            }
        }
        fetch
    })
    .await;

    match loaded {
        Ok(fetch) => fetch,
        Err(err) => {
            tracing::warn!(error = %err, "settings refresh task failed");
            SettingsFetch::Retry
        }
    }
}

/// Waits up to `deadline` for this scope's startup load and starts it if needed.
pub async fn await_startup_settings(
    query: SettingsQuery,
    deadline: Duration,
    cancel: &CancellationToken,
    policy_repair_pending: PolicyRepairPending,
) -> SettingsWait {
    if !is_eligible(&query, policy_repair_pending) {
        return SettingsWait::Ready(Box::new(SettingsOutcome::skipped()));
    }
    let Some(rx) = warm_and_subscribe(&query, policy_repair_pending) else {
        return SettingsWait::TimedOut;
    };
    observe(rx, Some(deadline), cancel).await
}

/// The settings in `wait` that still pass a recheck against the live scope.
pub fn consume_wait(
    wait: SettingsWait,
    auth: Option<&GrokAuth>,
    grok_com_config: &GrokComConfig,
    policy_repair_pending: PolicyRepairPending,
) -> Option<RemoteSettings> {
    match wait {
        SettingsWait::Ready(outcome) => {
            if outcome.install_allowed(grok_com_config, auth, policy_repair_pending) {
                outcome.settings
            } else {
                tracing::info!("startup settings discarded at consume: policy or identity changed");
                None
            }
        }
        SettingsWait::TimedOut | SettingsWait::Cancelled => None,
    }
}

/// A wait on the startup load with no deadline of its own.
/// The fetch timeouts still bound the owner.
#[cfg(any(test, feature = "test-support"))]
pub async fn get_startup_settings(
    query: SettingsQuery,
    policy_repair_pending: PolicyRepairPending,
) -> SettingsOutcome {
    if !is_eligible(&query, policy_repair_pending) {
        return SettingsOutcome::skipped();
    }
    let Some(rx) = warm_and_subscribe(&query, policy_repair_pending) else {
        return SettingsOutcome::failed();
    };
    match observe(rx, None, &CancellationToken::new()).await {
        SettingsWait::Ready(outcome) => *outcome,
        SettingsWait::TimedOut | SettingsWait::Cancelled => SettingsOutcome::failed(),
    }
}

/// The sole public entry point for starting the startup load.
pub fn warm_startup_settings(query: SettingsQuery, policy_repair_pending: PolicyRepairPending) {
    if !is_eligible(&query, policy_repair_pending) {
        return;
    }
    let _ = warm_and_subscribe(&query, policy_repair_pending);
}

/// A blocking wait for `spawn_blocking` bootstrap code on a multi-thread runtime.
/// Current-thread callers must call [`await_startup_settings`] first.
pub fn block_on_startup_settings(
    query: SettingsQuery,
    deadline: Duration,
    cancel: &CancellationToken,
    policy_repair_pending: PolicyRepairPending,
) -> SettingsWait {
    if !is_eligible(&query, policy_repair_pending) {
        return SettingsWait::Ready(Box::new(SettingsOutcome::skipped()));
    }
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        tracing::info!("settings wait has no tokio runtime; falling open");
        return SettingsWait::TimedOut;
    };

    // Subscribing first replaces a stale owner for another scope before the wait starts
    let Some(rx) = warm_and_subscribe(&query, policy_repair_pending) else {
        return SettingsWait::TimedOut;
    };
    if let Some(ready) = peek_published(&rx) {
        return ready;
    }
    match handle.runtime_flavor() {
        tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(|| handle.block_on(observe(rx, Some(deadline), cancel)))
        }
        _ => {
            tracing::info!(
                "settings wait on current-thread runtime has no published value; not blocking the runtime"
            );
            SettingsWait::TimedOut
        }
    }
}

fn peek_published(rx: &watch::Receiver<Option<SettingsOutcome>>) -> Option<SettingsWait> {
    rx.borrow()
        .clone()
        .map(|outcome| SettingsWait::Ready(Box::new(outcome)))
}

/// A wait for the owner to publish, bounded by `deadline` when it is `Some`.
/// The fetch timeouts still bound a wait with no deadline.
async fn observe(
    mut rx: watch::Receiver<Option<SettingsOutcome>>,
    deadline: Option<Duration>,
    cancel: &CancellationToken,
) -> SettingsWait {
    if let Some(ready) = peek_published(&rx) {
        return ready;
    }
    let cancel = cancel.clone();
    let published = async {
        rx.wait_for(Option::is_some)
            .await
            .ok()
            .and_then(|outcome| outcome.clone())
            .unwrap_or_else(SettingsOutcome::failed)
    };
    match deadline {
        Some(deadline) => tokio::select! {
            biased;
            _ = cancel.cancelled() => SettingsWait::Cancelled,
            result = tokio::time::timeout(deadline, published) => match result {
                Ok(outcome) => SettingsWait::Ready(Box::new(outcome)),
                Err(_elapsed) => SettingsWait::TimedOut,
            },
        },
        None => tokio::select! {
            biased;
            _ = cancel.cancelled() => SettingsWait::Cancelled,
            outcome = published => SettingsWait::Ready(Box::new(outcome)),
        },
    }
}

async fn load_settings(
    query: SettingsQuery,
    policy_repair_pending: PolicyRepairPending,
) -> SettingsOutcome {
    let loaded =
        tokio::task::spawn_blocking(move || load_settings_blocking(query, policy_repair_pending))
            .await;
    match loaded {
        Ok(outcome) => outcome,
        Err(err) => {
            tracing::warn!(error = %err, "settings getter task failed");
            SettingsOutcome::failed()
        }
    }
}

fn load_settings_blocking(
    query: SettingsQuery,
    policy_repair_pending: PolicyRepairPending,
) -> SettingsOutcome {
    if !is_eligible(&query, policy_repair_pending) {
        tracing::info!("settings getter skipped: ineligible");
        return SettingsOutcome::skipped();
    }
    let Some(auth) = query.auth.clone() else {
        return SettingsOutcome::skipped();
    };

    let _timer = {
        let mode = current_mode();
        match mode {
            InstrumentationMode::Chrome => {
                let span = tracing::info_span!(target: TARGET, "startup.settings_get");
                InstrumentationTimer::new_with_span(
                    "startup.settings_get",
                    mode,
                    Some(span.entered()),
                )
            }
            InstrumentationMode::Disabled
            | InstrumentationMode::Log
            | InstrumentationMode::Server => InstrumentationTimer::new("startup.settings_get"),
        }
    };

    let scope = query
        .endpoint
        .cache_scope(&auth, xai_grok_http::process_client_identifier());
    let origin = scope.account.origin.clone();
    let identity = scope.account.identity.clone();
    let load = settings_cache().load_or_fetch(scope, || {
        crate::fetch_settings_blocking(
            query.endpoint.origin(),
            &auth,
            query.endpoint.alpha_test_key(),
        )
        .into_option()
    });
    let (settings, write) = match load {
        SettingsLoad::Cached(settings) => {
            return SettingsOutcome {
                settings: Some(settings),
                attempted: false,
                origin,
                identity,
            };
        }
        SettingsLoad::Fetched { settings, write } => (settings, write),
        SettingsLoad::FetchFailed => return SettingsOutcome::failed(),
    };

    // Only a live fetch goes through the commit scope check
    match commit_for_fetch(&query, &origin, &identity, policy_repair_pending) {
        Commit::CacheAndServe => {
            if let Some(write) = write {
                warn_if_next_start_refetches(write.commit());
            }
        }
        Commit::ServeInMemory => {
            tracing::info!(
                "settings fetch served in memory; not cached until the session persists"
            );
        }
        Commit::Retry => {
            tracing::info!("settings getter discarded fetch: policy repair pending");
            return SettingsOutcome::skipped();
        }
        Commit::Abandon => {
            tracing::info!("settings getter discarded fetch: origin or policy changed");
            return SettingsOutcome::failed();
        }
    }
    SettingsOutcome {
        settings: Some(settings),
        attempted: true,
        origin,
        identity,
    }
}

fn settings_cache() -> SettingsCacheManager {
    SettingsCacheManager::new(
        &xai_grok_config::grok_home(),
        SettingsCacheMode::from_process(),
    )
}

fn warn_if_next_start_refetches(written: std::io::Result<()>) {
    if let Err(error) = written {
        tracing::warn!(%error, "settings cache write failed; the next start fetches settings again");
    }
}

/// The commit scope for a fetch by `query` under `origin` and `identity`, checked against the live proxy URL.
fn commit_for_fetch(
    query: &SettingsQuery,
    origin: &str,
    identity: &str,
    policy_repair_pending: PolicyRepairPending,
) -> Commit {
    let current_origin = EndpointsConfig::from_effective_config().proxy_url();
    evaluate_commit(
        &current_origin,
        origin,
        identity,
        query.auth_config.as_ref(),
        query.endpoint.alpha_test_key(),
        policy_repair_pending,
    )
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use tokio_util::sync::CancellationToken;

    use crate::SettingsEndpoint;

    use super::{
        SettingsOutcome, SettingsQuery, SettingsWait, await_startup_settings,
        reset_startup_settings_for_tests, warm_startup_settings,
    };

    fn ineligible_query() -> SettingsQuery {
        SettingsQuery {
            auth: None,
            endpoint: SettingsEndpoint::new("https://proxy.example".to_owned(), None),
            auth_config: None,
        }
    }

    #[tokio::test]
    #[serial_test::serial(startup_settings)]
    async fn ineligible_query_skips_without_starting_an_owner() {
        reset_startup_settings_for_tests();
        let query = ineligible_query();

        warm_startup_settings(query.clone(), || false);

        assert!(
            super::lock_state().owner.is_none(),
            "an ineligible warm must leave the slot free"
        );

        let wait = await_startup_settings(
            query,
            Duration::from_secs(5),
            &CancellationToken::new(),
            || false,
        )
        .await;

        let SettingsWait::Ready(outcome) = wait else {
            panic!("ineligible await must return immediately, got {wait:?}");
        };
        assert!(outcome.settings.is_none());
        assert!(!outcome.attempted);
        assert!(
            super::lock_state().owner.is_none(),
            "a skipped await must not seal the startup watch"
        );
    }

    #[tokio::test]
    #[serial_test::serial(startup_settings)]
    async fn caller_timeout_does_not_drop_a_later_publish() {
        reset_startup_settings_for_tests();
        let (tx, _rx0) = tokio::sync::watch::channel(None);
        let published = std::sync::Arc::new(AtomicBool::new(false));
        let flag = published.clone();
        let task_tx = tx.clone();

        let owner = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(80)).await;
            flag.store(true, Ordering::SeqCst);
            task_tx.send_replace(Some(SettingsOutcome {
                settings: None,
                attempted: true,
                origin: String::new(),
                identity: String::new(),
            }));
        });

        let wait = super::observe(
            tx.subscribe(),
            Some(Duration::from_millis(20)),
            &CancellationToken::new(),
        )
        .await;

        assert!(
            matches!(wait, SettingsWait::TimedOut),
            "a short observation must time out, got {wait:?}"
        );
        assert!(
            !published.load(Ordering::SeqCst),
            "timing out the observation must not stop the owner"
        );

        let later = tokio::time::timeout(
            Duration::from_secs(10),
            super::observe(tx.subscribe(), None, &CancellationToken::new()),
        )
        .await
        .expect("later publish must land");

        assert!(matches!(later, SettingsWait::Ready(_)));
        assert!(published.load(Ordering::SeqCst));

        owner.abort();
        reset_startup_settings_for_tests();
    }

    #[tokio::test]
    #[serial_test::serial(startup_settings)]
    async fn cancel_drops_only_the_observation() {
        reset_startup_settings_for_tests();
        let (tx, _rx) = tokio::sync::watch::channel(None);
        let cancel = CancellationToken::new();
        let cancel_for_wait = cancel.clone();
        let rx = tx.subscribe();
        let wait = tokio::spawn(async move {
            super::observe(rx, Some(Duration::from_secs(30)), &cancel_for_wait).await
        });

        cancel.cancel();

        match wait.await.expect("observe task") {
            SettingsWait::Cancelled => {}
            other => panic!("expected cancel, got {other:?}"),
        }

        drop(tx);
        reset_startup_settings_for_tests();
    }
}
