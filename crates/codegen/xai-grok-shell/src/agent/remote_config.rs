//! This module fetches the model catalog, caches it, and loads it at startup.

mod endpoint;
mod fetch;
mod manager;
mod model_fetch_auth;
mod prefetch;
mod resolution;
pub mod settings_get;
pub(crate) mod task_model_policy;

pub(crate) use model_fetch_auth::{
    ModelFetchAuth, external_provider_auth, models_fetch_enabled, task_model_error_for_catalog,
};
pub(in crate::agent::remote_config) use model_fetch_auth::{
    resolve_live_models_cache_scope, resolve_models_cache_scope,
};
pub(in crate::agent::remote_config) use prefetch::resolve_startup_endpoints;
pub(crate) use prefetch::{
    ResolvedModels, fetch_initial_models_blocking, start_initial_models_load,
};
pub(crate) use resolution::{
    CatalogSource, allowlist_denied_message, allowlist_excludes_all_message,
    allowlist_matches_nothing, available_models, fallback_model_id, models_endpoint_empty_message,
    resolve_catalog_key, resolve_default_model, resolve_model_catalog,
    selectable_catalog_key_for_persisted, validate_selectable,
};
#[cfg(test)]
pub(in crate::agent::remote_config) use xai_grok_cloud_config::Commit;
pub(in crate::agent) use xai_grok_cloud_config::SettingsRefresh;
pub(crate) use xai_grok_cloud_config::remote_settings::{
    DegradedStartCause, ModelGlobSet, ModelsCacheScope, is_campaign_only_flip,
    record_degraded_start,
};
pub(in crate::agent::remote_config) use xai_grok_cloud_config::resolve_disk_auth;

pub(in crate::agent::remote_config) type ModelsCacheManager =
    xai_grok_cloud_config::remote_settings::ModelsCacheManager<crate::agent::config::ModelEntry>;

pub(crate) use endpoint::{HttpModelsEndpoint, ModelsEndpoint};
pub(in crate::agent::remote_config) use fetch::fetch_models_uncommitted;
pub(crate) use fetch::prefetch_models_blocking;
pub(crate) use manager::ModelsManager;
#[cfg(test)]
pub(in crate::agent::remote_config) use xai_grok_cloud_config::remote_settings::ModelsPrefetch;
#[cfg(test)]
pub(in crate::agent::remote_config) use xai_grok_cloud_config::remote_settings::evaluate_models_commit;

// Re-exports reached only through the manager test module.
#[cfg(test)]
pub(crate) use endpoint::ModelsFetchFuture;
#[cfg(test)]
pub(crate) use fetch::build_prefetched_map;
#[cfg(test)]
pub(crate) use prefetch::resolve_prefetch_inputs_from_parts;
#[cfg(test)]
pub(crate) use xai_grok_cloud_config::remote_settings::{
    CACHE_TTL, CacheAuthMethod, MODELS_CACHE_FILE, degraded_log_level,
};
#[cfg(test)]
pub(crate) type ModelsCache =
    xai_grok_cloud_config::remote_settings::ModelsCache<crate::agent::config::ModelEntry>;
