//! The sandbox-violation card on the hub permission channel: one [`SandboxCard`], spliced twice
//! into the `permission_request` payload (at the top level beside the pre-run card's keys, and
//! whole under `sandbox_violation` for the chat relay), and the reply the desktop sends back. An
//! answer outside the offered scopes and expiries is clamped *down* and logged, never up.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};
use xai_grok_sandbox::command::canonical::{fold_dots, is_within};
use xai_grok_sandbox::command::grants::{
    Expiry, GrantScope, GrantSubject, HostPattern, split_host_port,
};
use xai_grok_sandbox::command::violation::{
    Blocked, Disposition, InformationalReason, Replay, Violation,
};
use xai_grok_sandbox::command::{
    BUILD_CACHE_TREES, BackendName, ProposalBounds, SandboxMode, canonical_path, is_too_broad,
};
use xai_tool_runtime::ToolApprovalPolicy;

use crate::permission::prompter::tool_name_for_access;
use crate::permission::types::AccessKind;

/// The wire `kind` that tells the renderer this is a sandbox card, not a pre-run prompt; also the
/// payload key the whole card is nested under.
pub const SANDBOX_VIOLATION_KIND: &str = "sandbox_violation";

/// The card's `schema_version`; a renderer that does not know it renders a
/// deny-only informational card and answers [`DECODE_FAILED_REASON`].
pub const SCHEMA_VERSION: u32 = 1;

/// The reply `reason` a renderer sends with `reject` when it could not decode the card.
pub const DECODE_FAILED_REASON: &str = "decode_failed";

/// `backend` on the wire when the host has no per-command backend (never `null`).
pub const BACKEND_NONE: &str = "none";

/// The payload keys that are not the card: the pre-run prompt's fields, kept top-level only, and
/// the nested copy itself. Everything else at the top level is a [`SandboxCard`] field.
pub const PRE_RUN_KEYS: [&str; 6] = [
    "tool_call_id",
    "tool_name",
    "description",
    "scope",
    "tool_approval_policy",
    SANDBOX_VIOLATION_KIND,
];

/// The longest a persisted grant may live under `grants_allowed` (7 days).
pub const MAX_PERSISTED_TTL_SECONDS: u64 = 7 * 24 * 3600;

/// The TTLs the card offers for a persisted scope, shortest first.
pub const OFFERED_TTLS_SECONDS: [u64; 3] = [3600, 86_400, MAX_PERSISTED_TTL_SECONDS];

/// The most of a reply's `followup_message` that reaches the model; the rest is dropped.
pub const MAX_FOLLOWUP_CHARS: usize = 2_000;

/// A grant duration as the card names it; ordered weakest to strongest so clamping is `min`.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, strum::IntoStaticStr,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum OfferedScope {
    Call,
    Session,
    Workspace,
    Global,
}

impl OfferedScope {
    fn into_grant_scope(self, workspace_root: &Path) -> GrantScope {
        match self {
            OfferedScope::Call => GrantScope::Call,
            OfferedScope::Session => GrantScope::Session,
            OfferedScope::Workspace => GrantScope::Workspace {
                root: workspace_root.to_path_buf(),
            },
            OfferedScope::Global => GrantScope::Global,
        }
    }
}

/// What the tenant ceiling lets this card offer. Empty `scopes` means the card has Deny only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ceiling {
    pub scopes: Vec<OfferedScope>,
    pub expiries: Vec<Expiry>,
}

impl Ceiling {
    /// Deny-only: an informational violation (protected target, capability, a connection the
    /// proxy never saw, a policy deny-list hit), or anything without a proposal.
    pub fn deny_only() -> Ceiling {
        Ceiling {
            scopes: Vec::new(),
            expiries: Vec::new(),
        }
    }

