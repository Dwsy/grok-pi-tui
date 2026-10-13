//! A project file entirely replaces a same-named server from the user config or from a project file closer to the repo root.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use indexmap::IndexMap;

use crate::mcp_servers::McpServerSource;
use crate::{
    KNOWN_MCP_SERVER_FIELDS, McpServerConfig, McpServerConfigProblem, McpServerProblemSeverity,
};

/// The shell's folder-trust check matches this tag to find project servers.
pub const MCP_SCOPE_PROJECT: &str = "project";
/// User scope covers the effective user config and every non-TOML source (`~/.claude.json`, `~/.cursor/mcp.json`, plugins).
pub const MCP_SCOPE_USER: &str = "user";

/// `project_configs` lists the repo root first.
pub fn get_mcp_server_config_with_project(
    name: &str,
    project_configs: &[PathBuf],
) -> Option<McpServerConfig> {
    project_configs
        .iter()
        .rev()
        .filter_map(|config_path| crate::load_config_file(config_path).ok())
        .find_map(|root| parse_mcp_servers_from_toml(&root).swap_remove(name))
        .or_else(|| get_mcp_server_config(name))
}

pub fn get_mcp_server_config(name: &str) -> Option<McpServerConfig> {
    let root = crate::effective_config::load_effective_config().ok()?;
    parse_mcp_servers_from_toml(&root).swap_remove(name)
}

pub fn mcp_server_scope(name: &str, project_configs: &[PathBuf]) -> &'static str {
    let is_project = project_configs.iter().any(|config_path| {
        crate::load_config_file(config_path)
            .is_ok_and(|root| parse_mcp_servers_from_toml(&root).contains_key(name))
    });
    if is_project {
        MCP_SCOPE_PROJECT
    } else {
        MCP_SCOPE_USER
    }
}

/// Includes servers with `enabled = false`.
pub fn all_toml_mcp_server_names(project_configs: &[PathBuf]) -> HashSet<String> {
    load_all_mcp_configs(project_configs).into_keys().collect()
}

pub fn load_mcp_server_problems_with_project(
    project_configs: &[PathBuf],
) -> Vec<McpServerConfigProblem> {
    let mut problems = Vec::new();
    if let Ok(global_config) = crate::effective_config::load_effective_config() {
        problems.extend(parse_mcp_servers_with_problems(&global_config).problems);
    }
    for config_path in project_configs {
        if let Ok(root) = crate::load_config_file(config_path) {
            problems.extend(parse_mcp_servers_with_problems(&root).problems);
        }
    }
    problems
}

pub fn disabled_mcp_server_names(project_configs: &[PathBuf]) -> HashSet<String> {
    let mut disabled: HashSet<String> = load_all_mcp_configs(project_configs)
        .into_iter()
        .filter(|(_, cfg)| !cfg.enabled)
        .map(|(name, _)| name)
        .collect();

    if let Ok(root) = crate::effective_config::load_effective_config() {
        disabled.extend(listed_disabled_mcp_servers(&root));
    }

    disabled
}

pub(crate) fn listed_disabled_mcp_servers(user_config: &toml::Value) -> HashSet<String> {
    user_config
        .get("disabled_mcp_servers")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str())
        .map(str::to_owned)
        .collect()
}

pub fn get_all_mcp_disabled_tools() -> HashMap<String, HashSet<String>> {
    let Ok(root) = crate::effective_config::load_effective_config() else {
        return HashMap::new();
    };
    disabled_mcp_tools_in(&root)
}

pub(crate) fn disabled_mcp_tools_in(user_config: &toml::Value) -> HashMap<String, HashSet<String>> {
    let Some(section) = user_config
        .get("disabled_mcp_tools")
        .and_then(|v| v.as_table())
    else {
        return HashMap::new();
    };
    section
        .iter()
        .filter_map(|(server, val)| {
            let tools: HashSet<String> = val
                .as_array()?
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
            if tools.is_empty() {
                None
            } else {
                Some((server.clone(), tools))
            }
        })
        .collect()
}

pub(crate) fn load_all_mcp_configs(
    project_configs: &[PathBuf],
) -> IndexMap<String, McpServerConfig> {
    load_mcp_server_configs_with_project(project_configs)
        .into_iter()
        .map(|(name, (config, _))| (name, config))
        .collect()
}

pub fn load_mcp_server_configs_with_project(
    project_configs: &[PathBuf],
) -> IndexMap<String, (McpServerConfig, &'static str)> {
    let global_config = crate::effective_config::load_effective_config()
        .unwrap_or_else(|_| toml::Value::Table(toml::map::Map::new()));
    toml_mcp_server_configs_from(&global_config, project_configs)
}

