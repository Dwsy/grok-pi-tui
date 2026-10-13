use crate::util::config::RemoteSettings;
use toml::Value as TomlValue;

/// Resolve whether ZDR users are allowed to use the product.
///
/// Precedence: requirements > env > config.toml > managed > remote settings > default (false).
pub fn resolve_zdr_access_enabled(
    requirements: Option<&TomlValue>,
    user: Option<&TomlValue>,
    managed: Option<&TomlValue>,
    remote: Option<&RemoteSettings>,
) -> bool {
    use crate::agent::config::BoolFlag;
    fn from_toml(v: Option<&TomlValue>) -> Option<bool> {
        v?.get("features")?.get("zdr_access_enabled")?.as_bool()
    }
    BoolFlag::env("GROK_ZDR_ACCESS_ENABLED")
        .requirement(from_toml(requirements))
        .config(from_toml(user))
        .managed(from_toml(managed))
        .feature_flag(remote.and_then(|r| r.zdr_access_enabled))
        .resolve()
        .value
}

pub(crate) fn turn_transient_retry_from_toml(v: Option<&TomlValue>) -> Option<bool> {
    v?.get("features")?.get("turn_transient_retry")?.as_bool()
}

/// Spawn-time kill switch: `[features] turn_transient_retry`, `GROK_TURN_TRANSIENT_RETRY`, remote key (below local), default on.
/// Loads local layers itself; call once per session.
pub(crate) fn resolve_turn_transient_retry(remote: Option<bool>) -> bool {
    let user_cfg = crate::config::load_effective_config().ok();
    // Merged (user, system, MDM last-wins) like the sibling resolvers, so an enterprise pin is not silently dropped
    let requirements = crate::config::load_merged_requirements();
    compose_turn_transient_retry(requirements.as_ref(), user_cfg.as_ref(), remote)
}

fn compose_turn_transient_retry(
    requirements: Option<&TomlValue>,
    user: Option<&TomlValue>,
    remote: Option<bool>,
) -> bool {
    use turn_transient_retry_from_toml as from_toml;
    crate::agent::config::resolve_turn_transient_retry(
        from_toml(requirements),
        /* cli          */ None,
        from_toml(user),
        /* managed: merged into the config layer by load_effective_config */ None,
        remote,
    )
    .value
}

pub use xai_grok_config::resolve_remote_fetch_enabled;

pub const REMOTE_FETCH_CONFIG_PATH: &str = "features.remote_fetch";

/// Keys whose dedicated resolver walks managed before user `config.toml`.
/// The effective merge lets the user file win for every other key.
pub const MANAGED_WINS_OVER_USER: &[&str] = &[REMOTE_FETCH_CONFIG_PATH];

#[cfg(test)]
mod turn_transient_retry_toml_tests {
    use super::turn_transient_retry_from_toml;

    #[test]
    fn composition_orders_requirements_over_config_over_remote() {
        let t = |b: bool| -> toml::Value {
            let mut f = toml::map::Map::new();
            f.insert("turn_transient_retry".into(), toml::Value::Boolean(b));
            let mut root = toml::map::Map::new();
            root.insert("features".into(), toml::Value::Table(f));
            toml::Value::Table(root)
        };
        // requirements beat user config; config beats remote; remote beats default.
        assert!(!super::compose_turn_transient_retry(
            Some(&t(false)),
            Some(&t(true)),
            None
        ));
        assert!(!super::compose_turn_transient_retry(
            None,
            Some(&t(false)),
            Some(true)
        ));
        assert!(!super::compose_turn_transient_retry(
            None,
            None,
            Some(false)
        ));
        assert!(super::compose_turn_transient_retry(None, None, None));
    }

    #[test]
    fn reads_the_features_key_and_defaults_absent_to_none() {
        let v: toml::Value = toml::toml! { [features] turn_transient_retry = false }.into();
        assert_eq!(turn_transient_retry_from_toml(Some(&v)), Some(false));
        let empty: toml::Value = toml::toml! { [features] }.into();
        assert_eq!(turn_transient_retry_from_toml(Some(&empty)), None);
        assert_eq!(turn_transient_retry_from_toml(None), None);
    }
}
