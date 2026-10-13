use super::*;
use crate::{Feature, FeatureSources, RemoteSettings};

#[test]
fn routes_resolve_env_over_config_over_remote() {
    assert_eq!(
        Some(Arc::from("env")),
        resolve_file_acceleration_routes(Some("env"), Some("config"), Some("remote"))
    );
    assert_eq!(
        Some(Arc::from("config")),
        resolve_file_acceleration_routes(None, Some("config"), Some("remote"))
    );
    assert_eq!(
        Some(Arc::from("remote")),
        resolve_file_acceleration_routes(None, None, Some("remote"))
    );
    assert_eq!(None, resolve_file_acceleration_routes(None, None, None));
}

#[test]
fn blank_routes_are_unset_at_every_tier() {
    assert_eq!(
        Some(Arc::from("remote")),
        resolve_file_acceleration_routes(Some(""), Some("  "), Some("remote"))
    );
    assert_eq!(
        None,
        resolve_file_acceleration_routes(Some(""), Some(""), Some("\t"))
    );
}

#[test]
fn routes_pass_through_unparsed() {
    assert_eq!(
        Some(Arc::from(" fuse=on, nfs=off ")),
        resolve_file_acceleration_routes(None, Some(" fuse=on, nfs=off "), None)
    );
}

/// The rollout contract: remote turns it on for a cohort, any local tier overrides remote either way, and a pin beats them all.
#[test]
fn feature_is_off_by_default_and_local_tiers_override_remote() {
    let remote_on = RemoteSettings {
        file_acceleration_enabled: Some(true),
        ..RemoteSettings::default()
    };
    let remote = Feature::FileAcceleration.remote_value(Some(&remote_on));
    let resolve = |sources: FeatureSources| Feature::FileAcceleration.resolve(sources).value;

    assert!(!resolve(FeatureSources::default()));
    assert!(resolve(FeatureSources {
        remote,
        ..FeatureSources::default()
    }));
    assert!(!resolve(FeatureSources {
        config: Some(false),
        remote,
        ..FeatureSources::default()
    }));
    assert!(!resolve(FeatureSources {
        env: Some(false),
        config: Some(true),
        remote,
        ..FeatureSources::default()
    }));
    assert!(resolve(FeatureSources {
        env: Some(true),
        remote: Some(false),
        ..FeatureSources::default()
    }));
    assert!(!resolve(FeatureSources {
        pin: Some(false),
        env: Some(true),
        config: Some(true),
        remote,
    }));
}
