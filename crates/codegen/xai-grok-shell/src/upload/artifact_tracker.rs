//! Shared, side-effect-free artifact accounting used by both Pi and Stock.
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, serde::Serialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ArtifactStatus {
    Succeeded,
    Failed,
    Skipped,
    Enqueued,
}

#[derive(Debug, serde::Serialize, Clone)]
pub(crate) struct FailureDetail {
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Default)]
pub(crate) struct ArtifactTrackerInner {
    pub statuses: HashMap<String, ArtifactStatus>,
    pub failures: HashMap<String, FailureDetail>,
    pub skips: HashMap<String, String>,
}

pub(crate) type ArtifactTracker = Arc<parking_lot::Mutex<ArtifactTrackerInner>>;

pub(crate) fn new_artifact_tracker() -> ArtifactTracker {
    Arc::new(parking_lot::Mutex::new(ArtifactTrackerInner::default()))
}

pub(crate) enum ArtifactResult<'a> {
    Succeeded,
    Enqueued,
    Failed {
        reason: &'a str,
        error: Option<&'a str>,
    },
}

pub(crate) fn record_artifact(
    tracker: &ArtifactTracker,
    filename: &str,
    result: ArtifactResult<'_>,
) {
    match result {
        ArtifactResult::Succeeded => {
            tracker
                .lock()
                .statuses
                .insert(filename.to_owned(), ArtifactStatus::Succeeded);
        }
        ArtifactResult::Enqueued => {
            tracker
                .lock()
                .statuses
                .insert(filename.to_owned(), ArtifactStatus::Enqueued);
        }
        ArtifactResult::Failed { reason, error } => {
            let key = filename.to_owned();
            let mut inner = tracker.lock();
            inner.statuses.insert(key.clone(), ArtifactStatus::Failed);
            inner.failures.insert(
                key,
                FailureDetail {
                    reason: reason.to_owned(),
                    error: error.map(|s| truncate(s).to_owned()),
                },
            );
        }
    }
}

fn truncate(s: &str) -> &str {
    match s.char_indices().nth(512) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

pub(crate) fn skip_artifact(tracker: &ArtifactTracker, filename: &str, reason: &str) {
    let key = filename.to_owned();
    let mut inner = tracker.lock();
    inner.statuses.insert(key.clone(), ArtifactStatus::Skipped);
    inner.skips.insert(key, reason.to_owned());
}
