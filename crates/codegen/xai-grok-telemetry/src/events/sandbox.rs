//! Per-command sandbox events (`grok-workspaced`'s Codex-style shell sandbox). Content-free by
//! design: enum-valued fields only, never a path, host, command or stderr fragment.

use serde::Serialize;

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq, strum::IntoStaticStr)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum SandboxModeTag {
    Off,
    Observe,
    Enforce,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq, strum::IntoStaticStr)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum SandboxCommandOutcome {
    Ran,
    Violation,
    WrapFailed,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq, strum::IntoStaticStr)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum SandboxBlockedKind {
    FsWrite,
    FsRead,
    Net,
    Capability,
    Unknown,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq, strum::IntoStaticStr)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum SandboxSettlement {
    Replay,
    Resume,
    Denied,
    Observed,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq, strum::IntoStaticStr)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum SandboxGrantSubjectKind {
    FsWriteRoot,
    FsRead,
    NetHost,
    /// The curated build-cache family for a workspace.
    BuildCaches,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq, strum::IntoStaticStr)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum SandboxGrantScopeKind {
    Call,
    Session,
    Workspace,
    Global,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq, strum::IntoStaticStr)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum SandboxExpiryKind {
    Ttl,
    At,
    Never,
}

/// One shell command finished under the per-command sandbox.
#[derive(Serialize, Clone, Copy, Debug)]
pub struct SandboxCommandEnded {
    pub mode: SandboxModeTag,
    /// The backend name as `xai_grok_sandbox::command::BackendName` spells it; `None` when the
    /// host has none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend: Option<&'static str>,
    pub outcome: SandboxCommandOutcome,
}

/// A decoded violation was settled: replayed under a grant, resumed at the proxy, kept blocked,
/// or only observed.
#[derive(Serialize, Clone, Copy, Debug)]
pub struct SandboxViolationSettled {
    pub mode: SandboxModeTag,
    pub kind: SandboxBlockedKind,
    /// Why the card could offer nothing, as `xai_grok_sandbox::command::InformationalReason`
    /// spells it (`protected_target`, `profile_deny`, `capability`, `unproxied_network`,
    /// `policy_denylist`, `unattributed`); `None` for a grantable violation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub informational: Option<&'static str>,
    pub settlement: SandboxSettlement,
}

/// The user allowed a violation and a grant row was recorded.
#[derive(Serialize, Clone, Copy, Debug)]
pub struct SandboxGrantRecorded {
    pub subject: SandboxGrantSubjectKind,
    pub scope: SandboxGrantScopeKind,
    pub expiry: SandboxExpiryKind,
}

/// A grant row was revoked from Settings or the control socket.
#[derive(Serialize, Clone, Copy, Debug)]
pub struct SandboxGrantRevoked {
    pub scope: SandboxGrantScopeKind,
}
