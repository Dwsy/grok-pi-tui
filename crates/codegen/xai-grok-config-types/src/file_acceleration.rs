//! The route override handed to an installed file accelerator.
//!
//! The value is opaque here; the accelerator parses it. A blank value at any tier is unset, so an empty
//! `GROK_FILE_ACCELERATION_ROUTES` falls through to config and remote instead of clearing them.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

pub const ENV_FILE_ACCELERATION_ROUTES: &str = "GROK_FILE_ACCELERATION_ROUTES";

/// `[file_acceleration]` in config.toml.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FileAccelerationConfig {
    /// See [`resolve_file_acceleration_routes`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub routes: Option<String>,
}

/// Env, then config, then remote. The first non-blank value wins and is passed through unchanged.
pub fn resolve_file_acceleration_routes(
    env: Option<&str>,
    config: Option<&str>,
    remote: Option<&str>,
) -> Option<Arc<str>> {
    [env, config, remote]
        .into_iter()
        .flatten()
        .find(|value| !value.trim().is_empty())
        .map(Arc::from)
}

#[cfg(test)]
#[path = "file_acceleration_tests.rs"]
mod tests;
