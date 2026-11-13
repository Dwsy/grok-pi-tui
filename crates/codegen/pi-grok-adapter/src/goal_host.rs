//! Minimal GoalHost for grok-pi External ACP (legacy goal tool path).
//!
//! Owns goal state and builds pager-facing `GoalUpdated` session notifications.
//! Full multi-agent classifier/planner/strategist is residual (see issue).

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const CONTINUATION_LOOP_WINDOW: Duration = Duration::from_secs(30);
const CONTINUATION_LOOP_LIMIT: usize = 3;
const CONTINUATION_LOOP_GUARD_MESSAGE: &str =
    "Auto-stopped after 3 goal continuations within 30 seconds to prevent a loop.";

/// Control file schema written by `extensions/pi-grok-goal`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GoalControl {
    /// Pi session identity. Empty legacy controls are never replayed across sessions.
    #[serde(default)]
    pub session_id: String,
    pub goal_id: String,
    pub objective: String,
    /// `active` | `user_paused` | `blocked` | `complete` | `cleared`
    pub status: String,
    /// `idle` | `planning` | `executing`
    #[serde(default = "default_phase")]
    pub phase: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_budget: Option<i64>,
    #[serde(default)]
    pub token_baseline: i64,
    #[serde(default)]
    pub tokens_used: i64,
    #[serde(default)]
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pause_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_event: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_event_detail: Option<String>,
}

fn default_phase() -> String {
    "executing".into()
}

impl GoalControl {
    pub fn cleared(session_id: String) -> Self {
        Self {
            session_id,
            goal_id: String::new(),
            objective: String::new(),
            status: "cleared".into(),
            phase: "idle".into(),
            token_budget: None,
            token_baseline: 0,
            tokens_used: 0,
            created_at: String::new(),
            pause_message: None,
            last_event: Some("goal_cleared".into()),
            last_event_detail: None,
        }
    }

    pub fn matches_session(&self, session_id: &str) -> bool {
        !session_id.is_empty() && self.session_id == session_id
    }

    pub fn is_active(&self) -> bool {
        self.status == "active"
    }

