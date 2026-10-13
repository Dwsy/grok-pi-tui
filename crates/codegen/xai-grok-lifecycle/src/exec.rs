//! Runs one exec handler (image or registered) for one trigger.
//!
//! The child is detached into its own session and process group, gets stdin from the null device and the
//! `GROK_LIFECYCLE_*` variables on top of the server's environment, and writes stdout and stderr through pipes into
//! the capped [`ExecLog`]. At its effective timeout (`min(timeout_ms, remaining trigger deadline)`) the whole group
//! gets SIGTERM, then SIGKILL [`TERM_GRACE`] later or [`KILL_SLACK`] past the trigger deadline, whichever comes first,
//! and the child is reaped. However the run ends, whatever is left in the group is killed, so background children never
//! outlive their handler. Nothing here outlives the trigger deadline by more than [`HANDLER_SLACK`].

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Child;
use tokio::time::Instant;
use tokio_util::task::AbortOnDropHandle;
use xai_tty_utils::{ProcessGroup, ProcessScope};

use crate::event::{HandlerOutcome, PreStopEvent};
use crate::exec_log::ExecLog;
use crate::exec_spec::ExecSpec;
use crate::token::HandlerName;
use crate::trigger::{RunOutcome, duration_ms};

/// Time between SIGTERM and SIGKILL when the trigger deadline leaves room for it.
pub(crate) const TERM_GRACE: Duration = Duration::from_secs(1);

/// SIGKILL goes out no later than this past the trigger deadline.
pub(crate) const KILL_SLACK: Duration = Duration::from_millis(100);

/// Reaping and the output drain end no later than this past the trigger deadline.
pub(crate) const HANDLER_SLACK: Duration = Duration::from_millis(150);

/// How long output may keep draining after the child exits (grandchildren can hold the pipes open).
const OUTPUT_DRAIN_GRACE: Duration = Duration::from_millis(50);

const OUTPUT_CHUNK_BYTES: usize = 8 * 1024;

pub(crate) const ENV_EVENT: &str = "GROK_LIFECYCLE_EVENT";
pub(crate) const ENV_REASON: &str = "GROK_LIFECYCLE_REASON";
pub(crate) const ENV_DISK: &str = "GROK_LIFECYCLE_DISK";
/// Milliseconds from spawn until the handler is cut: its own `timeout_ms` or the rest of the trigger deadline,
/// whichever is shorter.
pub(crate) const ENV_DEADLINE_MS: &str = "GROK_LIFECYCLE_DEADLINE_MS";
const EVENT_PRE_STOP: &str = "pre-stop";

/// Shared per-trigger resources of the exec runner.
#[derive(Clone)]
pub(crate) struct ExecContext {
    pub(crate) log: Arc<ExecLog>,
    pub(crate) processes: ProcessScope,
}

