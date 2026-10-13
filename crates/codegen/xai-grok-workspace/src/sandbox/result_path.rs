//! The hub's post-run step for a shell tool call: decode the finished command through
//! [`WorkspaceSandbox::finish`], settle a violation with the session owner, and either run the
//! call once more under the grant or hand the result back with the denial explained to the model.

use std::sync::Arc;

use futures::StreamExt;
use xai_grok_sandbox::command::CallId;
use xai_grok_sandbox::command::violation::CommandExit;
use xai_grok_tools::types::output::{ToolOutput, ToolRunResult};
use xai_tool_runtime::{ToolError, ToolErrorKind, ToolStream, ToolStreamItem};

use crate::handle::WorkspaceHandle;
use crate::permission::{
    PermissionHookTransport, SettleContext, ToolServerPermissionTransport, ViolationSettlement,
    settle_violation,
};
use crate::sandbox::{CallOwner, Finished, SandboxMode, WorkspaceSandbox};
use crate::session::WorkspaceSession;

/// What the hub does with a finished shell call.
pub(crate) enum AfterRun {
    /// Hand this result back (possibly with the denial appended to the prompt text).
    Keep(Box<ToolRunResult>),
    /// A grant was recorded: dispatch the same call once more (same call id) and pass its result
    /// through here again; the second violation, if any, is final.
    Replay,
}

/// How many more times the hub may dispatch one shell call under a sandbox grant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReplayBudget {
    /// The first run: a grant buys one replay.
    One,
    /// The replay ran (or the call may never replay): a further `Replay` is refused, not run.
    Spent,
}

impl ReplayBudget {
    /// Take the one replay; `false` when it is gone.
    pub(crate) fn spend(&mut self) -> bool {
        match self {
            ReplayBudget::One => {
                *self = ReplayBudget::Spent;
                true
            }
            ReplayBudget::Spent => false,
        }
    }
}

/// The marker the model sees in front of every sandbox note.
const NOTE_PREFIX: &str = "\n\n[sandbox] ";

/// Why a call the hub let past its pre-run approval on the enforce pin was refused at dispatch.
pub(crate) const PIN_LOST_TEXT: &str = "the command was not run: it skipped its approval because \
     the sandbox enforced, and the sandbox no longer holds it to that; run it again";

/// The transport a sandbox card for `session` travels: the sandbox's own when a host answers
/// cards itself, the hub's permission channel otherwise (`None` with no hub connected).
pub(crate) async fn card_transport(
    workspace: &WorkspaceHandle,
    session: &WorkspaceSession,
    sandbox: &WorkspaceSandbox,
) -> Option<Arc<dyn PermissionHookTransport>> {
    if let Some(transport) = sandbox.card_transport() {
        return Some(transport);
    }
    workspace
        .hub_server_blocking()
        .await
        .and_then(|server| {
            ToolServerPermissionTransport::from_session_id(server, session.session_id())
        })
        .map(|transport| Arc::new(transport) as Arc<dyn PermissionHookTransport>)
}

/// The hub's pre-dispatch step: remember who dispatched `call_id`, so a connection the proxy
/// holds while the command runs can be asked about. `command` is the tool's
/// `command` argument, what the card shows for a mid-command hold. A call the hub `pinned` binds
/// only while the sandbox still holds its pin: `false` means it must not run. A call the table
/// has no room for runs unbound: a spawn of it is refused at `prepare`, any other tool runs.
pub(crate) async fn bind_call_owner(
    workspace: &WorkspaceHandle,
    session: &WorkspaceSession,
    sandbox: &WorkspaceSandbox,
    call: &CallId,
    command: Option<&str>,
    pinned: bool,
) -> bool {
    let transport = card_transport(workspace, session, sandbox).await;
    let owner = CallOwner {
        session_id: session.session_id().to_owned(),
        policy: session.approval.policy(),
        transport,
        command: command.map(str::to_owned),
    };
    if pinned {
        return sandbox.bind_pinned_call(call, owner);
    }
    if let Err(error) = sandbox.bind_call(call, owner) {
        // Only a spawn needs the entry, and `prepare` refuses one the table has no room for;
        // a call that spawns nothing (stopping a background job) still runs
        tracing::warn!(%call, %error, "no room to bind the call; it runs unbound");
    }
    true
}

