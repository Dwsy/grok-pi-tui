//! [`SamplerConfig`] is the per-request configuration handed to the sampler.
//! The type itself lives in `xai-grok-sampling-types` (pure data + contract traits) so
//! the shell can hold it without the sampler runtime; this module re-exports it and
//! keeps the sampler-only `RetryPolicy`.

use serde::{Deserialize, Serialize};

use crate::retry::{DEFAULT_MAX_RETRIES, RATE_LIMIT_RETRY_THRESHOLD};

/// Auth scheme and per-request sampler config.
///
/// Moved to `xai_grok_sampling_types` (pure data + contract traits) so the shell can
/// hold and persist a `SamplerConfig` without depending on the sampler runtime.
pub use xai_grok_sampling_types::{
    AuthScheme, HeaderInjector, SamplerConfig, SharedHeaderInjector,
};

/// Cheap sync read of the current bearer for [`SamplerConfig::bearer_resolver`].
/// Moved to `xai_grok_sampling_types` (pure contract) so `xai-grok-login`
/// can implement it without depending on the sampler runtime.
pub use xai_grok_sampling_types::{BearerResolver, SharedBearerResolver};

/// Retry knobs for the sampler's internal transport-error retry loop.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryPolicy {
    pub max_retries: u32,
    /// Total-attempt ceiling for rate-limited requests before escalating to the caller.
    /// Lower than `max_retries` because rate-limit waits can be long.
    pub rate_limit_retry_threshold: u32,
    #[serde(default)]
    pub retry_only_before_output: bool,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: DEFAULT_MAX_RETRIES,
            rate_limit_retry_threshold: RATE_LIMIT_RETRY_THRESHOLD,
            retry_only_before_output: false,
        }
    }
}

/// Identity of the client that originated the request, used for User-Agent rendering.
/// The shell layer composes this with platform info into a final UA string.
///
/// Moved to `xai_grok_sampling_types` (pure data) so `xai-grok-http` and
/// `xai-grok-telemetry` can name it without depending on this runtime crate.
pub use xai_grok_sampling_types::OriginClientInfo;

#[cfg(test)]
mod tests {
    use super::*;
    use xai_grok_sampling_types::DoomLoopRecoveryPolicy;

    /// Configs serialized before the field existed must keep deserializing.
    #[test]
    fn config_without_doom_loop_recovery_deserializes_to_none() {
        let mut stripped = serde_json::to_value(SamplerConfig::default()).unwrap();
        let object = stripped.as_object_mut().unwrap();
        object.remove("doom_loop_recovery");
        object.remove("extra_response_includes");
        object.remove("mtls_cert_dir");
        object.remove("rate_limit_retry_threshold");
        let config: SamplerConfig = serde_json::from_value(stripped).unwrap();
        assert!(config.doom_loop_recovery.is_none());
        assert!(config.extra_response_includes.is_empty());
        assert!(config.mtls_cert_dir.is_none());
        assert!(config.rate_limit_retry_threshold.is_none());

        let with_policy = SamplerConfig {
            doom_loop_recovery: Some(DoomLoopRecoveryPolicy {
                max_threshold: 8,
                max_retries: 2,
                ..Default::default()
            }),
            ..Default::default()
        };
        let round_tripped: SamplerConfig =
            serde_json::from_value(serde_json::to_value(&with_policy).unwrap()).unwrap();
        assert_eq!(
            round_tripped.doom_loop_recovery,
            with_policy.doom_loop_recovery
        );
    }
}