pub(crate) async fn run_exec(
    name: HandlerName,
    spec: ExecSpec,
    event: PreStopEvent,
    ctx: ExecContext,
) -> RunOutcome {
    let started = Instant::now();
    let deadline = event.deadline();
    let remaining = deadline.saturating_duration_since(started);
    if remaining.is_zero() {
        return RunOutcome::DeadlineCut;
    }
    let (stop_at, cut) = match spec.timeout() {
        Some(timeout) if timeout < remaining => (started + timeout, RunOutcome::HandlerTimeout),
        Some(_) | None => (deadline, RunOutcome::DeadlineCut),
    };
    let Some((program, args)) = spec.argv().split_first() else {
        return RunOutcome::Finished(HandlerOutcome::Failed);
    };
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args)
        .stdin(xai_tty_utils::null_stdio())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env(ENV_EVENT, EVENT_PRE_STOP)
        .env(ENV_REASON, event.reason().as_ref())
        .env(ENV_DISK, <&'static str>::from(event.disk()))
        .env(
            ENV_DEADLINE_MS,
            duration_ms(stop_at.saturating_duration_since(started)).to_string(),
        )
        .kill_on_drop(true);
    xai_tty_utils::detach_command(&mut cmd);

    #[allow(clippy::disallowed_methods)]
    // Enrolled in the broker's process scope right below; the group is killed on every exit path.
    let spawned = cmd.spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(e) => {
            tracing::warn!(handler = %name, error = %e, "lifecycle exec handler failed to spawn");
            ctx.log
                .append(format!("[lifecycle] {name}: spawn failed: {e}\n").as_bytes())
                .await;
            return RunOutcome::Finished(HandlerOutcome::Failed);
        }
    };
    // No await between spawn and enroll: a run dropped in that gap would kill only the leader, not its group.
    let group = GroupGuard(match ctx.processes.enroll(&child) {
        Ok(group) => Some(group),
        Err(e) => {
            tracing::warn!(handler = %name, error = %e, "lifecycle exec handler has no process group; only the leader can be killed");
            None
        }
    });
    ctx.log
        .append(format!("[lifecycle] {name}: started\n").as_bytes())
        .await;
    let output = AbortOnDropHandle::new(tokio::spawn(capture_output(
        Arc::clone(&ctx.log),
        child.stdout.take(),
        child.stderr.take(),
    )));

    let outcome = tokio::select! {
        status = child.wait() => match status {
            Ok(status) if status.success() => RunOutcome::Finished(HandlerOutcome::Ok),
            Ok(_) => RunOutcome::Finished(HandlerOutcome::Failed),
            Err(e) => {
                tracing::warn!(handler = %name, error = %e, "waiting for a lifecycle exec handler failed");
                RunOutcome::Finished(HandlerOutcome::Failed)
            }
        },
        () = tokio::time::sleep_until(stop_at) => {
            stop(&mut child, &group, deadline).await;
            cut
        }
    };
    // Kills whatever the handler left in its group. A group id is not reused while a member lives, so this cannot
    // reach an unrelated group even though the leader is already reaped.
    drop(group);

    let drain_until = (Instant::now() + OUTPUT_DRAIN_GRACE).min(deadline + HANDLER_SLACK);
    if tokio::time::timeout_at(drain_until, output).await.is_err() {
        tracing::debug!(handler = %name, "lifecycle exec output still open after exit; dropping the rest");
    }
    ctx.log
        .append(
            format!(
                "[lifecycle] {name}: {} after {} ms\n",
                <&'static str>::from(outcome.to_wire()),
                duration_ms(started.elapsed())
            )
            .as_bytes(),
        )
        .await;
    outcome
}

/// SIGTERM the group, give it [`TERM_GRACE`] (bounded by the trigger deadline), SIGKILL it, and reap the leader.
async fn stop(child: &mut Child, group: &GroupGuard, deadline: Instant) {
    if group.is_armed() {
        group.signal(ProcessGroup::terminate);
        let kill_at = (Instant::now() + TERM_GRACE).min(deadline + KILL_SLACK);
        if tokio::time::timeout_at(kill_at, child.wait())
            .await
            .is_err()
        {
            tracing::debug!("lifecycle exec handler outlived SIGTERM");
        }
        group.signal(ProcessGroup::kill);
    } else if let Err(e) = child.start_kill()
        && e.kind() != std::io::ErrorKind::InvalidInput
    {
        tracing::debug!(error = %e, "killing the lifecycle exec leader failed");
    }
    if tokio::time::timeout_at(deadline + HANDLER_SLACK, child.wait())
        .await
        .is_err()
    {
        tracing::warn!(
            "lifecycle exec handler not reaped by the deadline; leaving it to the runtime reaper"
        );
    }
}

async fn capture_output(
    log: Arc<ExecLog>,
    stdout: Option<tokio::process::ChildStdout>,
    stderr: Option<tokio::process::ChildStderr>,
) {
    tokio::join!(copy_to_log(&log, stdout), copy_to_log(&log, stderr));
}

async fn copy_to_log(log: &ExecLog, stream: Option<impl AsyncRead + Unpin>) {
    let Some(mut stream) = stream else {
        return;
    };
    let mut buf = vec![0; OUTPUT_CHUNK_BYTES];
    loop {
        match stream.read(&mut buf).await {
            Ok(0) => return,
            Ok(n) => log.append(buf.get(..n).unwrap_or_default()).await,
            Err(e) => {
                tracing::debug!(error = %e, "reading lifecycle exec output failed");
                return;
            }
        }
    }
}

/// Kills the handler's process group when the run ends: the leader exited or was cut, or the run was dropped mid-flight.
#[must_use]
struct GroupGuard(Option<Arc<ProcessGroup>>);

impl GroupGuard {
    fn is_armed(&self) -> bool {
        self.0.is_some()
    }

    /// A failure means the group already exited.
    fn signal(&self, send: fn(&ProcessGroup) -> std::io::Result<()>) {
        if let Some(group) = &self.0
            && let Err(e) = send(group)
        {
            tracing::debug!(error = %e, "signalling a lifecycle exec process group failed");
        }
    }
}

impl Drop for GroupGuard {
    fn drop(&mut self) {
        self.signal(ProcessGroup::kill);
    }
}

#[cfg(test)]
#[path = "exec_tests.rs"]
mod tests;
