//! Plan mode lifecycle tracker for the Pi ↔ Grok adapter.
//!
//! Mirrors the semantics of Grok's native `PlanModeTracker`
//! (`xai-grok-shell/src/session/plan_mode.rs`) but adapted for the adapter's
//! stateless-RPC model: Pi has no mode concept, so the adapter owns the full
//! state machine and injects reminders as prompt prefixes.
//!
//! Design philosophy fusion:
//! - Grok: state-machine-as-truth, reminder-push model, full/sparse alternation
//! - Pi: hook-chain-as-control, per-turn recompute, no persistent mode
//! - Adapter: pure-protocol-translation → now also plan-mode-ownership

use std::path::{Path, PathBuf};

/// Plan mode lifecycle states (mirrors Grok `PlanModeState`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PiPlanState {
    /// Normal operating mode. No plan mode constraints.
    Inactive,
    /// Pager toggled plan mode ON, but no prompt has been sent yet.
    /// The model does not know about plan mode yet.
    Pending,
    /// Plan mode is active. The model has received plan mode instructions.
    /// Write tools are blocked except for the plan file.
    Active,
    /// Pager toggled plan mode OFF while a turn is in-flight.
    /// Wait for turn to finish, then cleanly exit.
    ExitPending,
}

/// Serializable snapshot for persistence across adapter restarts.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PiPlanSnapshot {
    pub state: PiPlanState,
    pub was_previously_active: bool,
    pub reminder_count: u32,
    pub pending_exit_reminder: bool,
    pub awaiting_plan_approval: bool,
}

/// Tracks the full plan mode lifecycle for a Pi session.
///
/// Pure state machine — no I/O, no async. The `PiAgent` calls methods at
/// appropriate points (set_session_mode, prompt, agent_settled).
pub struct PiPlanTracker {
    state: PiPlanState,
    /// Whether plan mode was previously active in this session (reentry detection).
    was_previously_active: bool,
    /// Counter for full/sparse reminder alternation. Even = full, odd = sparse.
    reminder_count: u32,
    /// Flag: inject exit reminder on the next prompt.
    pending_exit_reminder: bool,
    /// `exit_plan_mode` approval is outstanding.
    awaiting_plan_approval: bool,
    /// Absolute path to the plan file.
    plan_file_path: PathBuf,
}

impl PiPlanTracker {
    /// Create a tracker using a directory-local `plan.md` path.
    ///
    /// Kept for unit tests and callers which own a Grok-style per-session
    /// directory. Pi JSONL sessions use [`Self::with_plan_file`] instead.
    pub fn new(session_dir: PathBuf) -> Self {
        Self::with_plan_file(session_dir.join("plan.md"))
    }

    /// Create a tracker for an explicit session-private plan-file sidecar.
    pub fn with_plan_file(plan_file_path: PathBuf) -> Self {
        Self {
            state: PiPlanState::Inactive,
            was_previously_active: false,
            reminder_count: 0,
            pending_exit_reminder: false,
            awaiting_plan_approval: false,
            plan_file_path,
        }
    }

    /// Restore from a persisted snapshot. Transient states are collapsed:
    /// Pending → Inactive, ExitPending → Inactive (with exit reminder set).
    pub fn from_snapshot(session_dir: PathBuf, snapshot: PiPlanSnapshot) -> Self {
        Self::from_snapshot_with_plan_file(session_dir.join("plan.md"), snapshot)
    }

    /// Restore state for an explicit session-private plan-file sidecar.
    pub fn from_snapshot_with_plan_file(
        plan_file_path: PathBuf,
        mut snapshot: PiPlanSnapshot,
    ) -> Self {
        match snapshot.state {
            PiPlanState::Pending => {
                snapshot.state = PiPlanState::Inactive;
            }
            PiPlanState::ExitPending => {
                snapshot.state = PiPlanState::Inactive;
                snapshot.pending_exit_reminder = true;
            }
            _ => {}
        }
        Self {
            state: snapshot.state,
            was_previously_active: snapshot.was_previously_active,
            reminder_count: snapshot.reminder_count,
            pending_exit_reminder: snapshot.pending_exit_reminder,
            awaiting_plan_approval: snapshot.awaiting_plan_approval,
            plan_file_path,
        }
    }

