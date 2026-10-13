use std::path::PathBuf;

use super::*;
use crate::managed_cache::SyncMarker;
use crate::managed_cache::mark_managed_config_synced_at;
use crate::validation::RequirementsSource;

#[test]
fn evaluate_trusts_requirements_when_the_home_does_not_resolve() {
    let identity = ServingIdentity::Team("team-a".to_owned());
    assert_eq!(
        ManagedPolicyTrust::Trusted,
        ManagedPolicyTrust::evaluate(None, &[layer()], &identity),
    );
}

#[test]
fn evaluate_trusts_requirements_in_a_clean_home() {
    let home = tempfile::tempdir().expect("temp home is created");
    let present = vec![layer()];
    let identity = ServingIdentity::Team("team-a".to_owned());
    let cases = [
        (Vec::new(), ManagedPolicyTrust::Unmanaged),
        (present, ManagedPolicyTrust::Trusted),
    ];

    crate::signed_policy::test_seam::with_dark(|| {
        for (layers, expected) in cases {
            assert_eq!(
                expected,
                ManagedPolicyTrust::evaluate(Some(home.path()), &layers, &identity),
            );
        }
    });
}

#[test]
fn a_missing_policy_file_compromises_that_home() {
    let home = tempfile::tempdir().expect("temp home is created");
    let present = vec![layer()];
    let identity = ServingIdentity::Team("team-a".to_owned());

    std::fs::write(home.path().join("requirements.toml"), "[features]\n")
        .expect("requirements file is written");
    mark_managed_config_synced_at(
        home.path(),
        SyncMarker {
            principal: Some("team-a"),
            had_managed_config: false,
            had_requirements: true,
            key_fingerprint: None,
            fail_closed: true,
        },
    );
    std::fs::remove_file(home.path().join("requirements.toml"))
        .expect("requirements file is removed");

    let trust = crate::signed_policy::test_seam::with_dark(|| {
        ManagedPolicyTrust::evaluate(Some(home.path()), &present, &identity)
    });

    assert_eq!(
        ManagedPolicyTrust::Compromised(ManagedPolicyCompromise::PolicyFileMissing),
        trust,
    );
}

fn layer() -> RequirementsLayer {
    RequirementsLayer {
        value: toml::from_str("[models]\ndefault = \"grok-4\"\n")
            .expect("requirements TOML parses"),
        source: RequirementsSource::File(PathBuf::from("/etc/grok/requirements.toml")),
        is_system: true,
    }
}
