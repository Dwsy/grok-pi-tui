//! Stable tool classifications used by the stock registry, hook aliases, and
//! external clients. Metadata only; no tool execution implementation here.

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
