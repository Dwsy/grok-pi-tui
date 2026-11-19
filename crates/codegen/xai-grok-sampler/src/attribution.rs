//! The caller wires an [`Auth401AttributionCallback`] into [`crate::SamplerConfig::attribution_callback`].
//! The sampler invokes it at each UNAUTHORIZED arm with the bearer fragment that went on the wire.
//! An observer can then split "sent a stale snapshot" from "sent the live token and was still rejected".
//! `None` (the default) makes the 401 sites silent.
//!
//! This crate stays decoupled from `xai-grok-shell`: no shell types, no auth-manager dependency.

pub use xai_grok_auth::bearer_fragment::BEARER_SUFFIX_LEN;
/// 401-attribution contract, moved to `xai_grok_sampling_types` so
/// `xai-grok-login` can implement it without the sampler runtime.
pub use xai_grok_sampling_types::{
    Auth401AttributionCallback, SamplingConsumer, SharedAttributionCallback,
};