    /// Capture current state as a persistable snapshot.
    pub fn snapshot(&self) -> PiPlanSnapshot {
        PiPlanSnapshot {
            state: self.state,
            was_previously_active: self.was_previously_active,
            reminder_count: self.reminder_count,
            pending_exit_reminder: self.pending_exit_reminder,
            awaiting_plan_approval: self.awaiting_plan_approval,
        }
    }

    // ─── Queries ───────────────────────────────────────────────────────────

    pub fn state(&self) -> PiPlanState {
        self.state
    }

    pub fn is_active(&self) -> bool {
        self.state == PiPlanState::Active
    }

    pub fn plan_file_path(&self) -> &Path {
        &self.plan_file_path
    }

    /// Whether the next reminder should be the full variant (even count = full).
    pub fn should_use_full_reminder(&self) -> bool {
        self.reminder_count.is_multiple_of(2)
    }

    /// Whether this is a reentry (was previously active this session).
    pub fn is_reentry(&self) -> bool {
        self.was_previously_active && self.state == PiPlanState::Pending
    }

    /// Whether plan mode was ever active earlier in this session.
    pub fn was_previously_active(&self) -> bool {
        self.was_previously_active
    }

    pub fn is_awaiting_plan_approval(&self) -> bool {
        self.awaiting_plan_approval
    }

    pub fn set_awaiting_plan_approval(&mut self, awaiting: bool) {
        self.awaiting_plan_approval = awaiting;
    }

    /// Whether the given edit path targets the plan file (auto-approve in plan mode).
    pub fn should_auto_approve_edit(&self, edit_path: &Path) -> bool {
        self.is_active() && edit_path == self.plan_file_path
    }

    // ─── Transitions ───────────────────────────────────────────────────────

    /// Pager toggled plan mode ON. Returns true if state changed.
    pub fn enter_pending(&mut self) -> bool {
        match self.state {
            PiPlanState::Inactive => {
                self.state = PiPlanState::Pending;
                self.pending_exit_reminder = false;
                true
            }
            PiPlanState::ExitPending => {
                // Re-entry: cancel deferred exit, return to Active.
                self.state = PiPlanState::Active;
                self.pending_exit_reminder = false;
                true
            }
            _ => false,
        }
    }

    /// First prompt while Pending — activate plan mode. Returns true if changed.
    pub fn activate(&mut self) -> bool {
        if self.state != PiPlanState::Pending {
            return false;
        }
        self.state = PiPlanState::Active;
        self.was_previously_active = true;
        self.reminder_count = 0;
        true
    }

    /// Agent called `enter_plan_mode`: enter `Active` immediately (idempotent).
    /// Returns true if the tracker transitioned into `Active`.
    pub fn activate_from_tool(&mut self) -> bool {
        if self.state == PiPlanState::Active {
            return false;
        }
        self.state = PiPlanState::Active;
        self.was_previously_active = true;
        self.reminder_count = 0;
        self.pending_exit_reminder = false;
        self.awaiting_plan_approval = false;
        true
    }

    /// Pager toggled plan mode OFF. `turn_in_flight`: whether a turn is running.
    pub fn user_exit(&mut self, turn_in_flight: bool) {
        self.awaiting_plan_approval = false;
        match self.state {
            PiPlanState::Pending => {
                self.state = PiPlanState::Inactive;
            }
            PiPlanState::Active => {
                if turn_in_flight {
                    self.state = PiPlanState::ExitPending;
                } else {
                    self.state = PiPlanState::Inactive;
                    self.pending_exit_reminder = true;
                }
            }
            _ => {}
        }
    }

    /// Turn completed while in ExitPending → transition to Inactive.
    pub fn complete_deferred_exit(&mut self) {
        if self.state != PiPlanState::ExitPending {
            return;
        }
        self.state = PiPlanState::Inactive;
        self.pending_exit_reminder = true;
    }

    /// `exit_plan_mode` approved by user. Returns true if state changed.
    pub fn deactivate_approved(&mut self) -> bool {
        if self.state != PiPlanState::Active {
            return false;
        }
        self.state = PiPlanState::Inactive;
        self.reminder_count = 0;
        self.awaiting_plan_approval = false;
        true
    }

    /// Called after injecting a per-turn reminder. Advances the counter.
    pub fn record_reminder_injected(&mut self) {
        self.reminder_count += 1;
    }