    pub fn is_present(&self) -> bool {
        self.status != "cleared" && !self.goal_id.is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct GoalHost {
    control_path: PathBuf,
    started: Option<Instant>,
    tracked_goal: Option<(String, String)>,
    continuation_goal_id: Option<(String, String)>,
    continuation_sends: VecDeque<Instant>,
    continuation_guard_armed: bool,
}

impl GoalHost {
    pub fn new(control_path: PathBuf) -> Self {
        Self {
            control_path,
            started: None,
            tracked_goal: None,
            continuation_goal_id: None,
            continuation_sends: VecDeque::new(),
            continuation_guard_armed: false,
        }
    }

    pub fn load(&mut self) -> Option<GoalControl> {
        let raw = std::fs::read_to_string(&self.control_path).ok()?;
        let control: GoalControl = serde_json::from_str(&raw).ok()?;
        self.apply_control(control.clone());
        Some(control)
    }

    /// Reject stale per-process IPC projections before they mutate the host
    /// timer or continuation guard, even when another session just switched.
    pub fn load_for_session(&mut self, session_id: &str) -> Option<GoalControl> {
        let raw = std::fs::read_to_string(&self.control_path).ok()?;
        let control: GoalControl = serde_json::from_str(&raw).ok()?;
        if !control.matches_session(session_id) {
            return None;
        }
        self.apply_control(control.clone());
        Some(control)
    }

    pub fn apply_control(&mut self, control: GoalControl) {
        let identity = control
            .is_present()
            .then(|| (control.session_id.clone(), control.goal_id.clone()));
        // A new goal (or switched Pi session) must never inherit the old
        // goal's elapsed clock or continuation-loop guard.
        if self.tracked_goal != identity {
            self.started = identity.as_ref().map(|_| Instant::now());
            self.tracked_goal = identity;
        }
        let same_active_goal = control.is_active()
            && self.continuation_goal_id.as_ref()
                == Some(&(control.session_id.clone(), control.goal_id.clone()));
        if !same_active_goal {
            self.continuation_sends.clear();
            self.continuation_guard_armed = false;
            self.continuation_goal_id = control
                .is_active()
                .then(|| (control.session_id.clone(), control.goal_id.clone()));
        }
    }

    pub fn reset_session(&mut self) {
        self.started = None;
        self.tracked_goal = None;
        self.continuation_goal_id = None;
        self.continuation_sends.clear();
        self.continuation_guard_armed = false;
    }

    pub fn continuation_guard_armed(&self, control: &GoalControl) -> bool {
        control.is_active()
            && self.continuation_goal_id.as_ref()
                == Some(&(control.session_id.clone(), control.goal_id.clone()))
            && self.continuation_guard_armed
    }

    pub fn record_continuation(&mut self, control: &GoalControl) {
        self.record_continuation_at(control, Instant::now());
    }

    fn record_continuation_at(&mut self, control: &GoalControl, now: Instant) -> bool {
        if !control.is_active() {
            return false;
        }
        if self.continuation_goal_id.as_ref()
            != Some(&(control.session_id.clone(), control.goal_id.clone()))
        {
            self.continuation_goal_id = Some((control.session_id.clone(), control.goal_id.clone()));
            self.continuation_sends.clear();
            self.continuation_guard_armed = false;
        }
        while self
            .continuation_sends
            .front()
            .is_some_and(|sent| now.duration_since(*sent) >= CONTINUATION_LOOP_WINDOW)
        {
            self.continuation_sends.pop_front();
        }
        self.continuation_sends.push_back(now);
        if self.continuation_sends.len() >= CONTINUATION_LOOP_LIMIT {
            self.continuation_guard_armed = true;
        }
        self.continuation_guard_armed
    }

    /// Prevent ACP cancel from being followed by an idle Goal auto-continuation.
    /// The Pi agent_settled(aborted) hook records the same pause in the durable
    /// session JSONL, not in a separate database.
    pub fn pause_for_cancel(&mut self, control: &GoalControl) -> std::io::Result<GoalControl> {
        let mut paused = control.clone();
        paused.status = "user_paused".into();
        paused.phase = "idle".into();
        paused.pause_message = Some("Execution cancelled. Use /goal resume to continue.".into());
        paused.last_event = Some("goal_paused".into());
        paused.last_event_detail = Some("cancelled".into());
        let raw = serde_json::to_string_pretty(&paused)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        std::fs::write(&self.control_path, raw)?;
        self.apply_control(paused.clone());
        Ok(paused)
    }

    pub fn block_continuation_loop(
        &mut self,
        control: &GoalControl,
    ) -> std::io::Result<GoalControl> {
        let mut blocked = control.clone();
        blocked.status = "blocked".into();
        blocked.phase = "idle".into();
        blocked.pause_message = Some(CONTINUATION_LOOP_GUARD_MESSAGE.into());
        blocked.last_event = Some("goal_paused".into());
        blocked.last_event_detail = Some("loop_guard".into());
        let raw = serde_json::to_string_pretty(&blocked)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        std::fs::write(&self.control_path, raw)?;
        self.apply_control(blocked.clone());
        Ok(blocked)
    }

    pub fn elapsed_ms(&self) -> u64 {
        self.started
            .map(|t| t.elapsed().as_millis() as u64)
            .unwrap_or(0)
    }

    /// Build `x.ai/session_notification` payload for GoalUpdated.
    pub fn notification_payload(&self, session_id: &str, control: &GoalControl) -> Value {
        let status = if control.status == "cleared" {
            "cleared"
        } else {
            control.status.as_str()
        };
        // SessionNotification: camelCase wrapper; SessionUpdate: snake_case fields + sessionUpdate tag.
        json!({
            "sessionId": session_id,
            "update": {
                "sessionUpdate": "goal_updated",
                "goal_id": control.goal_id,
                "objective": control.objective,
                "status": status,
                "phase": control.phase,
                "token_budget": control.token_budget,
                "tokens_used": control.tokens_used,
                "elapsed_ms": self.elapsed_ms(),
                "total_deliverables": 0,
                "completed_deliverables": 0,
                "total_worker_rounds": 0,
                "total_verify_rounds": 0,
                "token_baseline": control.token_baseline,
                "finished_subagent_tokens": 0,
                "last_event": control.last_event,
                "last_event_detail": control.last_event_detail,
                "pause_message": control.pause_message,
            }
        })
    }

    pub fn continuation_directive(control: &GoalControl) -> String {
        format!(
            "<system-reminder>\n\
             Goal still ACTIVE. Objective: {obj}\n\
             Continue working. Do not stop until the objective is fully achieved\n\
             with verifiable evidence. When done, call\n\
             goal({{ action: \"update\", completed: true, message: \"...\" }}).\n\
             If blocked after repeated failures, call\n\
             goal({{ action: \"update\", blocked_reason: \"...\" }}). Do not claim\n\
             completion without running checks.\n\
             </system-reminder>",
            obj = control.objective
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn active_control() -> GoalControl {
        GoalControl {
            session_id: "sess".into(),
            goal_id: "g".into(),
            objective: "x".into(),
            status: "active".into(),
            phase: "executing".into(),
            token_budget: None,
            token_baseline: 0,
            tokens_used: 0,
            created_at: String::new(),
            pause_message: None,
            last_event: None,
            last_event_detail: None,
        }
    }

    #[test]
    fn load_and_notify() {
        let mut f = NamedTempFile::new().unwrap();
        write!(
            f,
            r#"{{"sessionId":"sess","goalId":"g1","objective":"done","status":"active","phase":"executing","tokenBaseline":0,"tokensUsed":10,"createdAt":"t"}}"#
        )
        .unwrap();
        let mut host = GoalHost::new(f.path().to_path_buf());
        let c = host.load().expect("load");
        assert!(c.is_active());
        let payload = host.notification_payload("sess", &c);
        assert_eq!(payload["update"]["sessionUpdate"], "goal_updated");
        assert_eq!(payload["update"]["goal_id"], "g1");
        assert_eq!(payload["update"]["status"], "active");
    }

    #[test]
    fn continuation_guard_arms_after_three_sends_within_30_seconds() {
        let f = NamedTempFile::new().unwrap();
        let mut host = GoalHost::new(f.path().to_path_buf());
        let control = active_control();
        host.apply_control(control.clone());
        let start = Instant::now();
        assert!(!host.record_continuation_at(&control, start));
        assert!(!host.record_continuation_at(&control, start + Duration::from_secs(10)));
        assert!(host.record_continuation_at(&control, start + Duration::from_secs(20)));
        assert!(host.continuation_guard_armed(&control));

        let blocked = host.block_continuation_loop(&control).unwrap();
        assert_eq!(blocked.status, "blocked");
        assert_eq!(blocked.last_event_detail.as_deref(), Some("loop_guard"));
        let stored: GoalControl =
            serde_json::from_str(&std::fs::read_to_string(f.path()).unwrap()).unwrap();
        assert_eq!(stored.status, "blocked");
    }

    #[test]
    fn continuation_guard_expires_old_sends() {
        let f = NamedTempFile::new().unwrap();
        let mut host = GoalHost::new(f.path().to_path_buf());
        let control = active_control();
        host.apply_control(control.clone());
        let start = Instant::now();
        assert!(!host.record_continuation_at(&control, start));
        assert!(!host.record_continuation_at(&control, start + Duration::from_secs(20)));
        assert!(!host.record_continuation_at(&control, start + Duration::from_secs(31)));
        assert!(!host.continuation_guard_armed(&control));
    }

    #[test]
    fn stale_control_does_not_mutate_host_session_guard() {
        let mut f = NamedTempFile::new().unwrap();
        let mut control = active_control();
        control.goal_id = "first".into();
        serde_json::to_writer(&mut f, &control).unwrap();
        let mut host = GoalHost::new(f.path().to_path_buf());
        assert!(host.load_for_session("sess").is_some());
        let tick = Instant::now();
        host.record_continuation_at(&control, tick);
        host.record_continuation_at(&control, tick + Duration::from_secs(1));
        // A stale IPC projection for B must not reset A's continuation guard.
        control.session_id = "other".into();
        std::fs::write(f.path(), serde_json::to_vec(&control).unwrap()).unwrap();
        assert!(host.load_for_session("sess").is_none());
        let mut same_a = control.clone();
        same_a.session_id = "sess".into();
        assert!(host.record_continuation_at(&same_a, tick + Duration::from_secs(2)));
        assert!(host.continuation_guard_armed(&same_a));
    }

    #[test]
    fn goal_identity_and_cancel_are_scoped_to_session() {
        let f = NamedTempFile::new().unwrap();
        let mut host = GoalHost::new(f.path().to_path_buf());
        let a = active_control();
        assert!(a.matches_session("sess"));
        assert!(!a.matches_session("other"));
        host.apply_control(a.clone());
        let tick = Instant::now();
        host.record_continuation_at(&a, tick);
        host.record_continuation_at(&a, tick + Duration::from_secs(1));
        let mut b = a.clone();
        b.session_id = "other".into();
        host.apply_control(b.clone());
        assert!(!host.continuation_guard_armed(&b));
        host.record_continuation_at(&b, tick + Duration::from_secs(2));
        assert!(!host.continuation_guard_armed(&b));
        let paused = host.pause_for_cancel(&b).unwrap();
        assert_eq!(paused.status, "user_paused");
        assert_eq!(paused.last_event_detail.as_deref(), Some("cancelled"));
        let restored: GoalControl =
            serde_json::from_str(&std::fs::read_to_string(f.path()).unwrap()).unwrap();
        assert!(restored.matches_session("other"));
        assert_eq!(restored.status, "user_paused");
    }

    #[test]
    fn cleared_not_present() {
        let c = GoalControl {
            session_id: "sess".into(),
            goal_id: "g".into(),
            objective: "x".into(),
            status: "cleared".into(),
            phase: "idle".into(),
            token_budget: None,
            token_baseline: 0,
            tokens_used: 0,
            created_at: String::new(),
            pause_message: None,
            last_event: None,
            last_event_detail: None,
        };
        assert!(!c.is_present());
    }
}
