use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use agent_client_protocol as acp;

use crate::mcp_bearer_token_file::configured_bearer_token_file;
use crate::mcp_servers::{
    McpServerDetails, McpServerOAuth, McpServerSkipReason, McpServerSource, McpServerSources,
    McpServerTimeouts, McpServerTransport, PluginMcpServers, SkippedMcpServer, config_toml,
    json_config,
};
use crate::{McpPreferencesFile, McpServerConfig, McpServerProblemSeverity, McpSetupResolution};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpServerList {
    pub servers: Vec<McpServerDetails>,
    /// This list omits a name a higher source already took.
    pub skipped: Vec<SkippedMcpServer>,
}

pub fn resolve_mcp_server_list(
    sources: &McpServerSources,
    plugins: &[PluginMcpServers],
    client_servers: Vec<acp::McpServer>,
) -> McpServerList {
    UserMcpConfig::load().server_list(sources, plugins, client_servers)
}

pub(crate) enum McpServerResolution {
    Server(Box<McpServerDetails>),
    Skipped(SkippedMcpServer),
}

impl McpServerResolution {
    pub(crate) fn into_server(self) -> Option<McpServerDetails> {
        match self {
            McpServerResolution::Server(server) => Some(*server),
            McpServerResolution::Skipped(_) => None,
        }
    }
}

pub(crate) struct DefinitionResolver<'a> {
    preferences: &'a McpPreferencesFile,
    substitute: &'a dyn Fn(&str) -> String,
}

impl<'a> DefinitionResolver<'a> {
    pub(crate) fn expanding_env(preferences: &'a McpPreferencesFile) -> DefinitionResolver<'a> {
        DefinitionResolver {
            preferences,
            substitute: &crate::expand_env_vars_in_string,
        }
    }

    /// [`UserMcpConfig::server_list`] fills timeouts and disabled tools by name.
    pub(crate) fn resolve(
        &self,
        name: &str,
        config: &McpServerConfig,
        source: McpServerSource,
    ) -> McpServerResolution {
        let skipped = |reason: McpServerSkipReason| {
            McpServerResolution::Skipped(SkippedMcpServer {
                name: name.to_owned(),
                source: source.clone(),
                reason,
            })
        };
        if !config.enabled {
            return skipped(McpServerSkipReason::DisabledInConfig);
        }
        let mut config = match config.resolve_setup(self.preferences.servers.get(name)) {
            McpSetupResolution::Resolved(config) => config,
            McpSetupResolution::Required(_) => return skipped(McpServerSkipReason::SetupRequired),
            McpSetupResolution::Invalid(reason) => {
                tracing::warn!(server = %name, error = %reason, "MCP setup config is invalid");
                return skipped(McpServerSkipReason::InvalidSetup { reason });
            }
        };
        config.expand_strings(self.substitute);
        let Some(transport) = config.resolve_transport(name) else {
            tracing::warn!(server = %name, "MCP server has an empty 'url'; skipping");
            return skipped(McpServerSkipReason::InvalidEntry {
                reason: "`url` is empty".to_owned(),
            });
        };
        McpServerResolution::Server(Box::new(McpServerDetails {
            name: name.to_owned(),
            transport,
            source,
            timeouts: McpServerTimeouts::default(),
            disabled_tools: BTreeSet::new(),
            oauth: config.oauth_settings(),
        }))
    }
}

pub(crate) struct UserMcpConfig {
    pub(super) effective_config: toml::Value,
    pub(super) preferences: McpPreferencesFile,
}

struct TomlDefinitions {
    resolutions: Vec<McpServerResolution>,
    claimed_timeouts: HashMap<String, McpServerTimeouts>,
    claimed_oauth: HashMap<String, McpServerOAuth>,
    disabled: HashSet<String>,
}

impl UserMcpConfig {
    pub(crate) fn load() -> UserMcpConfig {
        UserMcpConfig {
            effective_config: crate::effective_config::load_effective_config()
                .unwrap_or_else(|_| toml::Value::Table(toml::map::Map::new())),
            preferences: crate::mcp_servers::load_mcp_preferences().file(),
        }
    }

    pub(crate) fn configured_servers(
        &self,
        sources: &McpServerSources,
        plugins: &[PluginMcpServers],
        home: Option<&Path>,
    ) -> Vec<McpServerDetails> {
        let toml = self.toml_definitions(&sources.project_configs);
        merge_definitions(
            toml.resolutions,
            &toml.claimed_timeouts,
            non_toml_definitions(sources, plugins, &self.preferences, home),
        )
        .servers
    }

