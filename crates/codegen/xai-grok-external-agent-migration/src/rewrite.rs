use std::collections::HashSet;
use std::path::Path;

use toml::Value as TomlValue;
use toml::map::Map as TomlMap;
use tracing::{info, warn};

use super::config_values::{
    ImportPermission, ImportPlan, ImportResult, ImportRuleAction, ImportScope, ImportableItem,
    PathKind, format_rule_string,
};
use super::error::MigrationError;
use super::hooks_cla::apply_hooks_to_dir;
use super::mcp::merge_mcp_servers;
use super::scope::find_project_root;
use xai_grok_config::fs_atomic;
use xai_grok_config::{USER_CONFIG_FILENAME, grok_home, user_config_file};

pub fn write_import_marker(grok_home: &Path) -> Result<(), MigrationError> {
    let config_path = user_config_file(grok_home);
    let mut root = read_toml_root(&config_path, "refusing to write import marker")?;
    let table = root.as_table_mut().ok_or(MigrationError::NotATable {
        section: "config root",
    })?;

    let compat = table
        .entry("claude_compat")
        .or_insert_with(|| TomlValue::Table(TomlMap::new()));
    let compat_table = compat.as_table_mut().ok_or(MigrationError::NotATable {
        section: "[claude_compat]",
    })?;
    compat_table.insert("imported".to_string(), TomlValue::Boolean(true));

    write_toml_root(&config_path, &root, ImportScope::Global)?;
    Ok(())
}

pub fn apply_import(plan: &ImportPlan, cwd: &Path) -> Result<ImportResult, MigrationError> {
    let mut result = ImportResult::default();

    if !plan.global_items.is_empty() {
        result.global_count = apply_items_to_grok_dir(
            &grok_home(),
            &plan.global_items,
            ImportScope::Global,
            &mut result.modified_files,
        )?;
    }

    if !plan.project_items.is_empty() {
        let project_grok_dir = find_project_root(cwd).join(".grok");
        result.project_count = apply_items_to_grok_dir(
            &project_grok_dir,
            &plan.project_items,
            ImportScope::Project,
            &mut result.modified_files,
        )?;
    }

    Ok(result)
}

fn apply_items_to_grok_dir(
    grok_dir: &Path,
    items: &[ImportableItem],
    scope: ImportScope,
    modified_files: &mut Vec<String>,
) -> Result<usize, MigrationError> {
    let config_path = grok_dir.join(USER_CONFIG_FILENAME);
    let mut count = apply_items_to_config(&config_path, items, scope)?;
    if count > 0 {
        modified_files.push(config_path.to_string_lossy().to_string());
    }

    let hooks_dir = grok_dir.join("hooks");
    let hook_count = apply_hooks_to_dir(&hooks_dir, items)?;
    count += hook_count;
    if hook_count > 0 {
        modified_files.push(
            hooks_dir
                .join("imported-from-claude.json")
                .to_string_lossy()
                .to_string(),
        );
    }

    Ok(count)
}