    /// Called after injecting the exit reminder. Clears the flag.
    pub fn clear_pending_exit_reminder(&mut self) {
        self.pending_exit_reminder = false;
    }

    pub fn has_pending_exit_reminder(&self) -> bool {
        self.pending_exit_reminder
    }

    /// Called after compaction. Resets reminder counter.
    pub fn reset_after_compaction(&mut self) {
        if self.state == PiPlanState::Active {
            self.reminder_count = 0;
        }
    }

    // ─── Reminder Rendering ────────────────────────────────────────────────

    /// Build the prompt-prefix reminder for the current state.
    /// Returns `None` if no reminder should be injected this turn.
    ///
    /// Call order matters:
    /// 1. If Pending → activate + return activation reminder
    /// 2. If Active → return full/sparse per-turn reminder
    /// 3. If pending_exit_reminder → return exit reminder (one-shot)
    pub fn build_reminder_for_prompt(&mut self) -> Option<String> {
        // Case 1: Activation (Pending → Active)
        if self.state == PiPlanState::Pending {
            let is_reentry = self.is_reentry();
            self.activate();
            self.record_reminder_injected();
            let plan_path = self.plan_file_path.display().to_string();
            let template = if is_reentry {
                plan_mode_reentry_reminder()
            } else {
                plan_mode_full_reminder()
            };
            return Some(format_reminder(template, &plan_path));
        }

        // Case 2: Per-turn reminder (Active)
        if self.state == PiPlanState::Active {
            let use_full = self.should_use_full_reminder();
            self.record_reminder_injected();
            let plan_path = self.plan_file_path.display().to_string();
            let template = if use_full {
                plan_mode_full_reminder()
            } else {
                plan_mode_sparse_reminder()
            };
            return Some(format_reminder(template, &plan_path));
        }

        // Case 3: Exit reminder (one-shot after user exit or approved exit)
        if self.pending_exit_reminder {
            self.clear_pending_exit_reminder();
            return Some(format!(
                "<system-reminder>\n{}\n</system-reminder>",
                plan_mode_exit_reminder()
            ));
        }

        None
    }
}

// ─── Reminder Templates ──────────────────────────────────────────────────────
// Mirrors Grok's plan_mode.rs templates, adapted for prompt-prefix injection.

fn plan_mode_full_reminder() -> &'static str {
    "Plan mode is active. Do not make any edits or writes to the system, and do not run bash or mutating subagents (only capability_mode: \"read-only\" subagents are allowed for parallel exploration).\n\n\
     ## Plan File:\n\
     Write a concise, actionable plan to {plan_path} using the write or edit tool.\n\
     This is the only file you are allowed to edit.\n\
     Include: Goal, Ordered Steps (with concrete files/symbols), Write Scopes (if delegating to Subagents V2), Verification commands, and a short `Re-plan if:` trigger list.\n\n\
     Once the plan is written and no blocking requirement questions remain (use ask_user_question only if user input is needed), proactively call exit_plan_mode in this same turn rather than pasting the plan in chat and stopping."
}

fn plan_mode_sparse_reminder() -> &'static str {
    "Plan mode is still active. Do not make any edits or writes to the system \
     except for the plan file (mutating subagents are blocked unless capability_mode is \"read-only\"). \
     Once the plan is ready and no blocking question remains, call exit_plan_mode in this same turn."
}

fn plan_mode_reentry_reminder() -> &'static str {
    "## Returning to Plan Mode\n\n\
     You are entering plan mode again after having previously exited it.\n\
     A plan file exists at {plan_path} from your previous planning session.\n\n\
     Read and update {plan_path} with the revised steps, Write Scopes, and Re-plan if: triggers, then ask clarifying questions only if blocked or proactively call exit_plan_mode in this same turn."
}

fn plan_mode_exit_reminder() -> &'static str {
    "You have exited plan mode. You can now make edits, run tools, and take actions. Follow the approved plan; if execution hits a Re-plan if condition or architectural blocker, call enter_plan_mode to re-plan before guessing."
}

/// Rejection message for a write outside the plan file during plan mode.
pub fn plan_mode_edit_rejected(plan_path: &Path) -> String {
    format!(
        "Rejected: file edits are not allowed in plan mode - the only editable file is the plan file ({}).",
        plan_path.display()
    )
}

