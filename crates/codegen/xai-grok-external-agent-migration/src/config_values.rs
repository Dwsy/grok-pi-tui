use std::collections::HashMap;

use xai_grok_config::McpServerConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportScope {
    Global,
    Project,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathKind {
    Skill,
    Rule,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportRuleAction {
    Allow,
    Deny,
    Ask,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportTool {
    Any,
    Bash,
    Edit,
    Read,
    Grep,
    Mcp,
    WebFetch,
    WebSearch,
    AgentMessage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportPatternMode {
    Glob,
    Domain,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportPermission {
    pub action: ImportRuleAction,
    pub tool: ImportTool,
    pub pattern: Option<String>,
    pub pattern_mode: ImportPatternMode,
}

#[derive(Debug, Clone, Default)]
pub struct ClaudeSettingsSnapshot {
    pub permissions: Vec<ImportPermission>,
    pub env: HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub enum ImportableItem {
    Permission(ImportPermission),
    EnvVar {
        key: String,
        value: String,
    },
    McpServer {
        name: String,
        config: Box<McpServerConfig>,
    },
    Hook {
        event: String,
        matcher: Option<String>,
        command: String,
        timeout: Option<u64>,
    },
    PathEntry {
        kind: PathKind,
        path: String,
    },
}

#[derive(Debug, Clone, Default)]
pub struct ImportPlan {
    pub global_items: Vec<ImportableItem>,
    pub project_items: Vec<ImportableItem>,
}

impl ImportPlan {
    #[must_use]
    pub fn total_items(&self) -> usize {
        self.global_items.len() + self.project_items.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.global_items.is_empty() && self.project_items.is_empty()
    }
}

#[derive(Debug, Default)]
pub struct ImportResult {
    pub global_count: usize,
    pub project_count: usize,
    pub modified_files: Vec<String>,
}

impl ImportResult {
    #[must_use]
    pub fn total(&self) -> usize {
        self.global_count + self.project_count
    }
}

fn tool_label(tool: ImportTool) -> &'static str {
    match tool {
        ImportTool::Any => "",
        ImportTool::Bash => "Bash",
        ImportTool::Edit => "Edit",
        ImportTool::Read => "Read",
        ImportTool::Grep => "Grep",
        ImportTool::Mcp => "MCPTool",
        ImportTool::WebFetch => "WebFetch",
        ImportTool::WebSearch => "WebSearch",
        ImportTool::AgentMessage => "AgentMessage",
    }
}

pub(super) fn format_rule_string(rule: &ImportPermission) -> String {
    let tool_name = tool_label(rule.tool);
    match (rule.pattern.as_deref(), rule.tool) {
        (None, ImportTool::Any) => "*".to_string(),
        (Some(pattern), ImportTool::Any) => pattern.to_string(),
        (
            None,
            ImportTool::Bash
            | ImportTool::Edit
            | ImportTool::Read
            | ImportTool::Grep
            | ImportTool::Mcp
            | ImportTool::WebFetch
            | ImportTool::WebSearch
            | ImportTool::AgentMessage,
        ) => tool_name.to_string(),
        (
            Some(pattern),
            ImportTool::Bash
            | ImportTool::Edit
            | ImportTool::Read
            | ImportTool::Grep
            | ImportTool::Mcp
            | ImportTool::WebFetch
            | ImportTool::WebSearch
            | ImportTool::AgentMessage,
        ) => {
            let pattern = match rule.pattern_mode {
                ImportPatternMode::Domain => format!("domain:{pattern}"),
                ImportPatternMode::Glob => pattern.to_string(),
            };
            format!("{tool_name}({pattern})")
        }
    }
}

#[cfg(test)]
#[path = "config_values_tests.rs"]
mod tests;
