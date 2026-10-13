//! Dictation state machine. One enum holds mic-live, start-queued, hold, target, route, and the partial shown so
//! they cannot disagree (as booleans they drifted). A clip session that has shown no partial has committed nothing,
//! so submit and a new press wait for its one final; see [`VoiceState::owes_final`]. Production mutates the state
//! only through the `AppView::voice_*` methods below; the pipeline's events land through
//! [`crate::voice::handle_voice_event`].

use std::time::{Duration, Instant};

use xai_grok_voice::VoiceRoute;

use crate::app::agent::AgentId;
use crate::app::app_view::AppView;

/// How long an outstanding clip (stopped or uploading) is waited on before the pager gives up on its final: the
/// pipeline's own backstop plus room for the stop and drain, so a reader that died mid-upload cannot block presses
/// for good.
const OUTSTANDING_CLIP_LIMIT: Duration =
    xai_grok_voice::FINAL_TIMEOUT.saturating_add(Duration::from_secs(15));

/// Which prompt box in-flight voice dictation appends its finalized text to.
/// Captured when recording **starts** so a trailing STT final still lands where the user was dictating.
/// That holds even if they navigate away, or toggle a dashboard row's peek panel, mid-utterance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceTarget {
    /// A live agent session's prompt box.
    Agent(AgentId),
    /// The dashboard's new-agent dispatch input (no row peek was open at start).
    DashboardDispatch,
    /// The dashboard's peek reply input, bound to the agent whose peek was open at start.
    /// The id pins the row: selecting a different row mid-utterance stops capture (the reply widget is shared and clears on row change).
    /// A final therefore can't land on the wrong agent's reply.
    DashboardPeekReply(AgentId),
}

/// The partial transcript of one session. `Committed` is a partial that showed and was then moved into the draft (a
/// final replaced it, or submit promoted it): nothing is on screen, but the session's text is already in the draft,
/// so its final is not owed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Partial {
    #[default]
    None,
    Shown(String),
    Committed,
}

impl Partial {
    fn shown(&self) -> Option<&str> {
        match self {
            Partial::Shown(text) => Some(text),
            Partial::None | Partial::Committed => None,
        }
    }

    fn commit(&mut self) {
        if matches!(self, Partial::Shown(_)) {
            *self = Partial::Committed;
        }
    }
}

/// `hold` marks a session begun by a Ctrl+Space hold-press: its matching Ctrl+Space release ends it (and only it).
/// `/voice` and toggle sessions leave `hold` false so a Ctrl+Space release can't touch them.
/// `route` is `None` until the pipeline reports which STT route the session took, a few hundred ms after the press.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum VoiceState {
    #[default]
    Idle,
    /// A start was requested before the lazy pipeline existed; the event loop spawns it once and then opens the mic.
    ColdStart { hold: bool, target: VoiceTarget },
    Recording {
        hold: bool,
        target: VoiceTarget,
        partial: Partial,
        route: Option<VoiceRoute>,
    },
    /// Capture was explicitly stopped (Esc / Ctrl+Space / [stop] / Ctrl+Space release).
    /// The target (and the last interim) are kept so a trailing STT final still lands without the overlay flickering in the meantime.
    Stopping {
        target: VoiceTarget,
        partial: Partial,
        route: Option<VoiceRoute>,
    },
    /// The clip route closed the mic and is uploading the whole recording; the interim words stay italic
    /// until the final or an error.
    Transcribing {
        target: VoiceTarget,
        partial: Partial,
    },
}

impl VoiceState {
    pub fn is_listening(&self) -> bool {
        matches!(self, Self::Recording { .. })
    }

    pub fn is_pending_cold_start(&self) -> bool {
        matches!(self, Self::ColdStart { .. })
    }

    pub fn is_transcribing(&self) -> bool {
        matches!(self, Self::Transcribing { .. })
    }

    /// Whether a hold-press owns the current session (so its key release ends it).
    /// `/voice` and toggle-style starts leave this false.
    pub(crate) fn is_hold_owned(&self) -> bool {
        matches!(self, Self::ColdStart { hold, .. } | Self::Recording { hold, .. } if *hold)
    }

    pub fn target(&self) -> Option<VoiceTarget> {
        match self {
            Self::ColdStart { target, .. }
            | Self::Recording { target, .. }
            | Self::Stopping { target, .. }
            | Self::Transcribing { target, .. } => Some(*target),
            Self::Idle => None,
        }
    }

    /// The live partial shown in the prompt overlay, if any.
    pub fn interim(&self) -> Option<&str> {
        self.partial().and_then(Partial::shown)
    }

    fn partial(&self) -> Option<&Partial> {
        match self {
            Self::Recording { partial, .. }
            | Self::Stopping { partial, .. }
            | Self::Transcribing { partial, .. } => Some(partial),
            Self::Idle | Self::ColdStart { .. } => None,
        }
    }

