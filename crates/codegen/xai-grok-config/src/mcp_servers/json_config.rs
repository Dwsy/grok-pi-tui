use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use agent_client_protocol as acp;
use indexmap::IndexMap;

use crate::compat::CompatConfig;
use crate::mcp_servers::server_list::{DefinitionResolver, McpServerResolution};
use crate::mcp_servers::{
    McpServerSkipReason, McpServerSource, McpServerSources, SkippedMcpServer,
};
use crate::{ClaudeImport, McpConfig, McpOAuthConfigMap, McpPreferencesFile, McpServerConfig};

/// `mcp_json_files` lists the repo root first.
/// The file nearest the cwd wins a name that two files define.
pub fn load_mcp_json_servers(
    mcp_json_files: &[PathBuf],
    claude_import: ClaudeImport,
) -> Vec<acp::McpServer> {
    let preferences = crate::mcp_servers::load_mcp_preferences().file();
    load_acp_servers(&preferences, |resolver| {
        mcp_json_resolutions(mcp_json_files, claude_import, resolver)
    })
}

/// The returned list starts with the file nearest the cwd.
/// `mcp_json_files` lists the repo root first.
pub(crate) fn mcp_json_resolutions(
    mcp_json_files: &[PathBuf],
    claude_import: ClaudeImport,
    resolver: &DefinitionResolver<'_>,
) -> Vec<McpServerResolution> {
    if claude_import == ClaudeImport::Imported {
        return Vec::new();
    }
    mcp_json_files
        .iter()
        .rev()
        .flat_map(|path| {
            mcp_json_file_resolutions(
                path,
                McpServerSource::McpJson { path: path.clone() },
                resolver,
            )
        })
        .collect()
}

pub fn load_mcp_json_servers_as_configs(
    mcp_json_files: &[PathBuf],
    claude_import: ClaudeImport,
) -> IndexMap<String, McpServerConfig> {
    if claude_import == ClaudeImport::Imported {
        return IndexMap::new();
    }
    load_mcp_json_servers_as_configs_unfiltered(mcp_json_files)
}

/// The `/import-claude` scanner calls this to let users re-import servers they skipped before.
pub fn load_mcp_json_servers_as_configs_unfiltered(
    mcp_json_files: &[PathBuf],
) -> IndexMap<String, McpServerConfig> {
    let mut result = IndexMap::new();
    for mcp_path in mcp_json_files.iter().rev() {
        if let Some(config) = read_mcp_json(mcp_path) {
            for (name, cfg) in config.mcp_servers {
                result.entry(name).or_insert(cfg);
            }
        }
    }
    result
}

/// `projects.<cwd>.mcpServers` comes before the top-level `mcpServers`.
pub(crate) fn claude_json_resolutions(
    sources: &McpServerSources,
    resolver: &DefinitionResolver<'_>,
    home: Option<&Path>,
) -> Vec<McpServerResolution> {
    if !sources.compat.claude.mcps || sources.claude_import == ClaudeImport::Imported {
        return Vec::new();
    }
    let Some(home) = home else {
        return Vec::new();
    };
    claude_json_file_resolutions(&home.join(".claude.json"), &sources.cwd, resolver)
}

pub(crate) fn load_claude_json_mcp_servers_for_attribution(
    cwd: &Path,
    home: Option<&Path>,
    preferences: &McpPreferencesFile,
) -> Vec<acp::McpServer> {
    let Some(home) = home else {
        return Vec::new();
    };
    load_claude_json_mcp_servers_from(&home.join(".claude.json"), cwd, preferences)
}

pub fn load_claude_json_mcp_servers_as_configs(
    cwd: &Path,
    compat: &CompatConfig,
    claude_import: ClaudeImport,
) -> IndexMap<String, McpServerConfig> {
    if !compat.claude.mcps || claude_import == ClaudeImport::Imported {
        return IndexMap::new();
    }
    load_claude_json_mcp_servers_as_configs_unfiltered(cwd)
}

/// The `/import-claude` scanner calls this to let users re-import servers they skipped before.
pub fn load_claude_json_mcp_servers_as_configs_unfiltered(
    cwd: &Path,
) -> IndexMap<String, McpServerConfig> {
    let Some(home) = xai_dirs::home_dir() else {
        return IndexMap::new();
    };
    load_claude_json_mcp_servers_from_as_configs(&home.join(".claude.json"), cwd)
}