    /// `always_prompt` → call only; `grants_allowed` → call/session/workspace with TTL ≤ 7 d;
    /// `unattended_allowed` adds global and never. A violation whose [`Disposition`] is
    /// informational, or whose path the card cannot carry ([`is_wire_expressible`]), gets
    /// [`Ceiling::deny_only`] whatever the policy says. (A blocked connection
    /// with no proxy to grant it through is informational by construction — `unproxied_network` —
    /// so no proxy state is consulted here.)
    pub fn for_violation(policy: ToolApprovalPolicy, violation: &Violation) -> Ceiling {
        if !violation.is_grantable() || !is_wire_expressible(violation) {
            return Ceiling::deny_only();
        }
        let mut scopes = vec![OfferedScope::Call];
        let mut expiries: Vec<Expiry> = Vec::new();
        if matches!(
            policy,
            ToolApprovalPolicy::GrantsAllowed | ToolApprovalPolicy::UnattendedAllowed
        ) {
            scopes.extend([OfferedScope::Session, OfferedScope::Workspace]);
            expiries.extend(
                OFFERED_TTLS_SECONDS
                    .iter()
                    .map(|&seconds| Expiry::Ttl { seconds }),
            );
        }
        if policy == ToolApprovalPolicy::UnattendedAllowed {
            scopes.push(OfferedScope::Global);
            expiries.push(Expiry::Never);
        }
        Ceiling { scopes, expiries }
    }

    /// The card has Deny only: it is posted informational, settled before the user sees it.
    pub fn is_deny_only(&self) -> bool {
        self.scopes.is_empty()
    }

    fn strongest_scope(&self) -> Option<OfferedScope> {
        self.scopes.iter().copied().max()
    }

    fn offers_never(&self) -> bool {
        self.expiries.contains(&Expiry::Never)
    }
}

/// Everything about the call the card shows besides the violation itself.
#[derive(Clone, Copy, Debug)]
pub struct SandboxCardContext<'a> {
    pub tool_call_id: &'a str,
    pub command: &'a str,
    pub policy: ToolApprovalPolicy,
    pub mode: SandboxMode,
    pub backend: Option<BackendName>,
    pub reduced_sandbox: bool,
    /// When the gate stops waiting for the answer (unix seconds); `None` on an informational
    /// card, which is not waited for.
    pub deadline_unix: Option<i64>,
}

/// `replay` on the wire: how the command continues after an allow.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayKind {
    Rerun,
    Resume,
}

impl From<&Replay> for ReplayKind {
    fn from(replay: &Replay) -> ReplayKind {
        match replay {
            Replay::Rerun => ReplayKind::Rerun,
            Replay::Resume { .. } => ReplayKind::Resume,
        }
    }
}

/// `proposed_grant` on the wire.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct ProposedGrant<'a> {
    pub subject: &'a GrantSubject,
}

/// The sandbox card: every field the desktop renders. This is
/// the object the relay forwards verbatim, so it carries `kind` and `bash_command` itself.
#[derive(Clone, Debug, Serialize)]
pub struct SandboxCard<'a> {
    pub schema_version: u32,
    pub kind: &'static str,
    pub bash_command: &'a str,
    #[serde(serialize_with = "serialize_blocked")]
    pub blocked: &'a Blocked,
    /// Grantable, or informational with the reason: the daemon's decision, rendered as is. An
    /// informational card offers nothing (`offered_scopes: []`), is not waited for, and renders
    /// one `Got it`; the denial is already settled to the model.
    pub disposition: Disposition,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proposed_grant: Option<ProposedGrant<'a>>,
    /// The target was read from the command's output or is a guess at a directory that does not
    /// exist yet (the coarse decode): the card asks the user to check it before
    /// allowing. A connection the proxy held is exact and needs no confirmation.
    pub proposal_confirm: bool,
    pub offered_scopes: &'a [OfferedScope],
    pub offered_expiries: &'a [Expiry],
    pub replay: ReplayKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// The tail of the command's output, already bounded by the decoder.
    pub stderr_snippet: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deadline_unix: Option<i64>,
    /// `"seatbelt"`, or [`BACKEND_NONE`] on a host with no per-command backend; never `null`.
    #[serde(serialize_with = "serialize_backend")]
    pub backend: Option<BackendName>,
    pub reduced_sandbox: bool,
    pub sandbox_mode: SandboxMode,
}