    /// Whether the session's one final is still owed and must have a prompt to land in: a clip session that has
    /// shown no partial. Streaming commits on the way, and a shown partial is promoted on submit on either route, so
    /// a trailing final would repeat it. A route not yet reported has captured at most an event-loop tick of audio
    /// and is not owed either, so submit in that window is the same abort it was before the clip route existed.
    pub(crate) fn owes_final(&self) -> bool {
        match self {
            Self::Recording { route, partial, .. } | Self::Stopping { route, partial, .. } => {
                *route == Some(VoiceRoute::Clip) && *partial == Partial::None
            }
            Self::Transcribing { partial, .. } => *partial == Partial::None,
            Self::Idle | Self::ColdStart { .. } => false,
        }
    }

    /// A clip session whose words are the best transcript there is: an error ending it must not throw them away.
    pub(crate) fn is_on_clip_route(&self) -> bool {
        match self {
            Self::Recording { route, .. } | Self::Stopping { route, .. } => {
                *route == Some(VoiceRoute::Clip)
            }
            Self::Transcribing { .. } => true,
            Self::Idle | Self::ColdStart { .. } => false,
        }
    }

    /// A press would abort the reader mid-upload. A stop with no route yet does not block: a start the pipeline
    /// cancelled reports nothing, so blocking on it would wedge voice.
    pub(crate) fn blocks_new_capture(&self) -> bool {
        match self {
            Self::Stopping { route, .. } => *route == Some(VoiceRoute::Clip),
            Self::Transcribing { .. } => true,
            Self::Idle | Self::ColdStart { .. } | Self::Recording { .. } => false,
        }
    }
}

impl AppView {
    pub fn voice_listening(&self) -> bool {
        self.voice_state.is_listening()
    }

    pub fn voice_hold_owned(&self) -> bool {
        self.voice_state.is_hold_owned()
    }

    pub fn voice_recording_target(&self) -> Option<VoiceTarget> {
        self.voice_state.target()
    }

    pub fn voice_interim(&self) -> Option<&str> {
        self.voice_state.interim()
    }

    /// Best-effort one-shot command into the voice pipeline (no-op if it isn't up).
    fn voice_send(&self, cmd: xai_grok_voice::VoiceCommand) {
        if let Some(tx) = &self.voice_cmd_tx
            && tx.try_send(cmd).is_err()
        {
            tracing::trace!("voice command dropped: pipeline channel full or closed");
        }
    }

    /// Queue a start for the event loop to run once the lazy pipeline is up.
    pub(crate) fn voice_queue_cold_start(&mut self, target: VoiceTarget, hold: bool) {
        self.voice_state = VoiceState::ColdStart { hold, target };
    }

    /// Open the mic now (pipeline already up) and enter [`VoiceState::Recording`] bound to `target`.
    /// `hold` marks a Ctrl+Space hold-press start. A streaming session the press supersedes while it is stopping may
    /// still owe its last final; it is remembered so that one final lands (a clip that is owed blocks the press).
    pub(crate) fn voice_begin_recording(&mut self, target: VoiceTarget, hold: bool) {
        self.voice_trailing_final = match self.voice_state {
            VoiceState::Stopping { target, .. } => Some((self.voice_session, target)),
            _ => None,
        };
        self.voice_session = self.voice_session.next();
        self.voice_send(xai_grok_voice::VoiceCommand::PttPress {
            session: self.voice_session,
        });
        self.voice_state = VoiceState::Recording {
            hold,
            target,
            partial: Partial::None,
            route: None,
        };
        self.voice_sync_clip_deadline();
    }

    /// Arms the give-up deadline when the state starts blocking presses on an outstanding clip, and clears it when
    /// it stops; the deadline is not moved while a state stays blocking.
    fn voice_sync_clip_deadline(&mut self) {
        self.voice_clip_deadline = if self.voice_state.blocks_new_capture() {
            self.voice_clip_deadline
                .or_else(|| Some(Instant::now() + OUTSTANDING_CLIP_LIMIT))
        } else {
            None
        };
    }

    /// Give up on an outstanding clip whose final has not arrived by its deadline: the reader may have died with the
    /// event channel still open, and without this every press would toast forever. Run by the event loop each tick.
    pub fn voice_expire_outstanding_clip(&mut self, now: Instant) {
        if self
            .voice_clip_deadline
            .is_some_and(|deadline| now >= deadline)
        {
            let kept = crate::voice::commit_interim_into_prompt(self).is_some();
            self.voice_reset();
            self.show_toast(if kept {
                crate::voice::TRANSCRIPTION_TIMED_OUT_KEPT_TOAST
            } else {
                crate::voice::TRANSCRIPTION_TIMED_OUT_TOAST
            });
        }
    }

    /// Record which STT route the open session took. No-op unless a session is recording or stopping; a report
    /// for a session that was reset must not revive it.
    pub(crate) fn voice_set_route(&mut self, reported: VoiceRoute) {
        match &mut self.voice_state {
            VoiceState::Recording { route, .. } | VoiceState::Stopping { route, .. } => {
                *route = Some(reported);
            }
            VoiceState::Idle | VoiceState::ColdStart { .. } | VoiceState::Transcribing { .. } => {}
        }
        self.voice_sync_clip_deadline();
    }

