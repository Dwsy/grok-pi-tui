//! Wire types for the `xai-grok-workspace` API.
//!
//! This crate is intentionally pure-data and depends on nothing more than `base64`, `serde`, `serde_json`, `thiserror`, and `chrono`.
//! There is no tokio, no async-trait, no I/O.
//! This makes it cheap to depend on from anywhere, including the eventual WASM browser SDK.
//!
//! # Module overview
//!
//! - [`identity`]: session/tool/hunk identifiers.
//! - [`metadata`]: typed-string metadata map plus standard metadata key constants (`META_*`).
//! - [`request`]: the `RequestMessage<T>` envelope shared by all RPC requests.
//! - [`error`]: `WorkspaceError` and its serializable `IoKind`.
//! - [`requests`]: the request enums (`WorkspaceRequest`, `ToolRequest`, `WorkspaceOpsRequest`, `SessionLifecycleRequest`).
//! - [`chunks`]: the streaming response chunks (`ToolChunk`, `OpsChunk`, `SessionChunk`) plus the `ChunkKind` discriminator.
//! - [`events`]: pub/sub events ([`WorkspaceEvent`]) plus the [`EventLag`] backpressure signal.
//!   The EventBus only carries external state the workspace observed; there is no `SessionEvent` enum.
//! - [`rpc`]: canonical wire types for the hub-proxied `workspace.*` RPC dispatch (trait, envelope, hub tool ids, per-method types).
//! - [`types`]: supporting structs/enums referenced from requests, chunks, and events.
//!   Many of these are minimal placeholders; the final shapes will land when the corresponding subsystems are extracted into the new workspace crate.
//!
//! # Wire format
//!
//! Every enum that appears on the wire is **adjacently tagged** with `#[serde(tag = "type", content = "data")]`.
//! The JSON wire shape is:
//!
//! ```json
//! {"type": "<variant_name>", "data": <payload>}
//! ```
//!
//! Adjacent (rather than internal) tagging is required: internal tagging fails for newtype variants wrapping non-struct payloads.
//! E.g. `OpsChunk::GitMetadata(Option<...>)`, `SessionChunk::SessionId(SessionId)` (a string), `WorkspaceOpsRequest::ResolveFileRefs(Vec<String>)`.
//! Adjacent tagging works uniformly across all payload shapes (struct, newtype, unit) and gives the JSON wire format an explicit discriminator.
//! Each variant maps directly to a protobuf `oneof` regardless of the serde tagging form; the choice here only affects the JSON representation.
//!
//! Wire-format struct fields use **snake_case** to match gRPC field conventions (proto field names are snake_case).
//!
//! # Wire integer types
//!
//! Every `usize` field is replaced here with `u64`, or `u32` for known-bounded sizes like `MemorySearch.limit`.
//! That covers `BeginPrompt.idx`, `EndPrompt.idx`, `Rewind.target`, `MemorySearch.limit`, and `CodebaseIndexUpdated.files_indexed`.
//!
//! Rationale: `usize` is host-dependent (32 vs 64 bit) and serializes inconsistently across producers.
//! A 32-bit publisher could silently truncate a value that a 64-bit subscriber reconstructs as something different.
//! `u64` codegens cleanly to protobuf `uint64` and pins the wire width regardless of host.
//!
//! # TODO: proto generation
//!
//! A planned `build.rs` will walk the request, chunk, and event enums via reflection and emit a `.proto`.
//! That codegen step is **not** implemented yet; it will land alongside the `xai-grok-workspace-grpc` crate.
//! The Rust types defined here are the source of truth.

#![deny(clippy::indexing_slicing)]

pub mod binding;
pub mod chunks;
pub mod error;
pub mod events;
pub mod identity;
pub mod metadata;
pub mod request;
pub mod requests;
pub mod rpc;
pub mod types;

/// StartSession env whose value is the dest-less `grok-files` remount
/// command (knobs included). The bind hook applies dest from `session_root`
/// (conversation / parent id) per bind. Absent / empty keeps the no-op hook.
pub const ARTIFACTS_BIND_REMOUNT_ENV: &str = "GROK_ARTIFACTS_BIND_REMOUNT";

/// `grok-files mount` flag naming the token file the worker re-reads per
/// request.
pub const GROK_FILES_JWT_FILE_FLAG: &str = "--jwt-file";

/// Path of the Files token scoped to one conversation. The
/// `/workspace/<conversation_id>` remount reads it, so each mount
/// authenticates as its own conversation. Every caller derives it from the
/// same id (the remount dest's last path segment).
pub fn grok_files_conversation_jwt_path(conversation_id: &str) -> String {
    format!("/etc/secrets/terminal.{conversation_id}.jwt")
}

/// Append the conversation token file to a remount command. Kept as one
/// function so `mount_at` and the bind hook emit the same argv.
pub fn with_grok_files_jwt_file(mount_command: &str, jwt_file: &str) -> String {
    format!("{mount_command} {GROK_FILES_JWT_FILE_FLAG} {jwt_file}")
}

/// Flags whose next token is a value, not a dest path. `command_at` and
/// the bind hook must share this list so remount dest cannot
/// drift (boolean long flags must not be treated as value-taking).
pub fn grok_files_opt_takes_value(flag: &str) -> bool {
    flag == GROK_FILES_JWT_FILE_FLAG
        || matches!(
            flag,
            "--content-cache"
                | "--content-cache-max"
                | "--ttl"
                | "--deny-delete"
                | "--occ"
                | "--occ-conflict-policy"
                | "--revalidate"
                | "--revalidate-interval"
                | "--entry-ttl"
                | "--revalidate-exclude-prefix"
                | "--n-threads"
        )
}