impl<'a> SandboxCard<'a> {
    pub fn new(
        ctx: &SandboxCardContext<'a>,
        violation: &'a Violation,
        ceiling: &'a Ceiling,
    ) -> SandboxCard<'a> {
        let expressible = is_wire_expressible(violation);
        let informational = ceiling.is_deny_only() || !expressible;
        SandboxCard {
            schema_version: SCHEMA_VERSION,
            kind: SANDBOX_VIOLATION_KIND,
            bash_command: ctx.command,
            blocked: &violation.blocked,
            disposition: match violation.disposition {
                // The renderer's words for a card it could not read are the true ones here
                Disposition::Grantable if !expressible => Disposition::Informational {
                    reason: InformationalReason::DecodeFailed,
                },
                disposition => disposition,
            },
            proposed_grant: violation
                .proposed
                .as_ref()
                .filter(|_| expressible)
                .map(|subject| ProposedGrant { subject }),
            proposal_confirm: expressible && proposal_needs_confirm(violation),
            offered_scopes: if informational { &[] } else { &ceiling.scopes },
            offered_expiries: if informational {
                &[]
            } else {
                &ceiling.expiries
            },
            replay: ReplayKind::from(&violation.replay),
            exit_code: violation.exit_code,
            stderr_snippet: &violation.stderr_snippet,
            deadline_unix: if informational {
                None
            } else {
                ctx.deadline_unix
            },
            backend: ctx.backend,
            reduced_sandbox: ctx.reduced_sandbox,
            sandbox_mode: ctx.mode,
        }
    }

    /// The card as one JSON object. Paths travel lossily, so this does not fail; if it ever did,
    /// the renderer gets a sandbox card it cannot decode, answers `decode_failed`, and the denial
    /// stands.
    pub fn to_json(&self) -> serde_json::Map<String, Value> {
        let error = match serde_json::to_value(self) {
            Ok(Value::Object(map)) => return map,
            Ok(other) => format!("not an object: {other}"),
            Err(error) => error.to_string(),
        };
        tracing::error!(%error, "sandbox card did not serialize; sending one the renderer rejects");
        let mut map = serde_json::Map::new();
        map.insert(
            "schema_version".to_owned(),
            Value::from(self.schema_version),
        );
        map.insert("kind".to_owned(), Value::from(self.kind));
        map.insert("bash_command".to_owned(), Value::from(self.bash_command));
        map
    }
}

/// Whether every path the card would carry is UTF-8. A JSON string holds nothing else, and a
/// lossy spelling names another file, so a card that cannot carry its path offers nothing.
pub fn is_wire_expressible(violation: &Violation) -> bool {
    let blocked = match &violation.blocked {
        Blocked::FsWrite { path } | Blocked::FsRead { path } => path.to_str().is_some(),
        Blocked::Net { .. } | Blocked::Capability { .. } | Blocked::Unknown { .. } => true,
    };
    blocked && violation.proposed.as_ref().is_none_or(subject_is_utf8)
}

fn subject_is_utf8(subject: &GrantSubject) -> bool {
    match subject {
        GrantSubject::FsWriteRoot { root } | GrantSubject::FsRead { root } => {
            root.to_str().is_some()
        }
        GrantSubject::NetHost { .. } | GrantSubject::BuildCaches => true,
    }
}

fn lossy_path(path: &Path) -> PathBuf {
    PathBuf::from(path.to_string_lossy().into_owned())
}

/// Coarse decode — the only channel for a stopped command — guesses the target from the output;
/// the one exact fact is a connection the proxy held (`Replay::Resume`).
fn proposal_needs_confirm(violation: &Violation) -> bool {
    violation.proposed.is_some() && !matches!(violation.replay, Replay::Resume { .. })
}

/// `blocked` on the wire: the serde shape of [`Blocked`] minus the stderr content of `Unknown`
/// (the card shows the snippet from `stderr_snippet`, which the decoder already bounded), with a
/// path that is not UTF-8 spelled lossily for display.
fn serialize_blocked<S: serde::Serializer>(
    blocked: &&Blocked,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match blocked {
        Blocked::Unknown { .. } => json!({ "kind": "unknown" }).serialize(serializer),
        Blocked::FsWrite { path } => Blocked::FsWrite {
            path: lossy_path(path),
        }
        .serialize(serializer),
        Blocked::FsRead { path } => Blocked::FsRead {
            path: lossy_path(path),
        }
        .serialize(serializer),
        other @ (Blocked::Net { .. } | Blocked::Capability { .. }) => other.serialize(serializer),
    }
}

