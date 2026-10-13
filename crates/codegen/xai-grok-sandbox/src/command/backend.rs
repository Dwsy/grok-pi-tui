//! The backend seam: a backend renders a [`SandboxPolicy`] into a wrapper argv. `wrap` never
//! spawns and never falls back to an unsandboxed launch; an `Err` means "do not run". The one
//! backend is macOS Seatbelt ([`seatbelt`]); other hosts have none and `Enforce` refuses.

use std::ffi::OsString;
use std::path::PathBuf;

use serde::Serialize;

use crate::command::env;
use crate::command::mode::SandboxMode;
use crate::command::policy::{PolicyError, SandboxPolicy};
use crate::command::protected::{self, Alias, HardLinked};

pub use crate::command::detect::{BackendChoice, HostProbe, detect_backend};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, strum::IntoStaticStr)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum BackendName {
    Seatbelt,
}

/// What a backend can enforce relative to the full model. `reduced` is what the card and Settings
/// badge as "(reduced sandbox)".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct BackendCapabilities {
    pub reduced: bool,
}

/// What kind of spawn a [`CallId`] names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CallKind {
    /// A tool call the hub dispatched: its result reaches `finish`, a denial is decoded and
    /// carded, a grant replays it.
    Tool,
    /// One of the terminal backend's own spawns — the login shell, the static snapshot, the
    /// rc-file capture — that sources the user's files and never produces a tool result. The
    /// launch hook wraps it like any other command; no record is kept and no decoder runs.
    ShellInit,
}

/// The id of one shell spawn as the sandbox tracks it: the kind is a field,
/// not a prefix baked into a string. `Display` is the one wire spelling (`<id>` for a tool call,
/// `shell-init:<label>` for the backend's own spawns), which is what [`CommandTag`] embeds. A tool
/// id that itself begins with `shell-init:` or `tool:` is spelled `tool:<id>`, so no tool call
/// parses back as a shell-init spawn.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CallId {
    kind: CallKind,
    id: String,
}

impl CallId {
    const SHELL_INIT_PREFIX: &str = "shell-init:";
    const TOOL_PREFIX: &str = "tool:";

    pub fn tool(id: impl Into<String>) -> CallId {
        CallId {
            kind: CallKind::Tool,
            id: id.into(),
        }
    }

    pub fn shell_init(label: impl Into<String>) -> CallId {
        CallId {
            kind: CallKind::ShellInit,
            id: label.into(),
        }
    }

    /// The inverse of `Display`.
    pub fn parse(spelling: &str) -> CallId {
        if let Some(id) = spelling.strip_prefix(Self::TOOL_PREFIX) {
            return CallId::tool(id);
        }
        match spelling.strip_prefix(Self::SHELL_INIT_PREFIX) {
            Some(label) => CallId::shell_init(label),
            None => CallId::tool(spelling),
        }
    }

    pub fn kind(&self) -> CallKind {
        self.kind
    }

    /// The bare id, without the kind.
    pub fn as_str(&self) -> &str {
        &self.id
    }
}

impl std::fmt::Display for CallId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.kind {
            CallKind::Tool
                if self.id.starts_with(Self::SHELL_INIT_PREFIX)
                    || self.id.starts_with(Self::TOOL_PREFIX) =>
            {
                write!(f, "{}{}", Self::TOOL_PREFIX, self.id)
            }
            CallKind::Tool => f.write_str(&self.id),
            CallKind::ShellInit => write!(f, "{}{}", Self::SHELL_INIT_PREFIX, self.id),
        }
    }
}

/// Per-call attribution: `grok-<call_id>`. Embedded as the SBPL `(with message …)` tag and the
/// proxy's per-call identity, so denial lines and blocked requests map back to one tool call.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CommandTag(String);

impl CommandTag {
    const PREFIX: &str = "grok-";

    pub fn for_call(call: &CallId) -> CommandTag {
        CommandTag(format!("{}{call}", Self::PREFIX))
    }

    /// The call `for_call` embedded; `None` for a tag minted some other way (a plain shell's
    /// session-wide proxy credential).
    pub fn call_id(&self) -> Option<CallId> {
        self.0.strip_prefix(Self::PREFIX).map(CallId::parse)
    }
}

impl std::fmt::Display for CommandTag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for CommandTag {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// The command as the tool built it, before wrapping.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OriginalArgv {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub cwd: PathBuf,
}

