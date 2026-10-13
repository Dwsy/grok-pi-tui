//! Model-catalog fetch transport.

use indexmap::IndexMap;

use super::{ModelFetchAuth, ModelsCacheManager, resolve_models_cache_scope};
use crate::agent::config::{self, ModelEntry};
use crate::remote::{FetchModelsResult, ModelSource, active_model_source};
use xai_grok_cloud_config::remote_settings::{
    ModelsPrefetch, fetch_catalog, prefetch_catalog_blocking,
};
use xai_grok_login::GrokAuth;

pub(crate) fn build_prefetched_map(
    models: Vec<config::ModelEntryConfig>,
    api_base_url_override: Option<String>,
) -> IndexMap<String, ModelEntry> {
    let mut map: IndexMap<String, ModelEntry> = IndexMap::with_capacity(models.len());
    for model in models {
        let key = model.id.clone().unwrap_or_else(|| model.model.clone());
        let info = config::ModelInfo::from_config(&model);
        let entry = ModelEntry {
            info,
            mtls_cert_dir: None,
            api_key: None,
            env_key: None,
            auth_provider: None,
            api_base_url: model.api_base_url.clone().or(api_base_url_override.clone()),
        };
        map.insert(key, entry);
    }
    map
}

pub(crate) fn prefetch_models_blocking(
    endpoints: &config::EndpointsConfig,
    auth: Option<&GrokAuth>,
    fetch_auth: ModelFetchAuth,
) -> Option<IndexMap<String, ModelEntry>> {
    let source = active_model_source(endpoints, fetch_auth);
    let scope = resolve_models_cache_scope(endpoints, fetch_auth, auth);
    prefetch_catalog_blocking(
        &ModelsCacheManager::new(),
        &scope,
        crate::agent::remote_config::models_fetch_enabled(endpoints, auth),
        || fetched_model_map(&source, auth, endpoints, fetch_auth),
    )
}

pub(in crate::agent::remote_config) fn fetch_models_uncommitted(
    endpoints: &config::EndpointsConfig,
    auth: Option<&GrokAuth>,
    fetch_auth: ModelFetchAuth,
    remote_fetch_enabled: bool,
) -> ModelsPrefetch<ModelEntry> {
    let source = active_model_source(endpoints, fetch_auth);
    let scope = resolve_models_cache_scope(endpoints, fetch_auth, auth);
    fetch_catalog(
        &ModelsCacheManager::new(),
        &scope,
        remote_fetch_enabled,
        || fetched_model_map(&source, auth, endpoints, fetch_auth),
    )
}

fn fetched_model_map(
    source: &impl ModelSource,
    auth: Option<&GrokAuth>,
    endpoints: &config::EndpointsConfig,
    fetch_auth: ModelFetchAuth,
) -> Option<(IndexMap<String, ModelEntry>, Option<String>)> {
    let _timer = crate::instrumentation_timer!("startup.fetch_models_blocking");
    match source.fetch(auth) {
        Ok(FetchModelsResult { models, etag }) if !models.is_empty() => {
            let api_base_url_override = match fetch_auth {
                ModelFetchAuth::ApiKey => Some(endpoints.xai_api_base_url.clone()),
                ModelFetchAuth::Session
                | ModelFetchAuth::Deployment
                | ModelFetchAuth::CustomEndpoint => None,
            };
            let map = build_prefetched_map(models, api_base_url_override);
            tracing::info!(count = map.len(), etag = ?etag, "Prefetched models");
            Some((map, etag))
        }
        Ok(FetchModelsResult { .. }) => {
            tracing::warn!("Models endpoint returned empty list");
            None
        }
        Err(error) => {
            tracing::warn!(error = ?error, "Failed to fetch models");
            None
        }
    }
}
