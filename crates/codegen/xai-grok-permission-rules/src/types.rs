use serde::{Deserialize, Serialize};

#[derive(Clone, Debug)]
pub enum AccessKind {
    Read(Option<String>),
    Grep {
        path: Option<String>,
        glob: Option<String>,
    },
    /// A file edit, named by its path.
    Edit(String),
    Bash(String),
    MCPTool {
        name: String,
        input: serde_json::Value,
    },
    WebFetch(String),
    WebSearch(String),
    AgentMessage {
        subagent_id: String,
    },
    /// A mutating tool that is neither a file edit, a command, nor an MCP call (subagent spawn,
    /// scheduler, workflow, generation, deploy, feedback, browser, anything unclassified), named by
    /// its tool id. No grant scope covers it: every call prompts.
    Tool(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Ask,
    FollowupMessage(String),
    Reject(String),
    PolicyDeny(String),
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EditPolicy {
    #[default]
    Ask,
    Allow,
    Reject,
}

impl Serialize for EditPolicy {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(match self {
            Self::Ask => "ask",
            Self::Allow => "allow",
            Self::Reject => "reject",
        })
    }
}

impl<'de> Deserialize<'de> for EditPolicy {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = EditPolicy;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("one of: ask, allow, reject")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<EditPolicy, E> {
                match v {
                    "ask" => Ok(EditPolicy::Ask),
                    "allow" => Ok(EditPolicy::Allow),
                    "reject" => Ok(EditPolicy::Reject),
                    other => Err(E::unknown_variant(other, &["ask", "allow", "reject"])),
                }
            }
        }
        deserializer.deserialize_str(V)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PermissionConfig {
    pub rules: Vec<PermissionRule>,
    #[serde(default)]
    pub prompt_policy: PromptPolicy,
    #[serde(default)]
    pub default_mode_configured: bool,
}

impl PermissionConfig {
    pub fn new(rules: Vec<PermissionRule>) -> Self {
        Self {
            rules,
            prompt_policy: PromptPolicy::Ask,
            default_mode_configured: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PromptPolicy {
    #[default]
    Ask,
    Deny,
    Auto,
    Allow,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRule {
    pub action: RuleAction,
    #[serde(default)]
    pub tool: ToolFilter,
    pub pattern: Option<String>,
    #[serde(default)]
    pub pattern_mode: PatternMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PatternMode {
    #[default]
    Glob,
    Domain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RuleAction {
    Allow,
    #[default]
    Deny,
    Ask,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ToolFilter {
    #[default]
    Any,
    Bash,
    Edit,
    Read,
    Grep,
    Mcp,
    WebFetch,
    WebSearch,
    #[serde(rename = "agent_message", alias = "agentmessage")]
    AgentMessage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequirementSource {
    Unknown,
    Requirements { path: std::path::PathBuf },
    SystemRequirements { path: std::path::PathBuf },
    ManagedSettings { path: std::path::PathBuf },
    ManagedConfig { path: std::path::PathBuf },
    Config { path: std::path::PathBuf },
    Settings { path: std::path::PathBuf },
}

impl std::fmt::Display for RequirementSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unknown => f.write_str("<unknown>"),
            Self::Requirements { path } => write!(f, "{} (requirements)", path.display()),
            Self::SystemRequirements { path } => {
                write!(f, "{} (system requirements)", path.display())
            }
            Self::ManagedSettings { path } => write!(f, "{} (managed-settings)", path.display()),
            Self::ManagedConfig { path } => write!(f, "{} (managed config)", path.display()),
            Self::Config { path } => write!(f, "{} (config)", path.display()),
            Self::Settings { path } => write!(f, "{} (settings)", path.display()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Sourced<T> {
    pub value: T,
    pub source: RequirementSource,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_message_tool_filter_serde_is_dedicated_and_unknown_is_rejected() {
        let filter: ToolFilter = serde_json::from_str(r#""agent_message""#).unwrap();
        assert_eq!(filter, ToolFilter::AgentMessage);
        assert_eq!(
            serde_json::to_string(&filter).unwrap(),
            r#""agent_message""#
        );
        assert!(serde_json::from_str::<ToolFilter>(r#""future_tool""#).is_err());
    }
}
