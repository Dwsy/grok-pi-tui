use super::*;
use crate::events::TelemetryEvent;

#[test]
fn event_names_are_stable() {
    assert_eq!(
        "file_acceleration_session_started",
        FileAccelerationSessionStarted::NAME
    );
    assert_eq!(
        "file_acceleration_session_ended",
        FileAccelerationSessionEnded::NAME
    );
    assert_eq!(
        "file_acceleration_unavailable_hit",
        FileAccelerationUnavailableHit::NAME
    );
}

#[test]
fn events_serialize_to_content_free_fields() {
    assert_eq!(
        serde_json::json!({ "session_id": "s", "arm": "control" }),
        serde_json::to_value(FileAccelerationSessionStarted {
            session_id: "s".to_owned(),
            arm: FileAccelerationArm::Control,
        })
        .expect("serialize started"),
    );
    assert_eq!(
        serde_json::json!({
            "session_id": "s",
            "arm": "treatment",
            "duration_ms": 1200,
            "served": 3,
            "declined": { "uncovered": 2, "link_not_ready": 1 },
            "failed": { "incomplete": 1 },
            "served_latency_buckets": [2, 1, 0],
            "served_latency_bucket_edges_ms": [10, 50],
            "saturated": 1,
        }),
        serde_json::to_value(FileAccelerationSessionEnded {
            session_id: "s".to_owned(),
            arm: FileAccelerationArm::Treatment,
            duration_ms: 1200,
            served: 3,
            declined: BTreeMap::from([("uncovered", 2), ("link_not_ready", 1)]),
            failed: BTreeMap::from([("incomplete", 1)]),
            served_latency_buckets: vec![2, 1, 0],
            served_latency_bucket_edges_ms: vec![10, 50],
            saturated: 1,
        })
        .expect("serialize ended"),
    );
    assert_eq!(
        serde_json::json!({ "session_id": "s", "label": "connect", "retry_in_ms": 5000 }),
        serde_json::to_value(FileAccelerationUnavailableHit {
            session_id: "s".to_owned(),
            label: "connect",
            retry_in_ms: 5000,
        })
        .expect("serialize unavailable hit"),
    );
}
