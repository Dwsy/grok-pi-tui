use super::*;
use crate::pi_rpc::PiRpcTimeout;

/// Backoff before each respawn attempt within one recovery round.
const RESTART_BACKOFF: [Duration; 3] = [
    Duration::from_millis(500),
    Duration::from_secs(1),
    Duration::from_secs(2),
];

/// Deadline for the full re-bootstrap (`get_state` + models + commands) after
/// a respawn. Matches the launcher's `PI_BOOTSTRAP_TIMEOUT`.
const RESTART_BOOTSTRAP_DEADLINE: Duration = Duration::from_secs(60);

/// Heartbeat cadence and hang thresholds. A hang is only declared after
/// `HEARTBEAT_MAX_MISSES` consecutive deadline misses (~2 minutes of sustained
/// unresponsiveness), so long synchronous stretches in Pi (deep tree
/// serialization, giant session loads) do not trigger false restarts.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
const HEARTBEAT_DEADLINE: Duration = Duration::from_secs(15);
const HEARTBEAT_MAX_MISSES: u32 = 4;

/// Crash-storm guard: how many automatic recoveries are allowed per window
/// before the adapter stops respawning and asks for a manual restart.
const RECOVERY_WINDOW: Duration = Duration::from_secs(300);
const MAX_RECOVERIES_PER_WINDOW: usize = 3;

/// Sliding-window counter for automatic RPC recoveries. Purely local policy —
/// it never touches Pi state.
#[derive(Default)]
pub(super) struct RpcRecoveryTracker {
    recent: Vec<Instant>,
}

impl RpcRecoveryTracker {
    /// Register a recovery round starting at `now`. Returns `false` when the
    /// storm guard is exhausted and automatic recovery must stop.
    pub(super) fn try_begin(&mut self, now: Instant) -> bool {
        self.recent
            .retain(|started| now.duration_since(*started) < RECOVERY_WINDOW);
        if self.recent.len() >= MAX_RECOVERIES_PER_WINDOW {
            return false;
        }
        self.recent.push(now);
        true
    }
}

impl PiAgent {
    /// Bring the Pi RPC connection back after an unexpected child exit:
    /// respawn from the original launch config, re-bootstrap, and re-attach
    /// the session that was active before the crash.
    pub(super) async fn recover_rpc_connection(&self) {
        if !self
            .state
            .borrow_mut()
            .rpc_recovery
            .try_begin(Instant::now())
        {
            self.send_ui_notification(
                "Pi RPC crashed repeatedly; automatic restart is paused. Quit and relaunch grok-pi.",
                Some("error"),
            )
            .await;
            return;
        }
        let attempts = RESTART_BACKOFF.len();
        for (index, backoff) in RESTART_BACKOFF.iter().enumerate() {
            let attempt = index + 1;
            self.send_ui_notification(
                &format!("Pi RPC exited unexpectedly; restarting ({attempt}/{attempts})…"),
                Some("warning"),
            )
            .await;
            tokio::time::sleep(*backoff).await;
            match self.respawn_and_rebind().await {
                Ok(session_restored) => {
                    let message = if session_restored {
                        "Pi RPC restarted; session restored."
                    } else {
                        "Pi RPC restarted with a fresh session (previous session could not be reattached)."
                    };
                    self.send_ui_notification(message, Some("info")).await;
                    return;
                }
                Err(error) => {
                    tracing::warn!(%error, attempt, "Pi RPC recovery attempt failed");
                }
            }
        }
        self.send_ui_notification(
            "Pi RPC restart failed; quit and relaunch grok-pi.",
            Some("error"),
        )
        .await;
    }

    /// One respawn attempt: new child, fresh bootstrap, then re-attach the
    /// pre-crash session file. Returns whether the old session was restored.
    async fn respawn_and_rebind(&self) -> Result<bool> {
        let (session_file, session_id) = {
            let state = self.state.borrow();
            (
                state.bootstrap.state.session_file.clone(),
                state.acp_session_id.clone(),
            )
        };
        self.rpc.respawn().await?;
        let bootstrap =
            tokio::time::timeout(RESTART_BOOTSTRAP_DEADLINE, PiBootstrap::load(&self.rpc))
                .await
                .map_err(|_| anyhow!("Pi bootstrap timed out after restart"))??;
        // Bind to the fresh child's (new, empty) session first so the adapter
        // is consistent even when the old session cannot be reattached.
        self.replace_bootstrap(bootstrap);
        let mut restored = false;
        if let Some(path) = session_file
            .as_deref()
            .map(Path::new)
            .filter(|path| path.is_file())
        {
            match self.switch_session(path, &session_id).await {
                Ok(result) if !result.cancelled => restored = true,
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!(%error, session = %path.display(), "failed to reattach Pi session after restart");
                }
            }
        }
        let bootstrap = self.state.borrow().bootstrap.clone();
        self.publish_bootstrap(&bootstrap).await;
        self.refresh_context_usage().await;
        Ok(restored)
    }

    /// Watchdog for a live-but-wedged Pi child: cheap `get_state` heartbeats;
    /// after sustained deadline misses the child is killed *unintentionally*
    /// so the resulting exit event drives the normal crash-recovery path.
    ///
    /// Non-timeout failures reset the counter: a closed writer means the
    /// child already exited and the exit event owns recovery, while an RPC
    /// error response proves the process is responsive.
    pub(super) fn spawn_rpc_watchdog(self: Rc<Self>) {
        if !rpc_watchdog_enabled() {
            tracing::debug!("Pi RPC watchdog disabled via PI_GROK_RPC_WATCHDOG");
            return;
        }
        tokio::task::spawn_local(async move {
            let mut misses = 0u32;
            loop {
                tokio::time::sleep(HEARTBEAT_INTERVAL).await;
                match self
                    .rpc
                    .request_with_deadline(json!({ "type": "get_state" }), HEARTBEAT_DEADLINE)
                    .await
                {
                    Ok(_) => misses = 0,
                    Err(error) if error.downcast_ref::<PiRpcTimeout>().is_some() => {
                        misses += 1;
                        tracing::warn!(misses, "Pi RPC heartbeat missed its deadline");
                        if misses >= HEARTBEAT_MAX_MISSES {
                            misses = 0;
                            self.send_ui_notification(
                                "Pi RPC has been unresponsive for over 2 minutes; restarting it…",
                                Some("warning"),
                            )
                            .await;
                            self.rpc.kill_unresponsive().await;
                        }
                    }
                    Err(_) => misses = 0,
                }
            }
        });
    }
}

fn rpc_watchdog_enabled() -> bool {
    !matches!(
        std::env::var("PI_GROK_RPC_WATCHDOG").as_deref(),
        Ok("0") | Ok("false") | Ok("off")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storm_guard_allows_a_burst_then_pauses() {
        let mut tracker = RpcRecoveryTracker::default();
        let start = Instant::now();
        assert!(tracker.try_begin(start));
        assert!(tracker.try_begin(start + Duration::from_secs(10)));
        assert!(tracker.try_begin(start + Duration::from_secs(20)));
        assert!(
            !tracker.try_begin(start + Duration::from_secs(30)),
            "fourth crash inside the window must pause automatic recovery"
        );
    }

    #[test]
    fn storm_guard_recovers_after_the_window_expires() {
        let mut tracker = RpcRecoveryTracker::default();
        let start = Instant::now();
        for offset in [0, 10, 20] {
            assert!(tracker.try_begin(start + Duration::from_secs(offset)));
        }
        assert!(tracker.try_begin(start + RECOVERY_WINDOW + Duration::from_secs(21)));
    }
}
