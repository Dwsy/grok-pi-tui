//! Tool types and post-execution reminders.
//!
//! The tool runtime contract (`Tool` trait) lives in `xai_tool_runtime`.
//! Tool metadata (kind, namespace, fingerprinting, etc.) lives in
//! `crate::types::tool_metadata::ToolMetadata`.
//!
//! This module provides:
//! - `ToolNamespace`, `ToolKind` — classification enums
//! - `Reminder` — post-execution system reminders (per-tool + cross-cutting)
use crate::types::output::ToolOutput;
use crate::types::requirements::{Expr, ToolRequirement};
use crate::types::resources::SharedResources;
/// The toolset a tool belongs to. Serializes to snake_case (`grok_build`, `mcp`, …) for the canonical tool `_meta` wire contract. PascalCase
/// aliases are accepted on deserialize so legacy persisted/manifest values still parse. The `Display` impl remains PascalCase for existing
/// qualified id strings (e.g. `"GrokBuild:read_file"`); only the serde form goes on the wire.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    derive_more::Display,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
    strum::EnumIter,
)]
#[serde(rename_all = "snake_case")]
pub enum ToolNamespace {
    #[serde(alias = "GrokBuild")]
    GrokBuild,
    #[serde(alias = "GrokBuildConcise")]
    GrokBuildConcise,
    #[serde(alias = "GrokBuildHashline")]
    GrokBuildHashline,
    #[serde(alias = "Codex")]
    Codex,
    #[serde(rename = "opencode", alias = "OpenCode", alias = "open_code")]
    OpenCode,
    #[serde(rename = "mcp", alias = "MCP")]
    MCP,
}
// Canonical classification is shared with hooks; preserve stock path identity.
pub use xai_tool_types::ToolKind;

/// System reminders that fire after a tool call completes. **Per-tool reminders** on tool structs
/// (e.g., `ReadFileTool`: empty file, offset past end). **Cross-cutting reminders** on standalone
/// structs (e.g., `SkillDiscoveryReminder`) that react to any tool call.
#[async_trait::async_trait]
pub trait Reminder {
    /// Requirements for this reminder to be active.
    fn requires_expr(&self) -> Expr<ToolRequirement> {
        Expr::True
    }
    /// Collect reminders after a tool execution completes.
    async fn collect_reminders(
        &self,
        _resources: SharedResources,
        _tool_output: &ToolOutput,
    ) -> Vec<String> {
        vec![]
    }
}
