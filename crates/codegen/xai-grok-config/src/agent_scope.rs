//! Lightweight provenance for an agent definition.
//!
//! Kept in config so presentation code can inspect scope without depending on
//! the stock in-process agent runtime.

/// Where an agent definition was discovered.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AgentScope {
    /// .grok/agents/ (project-level, highest priority)
    Project,
    /// ~/.grok/agents/ (user-level)
    User,
    /// ~/.grok/bundled/agents/ (lowest-priority bundled cache)
    Bundled,
    /// Built-in agent (e.g., default_grok_build(), browser_use()).
    #[default]
    BuiltIn,
}

impl AgentScope {
    pub fn label(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::User => "user",
            Self::Bundled => "bundled",
            Self::BuiltIn => "built-in",
        }
    }
}

impl std::fmt::Display for AgentScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

#[cfg(test)]
mod tests {
    use super::AgentScope;

    #[test]
    fn stable_scope_labels() {
        assert_eq!(AgentScope::default(), AgentScope::BuiltIn);
        assert_eq!(AgentScope::Project.to_string(), "project");
        assert_eq!(AgentScope::User.to_string(), "user");
        assert_eq!(AgentScope::Bundled.to_string(), "bundled");
        assert_eq!(AgentScope::BuiltIn.to_string(), "built-in");
    }
}
