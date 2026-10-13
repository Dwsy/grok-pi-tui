//! Whole-clip transcription, the second STT route. When the login holds no credential `api.x.ai` accepts
//! ([`crate::auth::VoiceAuthError::ForeignSession`]) the pipeline records to a buffer and hands the clip to a
//! host-provided [`ClipTranscriber`]. The host owns the client and credentials; this crate only defines the seam.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

/// Backstop on the final request for a host transcriber that forgets to bound its own. Above any host's own bound
/// (a client scaling with clip size reaches ~220 s for the largest clip), so a slow but live upload still lands; a
/// consumer that waits on the final can give up shortly after this.
pub const FINAL_TIMEOUT: Duration = Duration::from_secs(240);

/// One finished recording, ready to upload. The container is always RIFF/WAVE (16-bit PCM mono), so a
/// transcriber labels it `audio/wav`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioClip {
    pub wav: Vec<u8>,
    /// Concrete language code from [`crate::language_for_api`]; never `auto`.
    pub language: String,
}

/// Why a clip could not be transcribed. `message` is shown to the user as the voice toast, so it must be
/// actionable on its own ("your plan does not include dictation", "sign in again"), not a transport dump.
/// A `fatal` error is one a retry with more audio cannot fix (plan gate, sign-in); an interim that fails this way
/// ends the session at once instead of after the stop.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct TranscribeError {
    pub message: String,
    pub fatal: bool,
}

impl TranscribeError {
    /// A transient failure: interims ignore it and the final reports it.
    pub fn new(message: impl Into<String>) -> Self {
        TranscribeError {
            message: message.into(),
            fatal: false,
        }
    }

    pub fn fatal(message: impl Into<String>) -> Self {
        TranscribeError {
            message: message.into(),
            fatal: true,
        }
    }
}

/// Transcribes a finished clip in one call.
///
/// Return the full transcript for the clip (empty when nothing was said) or a [`TranscribeError`] whose message can
/// be shown verbatim. Implementations resolve their own credential per call, because a pipeline outlives any single
/// token, and bound their own request time (the pipeline's own timeouts are a backstop). The call must be
/// cancellation-safe: the pipeline drops the future when the user tears voice down.
pub trait ClipTranscriber: std::fmt::Debug + Send + Sync + 'static {
    fn transcribe(
        &self,
        clip: AudioClip,
    ) -> Pin<Box<dyn Future<Output = Result<String, TranscribeError>> + Send + '_>>;
}

pub type SharedClipTranscriber = Arc<dyn ClipTranscriber>;