// A config.toml that does not parse stays unchanged.
// Rewriting it would drop the sections this import does not write.
fn read_toml_root(config_path: &Path, action: &'static str) -> Result<TomlValue, MigrationError> {
    match std::fs::read_to_string(config_path) {
        Ok(contents) => toml::from_str(&contents).map_err(|source| MigrationError::InvalidToml {
            action,
            path: config_path.to_path_buf(),
            source,
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(TomlValue::Table(TomlMap::new()))
        }
        Err(source) => Err(MigrationError::io("read", config_path, source)),
    }
}

// A global write follows a symlink at `path`.
// A project write replaces that symlink.
fn write_scoped(path: &Path, contents: &str, scope: ImportScope) -> Result<(), MigrationError> {
    let dest = match scope {
        ImportScope::Global => {
            let first = fs_atomic::bind_follow_destination(path)
                .map_err(|source| MigrationError::io("resolve", path, source))?;
            fs_atomic::require_same_bound_destination(path, &first)
                .map_err(|source| MigrationError::io("resolve", path, source))?
        }
        ImportScope::Project => fs_atomic::bind_slot_destination(path)
            .map_err(|source| MigrationError::io("resolve", path, source))?,
    };

    if let Some(parent) = dest.as_path().parent() {
        std::fs::create_dir_all(parent)
            .map_err(|source| MigrationError::io("create directory", parent, source))?;
    }
    fs_atomic::write_atomically_bound(&dest, contents, None)
        .map_err(|source| MigrationError::io("write", path, source))?;
    Ok(())
}

fn apply_items_to_config(
    config_path: &Path,
    items: &[ImportableItem],
    scope: ImportScope,
) -> Result<usize, MigrationError> {
    let mut root = read_toml_root(config_path, "refusing to import")?;
    let table = root.as_table_mut().ok_or(MigrationError::NotATable {
        section: "config root",
    })?;

    let mut permissions = Vec::new();
    let mut env_vars = Vec::new();
    let mut mcp_servers = Vec::new();
    let mut skill_dirs = Vec::new();
    let mut rule_dirs = Vec::new();
    for item in items {
        match item {
            ImportableItem::Permission(rule) => permissions.push(rule),
            ImportableItem::EnvVar { key, value } => env_vars.push((key.as_str(), value.as_str())),
            ImportableItem::McpServer { name, config } => {
                mcp_servers.push((name.as_str(), config.as_ref()))
            }
            ImportableItem::Hook { .. } => {}
            ImportableItem::PathEntry { kind, path } => match kind {
                PathKind::Skill => skill_dirs.push(path.as_str()),
                PathKind::Rule => rule_dirs.push(path.as_str()),
            },
        }
    }

    let mut count = 0usize;
    if !permissions.is_empty() {
        count += merge_permissions(table, &permissions)?;
    }
    if !skill_dirs.is_empty() {
        count += merge_paths(table, "extra_skill_dirs", &skill_dirs)?;
    }
    if !rule_dirs.is_empty() {
        count += merge_paths(table, "extra_rule_dirs", &rule_dirs)?;
    }
    if !env_vars.is_empty() {
        count += merge_env_vars(table, &env_vars);
    }
    if !mcp_servers.is_empty() {
        count += merge_mcp_servers(table, &mcp_servers)?;
    }

    if count > 0 {
        write_toml_root(config_path, &root, scope)?;
        info!(
            path = %config_path.display(),
            count,
            "Wrote imported settings to config.toml"
        );
    }

    Ok(count)
}

fn write_toml_root(
    config_path: &Path,
    root: &TomlValue,
    scope: ImportScope,
) -> Result<(), MigrationError> {
    let toml_str =
        toml::to_string_pretty(root).map_err(|source| MigrationError::TomlSerialize {
            path: config_path.to_path_buf(),
            source,
        })?;
    write_scoped(config_path, &toml_str, scope)
}

fn merge_permissions(
    table: &mut TomlMap<String, TomlValue>,
    rules: &[&ImportPermission],
) -> Result<usize, MigrationError> {
    let permission = table
        .entry("permission")
        .or_insert_with(|| TomlValue::Table(TomlMap::new()));
    let perm_table = permission.as_table_mut().ok_or(MigrationError::NotATable {
        section: "[permission]",
    })?;

    let mut allow_rules = Vec::new();
    let mut deny_rules = Vec::new();
    let mut ask_rules = Vec::new();
    for rule in rules {
        let formatted = format_rule_string(rule);
        match rule.action {
            ImportRuleAction::Allow => allow_rules.push(formatted),
            ImportRuleAction::Deny => deny_rules.push(formatted),
            ImportRuleAction::Ask => ask_rules.push(formatted),
        }
    }

    let mut count = 0;
    for (key, new_rules) in [
        ("allow", allow_rules),
        ("deny", deny_rules),
        ("ask", ask_rules),
    ] {
        if new_rules.is_empty() {
            continue;
        }
        let arr = perm_table
            .entry(key)
            .or_insert_with(|| TomlValue::Array(Vec::new()));
        let existing = arr
            .as_array_mut()
            .ok_or_else(|| MigrationError::NotAnArray {
                section: format!("permission.{key}"),
            })?;
        count += append_missing_strings(existing, new_rules);
    }
    Ok(count)
}

fn merge_env_vars(table: &mut TomlMap<String, TomlValue>, vars: &[(&str, &str)]) -> usize {
    let env = table
        .entry("env")
        .or_insert_with(|| TomlValue::Table(TomlMap::new()));
    let Some(env_table) = env.as_table_mut() else {
        warn!("[env] in config.toml is not a table, skipping env import");
        return 0;
    };

    let mut count = 0;
    for (key, value) in vars {
        if !env_table.contains_key(*key) {
            env_table.insert((*key).to_string(), TomlValue::String((*value).to_string()));
            count += 1;
        }
    }
    count
}

fn merge_paths(
    table: &mut TomlMap<String, TomlValue>,
    key: &str,
    new_paths: &[&str],
) -> Result<usize, MigrationError> {
    let paths = table
        .entry("paths")
        .or_insert_with(|| TomlValue::Table(TomlMap::new()));
    let paths_table = paths
        .as_table_mut()
        .ok_or(MigrationError::NotATable { section: "[paths]" })?;

    let arr = paths_table
        .entry(key)
        .or_insert_with(|| TomlValue::Array(Vec::new()));
    let existing = arr
        .as_array_mut()
        .ok_or_else(|| MigrationError::NotAnArray {
            section: format!("paths.{key}"),
        })?;
    Ok(append_missing_strings(existing, new_paths.iter().copied()))
}

fn append_missing_strings<S: AsRef<str> + Into<String>>(
    existing: &mut Vec<TomlValue>,
    new_values: impl IntoIterator<Item = S>,
) -> usize {
    let existing_set: HashSet<String> = existing
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();

    let mut count = 0usize;
    for value in new_values {
        if !existing_set.contains(value.as_ref()) {
            existing.push(TomlValue::String(value.into()));
            count += 1;
        }
    }
    count
}

#[cfg(test)]
#[path = "rewrite_tests.rs"]
mod tests;