/// Releases everything the sandbox holds for a call ([`WorkspaceSandbox::release_call`]) when
/// its stream ends without [`after_shell_run`] having kept a result: an error, a refused replay,
/// no terminal, or the consumer dropping the stream mid-call or before its first poll. There is
/// no result for the refusals it drains to go to. A kept result disarms it — the result path
/// already released the call, and a background start keeps its entry and credential for the
/// process it left running ([`WorkspaceSandbox::detach_call`]).
struct CallRelease {
    sandbox: Option<Arc<WorkspaceSandbox>>,
    call: CallId,
    armed: bool,
}

impl CallRelease {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for CallRelease {
    fn drop(&mut self) {
        if let Some(sandbox) = &self.sandbox
            && self.armed
        {
            let dropped = sandbox.release_call(&self.call);
            if !dropped.is_empty() {
                tracing::info!(call = %self.call, notes = dropped.len(), "call ended without a result; its refusal notes go with it");
            }
        }
    }
}

/// Drive one shell call's stream through the sandbox's result path: a `Replay` dispatches the
/// same call once more, and a second one is refused as a terminal error rather than run. However
/// the stream ends, the call's record goes with it ([`CallRelease`]); a background start's
/// outlives it. `command` is what a card raised mid-command shows; `pinned`: the call runs only
/// while the sandbox's enforce pin holds and is refused ([`PIN_LOST_TEXT`]) once it is gone.
pub(crate) fn run_shell_call_with_replay<F>(
    workspace: WorkspaceHandle,
    session: Arc<WorkspaceSession>,
    call_id: String,
    command: Option<String>,
    pinned: bool,
    mut dispatch: F,
) -> ToolStream<ToolRunResult>
where
    F: FnMut() -> ToolStream<ToolRunResult> + Send + 'static,
{
    let call = CallId::tool(call_id.as_str());
    let release = CallRelease {
        sandbox: workspace.shared.sandbox(),
        call: call.clone(),
        armed: true,
    };
    Box::pin(async_stream::stream! {
        let mut release = release;
        // The grant files and `workspaced.toml` may have changed under us (a hand edit, another
        // daemon's revoke): stats before the spawn so this call runs under the rows and mode on
        // disk; the proxy follows the mode (started, kept or stopped) for every call
        let sandbox = workspace.shared.sandbox();
        if let Some(sandbox) = &sandbox
            && let Err(error) = sandbox.sync_network().await
        {
            tracing::warn!(call_id, %error, "the folder's egress proxy did not start; the command runs with the network off");
        }
        // Under `off` the call is neither bound nor run under re-read grant files, unless the hub
        // pinned or floored it (either outlives the flip); any other mode engages the folder first
        // and binds the session to the call, so a held connection has an owner to ask
        let on = sandbox.clone().filter(|sandbox| {
            pinned || sandbox.floor_of(&call).is_some() || sandbox.mode() != SandboxMode::Off
        });
        let bound = match &on {
            Some(sandbox) => {
                sandbox.engage_unless_off().await;
                sandbox.refresh_grants().await;
                bind_call_owner(&workspace, &session, sandbox, &call, command.as_deref(), pinned)
                    .await
            }
            None => !pinned,
        };
        if !bound {
            // SECURITY: fail closed — the hub skipped this call's approval on the enforce pin,
            // and without the pin its spawn would follow the folder's mode, `off` included
            tracing::error!(call_id, "the sandbox lost a pinned call's enforce pin; refusing");
            yield ToolStreamItem::Terminal(Err(ToolError::new(
                ToolErrorKind::PermissionDenied,
                PIN_LOST_TEXT,
            )));
            return;
        }
        let mut budget = ReplayBudget::One;
        let mut inner = dispatch();
        loop {
            let mut replay = false;
            while let Some(item) = inner.next().await {
                match item {
                    ToolStreamItem::Progress(p) => yield ToolStreamItem::Progress(p),
                    ToolStreamItem::Terminal(Ok(run_result)) => {
                        match after_shell_run(&workspace, &session, &call, run_result).await {
                            AfterRun::Keep(run_result) => {
                                // The result path released the call (or detached a background
                                // start's entry and credential): nothing is left for the guard
                                release.disarm();
                                yield ToolStreamItem::Terminal(Ok(*run_result));
                                return;
                            }
                            AfterRun::Replay if budget.spend() => {
                                replay = true;
                                break;
                            }
                            AfterRun::Replay => {
                                // SECURITY: fail closed if the gate ever asks for a third run
                                tracing::error!(
                                    call_id,
                                    "the sandbox asked to replay a command a second time; refusing"
                                );
                                yield ToolStreamItem::Terminal(Err(ToolError::new(
                                    ToolErrorKind::PermissionDenied,
                                    "the sandbox asked to replay a command a second time; refusing",
                                )));
                                return;
                            }
                        }
                    }
                    ToolStreamItem::Terminal(Err(e)) => {
                        yield ToolStreamItem::Terminal(Err(e));
                        return;
                    }
                }
            }
            if !replay {
                yield ToolStreamItem::Terminal(Err(ToolError::new(
                    ToolErrorKind::TerminalError,
                    "tool stream ended without a terminal",
                )));
                return;
            }
            inner = dispatch();
        }
    })
}

/// Run the sandbox's result path for one terminal tool result. Every `Keep` is the call's final
/// result: the call is released (or detached, for a background start whose child still runs) on
/// the way out, and the refusals its owner answered while it ran are appended to the result.
pub(crate) async fn after_shell_run(
    workspace: &WorkspaceHandle,
    session: &WorkspaceSession,
    call: &CallId,
    run_result: ToolRunResult,
) -> AfterRun {
    let Some(sandbox) = workspace.shared.sandbox() else {
        return AfterRun::Keep(Box::new(run_result));
    };
    let ToolOutput::Bash(bash) = &run_result.output else {
        if matches!(run_result.output, ToolOutput::BackgroundTaskStarted(_)) {
            // The command is still running: its entry and proxy credential stay with the process
            let notes = sandbox.detach_call(call, session.session_id());
            return AfterRun::Keep(Box::new(with_notes(run_result, &notes)));
        }
        // Any other shape never reaches the decoder, so a refusal inside it gets no card
        if sandbox.held_mode(call) == Some(SandboxMode::Enforce) {
            tracing::warn!(
                %call,
                session = %session.session_id(),
                "shell call yielded no decodable output under enforce; a sandbox denial in it gets no card"
            );
        }
        let notes = sandbox.release_call(call);
        return AfterRun::Keep(Box::new(with_notes(run_result, &notes)));
    };
    let exit = CommandExit {
        code: Some(bash.exit_code),
        signal: bash.signal.as_deref().and_then(signal_number),
    };
    let mut notes = Vec::new();
    match sandbox.finish(call, exit, &bash.output).await {
        Finished::Ran => {}
        // The backend has no one-line reason of its own yet; the default names the limitation
        Finished::RefusedUnderGrant(refused) => notes.push(refused.text(None)),
        Finished::Violation(violation) => {
            let transport = card_transport(workspace, session, &sandbox).await;
            let settlement = settle_violation(
                SettleContext {
                    sandbox: &sandbox,
                    policy: session.approval.policy(),
                    call,
                    // The post-run card: the call is as the table has it, no spawn of it running
                    epoch: None,
                    session_id: session.session_id(),
                    command: &bash.command,
                    mode: violation.mode,
                    backend: violation.backend,
                    replayed_under: violation.replayed_under.as_ref(),
                },
                violation.violation,
                transport,
            )
            .await;
            match settlement {
                ViolationSettlement::Replay { grant } => {
                    tracing::info!(%call, grant = %grant.id, "re-running the command under the new grant");
                    // The owner stays bound: the replayed run is the same call
                    return AfterRun::Replay;
                }
                ViolationSettlement::Denied { model_text } => notes.push(model_text),
                ViolationSettlement::Recorded => {}
            }
        }
    }
    notes.extend(sandbox.release_call(call));
    AfterRun::Keep(Box::new(with_notes(run_result, &notes)))
}

/// What the sandbox adds to the tool result, after the command's output.
fn with_notes(mut run_result: ToolRunResult, notes: &[String]) -> ToolRunResult {
    for note in notes {
        run_result.prompt_text.push_str(NOTE_PREFIX);
        run_result.prompt_text.push_str(note);
    }
    run_result
}

/// The terminal backend reports a signal death as `"signal N"`; the tool's own markers
/// (`timeout`, `killed`, `oom`, …) are not signals.
fn signal_number(signal: &str) -> Option<i32> {
    signal.strip_prefix("signal ")?.trim().parse().ok()
}

#[cfg(test)]
#[path = "result_path_tests.rs"]
mod tests;