/// `backend` on the wire: the backend's name, or [`BACKEND_NONE`].
fn serialize_backend<S: serde::Serializer>(
    backend: &Option<BackendName>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match backend {
        Some(name) => name.serialize(serializer),
        None => BACKEND_NONE.serialize(serializer),
    }
}

/// The `permission_request` payload for a sandbox violation: the pre-run card's
/// keys, then the [`SandboxCard`] fields at the top level, then the same card whole under
/// `sandbox_violation`.
pub fn build_sandbox_violation_payload(
    ctx: SandboxCardContext<'_>,
    violation: &Violation,
    ceiling: &Ceiling,
) -> Value {
    let card = SandboxCard::new(&ctx, violation, ceiling).to_json();
    let mut payload = serde_json::Map::new();
    payload.insert("tool_call_id".to_owned(), Value::from(ctx.tool_call_id));
    payload.insert(
        "tool_name".to_owned(),
        Value::from(tool_name_for_access(&AccessKind::Bash(
            ctx.command.to_owned(),
        ))),
    );
    payload.insert(
        "description".to_owned(),
        Value::from("Grok's command was blocked by the sandbox"),
    );
    payload.insert("scope".to_owned(), Value::from("write"));
    payload.insert("tool_approval_policy".to_owned(), json!(ctx.policy));
    payload.extend(card.clone());
    payload.insert(SANDBOX_VIOLATION_KIND.to_owned(), Value::Object(card));
    Value::Object(payload)
}

/// The user's answer to a sandbox card, already clamped to the ceiling.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SandboxAnswer {
    Allow {
        subject: GrantSubject,
        scope: GrantScope,
        expires: Expiry,
    },
    /// `remember` is the card's "always reject": the gate may persist a deny row for it.
    Deny {
        followup: Option<String>,
        remember: bool,
    },
    Cancelled,
    /// The renderer could not decode the card and showed its own deny-only context:
    /// a denial the gate logs and tells the model about in its own words.
    DecodeFailed,
    /// An allow naming no sandbox decision (no subject, duration or expiry), as a client that
    /// approves every prompt without showing it answers: nothing is granted, the denial stands.
    NoSandboxDecision,
    /// An allow whose subject the violation does not tie to — a folder that does not hold the
    /// blocked path or is wider than the one offered, a host the connection was not to or a
    /// pattern wider than the offered one, a port other than the held one: the user allowed what
    /// the card showed, and this is not it. Nothing is granted; `reason` says what was refused.
    SubjectRefused {
        reason: String,
    },
}

/// The reply's `outcome`, as a name or as the legacy renderer's number (1–4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Outcome {
    Approve,
    Reject,
    AlwaysApprove,
    AlwaysReject,
    Cancelled,
}

impl Outcome {
    fn from_legacy_number(n: i64) -> Option<Outcome> {
        match n {
            1 => Some(Outcome::Approve),
            2 => Some(Outcome::Reject),
            3 => Some(Outcome::AlwaysApprove),
            4 => Some(Outcome::AlwaysReject),
            _ => None,
        }
    }
}

fn deserialize_outcome<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Outcome>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Named(Outcome),
        Number(i64),
        Other(serde::de::IgnoredAny),
    }
    Ok(match Raw::deserialize(deserializer)? {
        Raw::Named(outcome) => Some(outcome),
        Raw::Number(n) => Outcome::from_legacy_number(n),
        Raw::Other(_) => None,
    })
}

/// A field the reply may get wrong without spoiling the rest of it: a malformed value reads as
/// absent, and the decoder's default for that field applies.
fn lenient<'de, D: Deserializer<'de>, T: serde::de::DeserializeOwned>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    let value = Value::deserialize(deserializer)?;
    Ok(T::deserialize(value).ok())
}

/// The `scope.kind` the strict `ToolPermission` proto has for a bash answer; the desktop carries
/// the sandbox reply inside its value because the proto has no sandbox slot.
pub const BASH_COMMAND_SCOPE_KIND: &str = "bash_command";