    pub(crate) fn server_list(
        &self,
        sources: &McpServerSources,
        plugins: &[PluginMcpServers],
        client_servers: Vec<acp::McpServer>,
    ) -> McpServerList {
        let toml = self.toml_definitions(&sources.project_configs);
        let mut list = merge_definitions(
            toml.resolutions,
            &toml.claimed_timeouts,
            non_toml_definitions(
                sources,
                plugins,
                &self.preferences,
                xai_dirs::home_dir().as_deref(),
            ),
        );

        let blocked = crate::mcp_servers::disabled_vendor_block_keys(
            &sources.cwd,
            &sources.compat,
            xai_dirs::home_dir().as_deref(),
            &self.preferences,
        );
        for server in client_servers {
            if let Some(&vendor) = blocked.get(&crate::mcp_servers::vendor_block_key(&server)) {
                list.skipped.push(SkippedMcpServer {
                    name: crate::mcp_servers::mcp_server_name(&server).to_owned(),
                    source: McpServerSource::Client,
                    reason: McpServerSkipReason::VendorMcpsOff { vendor },
                });
                continue;
            }
            let (name, transport) = match server {
                acp::McpServer::Stdio(acp::McpServerStdio {
                    name,
                    command,
                    args,
                    env,
                    ..
                }) => (
                    name,
                    McpServerTransport::Stdio {
                        command,
                        args,
                        env: env
                            .into_iter()
                            .map(|variable| (variable.name, variable.value))
                            .collect(),
                        cwd: None,
                    },
                ),
                acp::McpServer::Http(acp::McpServerHttp {
                    name,
                    url,
                    headers,
                    meta,
                    ..
                }) => (
                    name,
                    McpServerTransport::Http {
                        url,
                        headers: headers
                            .into_iter()
                            .map(|header| (header.name, header.value))
                            .collect(),
                        bearer_token_file: configured_bearer_token_file(meta.as_ref()),
                    },
                ),
                acp::McpServer::Sse(acp::McpServerSse {
                    name,
                    url,
                    headers,
                    meta,
                    ..
                }) => (
                    name,
                    McpServerTransport::Sse {
                        url,
                        headers: headers
                            .into_iter()
                            .map(|header| (header.name, header.value))
                            .collect(),
                        bearer_token_file: configured_bearer_token_file(meta.as_ref()),
                    },
                ),
                // `McpServer` is #[non_exhaustive]; this ACP version has no other transport.
                _ => continue,
            };
            let replaced = list.servers.iter_mut().find(|known| known.name == name);
            let client = McpServerDetails {
                oauth: replaced.as_ref().and_then(|known| known.oauth.clone()),
                name,
                transport,
                source: McpServerSource::Client,
                timeouts: McpServerTimeouts::default(),
                disabled_tools: BTreeSet::new(),
            };
            match replaced {
                Some(known) => *known = client,
                None => list.servers.push(client),
            }
        }

        let listed = config_toml::listed_disabled_mcp_servers(&self.effective_config);
        let mut servers = Vec::with_capacity(list.servers.len());
        for server in list.servers {
            let reason = if toml.disabled.contains(&server.name) {
                McpServerSkipReason::DisabledInConfig
            } else if listed.contains(&server.name) {
                McpServerSkipReason::ListedInDisabledMcpServers
            } else {
                servers.push(server);
                continue;
            };
            list.skipped.push(SkippedMcpServer {
                name: server.name,
                source: server.source,
                reason,
            });
        }

        let disabled_tools = config_toml::disabled_mcp_tools_in(&self.effective_config);
        for server in &mut servers {
            server.timeouts = toml
                .claimed_timeouts
                .get(&server.name)
                .cloned()
                .unwrap_or_default();
            if server.oauth.is_none() {
                server.oauth = toml.claimed_oauth.get(&server.name).cloned();
            }
            server.disabled_tools = disabled_tools
                .get(&server.name)
                .map(|tools| tools.iter().cloned().collect())
                .unwrap_or_default();
        }
        list.servers = servers;
        list
    }

