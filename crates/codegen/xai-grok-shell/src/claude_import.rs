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
