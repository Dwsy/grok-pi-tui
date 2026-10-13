use std::path::PathBuf;

use serde_json::Value;

use crate::command::violation::{Blocked, Capability};

use super::{OBSERVE_SUMMARY_MAX_ROWS, ObserveSummary, WouldBlockKind, WouldVerdict};

fn write(path: &str) -> Blocked {
    Blocked::FsWrite {
        path: PathBuf::from(path),
    }
}

fn json_str(json: &Value, pointer: &str) -> Option<String> {
    json.pointer(pointer)
        .and_then(Value::as_str)
        .map(str::to_owned)
}

#[test]
fn same_kind_and_target_aggregate_into_one_row() {
    let mut summary = ObserveSummary::default();
    summary.record(&write("/etc/hosts"), WouldVerdict::Ask, 10);
    summary.record(&write("/etc/hosts"), WouldVerdict::Ask, 12);
    summary.record(
        &Blocked::FsRead {
            path: PathBuf::from("/etc/hosts"),
        },
        WouldVerdict::Ask,
        11,
    );
    assert_eq!(2, summary.would_block.len());
    let row = summary
        .would_block
        .iter()
        .find(|r| r.kind == WouldBlockKind::FsWrite)
        .unwrap();
    assert_eq!((2, 12), (row.count, row.last_at));
    assert_eq!("/etc/hosts", row.target);
}

#[test]
fn rows_are_most_recent_first() {
    let mut summary = ObserveSummary::default();
    summary.record(&write("/a"), WouldVerdict::Ask, 1);
    summary.record(&write("/b"), WouldVerdict::Ask, 5);
    summary.record(&write("/c"), WouldVerdict::Ask, 3);
    let order: Vec<&str> = summary
        .would_block
        .iter()
        .map(|r| r.target.as_str())
        .collect();
    assert_eq!(vec!["/b", "/c", "/a"], order);
}

#[test]
fn table_is_capped_and_evicts_the_oldest_row() {
    let mut summary = ObserveSummary::default();
    for i in 0..OBSERVE_SUMMARY_MAX_ROWS {
        summary.record(
            &write(&format!("/p/{i}")),
            WouldVerdict::Ask,
            i as i64 + 100,
        );
    }
    assert_eq!(OBSERVE_SUMMARY_MAX_ROWS, summary.would_block.len());
    summary.record(&write("/newest"), WouldVerdict::Ask, 10_000);
    assert_eq!(OBSERVE_SUMMARY_MAX_ROWS, summary.would_block.len());
    assert_eq!(1, summary.evicted);
    assert!(
        !summary.would_block.iter().any(|r| r.target == "/p/0"),
        "the oldest row (last_at 100) is the one dropped"
    );
    assert_eq!("/newest", summary.would_block.first().unwrap().target);
}

#[test]
fn targets_render_per_kind_and_counts_group_by_kind() {
    let mut summary = ObserveSummary::default();
    summary.record(
        &Blocked::Net {
            host: Some("registry.npmjs.org".to_owned()),
            port: Some(443),
        },
        WouldVerdict::Ask,
        1,
    );
    summary.record(
        &Blocked::Net {
            host: Some("example.com".to_owned()),
            port: None,
        },
        WouldVerdict::Ask,
        1,
    );
    summary.record(
        &Blocked::Capability {
            what: Capability::Ptrace,
        },
        WouldVerdict::Ask,
        1,
    );
    summary.record(
        &Blocked::Unknown {
            stderr_snippet: "secret stderr".to_owned(),
        },
        WouldVerdict::Ask,
        1,
    );
    let targets: Vec<&str> = summary
        .would_block
        .iter()
        .map(|r| r.target.as_str())
        .collect();
    assert!(targets.contains(&"registry.npmjs.org:443"));
    assert!(targets.contains(&"example.com"));
    assert!(targets.contains(&"ptrace"));
    assert!(targets.contains(&""), "unknown carries no stderr content");
    assert!(!targets.iter().any(|t| t.contains("secret")));
    let counts = summary.counts_by_kind();
    assert_eq!(Some(&2), counts.get(&WouldBlockKind::Net));
    assert_eq!(Some(&1), counts.get(&WouldBlockKind::Capability));
}

/// The summary shows the verdict `enforce` would have reached per host; a
/// row a deny row started covering mid-observe reads as refused from then on, and an older
/// observation arriving late does not roll it back.
#[test]
fn a_rows_verdict_is_the_latest_observations() {
    let mut summary = ObserveSummary::default();
    let npm = Blocked::Net {
        host: Some("registry.npmjs.org".to_owned()),
        port: Some(443),
    };
    summary.record(&npm, WouldVerdict::Ask, 10);
    assert_eq!(
        WouldVerdict::Ask,
        summary.would_block.first().unwrap().verdict
    );
    summary.record(&npm, WouldVerdict::DenyRow, 12);
    assert_eq!(
        WouldVerdict::DenyRow,
        summary.would_block.first().unwrap().verdict
    );
    summary.record(&npm, WouldVerdict::Ask, 11);
    let row = summary.would_block.first().unwrap();
    assert_eq!(
        (WouldVerdict::DenyRow, 3, 12),
        (row.verdict, row.count, row.last_at)
    );
    summary.record(
        &Blocked::Net {
            host: Some("tracker.example".to_owned()),
            port: Some(443),
        },
        WouldVerdict::PolicyDenylist,
        13,
    );
    let json = serde_json::to_value(&summary).unwrap();
    assert_eq!(
        Some("policy_denylist".to_owned()),
        json_str(&json, "/would_block/0/verdict")
    );
    assert_eq!(
        Some("deny_row".to_owned()),
        json_str(&json, "/would_block/1/verdict")
    );
}

#[test]
fn summary_round_trips_through_json_with_snake_case_kinds() {
    let mut summary = ObserveSummary::default();
    summary.record(&write("/x"), WouldVerdict::Ask, 7);
    let json = serde_json::to_value(&summary).unwrap();
    assert_eq!(
        Some("fs_write".to_owned()),
        json_str(&json, "/would_block/0/kind")
    );
    assert_eq!(
        Some("ask".to_owned()),
        json_str(&json, "/would_block/0/verdict")
    );
    let back: ObserveSummary = serde_json::from_value(json).unwrap();
    assert_eq!(summary, back);
    summary.clear();
    assert!(summary.would_block.is_empty());
}
