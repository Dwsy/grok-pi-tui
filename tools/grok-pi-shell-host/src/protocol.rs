use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Protocol version spoken by this host executable over stdin/stdout JSONL.
pub const PROTOCOL_VERSION: u32 = 1;
/// Trace summary schema version carried inside terminal `completed` events.
pub const TRACE_SCHEMA_VERSION: u32 = 1;
/// Hard ceiling on a single incoming JSONL request line (2 MiB).
pub const MAX_REQUEST_BYTES: usize = 2 * 1024 * 1024;
/// Hard ceiling on a single serialized `output` JSONL event payload (256 KiB).
pub const MAX_OUTPUT_EVENT_BYTES: usize = 256 * 1024;
/// Hard ceiling on serialized `ShellTraceSummary` JSON bytes (32 KiB).
pub const MAX_TRACE_JSON_BYTES: usize = 32 * 1024;
/// Maximum trace nodes kept in the summary before truncation.
pub const MAX_TRACE_NODES: usize = 96;
/// Maximum display characters kept per statement label in `ShellTraceNode`.
pub const MAX_NODE_DISPLAY_CHARS: usize = 160;

/// Single-invocation request sent as one JSON line on stdin.
#[derive(Debug, Clone, Deserialize)]
pub struct RunRequest {
    pub protocol_version: u32,
    pub command: String,
    pub cwd: String,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub trace: bool,
}

impl RunRequest {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.protocol_version != PROTOCOL_VERSION {
            return Err(ProtocolError::new(
                "unsupported_protocol_version",
                format!(
                    "Unsupported protocol_version {} (expected {})",
                    self.protocol_version, PROTOCOL_VERSION
                ),
            ));
        }
        if self.command.trim().is_empty() {
            return Err(ProtocolError::new(
                "invalid_request",
                "command must not be empty",
            ));
        }
        let cwd_path = Path::new(&self.cwd);
        if self.cwd.trim().is_empty() || !cwd_path.is_absolute() {
            return Err(ProtocolError::new(
                "invalid_cwd",
                format!("cwd must be an non-empty absolute path, got {:?}", self.cwd),
            ));
        }
        if !cwd_path.is_dir() {
            return Err(ProtocolError::new(
                "invalid_cwd",
                format!("cwd does not exist or is not a directory: {}", self.cwd),
            ));
        }
        if let Some(timeout_ms) = self.timeout_ms
            && timeout_ms == 0
        {
            return Err(ProtocolError::new(
                "invalid_timeout",
                "timeout_ms must be greater than 0 when specified",
            ));
        }
        Ok(())
    }
}

/// Handshake payload emitted for `--version` / `--handshake`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HandshakeInfo {
    pub name: String,
    pub version: String,
    pub protocol_version: u32,
    pub trace_version: u32,
    pub engine: String,
    pub omp_rev: String,
}

impl HandshakeInfo {
    pub fn current() -> Self {
        Self {
            name: env!("CARGO_PKG_NAME").to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            protocol_version: PROTOCOL_VERSION,
            trace_version: TRACE_SCHEMA_VERSION,
            engine: "omp-shell".to_string(),
            omp_rev: "57771ae562b9345e0579bceb2d68d585e3e6927e".to_string(),
        }
    }
}

/// Structured host/protocol error emitted as a terminal JSONL event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProtocolError {
    pub code: String,
    pub message: String,
}

impl ProtocolError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

/// JSONL response stream emitted on stdout.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostEvent {
    Output {
        text: String,
    },
    Completed {
        exit_code: Option<i32>,
        cancelled: bool,
        timed_out: bool,
        working_dir: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        trace: Option<ShellTraceSummary>,
    },
    Error {
        code: String,
        message: String,
    },
}

/// Bounded, redacted per-command execution trace projected from OMP `RunTrace`.
///
/// Explicitly excludes environment assignment values, expanded argv values,
/// PIDs, and unbounded output journals.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShellTraceSummary {
    pub version: u32,
    pub total_duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub truncated: bool,
    pub nodes: Vec<ShellTraceNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShellTraceNode {
    pub id: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<u32>,
    pub depth: u8,
    pub kind: String,
    pub command: String,
    pub state: ShellTraceNodeState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "is_one_u32")]
    pub runs: u32,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub background: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ShellTraceNodeState {
    Pending,
    Running,
    Ok,
    Fail,
    Skipped,
}

const fn is_one_u32(v: &u32) -> bool {
    *v == 1
}
