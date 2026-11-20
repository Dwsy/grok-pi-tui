use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use omp_shell::bash::trace::RunTrace;
use omp_shell::cancel::{AbortReason, CancelToken};
use omp_shell::{Shell, ShellRunOptions};

use crate::protocol::{HostEvent, MAX_OUTPUT_EVENT_BYTES, ProtocolError, RunRequest};
use crate::trace::project_trace;

/// Execute a single isolated `omp-shell` invocation and stream `HostEvent`s.
///
/// Each request creates a fresh `Shell::new(None)` so environment variables,
/// shell functions, and `cd` working directory changes never leak across calls.
pub async fn execute_request<F>(
    req: RunRequest,
    cancel_flag: Arc<AtomicBool>,
    mut emit: F,
) -> Result<(), ProtocolError>
where
    F: FnMut(HostEvent),
{
    let shell = Shell::new(None);
    let trace = req
        .trace
        .then(|| Arc::new(RunTrace::new(req.command.as_str())));

    let timeout_ms = req
        .timeout_ms
        .map(|ms| u32::try_from(ms).unwrap_or(u32::MAX));
    let mut cancel_token = CancelToken::new(timeout_ms);
    let abort_token = cancel_token.emplace_abort_token();

    let watcher_flag = Arc::clone(&cancel_flag);
    let watcher_abort = abort_token.clone();
    let watcher_done = Arc::new(AtomicBool::new(false));
    let watcher_done_clone = Arc::clone(&watcher_done);
    let signal_watcher = tokio::spawn(async move {
        while !watcher_done_clone.load(Ordering::Relaxed) {
            if watcher_flag.load(Ordering::Acquire) {
                watcher_abort.abort(AbortReason::Signal);
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });

    let (chunk_tx, chunk_rx) = flume::unbounded::<String>();
    let env_opt = (!req.env.is_empty()).then_some(req.env);

    let run_options = ShellRunOptions {
        command: req.command,
        cwd: Some(req.cwd),
        env: env_opt,
        timeout_ms: None,
        filesystem: None,
        pipe_taps: None,
        trace: trace.clone(),
        speech: None,
    };

    let run_fut = shell.run(run_options, Some(chunk_tx), cancel_token);
    tokio::pin!(run_fut);

    let run_result = loop {
        tokio::select! {
            res = &mut run_fut => {
                break res;
            }
            chunk = chunk_rx.recv_async() => {
                if let Ok(text) = chunk {
                    emit_bounded_output(&text, &mut emit);
                }
            }
        }
    };

    watcher_done.store(true, Ordering::Release);
    let _ = signal_watcher.await;

    while let Ok(text) = chunk_rx.try_recv() {
        emit_bounded_output(&text, &mut emit);
    }

    match run_result {
        Ok(result) => {
            if result.cancelled || result.timed_out || cancel_flag.load(Ordering::Acquire) {
                terminate_live_pids(&shell.pids()).await;
            }

            let trace_summary = trace.as_ref().map(|t| {
                let snap = t.snapshot();
                project_trace(&snap, result.exit_code)
            });

            emit(HostEvent::Completed {
                exit_code: result.exit_code,
                cancelled: result.cancelled,
                timed_out: result.timed_out,
                working_dir: result.working_dir,
                trace: trace_summary,
            });

            // Retain the per-call shell while external background jobs remain alive,
            // unless the invocation was cancelled or timed out.
            if !result.cancelled && !result.timed_out {
                wait_for_background_jobs(&shell, &cancel_flag).await;
            }

            Ok(())
        }
        Err(err) => {
            terminate_live_pids(&shell.pids()).await;
            Err(ProtocolError::new("execution_failed", err.to_string()))
        }
    }
}

fn emit_bounded_output<F>(text: &str, emit: &mut F)
where
    F: FnMut(HostEvent),
{
    if text.is_empty() {
        return;
    }
    if text.len() <= MAX_OUTPUT_EVENT_BYTES {
        emit(HostEvent::Output {
            text: text.to_string(),
        });
        return;
    }

    let mut start = 0;
    while start < text.len() {
        let mut end = (start + MAX_OUTPUT_EVENT_BYTES).min(text.len());
        while end > start && !text.is_char_boundary(end) {
            end -= 1;
        }
        if end == start {
            end = text.len();
        }
        emit(HostEvent::Output {
            text: text[start..end].to_string(),
        });
        start = end;
    }
}

async fn wait_for_background_jobs(shell: &Shell, cancel_flag: &AtomicBool) {
    loop {
        if cancel_flag.load(Ordering::Acquire) {
            terminate_live_pids(&shell.pids()).await;
            break;
        }
        let live = shell.live_background_job_count().await;
        if live == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[cfg(unix)]
async fn terminate_live_pids(pids: &[i32]) {
    if pids.is_empty() {
        return;
    }
    for &pid in pids {
        if pid > 0 {
            // SAFETY: Standard POSIX signal delivery to tracked child PIDs.
            unsafe {
                libc::kill(pid, libc::SIGTERM);
            }
        }
    }
    tokio::time::sleep(Duration::from_millis(120)).await;
    for &pid in pids {
        if pid > 0 {
            // SAFETY: Escalate to SIGKILL for any stubborn tracked child PID.
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
        }
    }
}

#[cfg(not(unix))]
async fn terminate_live_pids(_pids: &[i32]) {}
