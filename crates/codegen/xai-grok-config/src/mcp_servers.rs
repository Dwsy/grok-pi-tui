//! The user `config.toml` comes before project `.grok/config.toml` files.
//! A server with `enabled = false` still hides its name from later sources.
//! Plugins come after those toml files.
//! `~/.claude.json` comes after plugins.
//! Cursor project `mcp.json` comes before the Cursor user file.
//! Project `.mcp.json` files come last.
//! The caller applies folder trust and the managed MCP policy.

mod config_toml;
mod json_config;
mod preferences;
mod server_details;
mod server_list;
mod session_merge;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use agent_client_protocol as acp;
use indexmap::IndexMap;

use crate::compat::{CompatConfig, CompatVendor};
use crate::{
    ClaudeImport, McpConfig, McpOAuthConfigMap, McpPreferencesFile, McpServerConfig,
    McpSetupResolution,
};

pub use config_toml::{
    MCP_SCOPE_PROJECT, MCP_SCOPE_USER, all_toml_mcp_server_names, disabled_mcp_server_names,
    get_all_mcp_disabled_tools, get_mcp_server_config, get_mcp_server_config_with_project,
    load_mcp_server_configs_with_project, load_mcp_server_problems_with_project, mcp_server_scope,
    parse_mcp_servers_from_toml,
};
pub use json_config::{
    load_claude_json_mcp_servers_as_configs, load_claude_json_mcp_servers_as_configs_unfiltered,
    load_cursor_mcp_servers_as_configs, load_mcp_json_file, load_mcp_json_servers,
    load_mcp_json_servers_as_configs, load_mcp_json_servers_as_configs_unfiltered,
    mcp_config_from_json_value, parse_mcp_config_with_oauth, read_mcp_json,
};
pub use preferences::{
    McpPreferencesLoad, load_mcp_preferences, load_mcp_preferences_from, mcp_preferences_path,
};
pub use server_details::{
    McpServerDetails, McpServerOAuth, McpServerScope, McpServerSkipReason, McpServerSource,
    McpServerTimeouts, McpServerTransport, SkippedMcpServer,
};
pub use server_list::{McpServerList, resolve_mcp_server_list};
pub use session_merge::{
    SessionMcpServer, SessionMcpTier, canonicalize_mcp_maps, mcp_server_definitions_equal,
    resolve_session_mcp_servers,
};

