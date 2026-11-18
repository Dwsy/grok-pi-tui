//! Program Status Protocol (OSC 7501).
//!
//! Pi reports program status from its interactive mode only: `ProgramStatusReporter`
//! lives in `modes/interactive/`, and the RPC mode grok-pi drives never calls
//! `Terminal.setProgramStatus`. The Pager therefore derives the report from the same
//! state it already feeds the tab title, so one tick can never claim `working` in the
//! title while the terminal is told `idle`.
//!
//! Wire format: `ESC ] 7501 ; state=<s>[:app=<a>][:kind=<k>][:msg=<base64>] ST`.
//! `state=clear` removes a previously reported status instead of setting one.

use base64::Engine as _;

use super::title::TitleState;
use crate::acp::tracker::TurnActivity;

/// Stable program name for `app=`. The spec allows `[A-Za-z0-9_.+-]{1,32}`.
const APP_NAME: &str = "grok-pi";

/// Decoded `msg` limit from the spec; its base64 form stays under the 2732-byte encoded cap.
const MAX_MESSAGE_BYTES: usize = 2048;

/// Removes the reported status without reporting a new one.
const CLEAR_ESCAPE: &str = "\x1b]7501;state=clear\x1b\\";

/// What a blocked program waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockedKind {
    Permission,
}

impl BlockedKind {
    fn label(self) -> &'static str {
        match self {
            Self::Permission => "permission",
        }
    }
}

/// One reported state. `done` and `error` describe the run that just settled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgramStatus {
    Idle,
    Working,
    Done,
    Error,
    Blocked { kind: BlockedKind },
}

impl ProgramStatus {
    fn state_label(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Done => "done",
            Self::Error => "error",
            Self::Blocked { .. } => "blocked",
        }
    }

    fn blocked_kind(self) -> Option<BlockedKind> {
        match self {
            Self::Blocked { kind } => Some(kind),
            _ => None,
        }
    }
}

/// Encode one report.
///
/// Control characters become spaces and the text is cut to the spec limit: terminals
/// discard a report whose text still carries control characters, and an embedded
/// `BEL`/`ESC` would terminate the OSC early.
pub fn format_escape(status: ProgramStatus, message: Option<&str>) -> String {
    let mut out = String::from("\x1b]7501;state=");
    out.push_str(status.state_label());
    out.push_str(":app=");
    out.push_str(APP_NAME);

    if let Some(kind) = status.blocked_kind() {
        out.push_str(":kind=");
        out.push_str(kind.label());
    }

    let plain = message.map(sanitize_message).unwrap_or_default();
    if !plain.is_empty() {
        out.push_str(":msg=");
        out.push_str(&base64::engine::general_purpose::STANDARD.encode(plain.as_bytes()));
    }

    out.push_str("\x1b\\");
    out
}

fn sanitize_message(message: &str) -> String {
    let replaced: String = message
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    truncate_utf8(replaced.trim(), MAX_MESSAGE_BYTES).to_string()
}

fn truncate_utf8(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = 0;
    for (index, character) in text.char_indices() {
        let next = index + character.len_utf8();
        if next > max_bytes {
            break;
        }
        end = next;
    }
    &text[..end]
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or("Error").trim().to_string()
}

/// Derives the reported status from [`TitleState`] and remembers what the settled run
/// reported, so `done`/`error` survive until the next run starts.
pub struct ProgramStatusReporter {
    /// Status while no run is active: `idle` before the first run, then `done` or `error`.
    resting: ProgramStatus,
    /// Failure of the run in flight, consumed when that run settles.
    run_failed: bool,
    /// First line of the failure, reported as `msg` for the `error` state.
    error_message: Option<String>,
    was_busy: bool,
    /// Last emitted escape; a repeated identical report is dropped instead of re-sent every tick.
    last_escape: Option<String>,
}

impl ProgramStatusReporter {
    pub fn new() -> Self {
        Self {
            resting: ProgramStatus::Idle,
            run_failed: false,
            error_message: None,
            was_busy: false,
            last_escape: None,
        }
    }

    /// Record that the run in flight failed, so settling reports `error` instead of `done`.
    pub fn record_error(&mut self, message: &str) {
        self.error_message = Some(first_line(message));
        if self.was_busy {
            self.run_failed = true;
        } else {
            // The failure arrived after the run already settled (`done` was reported);
            // report `error` on the next tick instead of waiting for a run that never comes.
            self.resting = ProgramStatus::Error;
        }
    }

    /// Forget the previous session's run, for example after switching sessions.
    #[cfg(test)]
    pub fn reset(&mut self) {
        self.resting = ProgramStatus::Idle;
        self.run_failed = false;
        self.error_message = None;
        self.was_busy = false;
        self.last_escape = None;
    }

    /// Compose the report for this tick; `None` when the report is unchanged.
    pub fn update(&mut self, state: &TitleState<'_>) -> Option<String> {
        let status = self.current_status(state);
        self.was_busy = state.is_busy;

        let escape = format_escape(status, self.message_for(status, state));
        if self.last_escape.as_deref() == Some(escape.as_str()) {
            return None;
        }
        self.last_escape = Some(escape.clone());
        Some(escape)
    }

