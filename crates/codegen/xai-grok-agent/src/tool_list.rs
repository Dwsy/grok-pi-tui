//! Which toolset tools an agent's `tools` allowlist keeps and its `disallowed_tools` denylist removes

use std::collections::HashSet;

use xai_grok_tools::registry::types::ToolConfig;
use xai_grok_tools::types::tool::ToolKind;

use crate::config::{AGENT_TASK_CLASSIFIER_RE, short_tool_name, tool_id_eq, tool_id_matches};

/// An `Agent` or `Task` allowlist entry keeps the task tool and its lifecycle companions.
const TASK_DEPS: &[&str] = &["task", "get_task_output", "kill_task", "wait_tasks"];

/// The Grok [`ToolKind`] a vendor-compat `tools:` allowlist entry resolves to, so a plugin's upstream allowlist still binds.
/// Backed by the shared vendor-to-Grok tool registry in `xai-grok-tools` (also used by the hook matcher).
fn claude_tool_kind(name: &str) -> Option<ToolKind> {
    xai_grok_tools::types::kind_for(name)
}

#[derive(Debug, Clone, Copy)]
pub enum ToolList<'a> {
    /// Keeps only the tools it names. An entry among `known_tool_ids` names a real tool the toolset lacks.
    Allowlist { known_tool_ids: &'a HashSet<String> },
    /// Removes the tools it names by id.
    Denylist,
}

#[derive(Debug)]
#[must_use]
pub struct ListedTools<'a> {
    /// The toolset tools that remain, for an allowlist and a denylist alike.
    pub tools: Vec<&'a ToolConfig>,
    /// The `Agent` and `Task` entries, which restrict spawning instead of naming a tool.
    pub spawn_directives: Vec<&'a str>,
    /// Recognized names with no enabled tool.
    /// A vendor name that resolves to a kind the toolset enables does not land here, and neither does an `mcp__*` entry.
    /// A denylist entry that matches no tool id does.
    pub unmatched: Vec<&'a str>,
    /// Allowlist entries that name no tool this build knows. Any one keeps the whole toolset.
    pub unknown: Vec<&'a str>,
}

/// The subagent types a spawn directive such as `Agent(explore, plan)` names, lowercased; a bare `Agent` names none.
pub fn subagent_types(entry: &str) -> impl Iterator<Item = String> {
    AGENT_TASK_CLASSIFIER_RE
        .captures(entry)
        .and_then(|directive| directive.get(1))
        .into_iter()
        .flat_map(|types| types.as_str().split(','))
        .map(|name| name.trim().to_lowercase())
        .filter(|name| !name.is_empty())
}

pub fn listed_tools<'a>(
    list: ToolList<'_>,
    entries: &'a [String],
    toolset: &'a [ToolConfig],
) -> ListedTools<'a> {
    let (spawn_directives, names): (Vec<&str>, Vec<&str>) = entries
        .iter()
        .map(String::as_str)
        .partition(|entry| AGENT_TASK_CLASSIFIER_RE.is_match(entry));
    let in_toolset = |name: &str| toolset.iter().any(|tool| tool_id_eq(name, &tool.id));

    match list {
        ToolList::Denylist => ListedTools {
            tools: toolset
                .iter()
                .filter(|tool| !tool_id_matches(entries, &tool.id))
                .collect(),
            unmatched: names.into_iter().filter(|name| !in_toolset(name)).collect(),
            spawn_directives,
            unknown: Vec::new(),
        },
        ToolList::Allowlist { known_tool_ids } => {
            let present_kinds: HashSet<ToolKind> =
                toolset.iter().filter_map(|tool| tool.kind).collect();
            let mut allow_kinds = HashSet::new();
            let mut unmatched = Vec::new();
            let mut unknown = Vec::new();
            // `mcp__*` is always-on: never unknown, cannot trip the full-toolset fallback
            for name in names
                .into_iter()
                .filter(|name| !name.starts_with("mcp__") && !in_toolset(name))
            {
                match claude_tool_kind(name) {
                    // A recognized name must restrict, not restore the full toolset; an unavailable one (e.g. disabled web search) is a no-op.
                    Some(kind) if present_kinds.contains(&kind) => {
                        allow_kinds.insert(kind);
                    }
                    Some(_) => unmatched.push(name),
                    None if known_tool_ids.iter().any(|id| tool_id_eq(name, id)) => {
                        unmatched.push(name);
                    }
                    None => unknown.push(name),
                }
            }

            let spawns = !spawn_directives.is_empty();
            let tools = toolset
                .iter()
                .filter(|tool| {
                    // An unknown name (typo / a tool we don't model): keep the full toolset rather than cripple the agent
                    !unknown.is_empty()
                        || tool_id_matches(entries, &tool.id)
                        || tool.kind.is_some_and(|kind| allow_kinds.contains(&kind))
                        || (spawns && TASK_DEPS.contains(&short_tool_name(&tool.id)))
                        // MCP access is always-on under a `tools:` allowlist (compat parity): keep the `search_tool`/`use_tool` meta-tools
                        // To remove it, deny them via `disallowed_tools`; the denylist runs first and wins
                        || matches!(tool.kind, Some(ToolKind::SearchTool | ToolKind::UseTool))
                })
                .collect();
            ListedTools {
                tools,
                spawn_directives,
                unmatched,
                unknown,
            }
        }
    }
}

#[cfg(test)]
#[path = "tool_list_tests.rs"]
mod tests;
