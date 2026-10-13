//! What the agent learns from a denied command. It reads one channel — the exit status plus the
//! stderr keyword battery in [`coarse`] — so every target is a guess the card asks the user to
//! confirm. A connection the egress proxy held is the one exact fact and is
//! constructed by the proxy's decider, never decoded here. The proposal a card offers is
//! [`propose`]'s.

use std::borrow::Cow;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;

use serde::{Deserialize, Serialize};

use crate::command::grants::GrantSubject;
use crate::command::policy::SandboxPolicy;

pub mod attribute;
pub mod coarse;
pub mod propose;

pub use attribute::OwnTargets;
pub use propose::{ProposalBounds, bases_from_env, is_too_broad};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Blocked {
    FsWrite {
        path: PathBuf,
    },
    FsRead {
        path: PathBuf,
    },
    /// `host` is `None` when stderr showed a blocked connection or lookup but named no host;
    /// `port` is `None` when the message carried none (a resolver failure). The card then offers
    /// only "all network".
    Net {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        port: Option<u16>,
    },
    /// Seatbelt refused a privileged operation (a setuid exec, a debugger attach); never grantable.
    Capability {
        what: Capability,
    },
    /// Something was denied and the target is unknown (a marker line with no path). Never
    /// produces a card or model text — the tool result goes back unchanged and the event is
    /// recorded as `unknown`.
    Unknown {
        stderr_snippet: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, strum::IntoStaticStr)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum Capability {
    Ptrace,
    Mount,
    SetUid,
}

/// How an approval continues the command.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Replay {
    /// Filesystem: the whole command runs again under the widened policy.
    Rerun,
    /// Network: the egress proxy is holding the connection under `hold_id` and posts the
    /// violation mid-command; the answer goes back to the proxy's decider as a [`HoldAnswer`].
    /// Only the decider constructs this; a post-run decode never does.
    Resume { hold_id: String },
}

/// The session owner's answer to a held connection ([`Replay::Resume`]), as the gate hands it to
/// the proxy's decider.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, strum::IntoStaticStr)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum HoldAnswer {
    Allow,
    Deny,
}

/// Whether the card may offer to allow the violation, decided by whoever constructed it — the
/// decoder for a stopped command, the proxy's decider for a held connection — and carried on
/// the wire as is: the renderer derives nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Disposition {
    /// The card offers the proposal; an allow records a grant.
    Grantable,
    /// Nothing can be offered: the denial is final, the card has one `Got it`.
    Informational { reason: InformationalReason },
}

/// Why an informational violation cannot be granted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, strum::IntoStaticStr)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum InformationalReason {
    /// The target is in the protected floor.
    ProtectedTarget,
    /// A `deny` entry of the sandbox profile covers the target: no grant drops one, so the card
    /// only informs (the user edits `sandbox.toml` to allow it).
    ProfileDeny,
    /// A privileged operation (`Blocked::Capability`).
    Capability,
    /// A connection the egress proxy never saw: the tool did not use it, so no host grant can
    /// take effect. Every network denial the post-run decoder reads is this (the proxy holds
    /// and asks about the ones it sees mid-command).
    UnproxiedNetwork,
    /// The proxy refused the host because the organisation's policy denies it.
    PolicyDenylist,
    /// The denial names a path the command did not reach for — not its cwd, not a path its argv
    /// or environment named, not the served root ([`OwnTargets`]) — so no grant is proposed for
    /// it.
    Unattributed,
    /// The card cannot carry the request. The desktop renders it locally for a card it cannot
    /// decode and answers `{ "outcome": "reject", "reason": "decode_failed" }`; the daemon sends it
    /// for a grantable violation the wire cannot express (a path that is not UTF-8).
    DecodeFailed,
}

impl Disposition {
    pub const fn informational(reason: InformationalReason) -> Disposition {
        Disposition::Informational { reason }
    }

    pub fn is_grantable(self) -> bool {
        matches!(self, Disposition::Grantable)
    }