    /// Also accepted after a clip stops: an interim in flight at the stop lands while the final is out. A streaming
    /// partial after a stop is dropped so it cannot repopulate the overlay.
    pub(crate) fn voice_set_interim(&mut self, text: String) -> bool {
        match &mut self.voice_state {
            VoiceState::Recording { partial, .. }
            | VoiceState::Stopping {
                partial,
                route: Some(VoiceRoute::Clip),
                ..
            }
            | VoiceState::Transcribing { partial, .. } => {
                *partial = Partial::Shown(text);
                true
            }
            VoiceState::Idle | VoiceState::ColdStart { .. } | VoiceState::Stopping { .. } => false,
        }
    }

    /// The shown partial was moved into the draft (a final replaced it, or submit promoted it); the overlay drops
    /// it without a teardown.
    pub(crate) fn voice_commit_interim(&mut self) {
        match &mut self.voice_state {
            VoiceState::Recording { partial, .. }
            | VoiceState::Stopping { partial, .. }
            | VoiceState::Transcribing { partial, .. } => partial.commit(),
            VoiceState::Idle | VoiceState::ColdStart { .. } => {}
        }
    }

    /// Explicit stop (Esc / Ctrl+Space / `[stop]`): release the mic but keep the target and last interim so a trailing STT final still lands.
    /// Always allowed (never leaves a hot mic). No-op unless recording.
    pub(crate) fn voice_stop_keeping_final(&mut self) {
        let VoiceState::Recording {
            target,
            partial,
            route,
            ..
        } = &mut self.voice_state
        else {
            return;
        };
        let target = *target;
        let partial = std::mem::take(partial);
        let route = *route;
        self.voice_send(xai_grok_voice::VoiceCommand::PttRelease);
        self.voice_state = VoiceState::Stopping {
            target,
            partial,
            route,
        };
        self.voice_sync_clip_deadline();
    }

    /// The pipeline closed the mic and is uploading the clip. Reached from `Recording` when the clip cap closed the
    /// mic, or from `Stopping` after the user's own stop. Only a session already on the clip route can be uploading;
    /// with route unknown or streaming this is a stale event from a reader the next press aborted, and honouring it
    /// would wedge the new session in `Transcribing` (defence in depth beside the session-id gate).
    pub(crate) fn voice_mark_transcribing(&mut self) {
        match &mut self.voice_state {
            VoiceState::Recording {
                target,
                partial,
                route: Some(VoiceRoute::Clip),
                ..
            }
            | VoiceState::Stopping {
                target,
                partial,
                route: Some(VoiceRoute::Clip),
            } => {
                self.voice_state = VoiceState::Transcribing {
                    target: *target,
                    partial: std::mem::take(partial),
                };
            }
            VoiceState::Recording { .. }
            | VoiceState::Stopping { .. }
            | VoiceState::Idle
            | VoiceState::ColdStart { .. }
            | VoiceState::Transcribing { .. } => {}
        }
        self.voice_sync_clip_deadline();
    }

    /// Only a stop with no route yet can be the cancelled start; anything else belongs to a later press.
    pub(crate) fn voice_capture_cancelled(&mut self) {
        if matches!(self.voice_state, VoiceState::Stopping { route: None, .. }) {
            self.voice_state = VoiceState::Idle;
        }
    }

    /// The clip route emits exactly one final, so nothing else is owed and the next press or submit is not held
    /// back. No-op outside `Transcribing`.
    pub(crate) fn voice_finish_transcribing(&mut self) {
        if self.voice_state.is_transcribing() {
            self.voice_state = VoiceState::Idle;
            self.voice_sync_clip_deadline();
        }
    }

    /// Hard teardown (submit / error / kill-switch / navigate-away): drop the session and forget it (no trailing final,
    /// no queued start). `Abort`, not a release: a release would upload the abandoned clip and deliver its final into
    /// the next session.
    pub(crate) fn voice_reset(&mut self) {
        if matches!(
            self.voice_state,
            VoiceState::Recording { .. }
                | VoiceState::Stopping { .. }
                | VoiceState::Transcribing { .. }
        ) {
            self.voice_send(xai_grok_voice::VoiceCommand::Abort);
        }
        self.voice_state = VoiceState::Idle;
        self.voice_trailing_final = None;
        self.voice_clip_deadline = None;
    }

    /// Ctrl+Space hold release: end only a session a Ctrl+Space hold started.
    /// Cancel a queued hold cold-start, or stop a live hold recording (keeping its trailing final).
    /// A `/voice` / toggle session (`hold` false) is left untouched, so a Ctrl+Space release can neither cancel nor stop it.
    pub(crate) fn voice_hold_release(&mut self) {
        match self.voice_state {
            // Queued but never opened the mic: forget it (a quick tap records nothing).
            VoiceState::ColdStart { hold: true, .. } => self.voice_reset(),
            // Live: stop but keep the target for the trailing final.
            VoiceState::Recording { hold: true, .. } => self.voice_stop_keeping_final(),
            _ => {}
        }
    }
}

#[cfg(test)]
#[path = "voice_state_tests.rs"]
mod tests;