#[derive(Debug, Clone)]
pub struct McpServerSources {
    pub cwd: PathBuf,
    /// Repo root first.
    pub project_configs: Vec<PathBuf>,
    /// Repo root first.
    pub mcp_json_files: Vec<PathBuf>,
    pub compat: CompatConfig,
    pub claude_import: ClaudeImport,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpServerOrigin {
    /// `path` is the user `config.toml` even for a project definition.
    ConfigToml {
        path: PathBuf,
    },
    Plugin {
        plugin_name: String,
        path: PathBuf,
    },
    ClaudeJson {
        path: PathBuf,
    },
    /// A project `.mcp.json` or a Cursor `mcp.json`.
    McpJson {
        path: PathBuf,
    },
}

pub struct PluginMcpServers {
    pub plugin_name: String,
    pub root: PathBuf,
    pub data_dir: PathBuf,
    pub scope: McpServerScope,
    /// This path is read before the inline servers.
    /// The first definition of a name wins.
    pub mcp_config_path: Option<PathBuf>,
    pub inline_mcp_servers: Option<serde_json::Value>,
}

/// The file comes before the inline value.
/// One bad inline entry drops the inline value.
pub(crate) fn plugin_runtime_configs(plugin: &PluginMcpServers) -> Vec<McpConfig> {
    plugin
        .mcp_config_path
        .as_deref()
        .and_then(json_config::read_mcp_json)
        .into_iter()
        .chain(plugin.inline_mcp_servers.as_ref().and_then(|value| {
            let normalized = normalize_inline_mcp_servers(value);
            match serde_json::from_value(normalized) {
                Ok(config) => Some(config),
                Err(_) => {
                    tracing::warn!(
                        plugin = plugin.plugin_name,
                        "failed to parse plugin MCP config"
                    );
                    None
                }
            }
        }))
        .collect()
}

/// The file comes before the inline servers.
/// The other inline entries stay when one entry is bad.
pub fn plugin_setup_server_configs(plugin: &PluginMcpServers) -> IndexMap<String, McpServerConfig> {
    let mut servers = IndexMap::new();
    if let Some(config) = plugin
        .mcp_config_path
        .as_deref()
        .and_then(json_config::read_mcp_json)
    {
        for (name, server) in config.mcp_servers {
            servers.entry(name).or_insert(server);
        }
    }
    if let Some(value) = plugin.inline_mcp_servers.as_ref() {
        for (name, server) in
            json_config::mcp_config_from_json_value(&normalize_inline_mcp_servers(value))
                .mcp_servers
        {
            servers.entry(name).or_insert(server);
        }
    }
    servers
}

pub fn plugin_oauth_configs(plugins: &[PluginMcpServers]) -> McpOAuthConfigMap {
    let mut oauth_configs = McpOAuthConfigMap::new();
    for plugin in plugins {
        let substitute = plugin_substitute(&plugin.root, &plugin.data_dir);
        let label = format!("plugin:{}", plugin.plugin_name);
        for config in plugin_runtime_configs(plugin) {
            let (_, oauth) = json_config::parse_mcp_config_with_oauth(&config, &label, &substitute);
            for (name, cfg) in oauth {
                oauth_configs.entry(name).or_insert(cfg);
            }
        }
    }
    oauth_configs
}

pub(crate) fn plugin_substitute(root: &Path, data_dir: &Path) -> impl Fn(&str) -> String {
    let plugin_root = root.to_string_lossy().into_owned();
    let plugin_data = data_dir.to_string_lossy().into_owned();
    move |value: &str| {
        let value = value
            .replace("${CLAUDE_PLUGIN_ROOT}", &plugin_root)
            .replace("${GROK_PLUGIN_ROOT}", &plugin_root)
            .replace("${CLAUDE_PLUGIN_DATA}", &plugin_data)
            .replace("${GROK_PLUGIN_DATA}", &plugin_data);
        crate::expand_env_vars_in_string(&value)
    }
}

fn normalize_inline_mcp_servers(value: &serde_json::Value) -> serde_json::Value {
    let inner = match value.get("mcpServers") {
        Some(servers) if servers.is_object() => servers.clone(),
        Some(_) | None => value.clone(),
    };
    serde_json::json!({ "mcpServers": inner })
}

impl McpServerOrigin {
    /// These paths match what `ConfigSource` reports for the server.
    fn from_source(source: McpServerSource, cwd: &Path) -> McpServerOrigin {
        match source {
            McpServerSource::UserConfig { path } => McpServerOrigin::ConfigToml { path },
            McpServerSource::ProjectConfig { .. } => McpServerOrigin::ConfigToml {
                path: crate::grok_home().join(crate::USER_CONFIG_FILENAME),
            },
            McpServerSource::Plugin {
                plugin_name,
                path,
                scope: _,
            } => McpServerOrigin::Plugin { plugin_name, path },
            McpServerSource::ClaudeUser { path } | McpServerSource::ClaudeProject { path } => {
                McpServerOrigin::ClaudeJson { path }
            }
            McpServerSource::CursorUser { path } => McpServerOrigin::McpJson { path },
            McpServerSource::CursorProject { .. } => McpServerOrigin::McpJson {
                path: xai_dirs::home_dir()
                    .map(|home| home.join(".cursor").join("mcp.json"))
                    .unwrap_or_default(),
            },
            McpServerSource::McpJson { .. } => McpServerOrigin::McpJson {
                path: cwd.join(".mcp.json"),
            },
            McpServerSource::Client => {
                unreachable!("the config files and plugins never define a client server")
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpEnabledFilter {
    Respect,
    Ignore,
}

/// Excludes plugin and client-forwarded servers.
pub fn load_mcp_servers(sources: &McpServerSources) -> Vec<acp::McpServer> {
    let preferences = load_mcp_preferences().file();
    let sub = &crate::expand_env_vars_in_string;
    merge_file_mcp_server_configs(sources)
        .into_iter()
        .filter_map(|(name, config)| {
            materialize_mcp_config(&name, config, &preferences, sub, McpEnabledFilter::Respect)
        })
        .collect()
}

/// One read of the config files, so a managed or requirements server never starts without its OAuth client settings.
pub fn load_mcp_servers_with_oauth(
    sources: &McpServerSources,
) -> (Vec<acp::McpServer>, McpOAuthConfigMap) {
    let preferences = load_mcp_preferences().file();
    let sub = &crate::expand_env_vars_in_string;
    let mut oauth_configs = McpOAuthConfigMap::new();
    let mut acp_servers = Vec::new();
    for (name, config) in merge_file_mcp_server_configs(sources) {
        let Some(mut config) = resolve_mcp_setup(&name, &config, &preferences) else {
            continue;
        };
        config.expand_strings(sub);
        if let Some(oauth) = config.oauth_config() {
            oauth_configs.insert(name.clone(), oauth);
        }
        if let Some(acp_server) = config.to_acp_mcp_server(name) {
            acp_servers.push(acp_server);
        }
    }
    (acp_servers, oauth_configs)
}

fn merge_file_mcp_server_configs(sources: &McpServerSources) -> IndexMap<String, McpServerConfig> {
    let mut servers = config_toml::load_all_mcp_configs(&sources.project_configs);

    let claude_servers = json_config::load_claude_json_mcp_servers_as_configs(
        &sources.cwd,
        &sources.compat,
        sources.claude_import,
    );
    tracing::info!(
        count = claude_servers.len(),
        "Loaded MCP servers from ~/.claude.json"
    );
    for (name, config) in claude_servers {
        servers.entry(name).or_insert(config);
    }

    let cursor_servers =
        json_config::load_cursor_mcp_servers_as_configs(&sources.cwd, &sources.compat);
    tracing::info!(
        count = cursor_servers.len(),
        "Loaded Cursor MCP servers from ~/.cursor/mcp.json"
    );
    for (name, config) in cursor_servers {
        servers.entry(name).or_insert(config);
    }

    let mcp_json_servers = json_config::load_mcp_json_servers_as_configs(
        &sources.mcp_json_files,
        sources.claude_import,
    );
    tracing::info!(
        count = mcp_json_servers.len(),
        "Loaded .mcp.json MCP servers"
    );
    for (name, config) in mcp_json_servers {
        servers.entry(name).or_insert(config);
    }

    servers
}

pub fn load_mcp_servers_with_origin(
    sources: &McpServerSources,
    plugins: &[PluginMcpServers],
) -> Vec<(acp::McpServer, McpServerOrigin)> {
    server_list::UserMcpConfig::load()
        .configured_servers(sources, plugins, xai_dirs::home_dir().as_deref())
        .into_iter()
        .map(|server| {
            let origin = McpServerOrigin::from_source(server.source.clone(), &sources.cwd);
            (acp::McpServer::from(server), origin)
        })
        .collect()
}

pub fn load_mcp_servers_toml_only(project_configs: &[PathBuf]) -> Vec<acp::McpServer> {
    let preferences = load_mcp_preferences().file();
    let sub = &crate::expand_env_vars_in_string;
    config_toml::load_all_mcp_configs(project_configs)
        .into_iter()
        .filter_map(|(name, config)| {
            materialize_mcp_config(&name, config, &preferences, sub, McpEnabledFilter::Respect)
        })
        .collect()
}

pub fn materialize_mcp_config(
    name: &str,
    mut config: McpServerConfig,
    preferences: &McpPreferencesFile,
    sub: &dyn Fn(&str) -> String,
    enabled_filter: McpEnabledFilter,
) -> Option<acp::McpServer> {
    if matches!(enabled_filter, McpEnabledFilter::Ignore) {
        config.enabled = true;
    }
    let mut config = resolve_mcp_setup(name, &config, preferences)?;
    config.expand_strings(sub);
    config.to_acp_mcp_server(name)
}

fn resolve_mcp_setup(
    name: &str,
    config: &McpServerConfig,
    preferences: &McpPreferencesFile,
) -> Option<Box<McpServerConfig>> {
    match config.resolve_setup(preferences.servers.get(name)) {
        McpSetupResolution::Resolved(config) => Some(config),
        McpSetupResolution::Required(_) => None,
        McpSetupResolution::Invalid(reason) => {
            tracing::warn!(server = %name, error = %reason, "MCP setup config is invalid");
            None
        }
    }
}

/// The list can hold a name more than once. Callers keep the first entry for each name.
pub fn non_toml_mcp_servers_with_origin(
    sources: &McpServerSources,
    plugins: &[PluginMcpServers],
    toml_claimed_names: &HashSet<String>,
) -> Vec<(acp::McpServer, McpServerOrigin)> {
    let preferences = load_mcp_preferences().file();
    server_list::non_toml_definitions(
        sources,
        plugins,
        &preferences,
        xai_dirs::home_dir().as_deref(),
    )
    .into_iter()
    .filter_map(server_list::McpServerResolution::into_server)
    .filter(|server| !toml_claimed_names.contains(&server.name))
    .map(|server| {
        let origin = McpServerOrigin::from_source(server.source.clone(), &sources.cwd);
        (acp::McpServer::from(server), origin)
    })
    .collect()
}

pub fn mcp_server_name(server: &acp::McpServer) -> &str {
    match server {
        acp::McpServer::Http(acp::McpServerHttp { name, .. })
        | acp::McpServer::Sse(acp::McpServerSse { name, .. })
        | acp::McpServer::Stdio(acp::McpServerStdio { name, .. }) => name,
        // `McpServer` is #[non_exhaustive]. An unknown transport has no name
        _ => "",
    }
}

/// Drops client-forwarded servers that match a vendor's on-disk MCP config while that vendor's `mcps` setting is off.
/// Call this whenever a client sends servers, before storing them for hot reload.
#[must_use]
pub fn admit_client_mcp_servers(
    client_mcp_servers: Vec<acp::McpServer>,
    cwd: &Path,
    compat: &CompatConfig,
) -> Vec<acp::McpServer> {
    let preferences = load_mcp_preferences().file();
    admit_client_mcp_servers_at(
        client_mcp_servers,
        cwd,
        compat,
        xai_dirs::home_dir().as_deref(),
        &preferences,
    )
}

pub(crate) fn admit_client_mcp_servers_at(
    client_mcp_servers: Vec<acp::McpServer>,
    cwd: &Path,
    compat: &CompatConfig,
    home: Option<&Path>,
    preferences: &McpPreferencesFile,
) -> Vec<acp::McpServer> {
    let blocked = disabled_vendor_block_keys(cwd, compat, home, preferences);
    if blocked.is_empty() {
        return client_mcp_servers;
    }
    client_mcp_servers
        .into_iter()
        .filter(|s| !blocked.contains_key(&vendor_block_key(s)))
        .collect()
}

fn disabled_vendor_block_keys(
    cwd: &Path,
    compat: &CompatConfig,
    home: Option<&Path>,
    preferences: &McpPreferencesFile,
) -> HashMap<String, CompatVendor> {
    let mut blocked = HashMap::new();
    if !compat.cursor.mcps {
        let mut forced = *compat;
        forced.cursor.mcps = true;
        blocked.extend(
            json_config::load_cursor_mcp_servers(cwd, &forced, home, preferences)
                .iter()
                .map(|server| (vendor_block_key(server), CompatVendor::Cursor)),
        );
    }
    if !compat.claude.mcps {
        // The normal Claude loader returns nothing while `mcps` is off or after the import cutoff
        blocked.extend(
            json_config::load_claude_json_mcp_servers_for_attribution(cwd, home, preferences)
                .iter()
                .map(|server| (vendor_block_key(server), CompatVendor::Claude)),
        );
    }
    blocked
}

/// A client can forward the same URL under any display name.
fn vendor_block_key(server: &acp::McpServer) -> String {
    match server {
        acp::McpServer::Http(acp::McpServerHttp { url, .. })
        | acp::McpServer::Sse(acp::McpServerSse { url, .. }) => {
            url.trim_end_matches('/').to_owned()
        }
        acp::McpServer::Stdio(acp::McpServerStdio { name, .. }) => name.clone(),
        // `McpServer` is #[non_exhaustive]. The loaders never produce an unknown transport
        _ => String::new(),
    }
}

#[cfg(test)]
#[path = "mcp_servers_tests.rs"]
mod tests;