fn load_claude_json_mcp_servers_from_as_configs(
    claude_json_path: &Path,
    cwd: &Path,
) -> IndexMap<String, McpServerConfig> {
    let content = match std::fs::read_to_string(claude_json_path) {
        Ok(c) => c,
        Err(e) => {
            tracing::debug!(
                path = %claude_json_path.display(),
                error = %e,
                "failed to read ~/.claude.json"
            );
            return IndexMap::new();
        }
    };
    let value: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            tracing::debug!(
                path = %claude_json_path.display(),
                error = %e,
                "failed to parse ~/.claude.json"
            );
            return IndexMap::new();
        }
    };
    let config = claude_json_mcp_from_value(&value);

    let mut result = IndexMap::new();
    let cwd_key = cwd.to_string_lossy();
    if let Some(project) = config.projects.get(cwd_key.as_ref()) {
        for (name, cfg) in &project.mcp_servers {
            result.insert(name.clone(), cfg.clone());
        }
    }
    for (name, cfg) in &config.user_mcp.mcp_servers {
        result.entry(name.clone()).or_insert(cfg.clone());
    }

    tracing::info!(
        project_count = config
            .projects
            .get(cwd_key.as_ref())
            .map_or(0, |p| p.mcp_servers.len()),
        user_level_count = config.user_mcp.mcp_servers.len(),
        total_count = result.len(),
        "MCP servers loaded from ~/.claude.json"
    );

    result
}

fn load_claude_json_mcp_servers_from(
    claude_json_path: &Path,
    cwd: &Path,
    preferences: &McpPreferencesFile,
) -> Vec<acp::McpServer> {
    // One name can be a project URL and a user URL. The vendor block needs both.
    claude_json_file_resolutions(
        claude_json_path,
        cwd,
        &DefinitionResolver::expanding_env(preferences),
    )
    .into_iter()
    .filter_map(McpServerResolution::into_server)
    .map(acp::McpServer::from)
    .collect()
}

fn claude_json_file_resolutions(
    claude_json_path: &Path,
    cwd: &Path,
    resolver: &DefinitionResolver<'_>,
) -> Vec<McpServerResolution> {
    let Ok(content) = std::fs::read_to_string(claude_json_path) else {
        return Vec::new();
    };
    let value: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            tracing::debug!(
                path = %claude_json_path.display(),
                error = %e,
                "failed to parse claude.json"
            );
            return Vec::new();
        }
    };

    let path = claude_json_path.to_path_buf();
    let project = value
        .get("projects")
        .and_then(|projects| projects.get(cwd.to_string_lossy().as_ref()));
    let mut resolutions = Vec::new();
    if let Some(project) = project {
        resolutions.extend(resolve_mcp_json(
            parse_mcp_json_value(project),
            &McpServerSource::ClaudeProject { path: path.clone() },
            resolver,
        ));
    }
    resolutions.extend(resolve_mcp_json(
        parse_mcp_json_value(&value),
        &McpServerSource::ClaudeUser { path },
        resolver,
    ));
    resolutions
}

struct ClaudeJsonMcp {
    user_mcp: McpConfig,
    projects: HashMap<String, McpConfig>,
}

fn claude_json_mcp_from_value(value: &serde_json::Value) -> ClaudeJsonMcp {
    let user_mcp = mcp_config_from_json_value(value);
    let mut projects = HashMap::new();
    if let Some(entries) = value.get("projects").and_then(|v| v.as_object()) {
        for (path, project) in entries {
            projects.insert(path.clone(), mcp_config_from_json_value(project));
        }
    }
    ClaudeJsonMcp { user_mcp, projects }
}

pub(crate) fn load_cursor_mcp_servers(
    cwd: &Path,
    compat: &CompatConfig,
    home: Option<&Path>,
    preferences: &McpPreferencesFile,
) -> Vec<acp::McpServer> {
    load_acp_servers(preferences, |resolver| {
        cursor_resolutions(cwd, compat, resolver, home)
    })
}

/// `<cwd>/.cursor/mcp.json` comes before `<home>/.cursor/mcp.json`.
pub(crate) fn cursor_resolutions(
    cwd: &Path,
    compat: &CompatConfig,
    resolver: &DefinitionResolver<'_>,
    home: Option<&Path>,
) -> Vec<McpServerResolution> {
    if !compat.cursor.mcps {
        return Vec::new();
    }
    let project = cwd.join(".cursor").join("mcp.json");
    let mut resolutions = mcp_json_file_resolutions(
        &project,
        McpServerSource::CursorProject {
            path: project.clone(),
        },
        resolver,
    );
    if let Some(home) = home {
        let user = home.join(".cursor").join("mcp.json");
        resolutions.extend(mcp_json_file_resolutions(
            &user,
            McpServerSource::CursorUser { path: user.clone() },
            resolver,
        ));
    }
    resolutions
}

pub fn load_cursor_mcp_servers_as_configs(
    cwd: &Path,
    compat: &CompatConfig,
) -> IndexMap<String, McpServerConfig> {
    if !compat.cursor.mcps {
        return IndexMap::new();
    }
    let mut result = IndexMap::new();
    for path in cursor_mcp_json_paths(cwd) {
        if path.is_file()
            && let Some(config) = read_mcp_json(&path)
        {
            for (name, cfg) in config.mcp_servers {
                result.entry(name).or_insert(cfg);
            }
        }
    }
    result
}

/// Callers keep the server from the first path when both files define the same name.
fn cursor_mcp_json_paths(cwd: &Path) -> impl Iterator<Item = PathBuf> {
    let global_path = xai_dirs::home_dir().map(|home| home.join(".cursor").join("mcp.json"));
    std::iter::once(cwd.join(".cursor").join("mcp.json")).chain(global_path)
}