    pub fn reason(self) -> Option<InformationalReason> {
        match self {
            Disposition::Grantable => None,
            Disposition::Informational { reason } => Some(reason),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Violation {
    pub blocked: Blocked,
    /// The grant the card proposes: the nearest existing directory for a file path, the host for
    /// a connection. `None` when the disposition is informational.
    pub proposed: Option<GrantSubject>,
    pub disposition: Disposition,
    /// Output reached the model before the failure, so the card says the re-run starts over.
    /// Supplied by the caller when its terminal knows; omitted otherwise —
    /// the card then says "runs again from the start" whenever `replay` is a rerun.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partial_output: Option<bool>,
    pub replay: Replay,
    pub exit_code: Option<i32>,
    /// UTF-8-safe, at most [`STDERR_SNIPPET_MAX_BYTES`].
    pub stderr_snippet: String,
}

pub const STDERR_SNIPPET_MAX_BYTES: usize = 512;

/// How a command ended, as the decoder reads it: the exit code, or the signal that killed it.
/// Hosts that only see a tool's reported exit code (the hub result path) construct it directly;
/// a spawn site converts its [`ExitStatus`] with [`From`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandExit {
    pub code: Option<i32>,
    /// The terminating signal number on Unix; `None` when the command exited normally.
    pub signal: Option<i32>,
}

impl CommandExit {
    pub const fn code(code: i32) -> CommandExit {
        CommandExit {
            code: Some(code),
            signal: None,
        }
    }

    pub const fn signal(signal: i32) -> CommandExit {
        CommandExit {
            code: None,
            signal: Some(signal),
        }
    }

    pub fn success(self) -> bool {
        self.code == Some(0)
    }
}

impl From<ExitStatus> for CommandExit {
    fn from(status: ExitStatus) -> CommandExit {
        #[cfg(unix)]
        let signal = {
            use std::os::unix::process::ExitStatusExt;
            status.signal()
        };
        #[cfg(not(unix))]
        let signal = None;
        CommandExit {
            code: status.code(),
            signal,
        }
    }
}

impl From<&ExitStatus> for CommandExit {
    fn from(status: &ExitStatus) -> CommandExit {
        CommandExit::from(*status)
    }
}

impl Violation {
    /// A violation the card can offer to allow.
    pub fn is_grantable(&self) -> bool {
        self.disposition.is_grantable() && self.proposed.is_some()
    }

    /// The target is in the protected floor.
    pub fn protected_target(&self) -> bool {
        self.disposition.reason() == Some(InformationalReason::ProtectedTarget)
    }

    /// Whether the gate shows this violation to the user at all. [`Blocked::Unknown`] is the one
    /// shape that never does: the tool result is returned unchanged and the event is only
    /// recorded.
    pub fn produces_card(&self) -> bool {
        !matches!(self.blocked, Blocked::Unknown { .. })
    }

    /// The model-visible text when the user keeps the denial (asked-and-
    /// denied): [`denied_text`](Self::denied_text) with the user's answer.
    pub fn kept_blocked_text(&self) -> String {
        self.denied_text("User kept it blocked.")
    }

    /// The model-visible statement of the denial: what was refused, why the policy refused it,
    /// how the gate settled it (`settled`, one sentence), the exit code and
    /// the stderr tail. One function so every host says the same.
    ///
    /// A held connection ([`Replay::Resume`]) is refused while the command still runs, so it has
    /// no exit or stderr to report: the proxy answered it 403 and the command went on.
    pub fn denied_text(&self, settled: &str) -> String {
        let what = blocked_phrase(&self.blocked);
        let why = match (&self.blocked, self.disposition.reason()) {
            (Blocked::Unknown { .. }, _) => "target unknown",
            (_, Some(InformationalReason::ProtectedTarget)) => "protected",
            (_, Some(InformationalReason::ProfileDeny)) => "denied by your sandbox.toml deny",
            (_, Some(InformationalReason::Capability)) => "never allowed",
            (_, Some(InformationalReason::UnproxiedNetwork)) => "not through the proxy",
            (_, Some(InformationalReason::PolicyDenylist)) => {
                "denied by your organisation's policy"
            }
            (_, Some(InformationalReason::Unattributed)) => "not a path this command named",
            (_, Some(InformationalReason::DecodeFailed)) => "the request could not be shown",
            (Blocked::FsWrite { .. } | Blocked::FsRead { .. }, None) => "outside workspace",
            (Blocked::Net { .. }, None) => "not allowed",
            (Blocked::Capability { .. }, None) => "never allowed",
        };
        if matches!(self.replay, Replay::Resume { .. }) {
            return format!(
                "sandbox denied: {what} ({why}). {settled} The proxy refused the connection (403) and the command went on."
            );
        }
        let exit = exit_phrase(self.exit_code);
        format!(
            "sandbox denied: {what} ({why}). {settled} Exit {exit}. stderr: {}",
            self.stderr_snippet
        )
    }
}

/// A replayed command stopped on the very target its grant allows: the policy says yes, the
/// backend could not make it so. [`decode`] reads such a denial as the OS's and returns `None`;
/// on the run after a grant the result path asks [`refused_under_grant`] instead, so the model is
/// told the grant did not take rather than handed the raw OS error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefusedUnderGrant {
    pub blocked: Blocked,
    pub exit_code: Option<i32>,
    /// UTF-8-safe, at most [`STDERR_SNIPPET_MAX_BYTES`].
    pub stderr_snippet: String,
}

impl RefusedUnderGrant {
    /// The model-visible text: `limitation` is the backend's one-line reason when it has one;
    /// otherwise the text says only that the grant could not be applied. Never the raw OS error
    /// alone.
    pub fn text(&self, limitation: Option<&str>) -> String {
        let what = blocked_phrase(&self.blocked);
        let limitation = limitation
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .unwrap_or("the sandbox backend could not apply the grant to this target");
        format!(
            "sandbox: refused again under the grant: {what} is allowed by the policy but {limitation}. \
             Do not retry the same command; report it. Exit {}. stderr: {}",
            exit_phrase(self.exit_code),
            self.stderr_snippet
        )
    }
}

/// What a denial was about, as the model reads it: `write to <path>`, `connection to <host>`.
fn blocked_phrase(blocked: &Blocked) -> String {
    match blocked {
        Blocked::FsWrite { path } => format!("write to {}", path.display()),
        Blocked::FsRead { path } => format!("read of {}", path.display()),
        Blocked::Net { host, port } => {
            format!("connection to {}", net_target(host.as_deref(), *port))
        }
        Blocked::Capability { what } => format!("capability {}", <&str>::from(*what)),
        Blocked::Unknown { .. } => "an operation the sandbox refused".to_owned(),
    }
}

fn exit_phrase(exit_code: Option<i32>) -> String {
    exit_code.map_or_else(|| "signal".to_owned(), |code| code.to_string())
}

/// `host:port`, `host`, `:port` or `an unknown host` — how a blocked connection reads in text.
pub fn net_target(host: Option<&str>, port: Option<u16>) -> String {
    match (host, port) {
        (Some(host), Some(port)) => format!("{host}:{port}"),
        (Some(host), None) => host.to_owned(),
        (None, Some(port)) => format!("an unknown host, port {port}"),
        (None, None) => "an unknown host".to_owned(),
    }
}

/// Everything [`decode`] reads about one finished command.
pub struct DecodeInput<'a> {
    pub exit: CommandExit,
    /// The command's output as the terminal delivered it: one merged buffer.
    /// The coarse channel reads it as stderr.
    pub output: &'a [u8],
    /// Output reached the model before the failure — when the caller's terminal knows; `None`
    /// otherwise.
    pub partial_output: Option<bool>,
    pub cwd: &'a Path,
    /// The command's arguments without the program — what it named. The
    /// shell call's script is one argument and is scanned as text.
    pub argv: &'a [OsString],
    pub policy: &'a SandboxPolicy,
    /// False under `Off`/`Observe` or when the wrapper was not applied: nothing is decoded then.
    pub ran_sandboxed: bool,
    /// What bounds the directory a proposal may name.
    pub bounds: ProposalBounds<'a>,
}