    fn toml_definitions(&self, project_configs: &[PathBuf]) -> TomlDefinitions {
        let resolver = DefinitionResolver::expanding_env(&self.preferences);
        let mut definitions = TomlDefinitions {
            resolutions: Vec::new(),
            claimed_timeouts: HashMap::new(),
            claimed_oauth: HashMap::new(),
            disabled: HashSet::new(),
        };
        for (source, parsed) in
            config_toml::toml_mcp_server_layers(&self.effective_config, project_configs)
        {
            for problem in parsed.problems {
                if problem.severity == McpServerProblemSeverity::Error
                    && !definitions.claimed_timeouts.contains_key(&problem.server)
                {
                    definitions
                        .resolutions
                        .push(McpServerResolution::Skipped(SkippedMcpServer {
                            name: problem.server,
                            source: source.clone(),
                            reason: McpServerSkipReason::InvalidEntry {
                                reason: problem.message,
                            },
                        }));
                }
            }
            for (name, config) in parsed.servers {
                if definitions.claimed_timeouts.contains_key(&name) {
                    continue;
                }
                definitions
                    .resolutions
                    .push(resolver.resolve(&name, &config, source.clone()));
                if !config.enabled {
                    definitions.disabled.insert(name.clone());
                }
                if let Some(oauth) = config.oauth_settings() {
                    definitions.claimed_oauth.insert(name.clone(), oauth);
                }
                definitions.claimed_timeouts.insert(
                    name,
                    McpServerTimeouts {
                        startup_timeout_sec: config.startup_timeout_sec,
                        tool_timeout_sec: config.tool_timeout_sec,
                        tool_timeouts: config.tool_timeouts.into_iter().flatten().collect(),
                    },
                );
            }
        }
        definitions
    }
}

pub(crate) fn non_toml_definitions(
    sources: &McpServerSources,
    plugins: &[PluginMcpServers],
    preferences: &McpPreferencesFile,
    home: Option<&Path>,
) -> Vec<McpServerResolution> {
    let mut definitions = Vec::new();
    for plugin in plugins {
        let substitute = super::plugin_substitute(&plugin.root, &plugin.data_dir);
        let resolver = DefinitionResolver {
            preferences,
            substitute: &substitute,
        };
        let source = McpServerSource::Plugin {
            plugin_name: plugin.plugin_name.clone(),
            path: plugin.root.clone(),
            scope: plugin.scope,
        };
        let mut plugin_definitions = Vec::new();
        for config in super::plugin_runtime_configs(plugin) {
            for (name, config) in &config.mcp_servers {
                match resolver.resolve(name, config, source.clone()) {
                    McpServerResolution::Server(server) => {
                        plugin_definitions.retain(|existing| {
                            !matches!(
                                existing,
                                McpServerResolution::Skipped(skipped) if skipped.name == server.name
                            )
                        });
                        if plugin_definitions.iter().any(|existing| {
                            matches!(
                                existing,
                                McpServerResolution::Server(known) if known.name == server.name
                            )
                        }) {
                            continue;
                        }
                        plugin_definitions.push(McpServerResolution::Server(server));
                    }
                    McpServerResolution::Skipped(skipped) => {
                        if plugin_definitions.iter().any(|existing| match existing {
                            McpServerResolution::Server(known) => known.name == skipped.name,
                            McpServerResolution::Skipped(seen) => seen.name == skipped.name,
                        }) {
                            continue;
                        }
                        plugin_definitions.push(McpServerResolution::Skipped(skipped));
                    }
                }
            }
        }
        definitions.extend(plugin_definitions);
    }
    let resolver = DefinitionResolver::expanding_env(preferences);
    definitions.extend(json_config::claude_json_resolutions(
        sources, &resolver, home,
    ));
    definitions.extend(json_config::cursor_resolutions(
        &sources.cwd,
        &sources.compat,
        &resolver,
        home,
    ));
    definitions.extend(json_config::mcp_json_resolutions(
        &sources.mcp_json_files,
        sources.claude_import,
        &resolver,
    ));
    definitions
}

fn merge_definitions(
    toml: Vec<McpServerResolution>,
    toml_claimed: &HashMap<String, McpServerTimeouts>,
    non_toml: Vec<McpServerResolution>,
) -> McpServerList {
    let mut list = McpServerList::default();
    for resolution in toml {
        match resolution {
            McpServerResolution::Server(server) => list.servers.push(*server),
            McpServerResolution::Skipped(skipped) => list.skipped.push(skipped),
        }
    }
    let mut taken: HashSet<String> = toml_claimed.keys().cloned().collect();
    for resolution in non_toml {
        match resolution {
            McpServerResolution::Server(server) => {
                if taken.insert(server.name.clone()) {
                    list.servers.push(*server);
                }
            }
            McpServerResolution::Skipped(skipped) => {
                if !taken.contains(&skipped.name) {
                    list.skipped.push(skipped);
                }
            }
        }
    }
    list
}

#[cfg(test)]
#[path = "server_list_tests.rs"]
mod tests;
