//! Runs `rg` for the grep tool.
//!
//! [`RgArgs`] builds the `rg` command from the prepared inputs, detached from the
//! TTY with kill-on-drop. [`RgRunner`] spawns it, hands out stdout chunks for
//! [`CappedOutput`](crate::implementations::grok_build::grep::capped_output::CappedOutput),
//! and stops, drains and reaps the process. A stopped or timed-out `rg` is
//! always killed before anything waits on it.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::process::{Child, ChildStderr, ChildStdout, Command};

use crate::implementations::grok_build::grep::{GrepSearchInput, OutputMode};

/// After the line/byte budget is filled, how long to wait for one more byte to distinguish exact-fit (EOF) from
/// overflow. Must stay far below the tool wall-clock timeout: an unbounded probe can block until the outer timeout and
/// discard the already-buffered matches via `grep_timeout_output`.
const EXACT_FIT_PROBE_TIMEOUT: Duration = Duration::from_millis(100);

/// Cap on bytes read from `rg`'s stderr (always small in practice).
const MAX_STDERR_BYTES: u64 = 1_000_000;

const STDOUT_CHUNK_BYTES: usize = 8192;

/// `rg --max-columns`, with `--max-columns-preview`.
pub(crate) const MAX_COLUMNS: u32 = 1000;

/// `rg --max-filesize 5M` in bytes: larger files are skipped.
pub(crate) const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;

/// The prepared inputs `rg` runs with.
pub(crate) struct RgArgs<'a> {
    pub(crate) program: PathBuf,
    pub(crate) input: &'a GrepSearchInput,
    /// The output mode resolved by `prepare_grep`; `input.output_mode` is not
    /// re-read so the `None` default lives in one place.
    pub(crate) output_mode: &'a OutputMode,
    /// The search root, resolved from the model's `path`.
    pub(crate) root: &'a Path,
    /// `--glob` values in order.
    pub(crate) globs: &'a [String],
}

impl RgArgs<'_> {
    /// The `rg` command with piped stdout and stderr, detached from the TTY and
    /// killed on drop.
    pub(crate) fn into_command(self) -> Command {
        let RgArgs {
            program,
            input,
            output_mode,
            root,
            globs,
        } = self;
        let mut cmd = Command::new(program);
        cmd.arg("--heading")
            .arg("--with-filename")
            .arg("--line-number")
            .arg("--color=never")
            .arg("--max-columns")
            .arg(MAX_COLUMNS.to_string())
            .arg("--max-columns-preview");

        if input.case_insensitive {
            cmd.arg("--ignore-case");
        }

        for glob in globs {
            cmd.arg("--glob").arg(glob);
        }

        if let Some(t) = &input.r#type
            && !t.is_empty()
        {
            cmd.arg("--type").arg(t);
        }

        if input.multiline {
            cmd.arg("-U").arg("--multiline-dotall");
        }

        if let Some(c) = input.context
            && c > 0
        {
            cmd.arg("-C").arg(c.to_string());
        }
        if let Some(b) = input.before_context
            && b > 0
        {
            cmd.arg("-B").arg(b.to_string());
        }
        if let Some(a) = input.after_context
            && a > 0
        {
            cmd.arg("-A").arg(a.to_string());
        }

        match output_mode {
            OutputMode::FilesWithMatches => {
                cmd.arg("-l");
            }
            OutputMode::Count => {
                cmd.arg("-c");
            }
            OutputMode::Content => {}
        }

        cmd.arg("-e").arg(&input.pattern);
        cmd.arg(root.to_string_lossy().as_ref());
        cmd.arg("--max-filesize").arg("5M");

        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        crate::util::detach_search_command(&mut cmd);
        cmd
    }
}

/// A spawned `rg` and its pipes.
pub(crate) struct RgRunner {
    child: Child,
    stdout: Option<ChildStdout>,
    stderr: Option<ChildStderr>,
    /// Heap-allocated so `RgRunner` stays small to move through `GrepStep` and
    /// the futures that hold it.
    chunk: Box<[u8]>,
}

impl RgRunner {
    /// Spawn a command built by [`RgArgs::into_command`].
    ///
    /// # Errors
    ///
    /// Returns the OS error when the process cannot be spawned.
    pub(crate) fn spawn(mut command: Command) -> io::Result<RgRunner> {
        #[allow(clippy::disallowed_methods)]
        // search helper; killed and reaped with a bound on timeout/truncation,
        // abandoned to the orphan reaper if unreapable (D-state)
        let mut child = command.spawn()?;

        // Take pipes so the child remains accessible for cleanup on timeout.
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        Ok(RgRunner {
            child,
            stdout,
            stderr,
            chunk: vec![0; STDOUT_CHUNK_BYTES].into_boxed_slice(),
        })
    }

    /// The next chunk of stdout; `None` at EOF or on a read error. Cancel-safe.
    pub(crate) async fn read_stdout(&mut self) -> Option<&[u8]> {
        let stdout = self.stdout.as_mut()?;
        match stdout.read(&mut self.chunk).await {
            Ok(0) | Err(_) => None,
            Ok(n) => self.chunk.get(..n),
        }
    }

    /// Exact-fit probe once a budget filled on a chunk boundary: `false` only when
    /// stdout reaches EOF within [`EXACT_FIT_PROBE_TIMEOUT`].
    pub(crate) async fn has_more_stdout(&mut self) -> bool {
        let Some(stdout) = self.stdout.as_mut() else {
            return false;
        };
        match tokio::time::timeout(EXACT_FIT_PROBE_TIMEOUT, stdout.read(&mut self.chunk)).await {
            Ok(Ok(0)) => false,
            // Silence counts as overflow too: the caller kills `rg` and keeps the
            // buffer instead of escalating to the timeout path that drops matches.
            Ok(Ok(_)) | Ok(Err(_)) | Err(_) => true,
        }
    }

    /// Kill `rg` without reaping it. Call before [`RgRunner::drain_stderr`] when
    /// reading stopped early: `rg` may still be walking the tree and only notices
    /// the closed stdout on its next write, so draining first would stall until
    /// the deadline.
    pub(crate) fn stop(&mut self) {
        if let Err(error) = self.child.start_kill() {
            tracing::debug!(%error, "killing rg failed");
        }
    }

    /// Drain stderr until EOF. After an early stop, call [`RgRunner::stop`] first.
    pub(crate) async fn drain_stderr(&mut self, stderr: &mut Vec<u8>) {
        if let Some(pipe) = self.stderr.take()
            && let Err(error) = pipe.take(MAX_STDERR_BYTES).read_to_end(stderr).await
        {
            tracing::debug!(%error, "reading rg stderr failed");
        }
    }

    /// `rg`'s exit code. Truncated output means `rg` was already killed: bounded
    /// reap, and the exit code is defined as 0. A natural EOF means `rg` is exiting,
    /// so the plain wait is prompt; -1 when the code is unknown.
    pub(crate) async fn exit_code(&mut self, is_truncated: bool) -> i32 {
        if is_truncated {
            crate::util::reap_killed_search_child(&mut self.child).await;
            0
        } else {
            self.child
                .wait()
                .await
                .ok()
                .and_then(|s| s.code())
                .unwrap_or(-1)
        }
    }

    /// Kill and reap `rg` after the tool deadline passed.
    pub(crate) async fn kill_and_reap(&mut self) {
        self.stop();
        crate::util::reap_killed_search_child(&mut self.child).await;
    }
}

#[cfg(test)]
#[path = "rg_runner_tests.rs"]
mod tests;