impl DecodeInput<'_> {
    /// The paths this command reached for: its cwd, what its argv and environment named, the
    /// served root.
    fn own_targets(&self) -> OwnTargets {
        OwnTargets::of(
            self.cwd,
            self.argv.iter().map(OsString::as_os_str),
            self.bounds.workspace_root,
            self.bounds.extra_bases,
            self.bounds.user_home,
        )
    }
}

/// Turn a finished command into the violation the card shows, or `None` when the command was not
/// stopped by the sandbox. Order: quick rejects (not sandboxed, exit 0, 126/127), then the coarse
/// battery in [`coarse`]. A target the policy already allows is not a violation (the OS, not the
/// sandbox, refused it). A grantable path the command never named ([`OwnTargets`]) is
/// `informational: unattributed`. Post-run decode never derives a
/// [`Replay::Resume`]: a held connection is posted mid-command by the proxy's decider.
pub fn decode(input: DecodeInput<'_>) -> Option<Violation> {
    let (blocked, exit_code, output) = blocked_target(&input)?;
    if input.policy.would_allow(&blocked) || is_os_denial(&blocked, input.policy) {
        return None;
    }
    let propose::Proposal {
        mut subject,
        mut disposition,
    } = propose::propose(&blocked, input.policy, &input.bounds);
    // The build-cache family is its own attribution: a toolchain names its cache in no argv
    // (`cargo build` writes `~/.cargo/registry/src`), and the family is the daemon's curated
    // table, not a path read from the output
    if disposition.is_grantable()
        && !matches!(subject, Some(GrantSubject::BuildCaches))
        && let Blocked::FsWrite { path } | Blocked::FsRead { path } = &blocked
        && !input.own_targets().covers(path)
    {
        subject = None;
        disposition = Disposition::informational(InformationalReason::Unattributed);
    }
    Some(Violation {
        blocked,
        proposed: subject,
        disposition,
        partial_output: input.partial_output,
        replay: Replay::Rerun,
        exit_code,
        stderr_snippet: coarse::stderr_tail(&output, STDERR_SNIPPET_MAX_BYTES),
    })
}

