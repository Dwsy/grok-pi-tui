use std::collections::BTreeMap;

use pretty_assertions::assert_eq;
use xai_grok_tools::computer::{file_acceleration::LatencyBuckets, local::MockFs};

use super::*;
use crate::util::config::RemoteSettings;

const ALLOWED: GateInputs<'static> = GateInputs {
    is_feature_enabled: true,
    configured_profile: Some("off"),
    requested_confinement: None,
    is_inside_bwrap: false,
};

#[test]
fn gate_opens_only_with_the_feature_a_recorded_unconfined_profile_and_no_bwrap() {
    let cases = [
        (ALLOWED, GateDecision::Enabled),
        (
            GateInputs {
                is_feature_enabled: false,
                ..ALLOWED
            },
            GateDecision::FeatureOff,
        ),
        (
            GateInputs {
                configured_profile: None,
                ..ALLOWED
            },
            GateDecision::SandboxUnknown,
        ),
        (
            GateInputs {
                configured_profile: Some("workspace"),
                requested_confinement: Some("workspace"),
                ..ALLOWED
            },
            GateDecision::SandboxRequested,
        ),
        (
            GateInputs {
                is_inside_bwrap: true,
                ..ALLOWED
            },
            GateDecision::InsideBwrap,
        ),
        (
            GateInputs {
                is_feature_enabled: false,
                configured_profile: None,
                is_inside_bwrap: true,
                ..ALLOWED
            },
            GateDecision::FeatureOff,
        ),
    ];
    for (inputs, expected) in cases {
        assert_eq!(expected, decide_gate(inputs), "{inputs:?}");
    }
}

#[test]
fn acp_session_fs_never_reaches_the_accelerator() {
    let acp_fs: Arc<dyn AsyncFileSystem> = Arc::new(MockFs::new());

    let selected = select_session_fs(Some(Arc::clone(&acp_fs)), || {
        panic!("an ACP session must not build a file acceleration gate")
    });

    assert!(Arc::ptr_eq(&acp_fs, &selected.fs));
    assert!(selected.session.is_none());
}

#[tokio::test]
async fn local_session_fs_is_the_local_disk_when_the_gate_is_disabled() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("local");
    std::fs::write(&path, b"on disk").expect("write");
    let mut is_gate_built = false;

    let selected = select_session_fs(None, || {
        is_gate_built = true;
        FileAccelerationGate::Disabled
    });

    assert!(is_gate_built);
    assert!(selected.session.is_none());
    assert_eq!(
        b"on disk".to_vec(),
        selected.fs.read_file(&path).await.expect("local read")
    );
}

#[test]
fn settings_read_the_feature_and_the_routes_from_config_over_remote() {
    let mut config = Config::new_from_toml_cfg(
        &toml::from_str(
            "[features]\nfile_acceleration = true\n[file_acceleration]\nroutes = \"from-config\"\n",
        )
        .expect("toml"),
    )
    .expect("config");
    config.remote_settings = Some(RemoteSettings {
        file_acceleration_routes: Some("from-remote".to_owned()),
        ..RemoteSettings::default()
    });
    assert_eq!(
        FileAccelerationSettings {
            is_enabled: true,
            routes: Some(Arc::from("from-config")),
        },
        settings(&config)
    );

    config.file_acceleration.routes = None;
    assert_eq!(Some(Arc::from("from-remote")), settings(&config).routes);
}

#[test]
fn notices_become_content_free_session_events() {
    let mut served_latency = LatencyBuckets::default();
    served_latency[0] = 2;

    assert_eq!(
        AccelerationEvent::Started(FileAccelerationSessionStarted {
            session_id: "s".to_owned(),
            arm: FileAccelerationArm::Control,
        }),
        event_for(
            "s",
            AccelerationNotice::SessionStarted {
                arm: AccelerationArm::Control,
            }
        )
    );
    assert_eq!(
        AccelerationEvent::Ended(FileAccelerationSessionEnded {
            session_id: "s".to_owned(),
            arm: FileAccelerationArm::Treatment,
            duration_ms: 900,
            served: 2,
            declined: BTreeMap::from([("uncovered", 4)]),
            failed: BTreeMap::from([("incomplete", 1)]),
            served_latency_buckets: served_latency.to_vec(),
            served_latency_bucket_edges_ms: vec![10, 50, 100, 500, 1_000, 5_000],
            saturated: 3,
        }),
        event_for(
            "s",
            AccelerationNotice::SessionEnded {
                arm: AccelerationArm::Treatment,
                duration_ms: 900,
                served: 2,
                declined: BTreeMap::from([("uncovered", 4)]),
                failed: BTreeMap::from([("incomplete", 1)]),
                served_latency,
                saturated: 3,
            }
        )
    );
    assert_eq!(
        AccelerationEvent::UnavailableHit(FileAccelerationUnavailableHit {
            session_id: "s".to_owned(),
            label: "connect",
            retry_in_ms: 5_000,
        }),
        event_for(
            "s",
            AccelerationNotice::UnavailableHit {
                label: "connect",
                retry_in_ms: 5_000,
            }
        )
    );
}
