//! Reads the `[features] remote_fetch` setting from the config layers.

use crate::{Capability, Distribution};

/// Returns whether fetches of `/v1/models` and `/v1/settings` from xAI backends are allowed.
/// The startup prefetch calls this for its deployment-config sync, before any `AgentConfig` exists.
/// `[features] managed_config` controls the background managed-config sync.
pub fn resolve_remote_fetch_enabled() -> bool {
    match crate::ConfigLayers::load() {
        Ok(layers) => remote_fetch_enabled_from_layers(&layers),
        // A corrupt user `config.toml` must not discard the requirements and managed settings
        Err(_) => remote_fetch_enabled_from_policy_layers(
            crate::load_merged_requirements().as_ref(),
            crate::load_managed_config().ok().as_ref(),
            crate::load_system_managed_config().ok().as_ref(),
        ),
    }
}

/// `effective_config_base` puts the user layer over managed config.
/// Here a deployment's `remote_fetch = false` wins over a user's `remote_fetch = true`.
pub fn remote_fetch_enabled_from_layers(layers: &crate::ConfigLayers) -> bool {
    // Each new `ConfigLayers` field must go in the list below or be skipped with `_`
    // `env_overlay` and `campaigns` may not override a `remote_fetch = false` from the user or a deployment
    let crate::ConfigLayers {
        system_managed,
        managed,
        user,
        env_overlay: _,
        user_requirements,
        system_requirements,
        mdm_requirements,
        campaigns: _,
    } = layers;
    remote_fetch_enabled_first_match(
        Distribution::current(),
        [
            mdm_requirements.as_ref(),
            system_requirements.as_ref(),
            user_requirements.as_ref(),
            Some(managed),
            Some(system_managed),
            Some(user),
        ],
    )
}

/// The fallback for [`resolve_remote_fetch_enabled`] when `ConfigLayers::load` fails.
/// `load_merged_requirements` lets MDM override system and system override user, like `remote_fetch_enabled_from_layers`.
/// The user `config.toml` is skipped because it holds no deployment policy.
fn remote_fetch_enabled_from_policy_layers(
    merged_requirements: Option<&toml::Value>,
    managed: Option<&toml::Value>,
    system_managed: Option<&toml::Value>,
) -> bool {
    remote_fetch_enabled_first_match(
        Distribution::current(),
        [merged_requirements, managed, system_managed],
    )
}

/// Both public entry points end here, so neither can skip the distribution.
fn remote_fetch_enabled_first_match<'a>(
    distribution: Distribution,
    layers: impl IntoIterator<Item = Option<&'a toml::Value>>,
) -> bool {
    if !distribution.allows(Capability::RemoteFetch) {
        return false;
    }
    layers
        .into_iter()
        .flatten()
        .find_map(remote_fetch_value)
        .unwrap_or(true)
}

fn remote_fetch_value(v: &toml::Value) -> Option<bool> {
    v.get("features")?.get("remote_fetch")?.as_bool()
}

#[cfg(test)]
#[path = "remote_fetch_tests.rs"]
mod tests;