/// The reading of a **replayed** command's failure that [`decode`] gives up as the OS's: the
/// command was stopped on a target `input.policy` — the policy widened by the grant — allows.
/// `None` when the command was not stopped by the sandbox at all, or on a target the policy
/// still refuses (then [`decode`] has a violation to card). Only the run after a grant asks this;
/// on a first run the same shape is the OS refusing.
pub fn refused_under_grant(input: DecodeInput<'_>) -> Option<RefusedUnderGrant> {
    let (blocked, exit_code, output) = blocked_target(&input)?;
    if !input.policy.would_allow(&blocked) {
        return None;
    }
    Some(RefusedUnderGrant {
        blocked,
        exit_code,
        stderr_snippet: coarse::stderr_tail(&output, STDERR_SNIPPET_MAX_BYTES),
    })
}

/// The quick rejects and the coarse channel, shared by [`decode`] and [`refused_under_grant`]:
/// what the command was stopped on, its exit code and its output as text.
fn blocked_target<'a>(input: &DecodeInput<'a>) -> Option<(Blocked, Option<i32>, Cow<'a, str>)> {
    if !input.ran_sandboxed || input.exit.success() {
        return None;
    }
    let exit_code = input.exit.code;
    if matches!(exit_code, Some(126 | 127)) {
        return None;
    }
    let output = String::from_utf8_lossy(input.output);
    let blocked = coarse::decode_coarse(&output, input.cwd, input.policy)?;
    Some((blocked, exit_code, output))
}

/// The grant a card for `blocked` would propose, without the rest of the decode: the key an
/// observe-mode would-block row aggregates under, so ten denials below one
/// missing `~/.local/lib` tree are one row. `None` when the card would offer nothing (a protected
/// target, a capability).
pub fn proposed_subject(
    blocked: &Blocked,
    policy: &SandboxPolicy,
    bounds: &ProposalBounds<'_>,
) -> Option<GrantSubject> {
    propose::propose(blocked, policy, bounds).subject
}

/// A coarse write whose nearest existing ancestor the policy lets the command write is the OS
/// refusing (a mode bit, a busy file), not the sandbox: the sandbox would have allowed the whole
/// tree — unless the target itself is protected: `<ws>/.git/config` under a
/// writable `.git`, a first `mkdir <ws>/.grok`, are the floor holding the line, and the user is
/// told.
fn is_os_denial(blocked: &Blocked, policy: &SandboxPolicy) -> bool {
    match blocked {
        Blocked::FsWrite { path } => {
            !policy.is_protected(path)
                && policy.would_allow(&Blocked::FsWrite {
                    path: coarse::nearest_existing_dir(path),
                })
        }
        Blocked::FsRead { .. }
        | Blocked::Net { .. }
        | Blocked::Capability { .. }
        | Blocked::Unknown { .. } => false,
    }
}

#[cfg(test)]
#[path = "violation_tests.rs"]
mod tests;