pub fn load_mcp_json_file(path: &Path) -> Vec<acp::McpServer> {
    let preferences = crate::mcp_servers::load_mcp_preferences().file();
    load_acp_servers(&preferences, |resolver| {
        mcp_json_file_resolutions(
            path,
            McpServerSource::McpJson {
                path: path.to_path_buf(),
            },
            resolver,
        )
    })
}

fn load_acp_servers(
    preferences: &McpPreferencesFile,
    resolve: impl FnOnce(&DefinitionResolver<'_>) -> Vec<McpServerResolution>,
) -> Vec<acp::McpServer> {
    let mut seen_names = HashSet::new();
    resolve(&DefinitionResolver::expanding_env(preferences))
        .into_iter()
        .filter_map(McpServerResolution::into_server)
        .filter(|server| seen_names.insert(server.name.clone()))
        .map(acp::McpServer::from)
        .collect()
}

fn mcp_json_file_resolutions(
    path: &Path,
    source: McpServerSource,
    resolver: &DefinitionResolver<'_>,
) -> Vec<McpServerResolution> {
    if !path.is_file() {
        return Vec::new();
    }
    let Some(value) = read_mcp_json_value(path) else {
        return Vec::new();
    };
    resolve_mcp_json(parse_mcp_json_value(&value), &source, resolver)
}

fn resolve_mcp_json(
    parsed: ParsedMcpJson,
    source: &McpServerSource,
    resolver: &DefinitionResolver<'_>,
) -> Vec<McpServerResolution> {
    let invalid = parsed.invalid.into_iter().map(|(name, reason)| {
        McpServerResolution::Skipped(SkippedMcpServer {
            name,
            source: source.clone(),
            reason: McpServerSkipReason::InvalidEntry { reason },
        })
    });
    let resolved = parsed
        .config
        .mcp_servers
        .iter()
        .map(|(name, config)| resolver.resolve(name, config, source.clone()));
    invalid.chain(resolved).collect()
}

pub fn read_mcp_json(path: &Path) -> Option<McpConfig> {
    Some(mcp_config_from_json_value(&read_mcp_json_value(path)?))
}

fn read_mcp_json_value(path: &Path) -> Option<serde_json::Value> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| {
            tracing::warn!(error = %e, "failed to read MCP JSON");
        })
        .ok()?;
    serde_json::from_str(&content)
        .map_err(|e| {
            tracing::warn!(error = %e, "failed to parse MCP JSON");
        })
        .ok()
}

/// An `mcpServers` entry that fails to deserialize is skipped without dropping the others.
pub fn mcp_config_from_json_value(value: &serde_json::Value) -> McpConfig {
    parse_mcp_json_value(value).config
}

struct ParsedMcpJson {
    config: McpConfig,
    invalid: Vec<(String, String)>,
}

fn parse_mcp_json_value(value: &serde_json::Value) -> ParsedMcpJson {
    let mut mcp_servers = IndexMap::new();
    let mut invalid = Vec::new();
    if let Some(entries) = value.get("mcpServers").and_then(|v| v.as_object()) {
        for (name, entry) in entries {
            match serde_json::from_value::<McpServerConfig>(entry.clone()) {
                Ok(config) => {
                    mcp_servers.insert(name.clone(), config);
                }
                Err(error) => {
                    tracing::warn!(
                        server = %name,
                        error = %error,
                        "skipping invalid MCP server entry in JSON config"
                    );
                    invalid.push((name.clone(), error.to_string()));
                }
            }
        }
    }
    ParsedMcpJson {
        config: McpConfig { mcp_servers },
        invalid,
    }
}

pub fn parse_mcp_config_with_oauth(
    config: &McpConfig,
    source_label: &str,
    sub: &dyn Fn(&str) -> String,
) -> (Vec<acp::McpServer>, McpOAuthConfigMap) {
    let preferences = crate::mcp_servers::load_mcp_preferences().file();
    let mut servers = Vec::new();
    let mut oauth_configs = McpOAuthConfigMap::new();
    for (name, server_config) in &config.mcp_servers {
        let Some(mut server_config) =
            crate::mcp_servers::resolve_mcp_setup(name, server_config, &preferences)
        else {
            continue;
        };
        server_config.expand_strings(sub);
        if let Some(oauth) = server_config.oauth_config() {
            oauth_configs.insert(name.clone(), oauth);
        }
        if let Some(server) = server_config.to_acp_mcp_server(name.clone()) {
            servers.push(server);
        } else {
            tracing::warn!(
                source = source_label,
                server = name,
                "MCP server has no 'command' (stdio) or 'url' (http/sse); skipping"
            );
        }
    }

    if !servers.is_empty() {
        tracing::info!(
            source = source_label,
            count = servers.len(),
            "loaded MCP servers"
        );
    }

    (servers, oauth_configs)
}

#[cfg(test)]
#[path = "json_config_tests.rs"]
mod tests;
