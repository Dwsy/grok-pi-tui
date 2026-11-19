//! Origin/client identification used by the telemetry engine.
//!
//! [`OriginClientInfo`] is owned by `xai-grok-sampling-types` (pure data), so the
//! telemetry engine can label events without depending on the sampler runtime.

pub use xai_grok_sampling_types::OriginClientInfo;

/// Construct an [`OriginClientInfo`] from the `GROK_CLIENT_NAME` / `GROK_CLIENT_VERSION` env vars.
/// Returns `None` when `GROK_CLIENT_NAME` is unset.
/// This is a free function rather than an inherent method because the type lives in another crate.
pub fn origin_client_info_from_env() -> Option<OriginClientInfo> {
    std::env::var("GROK_CLIENT_NAME")
        .ok()
        .map(|product| OriginClientInfo {
            product,
            version: std::env::var("GROK_CLIENT_VERSION").ok(),
        })
}
