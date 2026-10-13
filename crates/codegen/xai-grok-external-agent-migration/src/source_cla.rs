use std::collections::HashSet;
use std::path::{Path, PathBuf};

use tracing::{debug, info};

use super::config_values::{ClaudeSettingsSnapshot, ImportPlan, ImportableItem, PathKind};
use super::hooks_cla::extract_hooks_from_settings_file;
use super::scope::find_project_root;
use xai_grok_config::mcp_servers::{
    load_claude_json_mcp_servers_as_configs_unfiltered, load_mcp_json_servers_as_configs_unfiltered,
};

pub fn scan_importable_settings(
    cwd: &Path,
    settings_paths: &[PathBuf],
    mut snapshot: impl FnMut(&Path) -> Option<ClaudeSettingsSnapshot>,
    mcp_json_files: &[PathBuf],
) -> ImportPlan {
    let mut plan = ImportPlan::default();
    let home = xai_dirs::home_dir();

    for path in settings_paths {
        let Some(settings) = snapshot(path) else {
            continue;
        };

        let is_global = home
            .as_ref()
            .is_some_and(|home| path.starts_with(home.join(".claude")));
        let target = if is_global {
            &mut plan.global_items
        } else {
            &mut plan.project_items
        };

        target.extend(
            settings
                .permissions
                .into_iter()
                .map(ImportableItem::Permission),
        );
        for (key, value) in settings.env {
            target.push(ImportableItem::EnvVar { key, value });
        }
        target.extend(extract_hooks_from_settings_file(path));
    }

    // The ~/.claude.json loader returns user servers and project servers in one map.
    // Both are stored in the global list.
    for (name, config) in load_claude_json_mcp_servers_as_configs_unfiltered(cwd) {
        plan.global_items.push(ImportableItem::McpServer {
            name,
            config: Box::new(config),
        });
    }

    for (name, config) in load_mcp_json_servers_as_configs_unfiltered(mcp_json_files) {
        plan.project_items.push(ImportableItem::McpServer {
            name,
            config: Box::new(config),
        });
    }

    scan_claude_path_dirs(cwd, &mut plan);

    if !plan.is_empty() {
        info!(
            global = plan.global_items.len(),
            project = plan.project_items.len(),
            "Scanned Claude settings for import"
        );
    }

    plan
}

// The same directory is not stored as both global and project.
// An import from the home directory finds the same directories in both scans.
fn scan_claude_path_dirs(cwd: &Path, plan: &mut ImportPlan) {
    let mut global_added = HashSet::new();

    if let Some(home) = xai_dirs::home_dir() {
        for (kind, dir) in existing_claude_path_dirs(&home) {
            let canonical = dunce::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
            global_added.insert(canonical);
            plan.global_items.push(ImportableItem::PathEntry {
                kind,
                path: dir.to_string_lossy().to_string(),
            });
        }
    }

    let project_root = find_project_root(cwd);
    for (kind, dir) in existing_claude_path_dirs(&project_root) {
        let canonical = dunce::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
        if global_added.contains(&canonical) {
            debug!(
                path = %dir.display(),
                "Skipping project .claude/ path that resolves to the same dir as the global entry"
            );
            continue;
        }
        plan.project_items.push(ImportableItem::PathEntry {
            kind,
            path: dir.to_string_lossy().to_string(),
        });
    }
}

fn existing_claude_path_dirs(root: &Path) -> impl Iterator<Item = (PathKind, PathBuf)> {
    [(PathKind::Skill, "skills"), (PathKind::Rule, "rules")]
        .into_iter()
        .map(move |(kind, sub)| (kind, root.join(".claude").join(sub)))
        .filter(|(_, dir)| dir.is_dir())
}
