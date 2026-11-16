//! Stable tool classifications used by the stock registry, hook aliases, and
//! external clients. Metadata only; no tool execution implementation here.

use std::borrow::Cow;

/// Categorizes tool behavior. Unknown newer values deserialize to `Other`.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    serde::Serialize,
    serde::Deserialize,
    strum::EnumCount,
    strum::EnumIter,
    strum::AsRefStr,
    strum::IntoStaticStr,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum ToolKind {
    Read,
    Edit,
    Delete,
    ListDir,
    Write,
    Move,
    Search,
    Lsp,
    Execute,
    Plan,
    WebSearch,
    WebFetch,
    BackgroundTaskAction,
    WaitTasksAction,
    KillTaskAction,
    List,
    Skill,
    MemorySearch,
    MemoryGet,
    Task,
    ActiveAgentMessage,
    EnterPlan,
    ExitPlan,
    AskUser,
    ImageGen,
    VideoGen,
    ImageToVideo,
    ReferenceToVideo,
    DeployApp,
    InitOrUpdateApp,
    SearchTool,
    UseTool,
    Monitor,
    GoalUpdate,
    Workflow,
    Feedback,
    #[serde(other)]
    Other,
}

impl ToolKind {
    pub const VARIANT_COUNT: usize = <Self as strum::EnumCount>::COUNT;

    pub fn as_key(self) -> &'static str {
        self.into()
    }
}

impl ToolKind {
    /// Unified, harness-independent display label for this semantic kind. A pure function of the kind, so equivalent tools
    /// across toolsets share it (`read_file` and `Read` → `Read`; `run_terminal_cmd` and `Shell` → `Run Command`). Display
    /// only; the model's tool name is `name` in `x.ai/tool`. Exhaustive, so a new `ToolKind` must add a label to compile.
    pub fn presentation_name(self) -> &'static str {
        match self {
            ToolKind::Read => "Read",
            ToolKind::Edit => "Edit",
            ToolKind::Delete => "Delete",
            ToolKind::Write => "Write",
            ToolKind::Move => "Move",
            ToolKind::ListDir => "List Files",
            ToolKind::List => "List Files",
            ToolKind::Search => "Search",
            ToolKind::Lsp => "Code Intelligence",
            ToolKind::Execute => "Run Command",
            ToolKind::Plan => "Plan",
            ToolKind::WebSearch => "Web Search",
            ToolKind::WebFetch => "Web Fetch",
            ToolKind::BackgroundTaskAction => "Background Task",
            ToolKind::WaitTasksAction => "Wait for Tasks",
            ToolKind::KillTaskAction => "Kill Task",
            ToolKind::Skill => "Skill",
            ToolKind::MemorySearch => "Memory Search",
            ToolKind::MemoryGet => "Memory Read",
            ToolKind::Task => "Subagent",
            ToolKind::ActiveAgentMessage => "Send Subagent Message",
            ToolKind::EnterPlan => "Enter Plan Mode",
            ToolKind::ExitPlan => "Exit Plan Mode",
            ToolKind::AskUser => "Ask User",
            ToolKind::ImageGen => "Generate Image",
            ToolKind::VideoGen => "Generate Video",
            ToolKind::ImageToVideo => "Generate Video",
            ToolKind::ReferenceToVideo => "Generate Video",
            ToolKind::DeployApp => "Deploy App",
            ToolKind::InitOrUpdateApp => "Init or Update App",
            ToolKind::SearchTool => "Search Tools",
            ToolKind::UseTool => "Use Tool",
            ToolKind::Monitor => "Monitor",
            ToolKind::GoalUpdate => "Update Goal",
            ToolKind::Workflow => "Workflow",
            ToolKind::Feedback => "Feedback",
            ToolKind::Other => "Tool",
        }
    }
    /// Whether this kind only reads (no workspace or external mutation) by default. The kind-level
    /// default for `ToolMetadata::is_read_only`, which individual tools may override. Exhaustive
    /// (no `_`) so a new kind must classify itself rather than silently defaulting to "mutating".
    pub fn is_read_only(self) -> bool {
        match self {
            ToolKind::Read
            | ToolKind::Search
            | ToolKind::Lsp
            | ToolKind::ListDir
            | ToolKind::List
            | ToolKind::MemorySearch
            | ToolKind::MemoryGet
            | ToolKind::WebSearch
            | ToolKind::WebFetch
            | ToolKind::EnterPlan
            | ToolKind::ExitPlan
            | ToolKind::AskUser => true,
            ToolKind::Edit
            | ToolKind::Delete
            | ToolKind::Write
            | ToolKind::Move
            | ToolKind::Execute
            | ToolKind::Plan
            | ToolKind::BackgroundTaskAction
            | ToolKind::WaitTasksAction
            | ToolKind::KillTaskAction
            | ToolKind::Skill
            | ToolKind::Task
            | ToolKind::ActiveAgentMessage
            | ToolKind::ImageGen
            | ToolKind::VideoGen
            | ToolKind::ImageToVideo
            | ToolKind::ReferenceToVideo
            | ToolKind::DeployApp
            | ToolKind::InitOrUpdateApp
            | ToolKind::SearchTool
            | ToolKind::UseTool
            | ToolKind::Monitor
            | ToolKind::GoalUpdate
            | ToolKind::Workflow
            | ToolKind::Feedback
            | ToolKind::Other => false,
        }
    }
}
impl schemars::JsonSchema for ToolKind {
    fn schema_name() -> Cow<'static, str> {
        "ToolKind".into()
    }
    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        use strum::IntoEnumIterator;
        let known = Self::iter()
            .filter_map(|k| serde_json::to_value(k).ok())
            .filter_map(|v| v.as_str().map(|s| format!("`{s}`")))
            .collect::<Vec<_>>()
            .join(", ");
        schemars::json_schema!({
            "type": "string",
            "description": format!(
                "Categorizes what a tool does at a high level. Open set — consumers must \
                 tolerate unknown values (Rust deserializes them to `other` via \
                 `#[serde(other)]`). Known values: {known}."
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_kind_wire_names_remain_stable() {
        assert_eq!(serde_json::to_string(&ToolKind::Read).unwrap(), "\"read\"");
        assert_eq!(ToolKind::WebSearch.as_key(), "web_search");
        assert_eq!(
            serde_json::from_str::<ToolKind>("\"future_tool\"").unwrap(),
            ToolKind::Other
        );
    }
}
