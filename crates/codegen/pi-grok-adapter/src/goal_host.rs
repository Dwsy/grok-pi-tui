//! Minimal GoalHost for grok-pi External ACP (legacy update_goal path).
//!
//! Owns goal state and builds pager-facing `GoalUpdated` session notifications.
//! Full multi-agent classifier/planner/strategist is residual (see issue).

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const CONTINUATION_LOOP_WINDOW: Duration = Duration::from_secs(30);
const CONTINUATION_LOOP_LIMIT: usize = 3;
const CONTINUATION_LOOP_GUARD_MESSAGE: &str =
    "Auto-stopped after 3 goal continuations within 30 seconds to prevent a loop.";

/// Control file schema written by `extensions/pi-grok-goal`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GoalControl {
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
    last_snapshot: Option<GoalControl>,
    continuation_goal_id: Option<String>,
    continuation_sends: VecDeque<Instant>,
    continuation_guard_armed: bool,
}

impl GoalHost {
    pub fn new(control_path: PathBuf) -> Self {
        Self {
            control_path,
            started: None,
            last_snapshot: None,
            continuation_goal_id: None,
            continuation_sends: VecDeque::new(),
            continuation_guard_armed: false,
        }
    }

    pub fn control_path(&self) -> &Path {
        &self.control_path
    }

    pub fn load(&mut self) -> Option<GoalControl> {
        let raw = std::fs::read_to_string(&self.control_path).ok()?;
        let control: GoalControl = serde_json::from_str(&raw).ok()?;
        self.apply_control(control.clone());
        Some(control)
    }

    pub fn apply_control(&mut self, control: GoalControl) {
        if control.is_present() && self.started.is_none() {
            self.started = Some(Instant::now());
        }
        if !control.is_present() {
            self.started = None;
        }
        let same_active_goal = control.is_active()
            && self.continuation_goal_id.as_deref() == Some(control.goal_id.as_str());
        if !same_active_goal {
            self.continuation_sends.clear();
            self.continuation_guard_armed = false;
            self.continuation_goal_id = control.is_active().then(|| control.goal_id.clone());
        }
        self.last_snapshot = Some(control);
    }

    pub fn continuation_guard_armed(&self, control: &GoalControl) -> bool {
        control.is_active()
            && self.continuation_goal_id.as_deref() == Some(control.goal_id.as_str())
            && self.continuation_guard_armed
    }

    pub fn record_continuation(&mut self, control: &GoalControl) {
        self.record_continuation_at(control, Instant::now());
    }

    fn record_continuation_at(&mut self, control: &GoalControl, now: Instant) -> bool {
        if !control.is_active() {
            return false;
        }
        if self.continuation_goal_id.as_deref() != Some(control.goal_id.as_str()) {
            self.continuation_goal_id = Some(control.goal_id.clone());
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

    pub fn snapshot(&self) -> Option<&GoalControl> {
        self.last_snapshot.as_ref()
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
             with verifiable evidence. When done, call update_goal with completed=true\n\
             and a short summary. If blocked after repeated failures, call update_goal\n\
             with blocked_reason. Do not claim completion without running checks.\n\
             </system-reminder>",
            obj = control.objective
        )
    }
}

/// Parse `--budget N` and objective from `/goal` args.
pub fn parse_goal_set_args(args: &str) -> (String, Option<i64>) {
    let mut budget = None;
    let mut parts = Vec::new();
    let tokens: Vec<&str> = args.split_whitespace().collect();
    let mut i = 0;
    while i < tokens.len() {
        if tokens[i] == "--budget" {
            if let Some(n) = tokens.get(i + 1).and_then(|s| s.parse::<i64>().ok()) {
                budget = Some(n.max(0));
                i += 2;
                continue;
            }
        }
        parts.push(tokens[i]);
        i += 1;
    }
    (parts.join(" "), budget)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn active_control() -> GoalControl {
        GoalControl {
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
    fn parse_goal_set_args_budget() {
        let (obj, budget) = parse_goal_set_args("ship auth --budget 50000");
        assert_eq!(obj, "ship auth");
        assert_eq!(budget, Some(50000));
    }

    #[test]
    fn load_and_notify() {
        let mut f = NamedTempFile::new().unwrap();
        write!(
            f,
            r#"{{"goalId":"g1","objective":"done","status":"active","phase":"executing","tokenBaseline":0,"tokensUsed":10,"createdAt":"t"}}"#
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
    fn cleared_not_present() {
        let c = GoalControl {
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