fn format_reminder(template: &str, plan_path: &str) -> String {
    let body = template.replace("{plan_path}", plan_path);
    format!("<system-reminder>\n{body}\n</system-reminder>")
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_tracker() -> PiPlanTracker {
        PiPlanTracker::new(PathBuf::from("/tmp/test-pi-session"))
    }

    #[test]
    fn user_initiated_lifecycle() {
        let mut t = test_tracker();
        assert_eq!(t.state(), PiPlanState::Inactive);

        // Toggle ON
        assert!(t.enter_pending());
        assert_eq!(t.state(), PiPlanState::Pending);

        // First prompt activates
        let reminder = t.build_reminder_for_prompt();
        assert!(reminder.is_some());
        assert!(reminder.unwrap().contains("Plan mode is active"));
        assert_eq!(t.state(), PiPlanState::Active);

        // Subsequent prompts get per-turn reminders
        let r2 = t.build_reminder_for_prompt();
        assert!(r2.is_some());
        // reminder_count was 1 after activation (odd) → sparse
        assert!(r2.unwrap().contains("still active"));

        // User exit (no turn in flight)
        t.user_exit(false);
        assert_eq!(t.state(), PiPlanState::Inactive);
        assert!(t.has_pending_exit_reminder());

        // Next prompt gets exit reminder
        let r3 = t.build_reminder_for_prompt();
        assert!(r3.is_some());
        assert!(r3.unwrap().contains("exited plan mode"));
        assert!(!t.has_pending_exit_reminder());
    }

    #[test]
    fn user_exit_while_turn_in_flight() {
        let mut t = test_tracker();
        t.enter_pending();
        t.build_reminder_for_prompt(); // activate
        assert_eq!(t.state(), PiPlanState::Active);

        // User exit with turn in flight
        t.user_exit(true);
        assert_eq!(t.state(), PiPlanState::ExitPending);

        // Turn completes
        t.complete_deferred_exit();
        assert_eq!(t.state(), PiPlanState::Inactive);
        assert!(t.has_pending_exit_reminder());
    }

    #[test]
    fn pending_cancel_is_clean() {
        let mut t = test_tracker();
        t.enter_pending();
        assert_eq!(t.state(), PiPlanState::Pending);

        // User toggles off before any prompt
        t.user_exit(false);
        assert_eq!(t.state(), PiPlanState::Inactive);
        assert!(!t.has_pending_exit_reminder());
    }

    #[test]
    fn reentry_detected() {
        let mut t = test_tracker();
        t.enter_pending();
        t.build_reminder_for_prompt(); // activate
        t.user_exit(false);
        t.build_reminder_for_prompt(); // consume exit reminder

        // Re-enter
        assert!(t.enter_pending());
        assert!(t.is_reentry());
        let reminder = t.build_reminder_for_prompt();
        assert!(reminder.unwrap().contains("Returning to Plan Mode"));
    }

    #[test]
    fn exit_pending_reentry_cancels_deferred_exit() {
        let mut t = test_tracker();
        t.enter_pending();
        t.build_reminder_for_prompt(); // activate
        t.user_exit(true); // turn in flight → ExitPending
        assert_eq!(t.state(), PiPlanState::ExitPending);

        // Re-enter while ExitPending
        assert!(t.enter_pending());
        assert_eq!(t.state(), PiPlanState::Active);
        assert!(!t.has_pending_exit_reminder());
    }

    #[test]
    fn reminder_alternation() {
        let mut t = test_tracker();
        t.enter_pending();

        // Activation = full (count 0 → even → full), then count becomes 1
        let r1 = t.build_reminder_for_prompt();
        assert!(r1.unwrap().contains("Plan mode is active"));

        // count=1 (odd) → sparse
        let r2 = t.build_reminder_for_prompt();
        assert!(r2.unwrap().contains("still active"));

        // count=2 (even) → full
        let r3 = t.build_reminder_for_prompt();
        assert!(r3.unwrap().contains("Plan mode is active"));

        // count=3 (odd) → sparse
        let r4 = t.build_reminder_for_prompt();
        assert!(r4.unwrap().contains("still active"));
    }

    #[test]
    fn plan_file_path_in_session_dir() {
        let t = test_tracker();
        assert_eq!(
            t.plan_file_path(),
            Path::new("/tmp/test-pi-session/plan.md")
        );
    }

    #[test]
    fn should_auto_approve_plan_file_edit() {
        let mut t = test_tracker();
        t.enter_pending();
        t.build_reminder_for_prompt(); // activate

        assert!(t.should_auto_approve_edit(Path::new("/tmp/test-pi-session/plan.md")));
        assert!(!t.should_auto_approve_edit(Path::new("/tmp/test-pi-session/other.md")));
        assert!(!t.should_auto_approve_edit(Path::new("/tmp/other/plan.md")));
    }

    #[test]
    fn snapshot_round_trip() {
        let mut t = test_tracker();
        t.enter_pending();
        t.build_reminder_for_prompt(); // activate

        let snapshot = t.snapshot();
        assert_eq!(snapshot.state, PiPlanState::Active);
        assert!(snapshot.was_previously_active);

        let restored =
            PiPlanTracker::from_snapshot(PathBuf::from("/tmp/test-pi-session"), snapshot);
        assert_eq!(restored.state(), PiPlanState::Active);
        assert!(restored.is_active());
    }

    #[test]
    fn snapshot_collapses_transient_states() {
        // Pending → Inactive on restore
        let snapshot = PiPlanSnapshot {
            state: PiPlanState::Pending,
            was_previously_active: false,
            reminder_count: 0,
            pending_exit_reminder: false,
            awaiting_plan_approval: false,
        };
        let restored = PiPlanTracker::from_snapshot(PathBuf::from("/tmp/s"), snapshot);
        assert_eq!(restored.state(), PiPlanState::Inactive);

        // ExitPending → Inactive + exit reminder
        let snapshot = PiPlanSnapshot {
            state: PiPlanState::ExitPending,
            was_previously_active: true,
            reminder_count: 2,
            pending_exit_reminder: false,
            awaiting_plan_approval: false,
        };
        let restored = PiPlanTracker::from_snapshot(PathBuf::from("/tmp/s"), snapshot);
        assert_eq!(restored.state(), PiPlanState::Inactive);
        assert!(restored.has_pending_exit_reminder());
    }

    #[test]
    fn deactivate_approved() {
        let mut t = test_tracker();
        t.enter_pending();
        t.build_reminder_for_prompt(); // activate
        t.set_awaiting_plan_approval(true);

        assert!(t.deactivate_approved());
        assert_eq!(t.state(), PiPlanState::Inactive);
        assert!(!t.is_awaiting_plan_approval());
    }

    #[test]
    fn edit_rejected_message() {
        let msg = plan_mode_edit_rejected(Path::new("/home/user/.pi/sessions/abc/plan.md"));
        assert!(msg.contains("Rejected"));
        assert!(msg.contains("plan.md"));
    }

    #[test]
    fn agent_initiated_activate_from_tool_transitions_and_is_idempotent() {
        let mut t = test_tracker();
        assert!(!t.was_previously_active());
        assert!(t.activate_from_tool());
        assert_eq!(t.state(), PiPlanState::Active);
        assert!(t.was_previously_active());
        assert!(
            !t.activate_from_tool(),
            "re-entering while Active is idempotent"
        );

        // ExitPending (mid-turn user toggle off) returns cleanly to Active.
        t.user_exit(true);
        assert_eq!(t.state(), PiPlanState::ExitPending);
        assert!(t.activate_from_tool());
        assert_eq!(t.state(), PiPlanState::Active);
        assert!(!t.has_pending_exit_reminder());

        // Pending (user toggle before prompt) collapses to Active without double entry.
        let mut t2 = test_tracker();
        assert!(t2.enter_pending());
        assert!(t2.activate_from_tool());
        assert_eq!(t2.state(), PiPlanState::Active);
    }

    #[test]
    fn reminders_encourage_proactive_exit_and_reentry() {
        assert!(plan_mode_full_reminder().contains("exit_plan_mode"));
        assert!(plan_mode_full_reminder().contains("Re-plan if:"));
        assert!(plan_mode_sparse_reminder().contains("exit_plan_mode"));
        assert!(plan_mode_reentry_reminder().contains("exit_plan_mode"));
        assert!(plan_mode_exit_reminder().contains("enter_plan_mode"));
    }
}
