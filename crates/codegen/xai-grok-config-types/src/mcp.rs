//! This module re-exports the MCP server config types from `xai-grok-config`.
//! It also holds `RelaySyncConfig`.

use serde::{Deserialize, Serialize};

pub use xai_grok_config::{
    KNOWN_MCP_SERVER_FIELDS, McpConfig, McpJsonOAuthBlock, McpPreferenceSource, McpPreferencesFile,
    McpServerConfig, McpServerConfigProblem, McpServerPreferences, McpServerProblemSeverity,
    McpServerTransportConfig, McpSetupConfig, McpSetupDerivedValue, McpSetupField,
    McpSetupFieldType, McpSetupOption, McpSetupResolution,
};

/// Configuration for relay session sharing, set in config.toml under the `[relay]` section.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RelaySyncConfig {
    pub enabled: Option<bool>,
}

impl RelaySyncConfig {
    /// Check if relay sync is enabled. `GROK_RELAY_SYNC_ENABLED` takes precedence over config.
    pub fn is_enabled(&self) -> bool {
        if let Ok(env_val) = std::env::var("GROK_RELAY_SYNC_ENABLED") {
            return env_val.eq_ignore_ascii_case("true") || env_val == "1";
        }
        self.enabled.unwrap_or(false)
    }
}
