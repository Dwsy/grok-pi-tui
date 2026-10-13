//! Config-value resolution types re-exported from xai-grok-config, and per-model laziness config extracted from xai-grok-shell

pub use xai_grok_config::{BoolFlag, ConfigSource, Resolved};

/// Per-model configuration for the Layer-3 LazinessDetector.
/// Every field defaults to disabled.
/// `enabled = true` alone only classifies and emits `LazinessClassifierFired`; a nudge also needs `max_nudges_per_session > 0`.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LazinessDetectorPerModelConfig {
    /// Master switch. When `false` (the default), the classifier never fires for this model.
    #[serde(default)]
    pub enabled: bool,
    /// Hard cap on `<system-reminder>` nudges injected per session for this model.
    /// The default `0` makes `enabled = true` observation-only: the classifier fires but injects nothing.
    #[serde(default)]
    pub max_nudges_per_session: u32,
    /// How long the session must be idle before the classifier runs.
    /// `None` defers to the harness default (10 seconds).
    #[serde(default)]
    pub idle_threshold_ms: Option<u64>,
    /// Minimum classifier confidence required to inject a nudge.
    /// `None` defers to the harness default (0.7).
    #[serde(default)]
    pub min_confidence: Option<f32>,
    /// When `Some(true)` (or `None`, the default), the classifier sees the assistant's plain-text reasoning as `[assistant reasoning]` lines.
    /// `Some(false)` drops those lines, the behavior before 2026-05.
    /// `None` defers to the harness default (`LAZINESS_INCLUDE_REASONING`, currently `true`).
    #[serde(default)]
    pub include_reasoning: Option<bool>,
}
