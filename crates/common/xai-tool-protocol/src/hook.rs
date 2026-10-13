//! Hook events delivered from the harness to tools.

use serde::{Deserialize, Serialize};

/// Internally-tagged hook payload. New variants land alongside `Custom`,
/// which keeps unknown future kinds round-trippable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum HookEvent {
    /// Ends the running call named by `call_id` on the enclosing `HookFrame`.
    /// A `Cancel` with neither `call_id` nor `tool_id` ends every running call in the session.
    /// A `Cancel` with a `tool_id` but no `call_id` ends nothing.
    Cancel,
    Pause,
    Resume,
    /// Broadcast to every tool server bound to the session.
    SessionEnded,
    /// Forward-compatible escape hatch.
    Custom {
        kind: String,
        payload: serde_json::Value,
    },
}