/// The grant kinds a reply's `scope` may name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ReplyScopeKind {
    FsWriteRoot,
    FsRead,
    NetHost,
    NetAll,
    /// The build-cache family: no value, the trees are the daemon's table.
    BuildCaches,
    /// [`BASH_COMMAND_SCOPE_KIND`]: the value is the embedded reply.
    BashCommand,
    #[serde(other)]
    Other,
}

#[derive(Clone, Debug, Deserialize)]
struct ReplyScope {
    kind: ReplyScopeKind,
    #[serde(default)]
    value: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
struct ReplyDuration {
    kind: OfferedScope,
}

/// One reply on the permission channel, every field optional: an absent or
/// unrecognised `outcome` is a rejection.
#[derive(Clone, Debug, Default, Deserialize)]
struct SandboxReply {
    #[serde(default, deserialize_with = "deserialize_outcome")]
    outcome: Option<Outcome>,
    #[serde(default, deserialize_with = "lenient")]
    scope: Option<ReplyScope>,
    #[serde(default, deserialize_with = "lenient")]
    duration: Option<ReplyDuration>,
    #[serde(default, deserialize_with = "lenient")]
    expires: Option<Expiry>,
    #[serde(default, deserialize_with = "lenient")]
    followup_message: Option<String>,
    /// Why a `reject` was not the user's: [`DECODE_FAILED_REASON`] is the one value read.
    #[serde(default, deserialize_with = "lenient")]
    reason: Option<String>,
}

impl SandboxReply {
    /// Whether the reply carries a decision only the sandbox card makes: a grant subject of a
    /// sandbox kind. `duration` and `expires` alone are the generic approval vocabulary a pre-run
    /// card or a client auto-approving every prompt sends too, so they decide nothing.
    fn carries_sandbox_decision(&self) -> bool {
        self.scope.as_ref().is_some_and(|scope| {
            !matches!(
                scope.kind,
                ReplyScopeKind::BashCommand | ReplyScopeKind::Other
            )
        })
    }

