//! The capped log file that receives exec handler output.
//!
//! Policy: the file holds the most recent trigger only. It is truncated when a trigger with exec handlers starts, and
//! once it reaches [`EXEC_LOG_CAP_BYTES`] (marker included) further output of that trigger is dropped, so the file
//! never exceeds the cap. Output keeps flowing through the pipes either way, so a chatty handler never blocks.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::time::Instant;

/// Hard cap on the log file size.
pub(crate) const EXEC_LOG_CAP_BYTES: usize = 64 * 1024;

/// Bound on truncating the log file when a trigger starts.
const LOG_IO_TIMEOUT: Duration = Duration::from_secs(1);

const TRUNCATION_MARKER: &[u8] = b"\n[lifecycle] output past the 64 KiB log cap dropped\n";

#[derive(Debug)]
pub(crate) struct ExecLog {
    path: PathBuf,
    sink: tokio::sync::Mutex<Option<LogSink>>,
    /// Set by [`ExecLog::finish_trigger`], cleared by [`ExecLog::start_trigger`]. An append that finds it set closes the
    /// sink instead of writing, so a close that lost the `try_lock` race still happens.
    is_finished: AtomicBool,
}

#[derive(Debug)]
struct LogSink {
    file: tokio::fs::File,
    written: usize,
    is_capped: bool,
}

impl ExecLog {
    pub(crate) fn new(path: PathBuf) -> Self {
        ExecLog {
            path,
            sink: tokio::sync::Mutex::new(None),
            is_finished: AtomicBool::new(true),
        }
    }

    /// Truncates the file for a new trigger, giving up at `until` at the latest. On failure output is discarded for
    /// this trigger.
    pub(crate) async fn start_trigger(&self, until: Instant) {
        let until = until.min(Instant::now() + LOG_IO_TIMEOUT);
        let reset = tokio::time::timeout_at(until, async {
            let mut options = tokio::fs::OpenOptions::new();
            options.create(true).write(true).truncate(true);
            #[cfg(unix)]
            options.mode(0o600);
            let file = options.open(&self.path).await;
            let mut sink = self.sink.lock().await;
            *sink = None;
            self.is_finished.store(false, Ordering::Release);
            let file = file?;
            *sink = Some(LogSink {
                file,
                written: 0,
                is_capped: false,
            });
            std::io::Result::Ok(())
        })
        .await;
        match reset {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                tracing::warn!(path = %self.path.display(), error = %e, "lifecycle exec log unavailable; discarding handler output");
            }
            Err(_elapsed) => {
                tracing::warn!(path = %self.path.display(), "opening the lifecycle exec log timed out; discarding handler output");
            }
        }
    }

    /// Closes the file without waiting; output that arrives later is dropped. Every append already flushed. When an
    /// append holds the sink right now, the next append closes it instead.
    pub(crate) fn finish_trigger(&self) {
        self.is_finished.store(true, Ordering::Release);
        if let Ok(mut sink) = self.sink.try_lock() {
            *sink = None;
        }
    }

    pub(crate) async fn append(&self, bytes: &[u8]) {
        let mut guard = self.sink.lock().await;
        if self.is_finished.load(Ordering::Acquire) {
            *guard = None;
            return;
        }
        let Some(sink) = guard.as_mut() else {
            return;
        };
        if sink.is_capped {
            return;
        }
        let room = EXEC_LOG_CAP_BYTES
            .saturating_sub(TRUNCATION_MARKER.len())
            .saturating_sub(sink.written);
        let (chunk, is_capped) = match bytes.get(..room) {
            Some(head) if head.len() < bytes.len() => (head, true),
            Some(_) | None => (bytes, false),
        };
        let mut result = sink.file.write_all(chunk).await;
        if is_capped && result.is_ok() {
            result = sink.file.write_all(TRUNCATION_MARKER).await;
        }
        if result.is_ok() {
            result = sink.file.flush().await;
        }
        match result {
            Ok(()) => {
                sink.written += chunk.len();
                sink.is_capped = is_capped;
            }
            Err(e) => {
                tracing::warn!(error = %e, "writing the lifecycle exec log failed; discarding further output");
                *guard = None;
            }
        }
    }
}

#[cfg(test)]
#[path = "exec_log_tests.rs"]
mod tests;