/// What `wrap` produced, kept per call so the decoder and the test suite can inspect it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RenderedPolicy {
    Sbpl {
        profile: String,
        params: Vec<(String, String)>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WrapReceipt {
    pub backend: BackendName,
    pub rendered: RenderedPolicy,
}

#[derive(Debug, thiserror::Error)]
pub enum SandboxCommandError {
    #[error("sandbox backend unavailable on this host: {reason}")]
    Unavailable { reason: String },
    #[error("policy cannot be rendered by {backend:?}: {reason}")]
    Unrenderable {
        backend: BackendName,
        reason: String,
    },
    #[error(transparent)]
    Policy(#[from] crate::command::policy::PolicyError),
}

/// Contract for a backend:
/// - `wrap` rewrites `cmd`'s program, args and environment so that `cmd.spawn()` runs `original`
///   inside the sandbox described by `policy`; it does not spawn, and it is deterministic for a
///   given `(policy, tag)` so tests assert the rendered argv.
/// - Mandatory denies (`policy.protected`, the control socket, and `policy.read.deny()` in every
///   read mode) are rendered after every allow.
/// - An error means the command must not run; no backend falls back to an unsandboxed launch.
/// - `wrap` is reached only through [`wrap_for_mode`], which has already filtered `cmd`'s
///   environment per `policy.env` and refused unread git metadata and hard-linked protected
///   files. A backend repeats none of those checks; one that replaces `cmd` carries over exactly
///   the environment `cmd` would have spawned with (inherited, with `cmd`'s explicit sets and
///   removals applied).
pub trait SandboxBackend: Send + Sync {
    fn name(&self) -> BackendName;
    fn capabilities(&self) -> BackendCapabilities;
    fn wrap(
        &self,
        cmd: &mut tokio::process::Command,
        original: &OriginalArgv,
        policy: &SandboxPolicy,
        tag: &CommandTag,
    ) -> Result<WrapReceipt, SandboxCommandError>;
}

/// The entry point every spawn site reaches through the launch hook.
///
/// - `Off`: `cmd` is untouched and `Ok(None)` is returned.
/// - `Observe`: the policy's `env.set` entries (the proxy pointers) are added and nothing is
///   removed; the command runs with its usual OS permissions and the proxy records what `Enforce`
///   would have asked about.
/// - `Enforce`: the environment is filtered per `policy.env` and `backend.wrap` rewrites the
///   program and arguments. Without a backend the command must not run; a git file the floor
///   could not read, or a protected file a command could write through another hard link,
///   refuses it before `cmd` is touched, whatever the backend. This is the only place either
///   the environment filter or those refusals run.
///
/// # Errors
/// [`SandboxCommandError::Unavailable`] under `Enforce` with no backend;
/// [`PolicyError::GitMetadataUnread`] when a git file the floor derives its entries from could
/// not be read; [`PolicyError::HardLinkedProtected`] when a protected file has a writable second
/// name no path rule can tell apart, [`PolicyError::HardLinkUnverified`] or
/// [`PolicyError::ProtectedTreeUnchecked`] when one cannot be ruled out; whatever `backend.wrap`
/// returns otherwise.
pub fn wrap_for_mode(
    mode: SandboxMode,
    backend: Option<&dyn SandboxBackend>,
    cmd: &mut tokio::process::Command,
    original: &OriginalArgv,
    policy: &SandboxPolicy,
    tag: &CommandTag,
) -> Result<Option<WrapReceipt>, SandboxCommandError> {
    match mode {
        SandboxMode::Off => Ok(None),
        SandboxMode::Observe => {
            env::apply_env_set_only(cmd, &policy.env);
            Ok(None)
        }
        SandboxMode::Enforce => {
            let backend = backend.ok_or_else(|| SandboxCommandError::Unavailable {
                reason: "no per-command sandbox backend on this host".to_owned(),
            })?;
            if let Some(unread) = policy.unread_git_metadata.first() {
                return Err(PolicyError::GitMetadataUnread {
                    unread: unread.clone(),
                }
                .into());
            }
            if let Some(linked) =
                protected::hard_linked_protected_file(&policy.protected, &policy.write_roots)
            {
                return Err(hard_link_refusal(linked).into());
            }
            env::apply_env_policy(cmd, &policy.env);
            backend.wrap(cmd, original, policy, tag).map(Some)
        }
    }
}

/// The refusal for what [`protected::hard_linked_protected_file`] reports.
fn hard_link_refusal(linked: HardLinked) -> PolicyError {
    let (path, nlink) = (linked.path, linked.nlink);
    match linked.alias {
        Alias::Writable(alias) => PolicyError::HardLinkedProtected { path, nlink, alias },
        Alias::Unsearched { root } => PolicyError::HardLinkUnverified { path, root },
        Alias::Unlisted { tree } => PolicyError::ProtectedTreeUnchecked { tree },
    }
}

#[cfg(target_os = "macos")]
pub mod seatbelt;
#[cfg(target_os = "macos")]
pub(crate) mod seatbelt_profile;

#[cfg(test)]
#[path = "backend_tests.rs"]
mod tests;
