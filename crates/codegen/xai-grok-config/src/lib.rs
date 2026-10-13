//! Grok config covers file loading and layering, service endpoints, remote settings, vendor compat, the campaign overlay, and the MCP server list.
//! `xai-grok-cloud-config` fetches, caches, and refreshes the remote settings.
//!
//! Merge order (lowest to highest priority):
//! 1. `/etc/grok/managed_config.toml`
//! 2. `$GROK_HOME/managed_config.toml`
//! 3. `$GROK_HOME/config.toml`
//! 4. `$GROK_HOME/requirements.toml` (cloud cache; Ed25519-signed at rest once a key is embedded, see [`signed_policy`])
//! 5. `/etc/grok/requirements.toml`
//! 6. macOS MDM managed preferences (`ai.x.grok`, admin-forced), macOS only
//!
//! Each layer applies its own [`[[version_overrides]]`](version_overrides) before merge.
//! Requirements layers (#4 through #6) may opt into fail-closed startup; see [`validate_requirements`].

#![deny(clippy::indexing_slicing)]

pub mod cache_file;
pub mod campaigns;
mod claude_import;
pub mod compat;
mod config_layers;
pub mod config_override;
mod config_requirements;
pub mod deserialize;
mod display_refresh;
mod distribution;
pub mod effective_config;
mod endpoints;
mod env_overlay;
pub mod fs_atomic;
pub mod global_hook_sources;
mod loader;
mod macos_managed;
mod managed_cache;
mod managed_policy_trust;
pub mod managed_text;
mod mcp_bearer_token_file;
mod mcp_server_config;
pub mod mcp_servers;
mod memory_v2;
mod paths;
mod remote_announcement;
mod remote_fetch;
mod remote_settings;
mod resolved;
pub mod sandbox_mode;
pub mod shell;
pub mod signed_policy;
mod validation;
pub mod version_overrides;

// Only the campaign items other crates need are re-exported at the root
// The rest stays reachable via the `pub mod` paths, keeping the root API narrow
pub use campaigns::{
    CampaignEntry, CampaignOverrides, filter_active_campaigns, ids_touching_paths,
};
pub use global_hook_sources::{
    GlobalHookSource, GlobalHookSourceError, GlobalHookSourceKind, ResolvedGlobalHookSources,
    TRUST_BOUNDARY_FILENAMES, ensure_grok_hook_slots, existing_ancestor_chain,
    is_direct_hook_json_name, list_direct_hook_json_files, missing_configured_sources,
    path_has_symlink_component, resolve_global_hook_sources, resolve_trust_boundary_sources,
    unique_ancestors_rootward,
};

pub use claude_import::{ClaudeImport, is_claude_import_marked, user_config_file};
pub use config_layers::{
    CampaignsState, ConfigLayers, campaigns_application_disabled, campaigns_state_path,
    load_dismissed_ids, load_dismissed_ids_from_home, load_effective_config_disk_only,
};
pub use display_refresh::DisplayRefreshSettings;
pub use distribution::{Capability, Distribution};
pub use endpoints::{CLI_CHAT_PROXY_BASE_URL_DEFAULT, EndpointsConfig};
pub use env_overlay::{
    GROK_CONFIG_ENV, GROK_CONFIG_PATH_ENV, OverlaySource, ResolvedOverlay, resolved_env_overlay,
};

#[cfg(unix)]
pub use global_hook_sources::{
    validate_direct_hook_json_file, validated_hook_json_files_for_sources,
};

pub use config_requirements::{
    AllowlistPin, CliStringPins, RequirementsToml, ServiceTogglePins, Sourced, ToolFeature,
    UploadTelemetryPins,
};
pub use loader::{
    HookConfigLayer, HookProvenance, MANAGED_CONFIG_FILENAME, ManagedConfigLayer,
    REQUIREMENTS_FILENAME, SANDBOX_CONFIG_FILENAME, TRUSTED_FOLDERS_FILENAME,
    TRUSTED_HOOK_PROJECTS_FILENAME, TRUSTED_PLUGINS_FILENAME, USER_CONFIG_FILENAME,
    apply_version_overrides_with_registered, deep_merge_toml, expand_env_vars_in_string,
    expand_env_vars_in_toml, hook_config_layers, hook_config_layers_at, load_config_file,
    load_from_disk, load_managed_config, load_system_managed_config, load_toml_file,
    managed_config_layers, managed_config_layers_at, toml_error_detail,
};
pub use macos_managed::MDM_REQUIREMENTS_SOURCE;
pub use managed_cache::{
    MANAGED_CONFIG_CACHE_FILE, ManagedPolicyCompromise, ServingIdentity, SyncMarker,
    bump_rollback_floor, bump_rollback_floor_with_now, confirmed_team_switch,
    confirmed_team_switch_at, fail_closed_policy_armed_at, is_managed_config_hard_stale_for,
    is_managed_config_stale_for, managed_config_identity_changed_at, managed_config_synced_at,
    managed_deployment_id, managed_policy_compromised_for, mark_managed_config_synced,
    mark_managed_config_synced_at, normalize_identity,
};
pub use managed_policy_trust::ManagedPolicyTrust;
pub use mcp_bearer_token_file::{BearerTokenPath, BearerTokenPathError};
pub use mcp_server_config::{
    KNOWN_MCP_SERVER_FIELDS, McpConfig, McpJsonOAuthBlock, McpOAuthConfig, McpOAuthConfigMap,
    McpPreferenceSource, McpPreferencesFile, McpServerConfig, McpServerConfigProblem,
    McpServerPreferences, McpServerProblemSeverity, McpServerTransportConfig, McpSetupConfig,
    McpSetupDerivedValue, McpSetupField, McpSetupFieldType, McpSetupOption, McpSetupResolution,
};
pub use memory_v2::{MemoryV2Rollout, MemoryV2Settings};
pub use paths::{
    claude_managed_settings_path, claude_managed_settings_probe_path, create_dir_all_owner_only,
    decode_cwd_from_dirname, default_grok_home, encode_cwd_dirname, ensure_sessions_cwd_dir,
    ensure_sessions_cwd_dir_in, grok_application, grok_application_in, grok_home,
    project_config_dir, project_config_dirname, sessions_cwd_dir, sessions_cwd_dir_in,
    set_dir_owner_only, system_config_dir, user_grok_home,
};
pub use remote_announcement::{AnnouncementCta, RemoteAnnouncement};
pub use remote_fetch::{remote_fetch_enabled_from_layers, resolve_remote_fetch_enabled};
pub use remote_settings::{
    CampaignOverride, ConsentGate, ContextualHintsRemote, DoomLoopRecoverySettings, GoalRoleModel,
    LongReasoningReminderSettings, RemoteRequestEncoding, RemoteSettings, WorktreeAutoGcSettings,
    WorktreeKindMaxAge, deserialize_tolerant,
};
pub use resolved::{BoolFlag, ConfigSource, Resolved, resolve_string_flag};
pub use validation::{
    RequirementsError, RequirementsLayer, RequirementsSource, load_merged_requirements,
    requirements_layers, validate_requirements,
};
pub use version_overrides::{VersionOverrideError, apply_version_overrides};

pub use xai_grok_env::env_bool;
