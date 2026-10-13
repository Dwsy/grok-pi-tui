//! Content-free product telemetry for file acceleration: counts, latency buckets, and built-in reason labels only.
//! Each event carries `session_id` because the shell may emit it outside the session's telemetry scope.

use std::collections::BTreeMap;

use serde::Serialize;

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FileAccelerationArm {
    Treatment,
    Control,
}

#[derive(Serialize, Debug, PartialEq, Eq)]
pub struct FileAccelerationSessionStarted {
    pub session_id: String,
    pub arm: FileAccelerationArm,
}

#[derive(Serialize, Debug, PartialEq, Eq)]
pub struct FileAccelerationSessionEnded {
    pub session_id: String,
    pub arm: FileAccelerationArm,
    pub duration_ms: u64,
    pub served: u64,
    /// Declined searches by the accelerator's built-in reason label.
    pub declined: BTreeMap<&'static str, u64>,
    /// Searches handed back to the fallback, by built-in reason label.
    pub failed: BTreeMap<&'static str, u64>,
    /// Counts per bucket, one more than `served_latency_bucket_edges_ms`; the last counts everything slower.
    pub served_latency_buckets: Vec<u64>,
    /// Upper bound of every bucket but the last, carried on the row so a later edge change stays readable.
    pub served_latency_bucket_edges_ms: Vec<u64>,
    pub saturated: u64,
}

#[derive(Serialize, Debug, PartialEq, Eq)]
pub struct FileAccelerationUnavailableHit {
    pub session_id: String,
    pub label: &'static str,
    pub retry_in_ms: u64,
}

#[cfg(test)]
#[path = "file_acceleration_tests.rs"]
mod tests;
