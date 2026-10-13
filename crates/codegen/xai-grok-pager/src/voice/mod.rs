//! Voice input: STT pipeline integration and prompt-box dictation.
//!
//! Layering (pager-owned):
//! - **Voice gate**: on by default in GA. Remote `voice_mode_enabled: false` is a kill switch that hides voice everywhere and shows no toast.
//!   An absent remote value falls through to on. `GROK_VOICE_MODE` overrides for local dev; the env var beats the remote flag, which beats the default.
//!   Free/X Basic still get the SuperGrok upsell via tier gates, not this flag.
//! - **Session mode** (`voice_ui_active`): this CLI run only; shows the mic.
//! - **Capture chord**: `/voice` or `Ctrl+Space` start dictation, and Esc or Enter stops it.
//!   `Ctrl+Space` decodes identically on every terminal, so the cheatsheet shows it whenever voice is enabled.
//! - **Hold-to-talk**: on terminals that report key releases (Kitty protocol), hold `Ctrl+Space` to record and release to stop.
//!   Elsewhere the same chord toggles: press starts, press again stops. Handled in `app::event_loop`.
//! - **STT route**: streaming when the account has a bearer `api.x.ai` accepts; otherwise, on a build with a clip
//!   transcriber, the clip so far is re-transcribed every few seconds for the same partials, the whole recording is
//!   uploaded on stop, and one final replaces the interim words.
//!
//! Final transcripts are inserted at the current cursor position in the recording target's prompt while capture stays open across speech pauses.
//! The target, captured at start via [`crate::app::app_view::VoiceTarget`], is the agent prompt or the dashboard's input for dispatching a new agent.
//! The user always submits with Enter; nothing is auto-sent.
//! Submit promotes any remaining interim text into the bound prompt, then resets capture; a clip that has shown no
//! partial yet keeps its target so its one final lands as the next draft.
mod auth;
mod handle;
pub use auth::build_stt_routes;
pub use handle::handle_tagged_voice_event;
#[cfg(test)]
pub(crate) use handle::handle_voice_event;
pub(crate) use handle::{
    RECORDING_DISCARDED_TOAST, TRANSCRIPTION_TIMED_OUT_KEPT_TOAST, TRANSCRIPTION_TIMED_OUT_TOAST,
};
pub(crate) use handle::{
    VoiceInterimCommit, commit_interim_into_prompt, merge_voice_fragment, prompt_blank_for_voice,
    space_voice_fragment,
};
pub use xai_grok_voice::maybe_run_capture_subprocess;
