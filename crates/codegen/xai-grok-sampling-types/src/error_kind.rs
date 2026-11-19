//! Wire-visible sampling error classification.

use serde::{Deserialize, Serialize};

/// Coarse classification of a sampling failure, carried on session wire updates.
///
/// Intentionally narrow: context-window-exceeded has NO variant because the sampler
/// lacks the tracked token counts to detect it reliably.
/// Do not "clean up" with `rename_all`.
#[derive(
    Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, strum::AsRefStr, strum::IntoStaticStr,
)]
#[strum(serialize_all = "snake_case")]
pub enum SamplingErrorKind {
    Auth,
    Http,
    Api,
    Serialization,
    IdleTimeout,
    RateLimited,
    EmptyResponse,
    MaxTokensTruncation,
    DoomLoopDetected,
}

/// [`SamplingErrorKind::from_str`] error: the wire string matched no known kind (a
/// newer peer's kind); callers degrade to untyped via `.ok()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnknownSamplingErrorKind;

/// Inverse of [`SamplingErrorKind::as_str`]; the round-trip test exercises both maps
/// for every listed variant.
impl std::str::FromStr for SamplingErrorKind {
    type Err = UnknownSamplingErrorKind;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "auth" => Self::Auth,
            "http" => Self::Http,
            "api" => Self::Api,
            "serialization" => Self::Serialization,
            "idle_timeout" => Self::IdleTimeout,
            "rate_limited" => Self::RateLimited,
            "empty_response" => Self::EmptyResponse,
            "max_tokens_truncation" => Self::MaxTokensTruncation,
            "doom_loop_detected" => Self::DoomLoopDetected,
            _ => return Err(UnknownSamplingErrorKind),
        })
    }
}
