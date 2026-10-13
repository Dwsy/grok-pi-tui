use super::*;
use crate::ConfigLayers;

fn features_remote_fetch(v: bool) -> toml::Value {
    toml::from_str(&format!("[features]\nremote_fetch = {v}\n"))
        .expect("remote_fetch fixture is valid TOML")
}

#[test]
fn remote_fetch_defaults_to_true_when_absent() {
    assert!(remote_fetch_enabled_from_layers(&ConfigLayers::default()));
}

#[test]
fn remote_fetch_reads_user_config() {
    let layers = ConfigLayers {
        user: features_remote_fetch(false),
        ..ConfigLayers::default()
    };
    assert!(!remote_fetch_enabled_from_layers(&layers));
}

#[test]
fn remote_fetch_managed_overrides_user() {
    let mut layers = ConfigLayers {
        user: features_remote_fetch(true),
        managed: features_remote_fetch(false),
        ..ConfigLayers::default()
    };
    assert!(
        !remote_fetch_enabled_from_layers(&layers),
        "managed=false must beat user=true"
    );

    layers.user = features_remote_fetch(false);
    layers.managed = features_remote_fetch(true);
    assert!(
        remote_fetch_enabled_from_layers(&layers),
        "managed=true must beat user=false"
    );
}

#[test]
fn remote_fetch_requirements_beat_managed_and_user() {
    let mut layers = ConfigLayers {
        user: features_remote_fetch(true),
        managed: features_remote_fetch(true),
        user_requirements: Some(features_remote_fetch(false)),
        ..ConfigLayers::default()
    };
    assert!(
        !remote_fetch_enabled_from_layers(&layers),
        "requirements=false must beat managed and user"
    );

    layers.user = features_remote_fetch(false);
    layers.managed = features_remote_fetch(false);
    layers.user_requirements = Some(features_remote_fetch(true));
    assert!(
        remote_fetch_enabled_from_layers(&layers),
        "requirements=true must beat managed and user"
    );
}

#[test]
fn remote_fetch_env_overlay_is_ignored_in_both_directions() {
    let layers = ConfigLayers {
        user: features_remote_fetch(false),
        env_overlay: Some(features_remote_fetch(true)),
        ..ConfigLayers::default()
    };
    assert!(!remote_fetch_enabled_from_layers(&layers));

    let layers = ConfigLayers {
        env_overlay: Some(features_remote_fetch(false)),
        ..ConfigLayers::default()
    };
    assert!(remote_fetch_enabled_from_layers(&layers));
}

#[test]
fn remote_fetch_prefers_managed_then_system_managed_then_user() {
    let mut layers = ConfigLayers {
        system_managed: features_remote_fetch(true),
        managed: features_remote_fetch(false),
        ..ConfigLayers::default()
    };
    assert!(!remote_fetch_enabled_from_layers(&layers));

    layers.managed = features_remote_fetch(true);
    layers.system_managed = features_remote_fetch(false);
    assert!(remote_fetch_enabled_from_layers(&layers));

    let layers = ConfigLayers {
        user: features_remote_fetch(true),
        system_managed: features_remote_fetch(false),
        ..ConfigLayers::default()
    };
    assert!(!remote_fetch_enabled_from_layers(&layers));
}

#[test]
fn remote_fetch_prefers_mdm_then_system_then_user_requirements() {
    let mut layers = ConfigLayers {
        user_requirements: Some(features_remote_fetch(true)),
        system_requirements: Some(features_remote_fetch(false)),
        ..ConfigLayers::default()
    };
    assert!(!remote_fetch_enabled_from_layers(&layers));

    layers.mdm_requirements = Some(features_remote_fetch(true));
    assert!(remote_fetch_enabled_from_layers(&layers));

    layers.mdm_requirements = Some(features_remote_fetch(false));
    layers.system_requirements = Some(features_remote_fetch(true));
    assert!(!remote_fetch_enabled_from_layers(&layers));
}

#[test]
fn remote_fetch_layer_load_failure_still_honors_policy_layers() {
    let off = features_remote_fetch(false);
    let on = features_remote_fetch(true);

    assert!(!remote_fetch_enabled_from_policy_layers(
        Some(&off),
        None,
        None
    ));

    assert!(!remote_fetch_enabled_from_policy_layers(
        None,
        None,
        Some(&off)
    ));

    assert!(remote_fetch_enabled_from_policy_layers(
        Some(&on),
        Some(&off),
        Some(&off)
    ));
    assert!(!remote_fetch_enabled_from_policy_layers(
        None,
        Some(&off),
        Some(&on)
    ));

    assert!(
        remote_fetch_enabled_from_policy_layers(None, None, None),
        "genuinely absent policy fails open"
    );
}

#[test]
fn a_distribution_that_withholds_remote_fetch_beats_every_layer() {
    let on = features_remote_fetch(true);
    assert!(!remote_fetch_enabled_first_match(
        Distribution::withholding(&[Capability::RemoteFetch]),
        [Some(&on), None]
    ));
    assert!(remote_fetch_enabled_first_match(
        Distribution::STOCK,
        [Some(&on), None]
    ));
}
