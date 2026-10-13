//! Startup model-catalog prefetch and handoff.

use indexmap::IndexMap;

use super::{
    ModelFetchAuth, ModelsCacheScope, fetch_models_uncommitted, models_fetch_enabled,
    resolve_disk_auth, resolve_live_models_cache_scope, resolve_models_cache_scope,
};
use crate::agent::config::{self, ModelEntry};
use xai_grok_cloud_config::remote_settings::{
    InitialModelsLoad, fetch_catalog_blocking, run_catalog_prefetch, start_catalog_load,
};
use xai_grok_login::{GrokAuth, GrokComConfig};

pub(crate) struct PrefetchInputs {
    pub(crate) auth: Option<GrokAuth>,
    pub(crate) endpoints: config::EndpointsConfig,
    pub(crate) model_fetch_auth: ModelFetchAuth,
}

/// Resolves startup endpoints from the effective config rather than env vars alone, so the prefetch cannot leak the bearer to api.x.ai.
pub(in crate::agent::remote_config) fn resolve_startup_endpoints() -> config::EndpointsConfig {
    let mut endpoints = config::EndpointsConfig::from_effective_config();
    if endpoints.deployment_key.is_none() {
        endpoints.deployment_key = xai_grok_cloud_config::managed_config::resolve_deployment_key();
    }
    endpoints
}

pub(crate) fn resolve_prefetch_inputs_from_parts(
    auth: Option<GrokAuth>,
    endpoints: config::EndpointsConfig,
    remote_fetch_enabled: bool,
) -> Option<PrefetchInputs> {
    if !remote_fetch_enabled {
        tracing::info!("startup model/settings prefetch skipped: remote_fetch disabled");
        return None;
    }

    let model_fetch_auth = ModelFetchAuth::resolve(&endpoints, auth.is_some());

    if auth.is_none()
        && !endpoints.has_custom_endpoint()
        && model_fetch_auth == ModelFetchAuth::Session
    {
        return None;
    }

    Some(PrefetchInputs {
        auth,
        endpoints,
        model_fetch_auth,
    })
}

/// Catalog from the async pre-resolve. Outer `Option`: whether pre-resolve ran;
/// inner: the catalog, or `None` on a failed or skipped fetch. Carried by value
/// in `BootstrapPrefetch` from the boot's settings resolve to sync bootstrap.
pub(crate) type ResolvedModels = Option<IndexMap<String, ModelEntry>>;

/// Resolved inputs for a models prefetch, or `None` when none would run.
///
/// Never syncs managed config: the refresh supervisor owns that, so a live
/// server cannot heal a tampered policy ahead of the fail-closed gate.
/// `grok_com_config` scopes the disk-auth read; the default config sees only env.
fn models_prefetch_inputs(
    grok_com_config: Option<GrokComConfig>,
    warmed_auth: Option<GrokAuth>,
) -> Option<ModelsPrefetchPlan> {
    if xai_grok_cloud_config::managed_config::policy_repair_pending() {
        return None;
    }
    // Prefer the live in-memory session so a just-refreshed or just-logged-in
    // credential drives the catalog fetch, not a stale or absent disk token.
    let auth = warmed_auth.or_else(|| resolve_disk_auth(grok_com_config.clone()));
    let endpoints = resolve_startup_endpoints();
    let remote = models_fetch_enabled(&endpoints, auth.as_ref());
    let env = resolve_prefetch_inputs_from_parts(auth.clone(), endpoints, remote)?;
    let expected = resolve_models_cache_scope(&env.endpoints, env.model_fetch_auth, auth.as_ref());
    Some(ModelsPrefetchPlan {
        env,
        expected,
        commit_config: grok_com_config,
    })
}

struct ModelsPrefetchPlan {
    env: PrefetchInputs,
    expected: ModelsCacheScope,
    commit_config: Option<GrokComConfig>,
}

/// Fetch the catalog and commit it under the policy/identity gate. The commit is
/// monotonic, so a late abandoned run cannot roll the cache back.
fn run_models_prefetch(
    plan: ModelsPrefetchPlan,
    cancel: &tokio_util::sync::CancellationToken,
) -> Option<IndexMap<String, ModelEntry>> {
    let ModelsPrefetchPlan {
        env,
        expected,
        commit_config,
    } = plan;
    let fetch_auth = env.model_fetch_auth;
    let endpoints = env.endpoints;
    let auth = env.auth;
    run_catalog_prefetch(
        cancel,
        &expected,
        || fetch_models_uncommitted(&endpoints, auth.as_ref(), fetch_auth, true),
        // Re-resolve under the fetch-time mode (stable origin) with live disk auth,
        // so an alpha flip, key rotation, or account switch is caught without
        // abandoning the catalog when disk auth is briefly absent.
        || {
            (
                resolve_live_models_cache_scope(fetch_auth, commit_config.as_ref()),
                models_fetch_enabled(&endpoints, auth.as_ref()),
            )
        },
    )
}

pub(crate) fn start_initial_models_load(
    cancel: tokio_util::sync::CancellationToken,
    grok_com_config: Option<GrokComConfig>,
    warmed_auth: Option<GrokAuth>,
) -> Option<InitialModelsLoad<ModelEntry>> {
    let plan = models_prefetch_inputs(grok_com_config, warmed_auth)?;
    start_catalog_load("grok-models-prefetch", cancel, move |cancel| {
        run_models_prefetch(plan, cancel)
    })
}

/// Poll slice bounding the sync models wait so a bootstrap cancel is observed promptly.
const MODELS_WAIT_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);

/// Synchronous catalog prefetch for a bootstrap that had no async pre-resolve.
/// A cancel or the fetch timeout ends the wait; the thread still lands its monotonic commit.
pub(crate) fn fetch_initial_models_blocking(
    cancel: &tokio_util::sync::CancellationToken,
    grok_com_config: Option<GrokComConfig>,
    warmed_auth: Option<GrokAuth>,
) -> Option<IndexMap<String, ModelEntry>> {
    let plan = models_prefetch_inputs(grok_com_config, warmed_auth)?;
    fetch_catalog_blocking(
        "grok-models-prefetch-sync",
        cancel,
        crate::http::STARTUP_FETCH_TIMEOUT,
        MODELS_WAIT_POLL_INTERVAL,
        move |cancel| run_models_prefetch(plan, cancel),
    )
}
