/// Which STT route a capture session took; the pager's submit path differs between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceRoute {
    /// Partials arrive while speaking and are committed on the way; a trailing final after submit is redundant.
    Streaming,
    Clip,
}

/// Minted per press by the pager; a reader the next press aborted may still have events queued, and they carry the
/// old id so the pager can drop them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct VoiceSessionId(pub(crate) u64);

impl VoiceSessionId {
    #[must_use]
    pub fn next(self) -> Self {
        VoiceSessionId(self.0.wrapping_add(1))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaggedVoiceEvent {
    pub session: VoiceSessionId,
    pub event: VoiceEvent,
}

/// Events emitted by [`crate::pipeline::run_voice_pipeline`] to the pager event loop. `IntoStaticStr` gives the
/// variant name for logs, which must never carry the transcript text.
#[derive(Debug, Clone, PartialEq, Eq, strum::IntoStaticStr)]
pub enum VoiceEvent {
    /// The mic is open and the session's route is decided; sent once per capture, before any transcript.
    CaptureStarted { route: VoiceRoute },

    /// Released before the mic opened; terminal for the press: no [`Self::CaptureStarted`] or transcript follows.
    CaptureCancelled,

    /// Partial transcript; each replaces the last and the final replaces them all. On the clip route it is the clip
    /// so far re-transcribed.
    InterimTranscript { text: String },

    /// Utterance complete (`speech_final` on streaming STT, or batch result).
    UtteranceFinal { text: String },

    /// A toast that ends nothing; the session's state is unchanged.
    Notice { message: String },

    /// The mic is closed and the recorded clip is being transcribed as a whole; a [`Self::UtteranceFinal`] or
    /// [`Self::Error`] follows. Only the clip route emits this; the streaming route commits text while recording.
    Transcribing,

    /// Non-fatal or fatal error from capture or STT.
    Error {
        /// Short description for a one-line toast.
        message: String,
        /// Optional longer fix steps, shown where more than one line fits.
        hint: Option<String>,
    },
}