/// Replace the dest positional (or insert after `/`) so a remount command
/// targets `dest` instead of any dest baked into `mount_command`.
pub fn rewrite_grok_files_mount_dest(mount_command: &str, dest: &str) -> String {
    let tokens: Vec<&str> = mount_command.split_whitespace().collect();
    if tokens.is_empty() {
        return format!("grok-files mount / {dest}");
    }

    let mut skip_next = false;
    let mut dest_idx = None;
    let mut source_idx = None;
    for (i, tok) in tokens.iter().enumerate() {
        // argv[0] is the binary; an absolute path there is not the mount dest.
        if i == 0 {
            continue;
        }
        if skip_next {
            skip_next = false;
            continue;
        }
        if let Some(rest) = tok.strip_prefix("--") {
            if !rest.contains('=') && grok_files_opt_takes_value(tok) {
                skip_next = true;
            }
            continue;
        }
        if tok.starts_with('-') && *tok != "-" {
            continue;
        }
        if *tok == "/" {
            source_idx = Some(i);
        } else if tok.starts_with('/') {
            dest_idx = Some(i);
        }
    }

    let mut out = tokens;
    if let Some(slot) = dest_idx.and_then(|i| out.get_mut(i)) {
        *slot = dest;
        return out.join(" ");
    }
    if let Some(i) = source_idx {
        out.insert(i + 1, dest);
        return out.join(" ");
    }
    format!("{mount_command} {dest}")
}

/// MCP tool name delimiter: server names are qualified as `"server__tool"`.
/// Lives here so the permission-validation and MCP transport layers can share it without dragging the full workspace or rmcp into each other.
/// Re-exported by `xai_grok_workspace::permission` for callers that historically imported it from there.
pub const MCP_TOOL_NAME_DELIMITER: &str = "__";

pub use crate::chunks::{ChunkKind, OpsChunk, SessionChunk, ToolChunk, ToolResponse};
pub use crate::error::{IoKind, WorkspaceError};
pub use crate::events::{EventLag, WorkspaceEvent, WorkspaceTopic, WorkspaceTopicSet};
pub use crate::identity::{HunkId, SessionId, ToolCallId};
pub use crate::metadata::{
    META_CLIENT_ID, META_GRPC_TIMEOUT, META_PROMPT_INDEX, META_SESSION_ID, META_TRACEPARENT,
    META_TRACESTATE, Metadata, STANDARD_META_KEYS,
};
pub use crate::request::RequestMessage;
pub use crate::requests::{
    SessionLifecycleRequest, ToolCallArgs, ToolRequest, WorkspaceOpsRequest, WorkspaceRequest,
};
pub use crate::types::{
    AgentSessionConfig, AgentSessionInfo, CapabilityMode, ContentMatch, FileReference, FsEventKind,
    FuzzyMatch, FuzzySearchArgs, GitBranchInfo, GitDiff, GitDiffArgs, GitMetadata, GitStatus,
    GitStatusOpts, HookInfo, Hunk, HunkAction, IsolationMode, LspServerStatus, McpServerStatus,
    MemoryChunk, PermissionDecision, PermissionPolicy, PermissionRequest, PlanModeDecision,
    PlanModeTransition, PluginInfo, ProjectConfig, ResolvedFile, RewindPoint, RewindResult,
    RipgrepArgs, RipgrepStats, SkillInfo, ToolCallResult, ToolDef, ToolOutputChunk, ToolProgress,
    ToolServerConfig, UserAnswer, UserQuestion, UserQuestionOption, VcsKind,
};

#[cfg(test)]
mod grok_files_mount_dest_tests {
    use super::{
        grok_files_conversation_jwt_path, rewrite_grok_files_mount_dest, with_grok_files_jwt_file,
    };

    #[test]
    fn rewrites_existing_dest() {
        assert_eq!(
            rewrite_grok_files_mount_dest("grok-files mount / /data", "/workspace/conv-a"),
            "grok-files mount / /workspace/conv-a"
        );
    }

    #[test]
    fn boolean_long_flags_are_not_value_taking() {
        assert_eq!(
            rewrite_grok_files_mount_dest(
                "custom-mnt mount --foreground / /data",
                "/workspace/conv-abc"
            ),
            "custom-mnt mount --foreground / /workspace/conv-abc"
        );
    }

    #[test]
    fn inserts_dest_after_source_when_missing() {
        assert_eq!(
            rewrite_grok_files_mount_dest("grok-files mount / --n-threads 4", "/workspace/conv-a"),
            "grok-files mount / /workspace/conv-a --n-threads 4"
        );
    }

    #[test]
    fn absolute_binary_is_not_treated_as_dest() {
        assert_eq!(
            rewrite_grok_files_mount_dest("/usr/bin/grok-files mount /", "/workspace/conv-a"),
            "/usr/bin/grok-files mount / /workspace/conv-a"
        );
    }

    #[test]
    fn jwt_file_value_is_not_treated_as_dest() {
        let installed = with_grok_files_jwt_file(
            "grok-files mount / /data",
            &grok_files_conversation_jwt_path("conv-a"),
        );
        assert_eq!(
            "grok-files mount / /workspace/conv-b --jwt-file /etc/secrets/terminal.conv-a.jwt",
            rewrite_grok_files_mount_dest(&installed, "/workspace/conv-b")
        );
    }
}
