//! The memory-v2 rollout stage and raw `[memory_v2]` settings, shared by the local config table and
//! the remote settings object.

use serde::{Deserialize, Serialize};

/// Session-pinned memory-v2 rollout stage.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryV2Rollout {
    /// Disable every v2 read, capture, Dream, and write path.
    Off,
    /// Persist extracted observations without exposing them in manifests or Dream.
    RecordOnly,
    /// Evaluate capture and Dream plans without committing curated topic changes.
    Shadow,
    /// Enable the complete v2 pipeline.
    #[default]
    Active,
}

impl MemoryV2Rollout {
    /// xai-codegen-lint: allow(manual_strum)
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::RecordOnly => "record_only",
            Self::Shadow => "shadow",
            Self::Active => "active",
        }
    }

    pub fn allows_capture(self) -> bool {
        self != Self::Off
    }

    pub fn exposes_memory(self) -> bool {
        self == Self::Active
    }

    pub fn commits_topics(self) -> bool {
        self == Self::Active
    }

    pub fn restrict(self, other: Self) -> Self {
        fn rank(value: MemoryV2Rollout) -> u8 {
            match value {
                MemoryV2Rollout::Off => 0,
                MemoryV2Rollout::RecordOnly => 1,
                MemoryV2Rollout::Shadow => 2,
                MemoryV2Rollout::Active => 3,
            }
        }
        if rank(self) <= rank(other) {
            self
        } else {
            other
        }
    }
}

/// Raw top-level `[memory_v2]` enablement, rollout, kill-switch, and retention
/// settings.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MemoryV2Settings {
    /// Primary opt-in for the memory-v2 implementation. Absent or false falls
    /// through to legacy memory enablement.
    pub enabled: Option<bool>,
    /// Emit capture lifecycle notifications to the user interface. Intended
    /// only for debugging; telemetry and tracing are always recorded.
    pub capture_status_enabled: Option<bool>,
    pub rollout: Option<MemoryV2Rollout>,
    pub capture_enabled: Option<bool>,
    pub automatic_dream_enabled: Option<bool>,
    pub manual_dream_enabled: Option<bool>,
    /// Local value wins over remote `memory_v2.batch_dream_enabled`; unset on both sides means off.
    /// The Dream kill switches still apply.
    pub batch_dream_enabled: Option<bool>,
    pub batch_dream_max_run_secs: Option<u64>,
    pub batch_dream_max_calls_per_batch: Option<usize>,
    pub batch_dream_max_batch_note_bytes: Option<usize>,
    /// Inject a titles-only topic index into the system prompt; pending
    /// observations and topic descriptions are left out so more topics fit the
    /// same byte budget. Local value wins over remote
    /// `memory_v2.compact_index_enabled`; unset on both sides means off.
    pub compact_index_enabled: Option<bool>,
    pub file_writes_enabled: Option<bool>,
    pub archived_retention_days: Option<u64>,
    pub job_retention_days: Option<u64>,
}
