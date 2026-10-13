use std::path::Path;
use std::sync::OnceLock;

use tracing::{debug, info};

use xai_grok_external_agent_migration::{
    ClaudeSettingsSnapshot, ImportPatternMode, ImportPermission, ImportPlan, ImportRuleAction,
    ImportTool,
};
use xai_grok_workspace::permission::claude_settings::{
    find_claude_settings_paths, load_claude_settings,
};
use xai_grok_workspace::permission::rules::parse_permission_rule;
use xai_grok_workspace::permission::types::{PatternMode, RuleAction, ToolFilter};

static MARKER_CACHE: std::sync::RwLock<Option<bool>> = std::sync::RwLock::new(None);

// A manual edit of the marker is read on the next start.
pub(crate) fn is_claude_import_marked() -> bool {
    if let Some(value) = *MARKER_CACHE.read().expect("MARKER_CACHE poisoned") {
        return value;
    }
    let value = xai_grok_config::is_claude_import_marked(&crate::util::grok_home::grok_home());
    *MARKER_CACHE.write().expect("MARKER_CACHE poisoned") = Some(value);
    value
}

pub(crate) fn refresh_marker_cache(value: bool) {
    *MARKER_CACHE.write().expect("MARKER_CACHE poisoned") = Some(value);
}

#[cfg(test)]
pub(crate) fn reset_marker_cache_for_test() {
    *MARKER_CACHE.write().expect("MARKER_CACHE poisoned") = None;
}

pub(crate) fn is_claude_import_marked_with_log(gate_name: &'static str) -> bool {
    static LOGGED: OnceLock<()> = OnceLock::new();
    let marked = is_claude_import_marked();
    if marked {
        LOGGED.get_or_init(|| {
            info!(
                first_gate = gate_name,
                "Claude compat disabled (marker set in config.toml)"
            );
        });
    }
    marked
}

pub(crate) fn import_marker() -> xai_grok_hooks::discovery::ClaudeImport {
    xai_grok_hooks::discovery::ClaudeImport::from_marker(is_claude_import_marked_with_log(
        "hook_and_plugin_sources",
    ))
}

pub fn mark_claude_imported() -> anyhow::Result<()> {
    xai_grok_external_agent_migration::write_import_marker(&crate::util::grok_home::grok_home())?;
    refresh_marker_cache(true);
    Ok(())
}

pub fn scan_importable_settings(cwd: &Path) -> ImportPlan {
    let paths = find_claude_settings_paths(cwd);
    let mcp_json_files = crate::util::config::find_mcp_json_files(cwd);
    xai_grok_external_agent_migration::scan_importable_settings(
        cwd,
        &paths,
        snapshot_at,
        &mcp_json_files,
    )
}

/// Apply an import plan by writing TOML patches to the appropriate config files. This is additive-only: existing entries are never removed.
/// New permission rules are appended; new env vars and MCP servers are added without overwriting existing keys or names.
/// Project items are written to the discovered repository root's product config directory, not the current subdirectory.
pub fn apply_import(plan: &ImportPlan, cwd: &Path) -> anyhow::Result<ImportResult> {
    let mut result = ImportResult::default();

    if !plan.global_items.is_empty() {
        let global_path = crate::util::grok_home::grok_home().join("config.toml");
        let count = apply_items_to_config(&global_path, &plan.global_items)?;
        result.global_count = count;
        if count > 0 {
            result
                .modified_files
                .push(global_path.to_string_lossy().to_string());
        }

        let hooks_dir = crate::util::grok_home::grok_home().join("hooks");
        let hook_count = apply_hooks_to_dir(&hooks_dir, &plan.global_items)?;
        result.global_count += hook_count;
        if hook_count > 0 {
            result.modified_files.push(
                hooks_dir
                    .join("imported-from-claude.json")
                    .to_string_lossy()
                    .to_string(),
            );
        }
    }

    if !plan.project_items.is_empty() {
        let project_root = find_project_root(cwd);
        let project_path = xai_grok_config::project_config_dir(&project_root).join("config.toml");
        let count = apply_items_to_config(&project_path, &plan.project_items)?;
        result.project_count = count;
        if count > 0 {
            result
                .modified_files
                .push(project_path.to_string_lossy().to_string());
        }

        let hooks_dir = xai_grok_config::project_config_dir(&project_root).join("hooks");
        let hook_count = apply_hooks_to_dir(&hooks_dir, &plan.project_items)?;
        result.project_count += hook_count;
        if hook_count > 0 {
            result.modified_files.push(
                hooks_dir
                    .join("imported-from-claude.json")
                    .to_string_lossy()
                    .to_string(),
            );
        }
    }

    // The slash command writes the import marker even when the plan is empty.
    Ok(result)
}

fn snapshot_at(path: &Path) -> Option<ClaudeSettingsSnapshot> {
    let settings = load_claude_settings(path)?;
    let mut permissions = Vec::new();
    if let Some(perms) = settings.permissions {
        for (action, entries) in [
            (RuleAction::Allow, perms.allow),
            (RuleAction::Deny, perms.deny),
            (RuleAction::Ask, perms.ask),
        ] {
            for rule_str in entries {
                match parse_permission_rule(&rule_str, action) {
                    Ok(rule) => match import_permission(rule) {
                        Some(rule) => permissions.push(rule),
                        None => debug!(
                            path = %path.display(),
                            rule = %rule_str,
                            "Skipping Claude permission rule with an unrecognized tool"
                        ),
                    },
                    Err(error) => {
                        debug!(
                            path = %path.display(),
                            rule = %rule_str,
                            error = %error,
                            "Skipping unparseable Claude permission rule"
                        );
                    }
                }
            }
        }
    }
    Some(ClaudeSettingsSnapshot {
        permissions,
        env: settings.env.unwrap_or_default(),
    })
}

fn import_permission(
    rule: xai_grok_workspace::permission::types::PermissionRule,
) -> Option<ImportPermission> {
    let tool = match rule.tool {
        ToolFilter::Any => ImportTool::Any,
        ToolFilter::Bash => ImportTool::Bash,
        ToolFilter::Edit => ImportTool::Edit,
        ToolFilter::Read => ImportTool::Read,
        ToolFilter::Grep => ImportTool::Grep,
        ToolFilter::Mcp => ImportTool::Mcp,
        ToolFilter::WebFetch => ImportTool::WebFetch,
        ToolFilter::WebSearch => ImportTool::WebSearch,
        ToolFilter::AgentMessage => ImportTool::AgentMessage,
    };
    let action = match rule.action {
        RuleAction::Allow => ImportRuleAction::Allow,
        RuleAction::Deny => ImportRuleAction::Deny,
        RuleAction::Ask => ImportRuleAction::Ask,
    };
    let pattern_mode = match rule.pattern_mode {
        PatternMode::Glob => ImportPatternMode::Glob,
        PatternMode::Domain => ImportPatternMode::Domain,
    };
    Some(ImportPermission {
        action,
        tool,
        pattern: rule.pattern,
        pattern_mode,
    })
}

#[cfg(test)]
#[path = "claude_import_tests.rs"]
mod tests;
