//! A session's plugin config and the plugin registry it starts with.

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use serde::Deserialize;
use serde::Serialize;
use xai_grok_agent::plugins::PluginRegistry;
use xai_grok_agent::plugins::SharedPluginRegistryHandle;
use xai_grok_agent::plugins::discovery::DiscoveryConfig;
use xai_grok_hooks::discovery::ClaudeImport;
use xai_grok_hooks::trust::Trust;

/// Plugin system configuration from `[plugins]` section in config.toml.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PluginsConfig {
    #[serde(default)]
    pub paths: Vec<String>,
    /// Plugin IDs or names to disable.
    /// Disabled plugins are discovered but their components are not loaded into the session.
    #[serde(default)]
    pub disabled: Vec<String>,
    /// Plugin IDs or names to explicitly enable.
    /// Used for project-scope plugins, which are disabled by default; adding a plugin here overrides that default.
    #[serde(default)]
    pub enabled: Vec<String>,
    /// CLI `--plugin-dir` paths (populated by CLI arg processing, not config file).
    #[serde(skip)]
    pub cli_plugin_dirs: Vec<PathBuf>,
}

impl PluginsConfig {
    /// Reads each list on its own and keeps its string entries.
    /// A malformed `paths` or `enabled` must not drop `disabled`, which would re-enable those plugins.
    fn from_config(config: &toml::Value) -> Self {
        let list = |key: &str| -> Vec<String> {
            config
                .get("plugins")
                .and_then(|plugins| plugins.get(key))
                .and_then(toml::Value::as_array)
                .map(|entries| {
                    entries
                        .iter()
                        .filter_map(toml::Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default()
        };
        Self {
            paths: list("paths"),
            disabled: list("disabled"),
            enabled: list("enabled"),
            cli_plugin_dirs: Vec::new(),
        }
    }
}

/// What a session's plugin config is resolved from, read by the caller.
#[derive(Debug, Clone, Copy)]
pub struct PluginConfigInputs<'a> {
    /// `None` when the config is unreadable, so it contributes no `[plugins]`.
    pub effective_config: Option<&'a toml::Value>,
    /// The home directory, which holds the user's Claude settings.
    pub home: Option<&'a Path>,
    /// The Grok home, whose `config.toml` is the user's and never a project's.
    pub grok_home: Option<&'a Path>,
    pub cwd: &'a Path,
    pub trust: Trust,
    pub claude_import: ClaudeImport,
}

/// A project's `[plugins].paths` merge only when `trust` allows project sources; its `disabled` list always merges.
/// Claude plugin lists come only from `<home>/.claude/settings.json`, and not after a Claude import.
pub fn resolve_effective_plugins_config(inputs: PluginConfigInputs<'_>) -> DiscoveryConfig {
    let PluginConfigInputs {
        effective_config,
        home,
        grok_home,
        cwd,
        trust,
        claude_import,
    } = inputs;
    let mut plugins = effective_config
        .map(PluginsConfig::from_config)
        .unwrap_or_default();

    for config_path in crate::project_config::find_project_configs_under(cwd, home, grok_home) {
        if let Ok(toml_val) = xai_grok_config::load_config_file(&config_path) {
            let project = PluginsConfig::from_config(&toml_val);
            // Project `paths` load as auto-trusted plugins, which is repo-controlled code execution.
            // `disabled` only removes plugins, so it needs no trust.
            if trust.allows_project_sources() {
                plugins.paths.extend(project.paths);
            }
            plugins.disabled.extend(project.disabled);
        }
    }

    // A repo's own `.claude/settings.json` is never read:
    // it could enable its plugins' hooks before the repo is trusted.
    if claude_import == ClaudeImport::NotImported
        && let Some(home) = home
    {
        let (claude_enabled, claude_disabled) =
            xai_grok_agent::plugins::marketplace::load_enabled_disabled_plugins(
                &home.join(".claude").join("settings.json"),
            );
        add_unlisted_claude_plugins(&mut plugins, claude_enabled, claude_disabled);
    }

    DiscoveryConfig {
        cli_plugin_dirs: Vec::new(),
        config_paths: plugins.paths.iter().map(PathBuf::from).collect(),
        disabled: plugins.disabled,
        enabled: plugins.enabled,
    }
}

/// Grok's own `enabled` and `disabled` lists take precedence over Claude's.
fn add_unlisted_claude_plugins(
    plugins: &mut PluginsConfig,
    claude_enabled: Vec<String>,
    claude_disabled: Vec<String>,
) {
    for name in claude_enabled {
        if !plugins.disabled.contains(&name) && !plugins.enabled.contains(&name) {
            plugins.enabled.push(name);
        }
    }
    for name in claude_disabled {
        if !plugins.enabled.contains(&name) && !plugins.disabled.contains(&name) {
            plugins.disabled.push(name);
        }
    }
}

/// Plugin directories that are absolute, existing, and distinct.
/// Only [`SessionPluginDirs::parse`] builds one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionPluginDirs {
    dirs: Vec<PathBuf>,
}

impl SessionPluginDirs {
    pub fn parse(entries: &[serde_json::Value]) -> Self {
        let mut dirs = Vec::new();
        for entry in entries {
            let Some(raw) = entry.as_str() else {
                tracing::warn!(?entry, "pluginDirs entry is not a string; skipping");
                continue;
            };
            let path = PathBuf::from(raw);
            if !path.is_absolute() {
                tracing::warn!("pluginDirs entry is not absolute; skipping");
                continue;
            }
            let canonical = dunce::canonicalize(&path).unwrap_or(path);
            if !canonical.is_dir() {
                tracing::warn!("pluginDirs entry is not a directory; skipping");
                continue;
            }
            if !dirs.contains(&canonical) {
                dirs.push(canonical);
            }
        }
        Self { dirs }
    }

    pub fn as_paths(&self) -> &[PathBuf] {
        &self.dirs
    }
}

/// Copies local plugin installs again, then discovers plugins for `inputs.cwd`.
/// Project plugins and `[plugins].paths` load only when `inputs.trust` allows project hooks.
pub fn session_plugin_registry(
    handle: &SharedPluginRegistryHandle,
    plugin_dirs: &SessionPluginDirs,
    inputs: PluginConfigInputs<'_>,
) -> Option<Arc<PluginRegistry>> {
    let disk_config = resolve_effective_plugins_config(inputs);
    handle.refresh_and_build_for_cwd(
        inputs.cwd,
        &disk_config,
        plugin_dirs.as_paths(),
        inputs.trust.allows_project_sources(),
    )
}

#[cfg(test)]
#[path = "plugins_tests.rs"]
mod tests;
