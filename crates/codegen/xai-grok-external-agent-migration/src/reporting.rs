use std::path::Path;

use super::config_values::{
    ImportPlan, ImportRuleAction, ImportableItem, PathKind, format_rule_string,
};
use super::scope::find_project_root;

impl ImportPlan {
    #[must_use]
    pub fn summary(&self, cwd: &Path) -> String {
        if self.is_empty() {
            return "No Claude settings found to import.".to_string();
        }

        let mut out = String::from("Found Claude settings to import:\n");
        if !self.global_items.is_empty() {
            out.push_str("\nGlobal (~/.grok/config.toml):\n");
            out.push_str(&format_item_summary(&self.global_items));
        }
        if !self.project_items.is_empty() {
            out.push_str(&format!(
                "\nProject ({}/.grok/config.toml):\n",
                find_project_root(cwd).display()
            ));
            out.push_str(&format_item_summary(&self.project_items));
        }
        out
    }
}

fn format_item_summary(items: &[ImportableItem]) -> String {
    let mut out = String::new();

    let perms: Vec<_> = items
        .iter()
        .filter_map(|item| match item {
            ImportableItem::Permission(rule) => Some(rule),
            ImportableItem::EnvVar { .. }
            | ImportableItem::McpServer { .. }
            | ImportableItem::Hook { .. }
            | ImportableItem::PathEntry { .. } => None,
        })
        .collect();
    if !perms.is_empty() {
        let mut allow = 0u32;
        let mut deny = 0u32;
        let mut ask = 0u32;
        for rule in &perms {
            match rule.action {
                ImportRuleAction::Allow => allow += 1,
                ImportRuleAction::Deny => deny += 1,
                ImportRuleAction::Ask => ask += 1,
            }
        }
        let mut parts = Vec::new();
        if allow > 0 {
            parts.push(format!("{allow} allow"));
        }
        if deny > 0 {
            parts.push(format!("{deny} deny"));
        }
        if ask > 0 {
            parts.push(format!("{ask} ask"));
        }
        out.push_str(&format!(
            "  - {} permission rule(s) ({})\n",
            perms.len(),
            parts.join(", ")
        ));
        for rule in &perms {
            let action = match rule.action {
                ImportRuleAction::Allow => "allow",
                ImportRuleAction::Deny => "deny",
                ImportRuleAction::Ask => "ask",
            };
            out.push_str(&format!("      {action} {}\n", format_rule_string(rule)));
        }
    }

    let envs: Vec<_> = items
        .iter()
        .filter_map(|item| match item {
            ImportableItem::EnvVar { key, value } => Some((key, value)),
            ImportableItem::Permission(_)
            | ImportableItem::McpServer { .. }
            | ImportableItem::Hook { .. }
            | ImportableItem::PathEntry { .. } => None,
        })
        .collect();
    if !envs.is_empty() {
        out.push_str(&format!("  - {} environment variable(s)\n", envs.len()));
        for (key, value) in &envs {
            // An env value can be a token. The summary shows only its length.
            out.push_str(&format!(
                "      {key} = <redacted, {} chars>\n",
                value.len()
            ));
        }
    }

    let mcps: Vec<_> = items
        .iter()
        .filter_map(|item| match item {
            ImportableItem::McpServer { name, .. } => Some(name),
            ImportableItem::Permission(_)
            | ImportableItem::EnvVar { .. }
            | ImportableItem::Hook { .. }
            | ImportableItem::PathEntry { .. } => None,
        })
        .collect();
    if !mcps.is_empty() {
        out.push_str(&format!("  - {} MCP server(s)\n", mcps.len()));
        for name in &mcps {
            out.push_str(&format!("      {name}\n"));
        }
    }

    let hooks: Vec<_> = items
        .iter()
        .filter_map(|item| match item {
            ImportableItem::Hook {
                event,
                matcher,
                command,
                timeout,
            } => Some((event, matcher, command, timeout)),
            ImportableItem::Permission(_)
            | ImportableItem::EnvVar { .. }
            | ImportableItem::McpServer { .. }
            | ImportableItem::PathEntry { .. } => None,
        })
        .collect();
    if !hooks.is_empty() {
        out.push_str(&format!("  - {} hook(s)\n", hooks.len()));
        for (event, matcher, command, timeout) in &hooks {
            let matcher = matcher.as_deref().unwrap_or("*");
            let timeout_suffix = match timeout {
                Some(timeout) => format!(" (timeout: {timeout}s)"),
                None => String::new(),
            };
            out.push_str(&format!(
                "      {event} [{matcher}] {command}{timeout_suffix}\n"
            ));
        }
    }

    let paths = items.iter().filter_map(|item| match item {
        ImportableItem::PathEntry { kind, path } => Some((*kind, path)),
        ImportableItem::Permission(_)
        | ImportableItem::EnvVar { .. }
        | ImportableItem::McpServer { .. }
        | ImportableItem::Hook { .. } => None,
    });
    let skill_paths: Vec<&String> = paths
        .clone()
        .filter_map(|(kind, path)| match kind {
            PathKind::Skill => Some(path),
            PathKind::Rule => None,
        })
        .collect();
    let rule_paths: Vec<&String> = paths
        .filter_map(|(kind, path)| match kind {
            PathKind::Rule => Some(path),
            PathKind::Skill => None,
        })
        .collect();
    if !skill_paths.is_empty() {
        out.push_str(&format!("  - {} extra skill dir(s)\n", skill_paths.len()));
        for path in &skill_paths {
            out.push_str(&format!("      {path}\n"));
        }
    }
    if !rule_paths.is_empty() {
        out.push_str(&format!("  - {} extra rule dir(s)\n", rule_paths.len()));
        for path in &rule_paths {
            out.push_str(&format!("      {path}\n"));
        }
    }

    out
}
