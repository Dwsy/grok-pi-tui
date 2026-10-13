//! Settling a sandbox violation: the card goes to the session owner over the hub permission
//! transport the pre-run gate uses; an allow records a grant and the command is replayed once (a
//! held connection is released by the proxy's sink instead); anything else keeps the denial and
//! tells the model why, in fixed words.
use crate::permission::hub_gate::{PromptGate, ToolApprovalGate};
use crate::permission::policy::resolve_following_symlinks;
use crate::permission::sandbox_wire::{
    Ceiling, MAX_PERSISTED_TTL_SECONDS, SandboxAnswer, SandboxCardContext,
    build_sandbox_violation_payload, decode_reply,
};
use crate::permission::{AccessKind, PermissionHookTransport};
use crate::sandbox::metrics;
use crate::sandbox::{MODE_LAYER_WRITE_TEXT, WorkspaceSandbox};
use crate::session::WorkspaceSession;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use xai_computer_hub_sdk::ToolServer;
use xai_grok_sandbox::command::grants::{
    Expiry, Grant, GrantDecision, GrantId, GrantScope, GrantSubject, PROVENANCE_COMMAND_MAX_CHARS,
    Provenance,
};
use xai_grok_sandbox::command::violation::{Blocked, Replay};
use xai_grok_sandbox::command::{
    BackendName, CallId, SandboxMode, Violation, canonical_path, is_same_path,
};
use xai_grok_telemetry::events::SandboxSettlement;
use xai_grok_tools::implementations::codex::apply_patch::{Hunk, parse_patch};
use xai_grok_tools::types::ToolInput;
use xai_grok_tools::types::resources::resolve_model_path;
use xai_grok_tools::types::tool::{ToolKind, ToolNamespace};
use xai_tool_runtime::{ToolApprovalPolicy, ToolError, ToolErrorKind};
/// What the hub does with one tool call on a folder, decided before dispatch from the tool's
/// kind and namespace and the sandbox's mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SandboxPath {
    /// Not a shell tool: the call never meets the sandbox — no pin, no call-table entry, no
    /// decode. Its pre-run approval is its only gate.
    None,
    /// A shell tool whose run the result path follows (bound in the call table, its output
    /// handed to the decoder) but whose pre-run approval stands: the folder does not enforce, or
    /// the tool's output is not one the decoder reads, so no card could stand in for the prompt.
    Shell,
    /// A shell tool whose output the decoder reads, on a folder whose sandbox enforces: the
    /// sandbox card is the prompt, and the spawn is pinned to `enforce` once the folder's grants
    /// let the call past.
    Gated,
}
impl SandboxPath {
    /// `tool` is the toolset's metadata for the call, `None` for a tool it does not know (the
    /// toolset refuses such a call itself); `mode` is `None` on a workspace with no sandbox.
    pub(crate) fn for_tool(
        tool: Option<(ToolKind, ToolNamespace)>,
        mode: Option<SandboxMode>,
    ) -> SandboxPath {
        let Some((ToolKind::Execute, namespace)) = tool else {
            return SandboxPath::None;
        };
        if mode == Some(SandboxMode::Enforce) && shell_output_is_decodable(namespace) {
            SandboxPath::Gated
        } else {
            SandboxPath::Shell
        }
    }
    /// The path once the approval gate and the call's `args` are known. Under an enforced gate a
    /// run requested in the background is `Shell`: it reports a start, never output the decoder
    /// reads, so no card could stand in for its prompt. With the gate off nothing prompts: as is.
    pub(crate) async fn for_call(
        self,
        gate: ToolApprovalGate,
        session: &WorkspaceSession,
        tool_name: &str,
        args: &Value,
    ) -> SandboxPath {
        if self != SandboxPath::Gated || gate == ToolApprovalGate::Off {
            return self;
        }
        match session.toolset().try_parse(tool_name, args).await {
            Ok(input) if requests_background(&input) => SandboxPath::Shell,
            _ => self,
        }
    }
    pub(crate) fn prompt_gate(self) -> PromptGate {
        match self {
            SandboxPath::None | SandboxPath::Shell => PromptGate::PreRun,
            SandboxPath::Gated => PromptGate::SandboxCard,
        }
    }
    /// Whether the call is dispatched through the sandbox's result path.
    pub(crate) fn follows_result_path(self) -> bool {
        match self {
            SandboxPath::None => false,
            SandboxPath::Shell | SandboxPath::Gated => true,
        }
    }
}
/// Whether a namespace's shell tool reports its run as the `Bash` output the decoder reads (or a
/// background start). The terminal-backed shells do; Pi's bash reports `ToolOutput::Pi`,
/// MiniSweAgent's plain text, and an MCP tool whatever the server sent. A namespace not listed
/// here keeps the pre-run prompt: the safe side when a new shell tool appears.
fn shell_output_is_decodable(namespace: ToolNamespace) -> bool {
    matches!(
        namespace,
        ToolNamespace::GrokBuild | ToolNamespace::GrokBuildConcise | ToolNamespace::OpenCode
    )
}
/// Whether the call asks for the background up front, as its tool exposes the request: the
/// `is_background` flag or a zero wait (`block_until_ms`, or the legacy `timeout` the single-knob
/// contract reads the same way). A wait a contract ignores costs one prompt, never loses one.
fn requests_background(input: &ToolInput) -> bool {
    match input {
        ToolInput::Bash(bash) => {
            bash.is_background || bash.block_until_ms == Some(0) || bash.timeout == Some(0)
        }
        _ => false,
    }
}
/// Under `enforce` a tool that is no shell never writes a mode layer, however the path is spelled
/// (one that does not resolve included): only the owner changes the mode, and a command's writes
/// there meet the floor instead.
pub(crate) async fn refuse_mode_layer_write(
    sandbox: &WorkspaceSandbox,
    session: &WorkspaceSession,
    tool_name: &str,
    args: &Value,
) -> Result<(), ToolError> {
    if sandbox.mode() != SandboxMode::Enforce {
        return Ok(());
    }
    let args = match session.path_virtualization() {
        Some(virt) => virt.rewrite_json_inbound(args.clone()),
        None => args.clone(),
    };
    let Ok(input) = session.toolset().try_parse(tool_name, &args).await else {
        return Ok(());
    };
    let layers = sandbox
        .mode_layers()
        .map(|layer| write_target(&layer).unwrap_or(layer));
    let refused = written_paths(&input, session.cwd()).iter().any(|path| {
        write_target(path)
            .is_none_or(|target| layers.iter().any(|layer| is_same_path(layer, &target)))
    });
    if refused {
        return Err(ToolError::new(
            ToolErrorKind::PermissionDenied,
            MODE_LAYER_WRITE_TEXT,
        ));
    }
    Ok(())
}
/// The files `input` writes, deletes or moves, resolved against `cwd` as its tool resolves them.
fn written_paths(input: &ToolInput, cwd: &Path) -> Vec<PathBuf> {
    if let ToolInput::ApplyPatch(patch) = input {
        let Ok(parsed) = parse_patch(&patch.patch) else {
            return Vec::new();
        };
        return parsed
            .hunks
            .iter()
            .flat_map(|hunk| match hunk {
                Hunk::AddFile { path, .. } | Hunk::DeleteFile { path } => vec![path],
                Hunk::UpdateFile {
                    path, move_path, ..
                } => std::iter::once(path).chain(move_path).collect(),
            })
            .map(|path| cwd.join(path))
            .collect();
    }
    match AccessKind::from(input) {
        AccessKind::Edit(path) => vec![resolve_model_path(cwd, None, &path)],
        _ => Vec::new(),
    }
}
/// Where a write to `path` lands: every link followed, a missing leaf included, in the floor's
/// spelling; `None` when that cannot be told.
fn write_target(path: &Path) -> Option<PathBuf> {
    resolve_following_symlinks(path).map(|target| canonical_path(&target))
}
/// Whether the folder's egress proxy is bound, for the model text of an informational network
/// denial: with no proxy the network is off for the folder; with one, the tool bypassed it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EgressProxyState {
    Running,
    Off,
}
/// What the gate decided for one violation.
#[derive(Debug)]
pub(crate) enum ViolationSettlement {
    /// The grant is recorded. Under `Replay::Rerun` the command runs once more under it; under
    /// `Replay::Resume` the caller (the proxy's violation sink) releases the held connection and
    /// the command is not re-run.
    Replay { grant: Grant },
    /// The denial stands; `model_text` is appended to the tool result.
    Denied { model_text: String },
    /// Recorded only: a coarse `Unknown` never shows a card or changes the
    /// tool result.
    Recorded,
}
impl ViolationSettlement {
    /// The settlement the violation counter and its event name for this outcome.
    fn counted_as(&self, violation: &Violation) -> SandboxSettlement {
        match self {
            ViolationSettlement::Recorded => SandboxSettlement::Observed,
            ViolationSettlement::Denied { .. } => SandboxSettlement::Denied,
            ViolationSettlement::Replay { .. } => match violation.replay {
                Replay::Resume { .. } => SandboxSettlement::Resume,
                Replay::Rerun => SandboxSettlement::Replay,
            },
        }
    }
}
/// Everything about the call the card and the grant row need.
#[derive(Clone, Copy)]
pub(crate) struct SettleContext<'a> {
    pub sandbox: &'a WorkspaceSandbox,
    /// The tenant ceiling the session was bound with.
    pub policy: ToolApprovalPolicy,
    pub call: &'a CallId,
    /// The call's epoch as the card was raised: a call-scoped grant counts only while the call
    /// is still under it (a hold parked before a background start is not the child's). `None`
    /// is the call as the table has it when the grant lands (the post-run card).
    pub epoch: Option<u64>,
    /// The hub session the card goes to; its "for this conversation" grants live with it.
    pub session_id: &'a str,
    pub command: &'a str,
    pub mode: SandboxMode,
    pub backend: Option<BackendName>,
    /// The violation came from the replayed run under this grant: no card, no third run.
    pub replayed_under: Option<&'a GrantSubject>,
}
impl SettleContext<'_> {
    /// `"hub:<session_id>"`: who the grant row says gave it.
    fn granted_by(&self) -> String {
        format!("hub:{}", self.session_id)
    }
}
/// The hub's backstop on a permission round trip, which is when the gate stops waiting for the
/// card's answer; the card counts down to it.
const CARD_DEADLINE: std::time::Duration = ToolServer::HOOK_REQUEST_BACKSTOP_TIMEOUT;
/// Settle one decoded violation, counted once under the settlement it reached. Fails closed:
/// without a transport, on a transport error, on a cancelled card or a grant that cannot be
/// recorded the denial stands.
pub(crate) async fn settle_violation(
    ctx: SettleContext<'_>,
    violation: Violation,
    transport: Option<Arc<dyn PermissionHookTransport>>,
) -> ViolationSettlement {
    let settlement = decide(ctx, &violation, transport).await;
    metrics::violation_settled(ctx.mode, &violation, settlement.counted_as(&violation));
    settlement
}
async fn decide(
    ctx: SettleContext<'_>,
    violation: &Violation,
    transport: Option<Arc<dyn PermissionHookTransport>>,
) -> ViolationSettlement {
    if !violation.produces_card() {
        return ViolationSettlement::Recorded;
    }
    let row_subject = match (&violation.proposed, &violation.blocked) {
        (Some(proposed), _) => Some(proposed.clone()),
        (None, Blocked::FsWrite { path }) => Some(GrantSubject::FsWriteRoot { root: path.clone() }),
        (None, Blocked::FsRead { path }) => Some(GrantSubject::FsRead { root: path.clone() }),
        (None, Blocked::Net { .. } | Blocked::Capability { .. } | Blocked::Unknown { .. }) => None,
    };
    if row_subject
        .is_some_and(|subject| ctx.sandbox.deny_row_covers(Some(ctx.session_id), &subject))
    {
        return ViolationSettlement::Denied {
            model_text: text::rejected_by_row(violation),
        };
    }
    if let Some(granted) = ctx.replayed_under {
        return ViolationSettlement::Denied {
            model_text: text::after_replay(violation, granted),
        };
    }
    let Some(transport) = transport else {
        tracing::warn!(
            call = %ctx.call,
            "no hub transport for the sandbox card; keeping the denial"
        );
        return ViolationSettlement::Denied {
            model_text: text::no_transport(violation),
        };
    };
    let ceiling = Ceiling::for_violation(ctx.policy, violation);
    if ceiling.is_deny_only() {
        let proxy = if ctx.sandbox.proxy().is_some() {
            EgressProxyState::Running
        } else {
            EgressProxyState::Off
        };
        return settle_informational(ctx, violation, &ceiling, proxy, transport);
    }
    let _turn = ctx.sandbox.card_turn(ctx.session_id).await;
    let now = ctx.sandbox.now_unix();
    let deadline_unix = now.saturating_add(i64::try_from(CARD_DEADLINE.as_secs()).unwrap_or(0));
    let payload = build_sandbox_violation_payload(
        card_context(ctx, Some(deadline_unix)),
        violation,
        &ceiling,
    );
    let reply = match transport.request_permission(payload).await {
        Ok(reply) => reply,
        Err(error) => {
            tracing::warn!(call = %ctx.call, %error, "sandbox card failed; keeping the denial");
            return ViolationSettlement::Denied {
                model_text: text::transport_failed(violation, &error),
            };
        }
    };
    let now = ctx.sandbox.now_unix();
    let answer = decode_reply(&reply, violation, &ceiling, &ctx.sandbox.bounds(&[]), now);
    match answer {
        SandboxAnswer::Allow {
            subject,
            scope,
            expires,
        } => {
            let grant = Grant {
                id: new_grant_id(),
                subject,
                scope,
                expires,
                decision: GrantDecision::Allow,
                granted_at: now,
                granted_by: ctx.granted_by(),
                via: Some(provenance(ctx)),
            };
            if let Err(error) = ctx
                .sandbox
                .record_grant(ctx.call, ctx.epoch, ctx.session_id, grant.clone())
                .await
            {
                tracing::warn!(call = %ctx.call, %error, "sandbox grant not recorded; keeping the denial");
                return ViolationSettlement::Denied {
                    model_text: text::allowed_not_recorded(violation, &error),
                };
            }
            if matches!(violation.replay, Replay::Rerun) {
                ctx.sandbox.mark_replay(ctx.call, grant.subject.clone());
            }
            ViolationSettlement::Replay { grant }
        }
        SandboxAnswer::Deny { followup, remember } => {
            let deny_scope = if remember {
                Some(GrantScope::Workspace {
                    root: ctx.sandbox.workspace_root().to_path_buf(),
                })
            } else if matches!(violation.replay, Replay::Resume { .. }) {
                Some(GrantScope::Call)
            } else {
                None
            };
            if let Some(scope) = deny_scope
                && violation.is_grantable()
            {
                record_deny_row(ctx, violation, scope, now).await;
            }
            ViolationSettlement::Denied {
                model_text: text::kept_blocked(violation, followup.as_deref()),
            }
        }
        SandboxAnswer::Cancelled => ViolationSettlement::Denied {
            model_text: text::cancelled(violation),
        },
        SandboxAnswer::SubjectRefused { reason } => {
            tracing::warn!(
                call = %ctx.call,
                %reason,
                "the sandbox card's allow named a subject the violation does not tie to; keeping the denial"
            );
            ViolationSettlement::Denied {
                model_text: text::allowed_not_recorded(violation, &reason),
            }
        }
        SandboxAnswer::DecodeFailed => {
            tracing::warn!(
                call = %ctx.call,
                "the renderer could not decode the sandbox card (schema_version {}); the denial stands",
                crate::permission::sandbox_wire::SCHEMA_VERSION
            );
            ViolationSettlement::Denied {
                model_text: text::decode_failed(violation),
            }
        }
        SandboxAnswer::NoSandboxDecision => {
            tracing::warn!(
                call = %ctx.call,
                "the sandbox card was approved with no sandbox decision; the denial stands"
            );
            ViolationSettlement::Denied {
                model_text: text::no_sandbox_decision(violation),
            }
        }
    }
}
/// A violation the card can offer nothing for: settled `Denied` to the
/// model now; the card is posted informational and its acknowledgement is awaited off the tool
/// call, so a card nobody answers never holds the command's result.
fn settle_informational(
    ctx: SettleContext<'_>,
    violation: &Violation,
    ceiling: &Ceiling,
    proxy: EgressProxyState,
    transport: Arc<dyn PermissionHookTransport>,
) -> ViolationSettlement {
    let payload = build_sandbox_violation_payload(card_context(ctx, None), violation, ceiling);
    let call = ctx.call.clone();
    ctx.sandbox.spawn_owned(async move {
        if let Err(error) = transport.request_permission(payload).await {
            tracing::debug!(%call, %error, "informational sandbox card not acknowledged");
        }
    });
    ViolationSettlement::Denied {
        model_text: text::not_grantable(violation, proxy),
    }
}
fn card_context(ctx: SettleContext<'_>, deadline_unix: Option<i64>) -> SandboxCardContext<'_> {
    SandboxCardContext {
        tool_call_id: ctx.call.as_str(),
        command: ctx.command,
        policy: ctx.policy,
        mode: ctx.mode,
        backend: ctx.backend,
        reduced_sandbox: ctx.sandbox.reduced_sandbox(),
        deadline_unix,
    }
}
/// A deny row for the proposal, so the card is not shown again for the same target while the row
/// lives: "Always reject" records it for the workspace (seven days), a plain "Keep blocked" on a
/// held connection for the call. Best effort; a persist failure only means
/// the card returns.
async fn record_deny_row(
    ctx: SettleContext<'_>,
    violation: &Violation,
    scope: GrantScope,
    now: i64,
) {
    let Some(subject) = violation.proposed.clone() else {
        return;
    };
    let expires = if scope.is_persisted() {
        Expiry::Ttl {
            seconds: MAX_PERSISTED_TTL_SECONDS,
        }
    } else {
        Expiry::Never
    };
    let grant = Grant {
        id: new_grant_id(),
        subject,
        scope,
        expires,
        decision: GrantDecision::Deny,
        granted_at: now,
        granted_by: ctx.granted_by(),
        via: Some(provenance(ctx)),
    };
    if let Err(error) = ctx
        .sandbox
        .record_grant(ctx.call, ctx.epoch, ctx.session_id, grant)
        .await
    {
        tracing::warn!(call = %ctx.call, %error, "sandbox deny row not recorded");
    }
}
fn provenance(ctx: SettleContext<'_>) -> Provenance {
    Provenance {
        command: ctx
            .command
            .chars()
            .take(PROVENANCE_COMMAND_MAX_CHARS)
            .collect(),
        tool_call_id: ctx.call.as_str().to_owned(),
    }
}
fn new_grant_id() -> GrantId {
    GrantId::new(uuid::Uuid::now_v7().to_string())
}
/// The model-visible texts, one per way a violation settles.
/// Each says what the user answered or why nobody was asked, never what the
/// daemon did not do.
pub(crate) mod text {
    use super::EgressProxyState;
    use crate::permission::sandbox_wire::is_wire_expressible;
    use std::fmt::Display;
    use xai_grok_sandbox::command::Violation;
    use xai_grok_sandbox::command::grants::GrantSubject;
    use xai_grok_sandbox::command::violation::InformationalReason;
    /// The way out of a denial no grant can lift — an unproxied connection,
    /// a path the command never named: the same words the informational card shows.
    pub const RECOVERY_LINE: &str = "Run it in your terminal, or set the folder to `observe` in Settings (`[sandbox] mode = \"observe\"` in `.grok/workspaced.toml`).";
    /// Asked and denied.
    pub fn kept_blocked(violation: &Violation, followup: Option<&str>) -> String {
        let mut model_text = violation.kept_blocked_text();
        if let Some(followup) = followup {
            model_text.push_str(" User says: ");
            model_text.push_str(followup);
        }
        model_text
    }
    /// A live deny row covers the target: the user's "Always reject" answered it, unasked.
    pub fn rejected_by_row(violation: &Violation) -> String {
        violation
            .denied_text(
                "The user chose \"Always reject\" for this target, so nobody was asked again. Do not retry it; the user can revoke that choice in Settings.",
            )
    }
    /// Allowed, but the grant could not be recorded: the command did not run again.
    pub fn allowed_not_recorded(violation: &Violation, error: &impl Display) -> String {
        violation
            .denied_text(
                &format!(
            "User allowed it, but the grant could not be recorded ({error}); the command was not run again."
        ),
            )
    }
    /// A violation on the replay: the grant for `granted` was applied, the command stopped again.
    /// A second target means another ask on the next run; the same target means the grant did
    /// not cover what the command did.
    pub fn after_replay(violation: &Violation, granted: &GrantSubject) -> String {
        let same_target = violation.proposed.as_ref() == Some(granted);
        let granted = describe(granted);
        let settled = if same_target {
            format!(
                "The command was already run again under the user's grant for {granted}, which did not unblock it; it is not run a third time."
            )
        } else {
            format!(
                "The command was already run again under the user's grant for {granted} and then stopped here. Run it again to be asked about this one."
            )
        };
        violation.denied_text(&settled)
    }
    /// A grant subject as the model reads it.
    pub fn describe(subject: &GrantSubject) -> String {
        match subject {
            GrantSubject::FsWriteRoot { root } => {
                format!("writes under {}", root.display())
            }
            GrantSubject::FsRead { root } => format!("reads under {}", root.display()),
            GrantSubject::NetHost { host, port: Some(port) } => {
                format!("connections to {host}:{port}")
            }
            GrantSubject::NetHost { host, port: None } => {
                format!("connections to {host}")
            }
            GrantSubject::BuildCaches => {
                "the build caches for this workspace (cargo, npm, pnpm, Go, Gradle, Maven, ~/.cache, ~/Library/Caches)"
                    .to_owned()
            }
        }
    }
    /// No channel to the session owner: nobody was asked.
    pub fn no_transport(violation: &Violation) -> String {
        violation
            .denied_text(
                "Nobody was asked: there is no channel to the session owner. Stop and report this to the user.",
            )
    }
    /// The channel failed mid-ask: nobody answered.
    pub fn transport_failed(violation: &Violation, error: &impl Display) -> String {
        violation
            .denied_text(
                &format!(
            "Nobody answered: the permission prompt failed ({error}). Stop and report this to the user."
        ),
            )
    }
    /// The card was cancelled before an answer.
    pub fn cancelled(violation: &Violation) -> String {
        violation.denied_text("The permission prompt was cancelled before the user answered.")
    }
    /// The renderer could not decode the card: the user saw a deny-only
    /// notice, not the request.
    pub fn decode_failed(violation: &Violation) -> String {
        violation
            .denied_text(
                "The user's app could not display this request (it rejected the card as undecodable), so nobody was asked. Stop and report this to the user.",
            )
    }
    /// The reply approved without naming a sandbox decision: the answering app approves prompts
    /// without showing them, so nobody decided this one.
    pub fn no_sandbox_decision(violation: &Violation) -> String {
        violation
            .denied_text(
                "The permission reply approved it without a sandbox decision (the answering app approves prompts without showing them), so nothing was granted. Stop and report this to the user.",
            )
    }
    /// Nothing could be offered: the user was told, not
    /// asked. An informational network denial names the way out.
    pub fn not_grantable(violation: &Violation, proxy: EgressProxyState) -> String {
        let settled = match violation.disposition.reason() {
            None if !is_wire_expressible(violation) => {
                format!(
                "The denied path is not valid UTF-8, so the permission card cannot name it and no grant was offered; the user has been told. {RECOVERY_LINE}"
            )
            }
            Some(InformationalReason::ProtectedTarget) => {
                "This target is protected and can never be allowed; the user has been told."
                    .to_owned()
            }
            Some(InformationalReason::ProfileDeny) => {
                "The user's sandbox.toml denies this path, so no grant can open it; the user has been told. Do not retry it."
                    .to_owned()
            }
            Some(
                InformationalReason::UnproxiedNetwork,
            ) if proxy == EgressProxyState::Off => {
                format!(
                    "Network is off for this workspace (the local proxy is not running); grants cannot unblock it and the user has been told. {RECOVERY_LINE}"
                )
            }
            Some(InformationalReason::UnproxiedNetwork) => {
                format!(
                "This tool did not use the workspace proxy (it honours neither HTTP_PROXY nor HTTPS_PROXY, or connected on another port), so a host grant cannot unblock it; the user has been told. {RECOVERY_LINE}"
            )
            }
            Some(InformationalReason::PolicyDenylist) => {
                "Your organisation's policy denies this host; no grant can allow it and the user has been told. Do not retry it."
                    .to_owned()
            }
            Some(InformationalReason::Unattributed) => {
                format!(
                "The denied path is not one this command named (not its working directory, not a path in its arguments or environment, not the workspace), so no grant was offered; the user has been told. Name the path in the command (or its environment, e.g. `PYTHONUSERBASE`) to be offered a grant. {RECOVERY_LINE}"
            )
            }
            Some(InformationalReason::Capability)
            | Some(InformationalReason::DecodeFailed)
            | None => {
                "This can never be allowed from here; the user has been told.".to_owned()
            }
        };
        violation.denied_text(&settled)
    }
}
#[cfg(test)]
#[path = "sandbox_gate_tests.rs"]
mod tests;
