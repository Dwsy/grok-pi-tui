//! Auth resolution for `/v1/models` fetching.

use indexmap::IndexMap;

use crate::agent::auth_method::read_xai_api_key_env;
use crate::agent::config::{self, ModelEntry};
use crate::remote::{ModelSource, active_model_source};
use xai_grok_cloud_config::remote_settings::{CacheAuthMethod, ModelsCacheScope, scope_hash};
use xai_grok_login::{AuthMode, GrokAuth, GrokComConfig};

/// Returns a login minted by an `auth_provider_command`, which issues it for the operator's own endpoints.
/// The provider controls the issuer it declares, so the issuer does not change the answer.
pub(crate) fn external_provider_auth(auth: Option<&GrokAuth>) -> Option<&GrokAuth> {
    auth.filter(|a| a.auth_mode == AuthMode::External)
}

/// Whether the models list may be fetched.
/// `remote_fetch = false` does not stop an external provider login from reading its own custom models endpoint.
pub(crate) fn models_fetch_enabled(
    endpoints: &config::EndpointsConfig,
    auth: Option<&GrokAuth>,
) -> bool {
    (endpoints.has_custom_endpoint() && external_provider_auth(auth).is_some())
        || crate::util::config::resolve_remote_fetch_enabled()
}

/// Credential for `/v1/models` fetching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModelFetchAuth {
    Session,
    ApiKey,
    Deployment,
    CustomEndpoint,
}

impl ModelFetchAuth {
    /// Precedence: custom_endpoint, then session, then deployment, then API key.
    pub(crate) fn resolve(endpoints: &config::EndpointsConfig, has_cached_session: bool) -> Self {
        if endpoints.has_custom_endpoint() {
            Self::CustomEndpoint
        } else if has_cached_session {
            Self::Session
        } else if endpoints.deployment_key.is_some() {
            Self::Deployment
        } else if crate::agent::auth_method::has_xai_api_key_env() {
            Self::ApiKey
        } else {
            Self::Session
        }
    }

    pub(in crate::agent::remote_config) fn cache_auth_method(&self) -> CacheAuthMethod {
        match self {
            Self::CustomEndpoint | Self::ApiKey => CacheAuthMethod::ApiKey,
            Self::Session => CacheAuthMethod::Session,
            Self::Deployment => CacheAuthMethod::Deployment,
        }
    }
}

pub(in crate::agent::remote_config) fn resolve_models_cache_scope(
    endpoints: &config::EndpointsConfig,
    fetch_auth: ModelFetchAuth,
    auth: Option<&GrokAuth>,
) -> ModelsCacheScope {
    let origin = active_model_source(endpoints, fetch_auth).cache_origin();
    let alpha = endpoints.alpha_test_key.as_deref();
    let identity = match fetch_auth {
        // Session identity matches the settings cache so a session boot keeps
        // hitting its existing entry; the empty fallback (no credential) still
        // misses safely.
        ModelFetchAuth::Session => auth
            .map(|a| xai_grok_cloud_config::settings_cache_identity(a, alpha))
            .unwrap_or_default(),
        ModelFetchAuth::ApiKey => {
            let key = read_xai_api_key_env().unwrap_or_default();
            scope_hash(&["models-api-key", key.as_str(), alpha.unwrap_or("")])
        }
        ModelFetchAuth::Deployment => scope_hash(&[
            "models-deployment",
            endpoints.deployment_key.as_deref().unwrap_or(""),
            alpha.unwrap_or(""),
        ]),
        // Custom endpoints authenticate with `XAI_API_KEY`, else an external provider token.
        // Any other login identity never scopes this cache.
        ModelFetchAuth::CustomEndpoint => {
            match (read_xai_api_key_env(), external_provider_auth(auth)) {
                (Err(_), Some(provider)) => scope_hash(&[
                    "models-custom-endpoint-external",
                    origin.as_str(),
                    xai_grok_cloud_config::settings_cache_identity(provider, alpha).as_str(),
                ]),
                (key, _) => scope_hash(&[
                    "models-custom-endpoint",
                    origin.as_str(),
                    key.unwrap_or_default().as_str(),
                    alpha.unwrap_or(""),
                ]),
            }
        }
    };
    ModelsCacheScope {
        auth_method: fetch_auth.cache_auth_method(),
        origin,
        identity,
    }
}

/// Re-resolve the scope for the commit gate under the fetch-time mode (so the
/// origin reflects what was actually fetched), while reading live disk auth
/// for the identity. Re-deriving the mode from live auth would flip the origin
/// (e.g. proxy to api.x.ai) in the just-logged-in / sign-out window and wrongly
/// abandon a good catalog; the identity still owns real credential changes.
pub(in crate::agent::remote_config) fn resolve_live_models_cache_scope(
    fetch_auth: ModelFetchAuth,
    commit_config: Option<&GrokComConfig>,
) -> ModelsCacheScope {
    let endpoints = super::resolve_startup_endpoints();
    let auth = super::resolve_disk_auth(commit_config.cloned());
    resolve_models_cache_scope(&endpoints, fetch_auth, auth.as_ref())
}

pub(crate) fn task_model_error_for_catalog(
    requested: &str,
    available: &IndexMap<String, ModelEntry>,
    is_session_auth: bool,
) -> Option<String> {
    let is_available = |entry: &ModelEntry| entry.info.is_picker_eligible(is_session_auth);
    if config::find_model_by_id(available, requested).is_some_and(&is_available) {
        return None;
    }

    let mut slugs = available
        .iter()
        .filter(|(_, entry)| is_available(entry))
        .map(|(slug, _)| slug.as_str())
        .collect::<Vec<_>>();
    slugs.sort_unstable();
    let guidance = if slugs.is_empty() {
        "No valid model slugs are currently available. Omit `model` to inherit the parent model."
            .to_string()
    } else {
        format!(
            "Valid model slugs: {}. Omit `model` to inherit the parent model.",
            slugs.join(", ")
        )
    };
    Some(format!("Unknown Task.model slug '{requested}'. {guidance}"))
}