    /// Parse the reply, unfolding the desktop's `sandboxAllow` shape: `approve` / `always_approve`
    /// with `scope = { kind: "bash_command", value: "<reply JSON>" }`. The value always
    /// starts with `{`, which a real always-allow bash prefix never does; the embedded `outcome`,
    /// `scope`, `duration`, `expires`, `followup_message` and `reason` replace the outer ones
    /// (a `decode_failed` reject travels the same embedded way after the relay). A value that is not JSON drops the scope, so
    /// the proposal stands.
    fn parse(reply: &Value) -> SandboxReply {
        // serde reads a struct from a sequence too; a reply is an object or nothing
        if !reply.is_object() {
            tracing::warn!("sandbox card reply is not an object; rejecting");
            return SandboxReply::default();
        }
        let mut parsed: SandboxReply =
            serde_json::from_value(reply.clone()).unwrap_or_else(|error| {
                tracing::warn!(%error, "sandbox card reply is malformed; rejecting");
                SandboxReply::default()
            });
        let Some(ReplyScope {
            kind: ReplyScopeKind::BashCommand,
            value: Some(value),
        }) = &parsed.scope
        else {
            return parsed;
        };
        let value = value.trim();
        if !value.starts_with('{') {
            parsed.scope = None;
            return parsed;
        }
        match serde_json::from_str::<SandboxReply>(value) {
            Ok(embedded) => {
                parsed.scope = embedded.scope;
                if embedded.outcome.is_some() {
                    parsed.outcome = embedded.outcome;
                }
                if embedded.followup_message.is_some() {
                    parsed.followup_message = embedded.followup_message;
                }
                if embedded.duration.is_some() {
                    parsed.duration = embedded.duration;
                }
                if embedded.expires.is_some() {
                    parsed.expires = embedded.expires;
                }
                if embedded.reason.is_some() {
                    parsed.reason = embedded.reason;
                }
            }
            Err(error) => {
                tracing::warn!(%error, "sandbox card reply carried unparsable JSON in bash_command; using the proposal");
                parsed.scope = None;
            }
        }
        parsed
    }
}

/// Decode one reply: an answer outside the offered set is clamped down and logged, and a subject
/// the violation does not tie to is [`SandboxAnswer::SubjectRefused`], never widened to the
/// proposal. `now_unix` bounds an `at` expiry; `bounds` is what a typed folder is held to.
pub fn decode_reply(
    reply: &Value,
    violation: &Violation,
    ceiling: &Ceiling,
    bounds: &ProposalBounds<'_>,
    now_unix: i64,
) -> SandboxAnswer {
    let reply = SandboxReply::parse(reply);
    let followup = reply
        .followup_message
        .as_deref()
        .filter(|message| !message.is_empty())
        .map(|message| message.chars().take(MAX_FOLLOWUP_CHARS).collect::<String>());
    let deny = |remember: bool| SandboxAnswer::Deny {
        followup: followup.clone(),
        remember,
    };
    match reply.outcome {
        Some(outcome @ (Outcome::Approve | Outcome::AlwaysApprove)) => {
            let Some(strongest) = ceiling.strongest_scope() else {
                // SECURITY: a deny-only card (protected target, capability, network off) never
                // yields a grant
                tracing::warn!("sandbox card allowed a non-grantable violation; denying");
                return deny(false);
            };
            if !reply.carries_sandbox_decision() {
                // SECURITY: the card's own answer always names a subject or a duration; a bare
                // approve is a client approving every prompt unseen, and grants nothing
                tracing::warn!("sandbox card approved with no sandbox decision; denying");
                return SandboxAnswer::NoSandboxDecision;
            }
            let subject = match answered_subject(&reply, violation, bounds) {
                Answered::Subject(subject) => subject,
                Answered::Proposal => match violation.proposed.clone() {
                    Some(proposed) => proposed,
                    None => {
                        tracing::warn!(
                            "sandbox card allowed a violation with nothing to grant; denying"
                        );
                        return deny(false);
                    }
                },
                Answered::Refused(reason) => {
                    // SECURITY: the user allowed what the card showed; a subject the violation
                    // does not tie to grants nothing, and is never widened to the proposal
                    tracing::warn!(
                        %reason,
                        "sandbox card allowed a subject the violation does not tie to; refusing"
                    );
                    return SandboxAnswer::SubjectRefused { reason };
                }
            };
            let asked = reply
                .duration
                .map(|duration| duration.kind)
                .unwrap_or(match outcome {
                    Outcome::AlwaysApprove => strongest,
                    _ => OfferedScope::Call,
                });
            let scope = if ceiling.scopes.contains(&asked) {
                asked
            } else {
                let clamped = strongest.min(asked);
                tracing::info!(
                    asked = <&str>::from(asked),
                    clamped = <&str>::from(clamped),
                    "sandbox grant duration outside the offered set; clamped"
                );
                clamped
            };
            let Some(expires) = clamp_expiry(reply.expires, scope, ceiling, now_unix) else {
                tracing::warn!(
                    expires = ?reply.expires,
                    "sandbox card allowed with a lifetime already over; denying"
                );
                return deny(false);
            };
            SandboxAnswer::Allow {
                subject,
                scope: scope.into_grant_scope(bounds.workspace_root),
                expires,
            }
        }
        Some(Outcome::AlwaysReject) => deny(true),
        Some(Outcome::Cancelled) => SandboxAnswer::Cancelled,
        Some(Outcome::Reject) if reply.reason.as_deref() == Some(DECODE_FAILED_REASON) => {
            SandboxAnswer::DecodeFailed
        }
        Some(Outcome::Reject) | None => deny(false),
    }
}

/// Call and session rows die with the call or the daemon, so they carry `Never`. A persisted
/// scope gets the asked TTL capped at seven days, an `at` within seven days, or the default
/// seven days; `never` only where the ceiling offers it. A zero TTL or an `at` already past is
/// `None`: any lifetime this could give would be longer than the one asked for.
fn clamp_expiry(
    asked: Option<Expiry>,
    scope: OfferedScope,
    ceiling: &Ceiling,
    now_unix: i64,
) -> Option<Expiry> {
    if matches!(scope, OfferedScope::Call | OfferedScope::Session) {
        return Some(Expiry::Never);
    }
    let max_ttl = Expiry::Ttl {
        seconds: MAX_PERSISTED_TTL_SECONDS,
    };
    match asked {
        Some(Expiry::Ttl { seconds: 0 }) => None,
        Some(Expiry::At { unix }) if unix <= now_unix => None,
        Some(Expiry::Ttl { seconds }) if seconds <= MAX_PERSISTED_TTL_SECONDS => {
            Some(Expiry::Ttl { seconds })
        }
        Some(Expiry::At { unix })
            if unix
                <= now_unix.saturating_add(
                    i64::try_from(MAX_PERSISTED_TTL_SECONDS).unwrap_or(i64::MAX),
                ) =>
        {
            Some(Expiry::At { unix })
        }
        Some(Expiry::Never) if ceiling.offers_never() => Some(Expiry::Never),
        None => Some(max_ttl),
        Some(other) => {
            tracing::info!(
                ?other,
                "sandbox grant expiry outside the offered set; clamped to 7 days"
            );
            Some(max_ttl)
        }
    }
}

/// What the reply's `scope` amounts to.
enum Answered {
    /// A valid subject of the violation's kind, tied to it ([`tied_to_violation`]).
    Subject(GrantSubject),
    /// Nothing usable was named: the decoder's proposal stands.
    Proposal,
    /// The reply names a subject the violation does not tie to: the allow is refused rather
    /// than widened to the proposal. The text says what was refused.
    Refused(String),
}

/// The reply's subject when of the violation's kind and tied to it, else the proposal: a typed
/// path folded, never resolved, and held to [`is_too_broad`]; `net_host` the host (any `:<port>`
/// the held one); `net_all` and `build_caches` only on the card that offered them.
fn answered_subject(
    reply: &SandboxReply,
    violation: &Violation,
    bounds: &ProposalBounds<'_>,
) -> Answered {
    let Some(scope) = &reply.scope else {
        return Answered::Proposal;
    };
    let value = scope.value.as_deref().filter(|s| !s.is_empty());
    let answered = match (scope.kind, value, &violation.blocked) {
        (ReplyScopeKind::FsWriteRoot, Some(path), Blocked::FsWrite { .. }) => {
            GrantSubject::FsWriteRoot {
                root: fold_dots(Path::new(path)),
            }
        }
        (ReplyScopeKind::FsRead, Some(path), Blocked::FsRead { .. }) => GrantSubject::FsRead {
            root: fold_dots(Path::new(path)),
        },
        (ReplyScopeKind::NetHost, Some(value), Blocked::Net { port, .. }) => {
            match net_host_value(value, *port) {
                Some(host) => GrantSubject::NetHost {
                    host: HostPattern::new(host),
                    port: *port,
                },
                None => {
                    return Answered::Refused(format!(
                        "the reply named {value}, a port other than the held {}",
                        port.map_or("one".to_owned(), |port| port.to_string())
                    ));
                }
            }
        }
        (ReplyScopeKind::NetAll, _, Blocked::Net { .. }) => GrantSubject::NetHost {
            host: HostPattern::all(),
            port: None,
        },
        // The family is what the card offered, or nothing: a `build_caches` answer to a card
        // that proposed a folder would widen to trees the user was never shown
        (ReplyScopeKind::BuildCaches, _, Blocked::FsWrite { .. })
            if violation.proposed == Some(GrantSubject::BuildCaches) =>
        {
            GrantSubject::BuildCaches
        }
        (ReplyScopeKind::BuildCaches, _, _) => {
            tracing::warn!(
                "sandbox card answered build_caches on a card that did not offer it; using the proposal"
            );
            return Answered::Proposal;
        }
        _ => return Answered::Proposal,
    };
    match answered {
        GrantSubject::FsWriteRoot { root } | GrantSubject::FsRead { root }
            if !root.is_absolute() =>
        {
            tracing::warn!("sandbox card answered a relative path; using the proposal");
            Answered::Proposal
        }
        GrantSubject::FsWriteRoot { root } | GrantSubject::FsRead { root }
            if is_too_broad(&root, bounds) =>
        {
            Answered::Refused(format!(
                "the reply named {}, a folder too broad to be a grant root",
                root.display()
            ))
        }
        subject => match tied_to_violation(&subject, violation, bounds) {
            Ok(()) => Answered::Subject(subject),
            Err(reason) => Answered::Refused(reason),
        },
    }
}

/// Whether `answered` is a subject of *this* violation and no wider than what its card offered:
/// a folder holds the blocked path and lies at or under the proposed root (under one of the
/// family's trees when the card offered the build caches); a host pattern matches the blocked
/// host and is covered by the proposed pattern. `Err` says what does not hold.
fn tied_to_violation(
    answered: &GrantSubject,
    violation: &Violation,
    bounds: &ProposalBounds<'_>,
) -> Result<(), String> {
    match (answered, &violation.blocked) {
        (
            GrantSubject::FsWriteRoot { root } | GrantSubject::FsRead { root },
            Blocked::FsWrite { path } | Blocked::FsRead { path },
        ) => {
            if !is_within(&canonical_path(path), root) {
                return Err(format!(
                    "the reply named {}, which does not contain the blocked path {}",
                    root.display(),
                    path.display()
                ));
            }
            match &violation.proposed {
                Some(GrantSubject::FsWriteRoot { root: offered })
                | Some(GrantSubject::FsRead { root: offered }) => {
                    if !is_within(root, &fold_dots(offered)) {
                        return Err(format!(
                            "the reply named {}, wider than the offered {}",
                            root.display(),
                            offered.display()
                        ));
                    }
                }
                Some(GrantSubject::BuildCaches) => {
                    let Some(home) = bounds.user_home else {
                        return Err(format!(
                            "the reply named {}, but no home directory bounds the offered build caches",
                            root.display()
                        ));
                    };
                    let home = canonical_path(home);
                    if !BUILD_CACHE_TREES
                        .iter()
                        .any(|tree| is_within(root, &home.join(tree)))
                    {
                        return Err(format!(
                            "the reply named {}, which is not inside the offered build caches",
                            root.display()
                        ));
                    }
                }
                Some(GrantSubject::NetHost { .. }) | None => {
                    return Err(format!(
                        "the reply named {}, but the card offered no folder",
                        root.display()
                    ));
                }
            }
            Ok(())
        }
        (GrantSubject::NetHost { host: pattern, .. }, Blocked::Net { host, .. }) => {
            match host {
                Some(host) if !pattern.matches(host) => {
                    return Err(format!(
                        "the reply named {pattern}, which does not match the blocked host {host}"
                    ));
                }
                None if !pattern.is_all() => {
                    return Err(format!(
                        "the reply named {pattern}, but the blocked connection's host is not known"
                    ));
                }
                Some(_) | None => {}
            }
            match &violation.proposed {
                Some(GrantSubject::NetHost { host: offered, .. }) => {
                    if !pattern.is_covered_by(offered) {
                        return Err(format!(
                            "the reply named {pattern}, wider than the offered {offered}"
                        ));
                    }
                }
                Some(_) | None => {
                    return Err(format!(
                        "the reply named {pattern}, but the card offered no host"
                    ));
                }
            }
            Ok(())
        }
        // The family is accepted only on the card that proposed it ([`answered_subject`])
        (GrantSubject::BuildCaches, _) => Ok(()),
        // [`answered_subject`] builds a folder only for a blocked path and a host only for a
        // blocked connection; anything else here is a subject of another kind than the violation
        (
            GrantSubject::FsWriteRoot { .. }
            | GrantSubject::FsRead { .. }
            | GrantSubject::NetHost { .. },
            _,
        ) => Err("the reply named a subject of another kind than what was blocked".to_owned()),
    }
}

/// The host in a `net_host` reply value, split by [`split_host_port`]: the host alone, without the
/// trailing `:<port>` when that port is the violation's (`[::1]:443` answers a connection to
/// `::1` port 443). `None` when the value names another port.
fn net_host_value(value: &str, held_port: Option<u16>) -> Option<&str> {
    let (host, port) = split_host_port(value);
    let Some(port) = port else {
        return Some(host);
    };
    if host.is_empty() {
        return Some(value);
    }
    match port.parse::<u16>() {
        Ok(port) if Some(port) == held_port => Some(host),
        Ok(_) => None,
        // Not a port: a host as written (`HostPattern` judges it)
        Err(_) => Some(value),
    }
}

#[cfg(test)]
#[path = "sandbox_wire_tests.rs"]
mod tests;