fn toml_mcp_server_configs_from(
    global_config: &toml::Value,
    project_configs: &[PathBuf],
) -> IndexMap<String, (McpServerConfig, &'static str)> {
    let mut servers: IndexMap<String, (McpServerConfig, &'static str)> =
        parse_mcp_servers_from_toml(global_config)
            .into_iter()
            .map(|(name, config)| (name, (config, MCP_SCOPE_USER)))
            .collect();

    // Nearer project files come later in `project_configs`
    for config_path in project_configs {
        if let Ok(root) = crate::load_config_file(config_path) {
            let project_servers = parse_mcp_servers_from_toml(&root);
            if !project_servers.is_empty() {
                tracing::debug!(
                    count = project_servers.len(),
                    path = %config_path.display(),
                    "Loaded project-scoped MCP servers from .grok/config.toml"
                );
                for (name, config) in project_servers {
                    servers.insert(name, (config, MCP_SCOPE_PROJECT));
                }
            }
        }
    }

    servers
}

pub fn parse_mcp_servers_from_toml(root: &toml::Value) -> IndexMap<String, McpServerConfig> {
    parse_mcp_servers_logging_problems(root).servers
}

/// The first layer is the nearest project file.
/// The last layer is the effective user config.
pub(crate) fn toml_mcp_server_layers(
    user_config: &toml::Value,
    project_configs: &[PathBuf],
) -> Vec<(McpServerSource, ParsedMcpServers)> {
    let user = (
        McpServerSource::UserConfig {
            path: crate::grok_home().join(crate::USER_CONFIG_FILENAME),
        },
        parse_mcp_servers_logging_problems(user_config),
    );
    project_configs
        .iter()
        .rev()
        .filter_map(|path| {
            let root = crate::load_config_file(path).ok()?;
            Some((
                McpServerSource::ProjectConfig { path: path.clone() },
                parse_mcp_servers_logging_problems(&root),
            ))
        })
        .chain(std::iter::once(user))
        .collect()
}

fn parse_mcp_servers_logging_problems(root: &toml::Value) -> ParsedMcpServers {
    let parsed = parse_mcp_servers_with_problems(root);
    for problem in &parsed.problems {
        tracing::warn!(server = %problem.server, "{}", problem.message);
    }
    parsed
}

pub(crate) struct ParsedMcpServers {
    pub(crate) servers: IndexMap<String, McpServerConfig>,
    pub(crate) problems: Vec<McpServerConfigProblem>,
}

fn parse_mcp_servers_with_problems(root: &toml::Value) -> ParsedMcpServers {
    let mut servers = IndexMap::new();
    let mut problems = Vec::new();

    let Some(entries) = root.get("mcp_servers").and_then(toml::Value::as_table) else {
        return ParsedMcpServers { servers, problems };
    };

    for (name, value) in entries {
        match deserialize_mcp_server_config(value) {
            Ok((config, unknown_fields)) => {
                for field in unknown_fields {
                    problems.push(McpServerConfigProblem {
                        server: name.clone(),
                        field: Some(field.clone()),
                        severity: McpServerProblemSeverity::Warning,
                        message: format!(
                            "`mcp_servers.{name}` has an unrecognized field `{field}`; it is \
                             ignored. See ~/.grok/docs/user-guide/07-mcp-servers.md"
                        ),
                    });
                }
                if let Some(problem) = blank_transport_problem(name, &config) {
                    problems.push(problem);
                    continue;
                }
                servers.insert(name.clone(), config);
            }
            Err(error) => problems.push(diagnose_invalid_entry(name, value, &error)),
        }
    }

    ParsedMcpServers { servers, problems }
}

fn deserialize_mcp_server_config(
    value: &toml::Value,
) -> Result<(McpServerConfig, Vec<String>), String> {
    let unknown_fields = value.as_table().map_or_else(Vec::new, |table| {
        table
            .keys()
            .filter(|field| !KNOWN_MCP_SERVER_FIELDS.contains(&field.as_str()))
            .cloned()
            .collect()
    });
    let config = toml::Value::try_into::<McpServerConfig>(value.clone())
        .map_err(|error| error.to_string())?;
    Ok((config, unknown_fields))
}

fn diagnose_invalid_entry(name: &str, value: &toml::Value, error: &str) -> McpServerConfigProblem {
    let has_command = value.get("command").is_some();
    let has_url = value.get("url").is_some();
    let message = if !has_command && !has_url {
        format!(
            "`mcp_servers.{name}` has no transport. To run it, set `command = \"...\"` or \
             `url = \"...\"`. To turn it off, add \"{name}\" to `disabled_mcp_servers` instead of \
             leaving an entry with no transport. \
             See ~/.grok/docs/user-guide/07-mcp-servers.md"
        )
    } else {
        format!(
            "`mcp_servers.{name}` has an invalid transport: {error}. \
             See ~/.grok/docs/user-guide/07-mcp-servers.md"
        )
    };
    McpServerConfigProblem {
        server: name.to_owned(),
        field: None,
        severity: McpServerProblemSeverity::Error,
        message,
    }
}

fn blank_transport_problem(name: &str, config: &McpServerConfig) -> Option<McpServerConfigProblem> {
    if config.enabled
        && let Some(field) = config.blank_transport_field()
    {
        Some(McpServerConfigProblem {
            server: name.to_owned(),
            field: Some(field.to_owned()),
            severity: McpServerProblemSeverity::Error,
            message: format!(
                "`mcp_servers.{name}` is enabled but its `{field}` is blank. \
                 Set a value, or add \"{name}\" to `disabled_mcp_servers` to turn it \
                 off. See ~/.grok/docs/user-guide/07-mcp-servers.md"
            ),
        })
    } else {
        None
    }
}

#[cfg(test)]
#[path = "config_toml_tests.rs"]
mod tests;