    /// Report `clear` so the terminal does not keep a stale status after exit. Emitted once.
    pub fn clear(&mut self) -> Option<&'static str> {
        self.last_escape.take()?;
        Some(CLEAR_ESCAPE)
    }

    fn current_status(&mut self, state: &TitleState<'_>) -> ProgramStatus {
        if state.has_pending_permissions {
            return ProgramStatus::Blocked {
                kind: BlockedKind::Permission,
            };
        }

        if state.is_busy {
            return ProgramStatus::Working;
        }

        if self.was_busy {
            self.resting = if self.run_failed {
                ProgramStatus::Error
            } else {
                ProgramStatus::Done
            };
            self.run_failed = false;
        }

        self.resting
    }

    fn message_for(&self, status: ProgramStatus, state: &TitleState<'_>) -> Option<&str> {
        match status {
            ProgramStatus::Error => self.error_message.as_deref(),
            // Report the compaction cause so a long silent run explains itself in the terminal.
            ProgramStatus::Working
                if matches!(state.activity, Some(TurnActivity::AutoCompacting)) =>
            {
                Some("Compacting context")
            }
            _ => state.session_name,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn title_state(is_busy: bool, has_pending_permissions: bool) -> TitleState<'static> {
        TitleState {
            session_name: Some("demo"),
            model: None,
            activity: None,
            has_pending_permissions,
            cwd: None,
            turn_elapsed: None,
            is_busy,
            focused: true,
        }
    }

    #[test]
    fn formats_working_with_base64_message() {
        let escape = format_escape(ProgramStatus::Working, Some("demo"));
        // "demo" in base64.
        assert_eq!(
            escape,
            "\x1b]7501;state=working:app=grok-pi:msg=ZGVtbw==\x1b\\"
        );
    }

    #[test]
    fn formats_blocked_with_kind_and_omits_message_when_empty() {
        let escape = format_escape(
            ProgramStatus::Blocked {
                kind: BlockedKind::Permission,
            },
            None,
        );
        assert_eq!(
            escape,
            "\x1b]7501;state=blocked:app=grok-pi:kind=permission\x1b\\"
        );
    }

    #[test]
    fn replaces_control_characters_in_message() {
        let escape = format_escape(ProgramStatus::Working, Some("a\x07b\nc"));
        let expected = base64::engine::general_purpose::STANDARD.encode("a b c");
        assert!(escape.contains(&format!(":msg={expected}")), "{escape}");
    }

    #[test]
    fn clears_only_after_a_report() {
        let mut reporter = ProgramStatusReporter::new();
        assert_eq!(reporter.clear(), None);

        reporter.update(&title_state(true, false));
        assert_eq!(reporter.clear(), Some(CLEAR_ESCAPE));
        assert_eq!(reporter.clear(), None);
    }

    #[test]
    fn reports_working_then_done_once_each() {
        let mut reporter = ProgramStatusReporter::new();

        let working = reporter.update(&title_state(true, false)).expect("working");
        assert!(working.contains("state=working"));
        assert_eq!(reporter.update(&title_state(true, false)), None);

        let done = reporter.update(&title_state(false, false)).expect("done");
        assert!(done.contains("state=done"));
        assert_eq!(reporter.update(&title_state(false, false)), None);
    }

    #[test]
    fn pending_permission_reports_blocked_before_busy() {
        let mut reporter = ProgramStatusReporter::new();
        let escape = reporter
            .update(&title_state(true, true))
            .expect("blocked report");
        assert!(escape.contains("state=blocked"), "{escape}");
        assert!(escape.contains("kind=permission"), "{escape}");
    }

    #[test]
    fn failed_run_settles_as_error_with_first_line() {
        let mut reporter = ProgramStatusReporter::new();
        reporter.update(&title_state(true, false));
        reporter.record_error("boom\nstack line");

        let settled = reporter.update(&title_state(false, false)).expect("error");
        assert!(settled.contains("state=error"), "{settled}");
        let encoded = base64::engine::general_purpose::STANDARD.encode("boom");
        assert!(settled.contains(&format!(":msg={encoded}")), "{settled}");
    }

    #[test]
    fn error_recorded_after_settle_reports_immediately() {
        let mut reporter = ProgramStatusReporter::new();
        reporter.update(&title_state(true, false));
        let done = reporter.update(&title_state(false, false)).expect("done");
        assert!(done.contains("state=done"), "{done}");

        reporter.record_error("boom");
        let failed = reporter
            .update(&title_state(false, false))
            .expect("error after settle");
        assert!(failed.contains("state=error"), "{failed}");
    }

    #[test]
    fn reset_forgets_the_previous_run() {
        let mut reporter = ProgramStatusReporter::new();
        reporter.update(&title_state(true, false));
        reporter.update(&title_state(false, false));
        reporter.reset();
        assert!(reporter.clear().is_none());
    }
}
