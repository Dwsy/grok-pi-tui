//! Client-origin identity carried on every sampling request.

use serde::{Deserialize, Serialize};

/// Identity of the client that originated the request, used for User-Agent rendering.
/// The shell layer composes this with platform info into a final UA string.
///
/// Lives here (pure data, no I/O) so `xai-grok-http` and `xai-grok-telemetry` can
/// name it without depending on the sampler runtime.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OriginClientInfo {
    pub product: String,
    pub version: Option<String>,
}
