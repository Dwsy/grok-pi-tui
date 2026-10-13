//! Voice pipeline: mic capture goes to STT and transcripts come back as pager events.
//!
//! Two STT routes share one capture path, chosen per session by [`SttRoutes`]: streaming when `api.x.ai` accepts
//! the bearer, else the host's clip transcriber. The pager sees the same commands and events either way, plus
//! [`VoiceEvent::Transcribing`] after a clip stops.
//!
//! The pager drives capture with press/release commands.
//! They back both a toggle (`/voice`, `Ctrl+Shift+M`) and true push-to-talk (F12 hold), hence the `Ptt*` names.
//! A press may be followed by a release after a long hold or, for a toggle, a later stop.

#[cfg(feature = "audio")]
use std::collections::VecDeque;

use std::future::Future;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::auth::SharedVoiceAuth;
#[cfg(any(test, feature = "audio"))]
use crate::auth::VoiceAuthError;
use crate::config::VoiceConfig;
use crate::error::VoiceError;
#[cfg(any(test, feature = "audio"))]
use crate::event::VoiceRoute;
use crate::event::{TaggedVoiceEvent, VoiceEvent, VoiceSessionId};
#[cfg(feature = "audio")]
use crate::stt::{StreamingSttEvent, StreamingSttSession};
use crate::transcriber::SharedClipTranscriber;

/// Commands from the pager event loop (toggle start/stop, or F12 push-to-talk).
#[derive(Debug, PartialEq, Eq)]
pub enum VoiceCommand {
    /// Begin a capture session (mic open until [`VoiceCommand::PttRelease`]); its events carry `session`.
    PttPress { session: VoiceSessionId },
    /// End the current capture session and deliver its transcript (`audio.done`, or the clip upload).
    PttRelease,
    /// Drop the current capture session without a transcript: the mic closes and no transcript is delivered. An
    /// interim request already in flight may have left the process; nothing further is sent.
    Abort,
    /// Tear down the pipeline task.
    Shutdown,
}

#[derive(Debug, Clone)]
pub(crate) struct SessionEvents {
    session: VoiceSessionId,
    tx: mpsc::Sender<TaggedVoiceEvent>,
}

impl SessionEvents {
    #[cfg(test)]
    pub(crate) fn for_test(tx: mpsc::Sender<TaggedVoiceEvent>) -> Self {
        SessionEvents {
            session: VoiceSessionId(1),
            tx,
        }
    }

    pub(crate) async fn send(
        &self,
        event: VoiceEvent,
    ) -> Result<(), mpsc::error::SendError<TaggedVoiceEvent>> {
        self.tx
            .send(TaggedVoiceEvent {
                session: self.session,
                event,
            })
            .await
    }
}

/// The STT backends a pipeline may use, resolved again at every capture start so a login change mid-session
/// takes effect on the next press.
#[derive(Debug, Clone)]
pub struct SttRoutes {
    pub auth: SharedVoiceAuth,
    /// Whole-clip fallback for an account whose login the streaming socket refuses; `None` keeps that refusal
    /// as the user-facing error.
    pub clip_transcriber: Option<SharedClipTranscriber>,
}

/// Generic so the choice is made from the bearer alone (`RouteChoice<String>`, testable without a socket) before
/// the streaming leg connects.
#[cfg(any(test, feature = "audio"))]
#[derive(Debug)]
enum RouteChoice<S> {
    Streaming(S),
    Clip(SharedClipTranscriber),
}

#[cfg(any(test, feature = "audio"))]
impl<S> RouteChoice<S> {
    fn kind(&self) -> VoiceRoute {
        match self {
            RouteChoice::Streaming(_) => VoiceRoute::Streaming,
            RouteChoice::Clip(_) => VoiceRoute::Clip,
        }
    }
}

/// # Errors
///
/// [`VoiceError::Auth`] with the bearer error's own text when the streaming socket has no bearer and no clip
/// transcriber can stand in; a foreign login without a transcriber keeps its "needs an xAI credential" message.
#[cfg(any(test, feature = "audio"))]
fn choose_route(
    bearer: Result<String, VoiceAuthError>,
    clip_transcriber: Option<&SharedClipTranscriber>,
) -> Result<RouteChoice<String>, VoiceError> {
    match (bearer, clip_transcriber) {
        (Ok(bearer), _) => Ok(RouteChoice::Streaming(bearer)),
        (Err(VoiceAuthError::ForeignSession), Some(transcriber)) => {
            Ok(RouteChoice::Clip(transcriber.clone()))
        }
        (Err(error @ (VoiceAuthError::ForeignSession | VoiceAuthError::NotSignedIn)), _) => {
            Err(VoiceError::Auth(error.to_string()))
        }
    }
}

struct ActivePtt {
    finish_tx: mpsc::Sender<()>,
    reader: JoinHandle<()>,
}

/// The reader task owns the capture handle, so it stops the mic and sends `audio.done` (or uploads the clip) in one
/// place. Non-blocking: the clip reader stops receiving once it leaves collection, and a repeated release must not
/// stall this loop behind a slot it will never drain.
fn release(session: &ActivePtt) {
    if let Err(error) = session.finish_tx.try_send(()) {
        tracing::debug!(%error, "voice: release not delivered; the session is already stopping");
    }
}

/// Run until [`VoiceCommand::Shutdown`].
pub async fn run_voice_pipeline(
    config: VoiceConfig,
    routes: SttRoutes,
    mut cmd_rx: mpsc::Receiver<VoiceCommand>,
    event_tx: mpsc::Sender<TaggedVoiceEvent>,
) {
    let mut active: Option<ActivePtt> = None;

    while let Some(cmd) = cmd_rx.recv().await {
        match cmd {
            VoiceCommand::Shutdown => break,
            // Dropping the reader closes the mic and the socket or upload with it; nothing more is sent for the session
            VoiceCommand::Abort => {
                if let Some(prev) = active.take() {
                    prev.reader.abort();
                }
            }
            VoiceCommand::PttPress { mut session } => {
                // Aborting drops the old reader's capture and STT session at once. The pager releases between presses and
                // waits for a clip's final (`blocks_new_capture`), so `active` here is a stopping streaming session, never
                // a live duplicate or an upload worth keeping. The old reader is not joined; cpal handles the brief overlap
                if let Some(prev) = active.take() {
                    prev.reader.abort();
                }

                let mut events = SessionEvents {
                    session,
                    tx: event_tx.clone(),
                };
                let start = open_session(&config, &routes, &events);
                match race_start_with_next_command(start, &mut cmd_rx, &events).await {
                    StartOutcome::Opened(opened) => active = opened,
                    StartOutcome::Cancelled => {}
                    StartOutcome::Shutdown => break,
                    // The pager always sends a release between presses, so this is unreachable; start fresh defensively
                    StartOutcome::PressedAgain(next) => {
                        session = next;
                        events.session = session;
                        active = open_session(&config, &routes, &events).await;
                    }
                }
            }
            VoiceCommand::PttRelease => {
                if let Some(session) = active.as_ref() {
                    release(session);
                }
            }
        }
    }

    if let Some(session) = active {
        session.reader.abort();
    }
}

enum StartOutcome {
    /// The session opened (or failed with an `Error` event already sent), before any other command.
    Opened(Option<ActivePtt>),
    /// Released before the mic was ready; the start was dropped and [`VoiceEvent::CaptureCancelled`] sent.
    Cancelled,
    Shutdown,
    PressedAgain(VoiceSessionId),
}

/// `biased`: the start is polled first so a just-completed session is always kept (dropping it would leak its
/// reader). On a release the start future is dropped; a mic-open still in flight completes and its handle is
/// dropped, releasing the device. `CaptureCancelled` tells the pager nothing else follows.
async fn race_start_with_next_command(
    start: impl Future<Output = Option<ActivePtt>>,
    cmd_rx: &mut mpsc::Receiver<VoiceCommand>,
    events: &SessionEvents,
) -> StartOutcome {
    tokio::select! {
        biased;
        opened = start => StartOutcome::Opened(opened),
        next = cmd_rx.recv() => match next {
            Some(VoiceCommand::PttRelease | VoiceCommand::Abort) => {
                let _ = events.send(VoiceEvent::CaptureCancelled).await;
                StartOutcome::Cancelled
            }
            Some(VoiceCommand::Shutdown) | None => StartOutcome::Shutdown,
            Some(VoiceCommand::PttPress { session }) => StartOutcome::PressedAgain(session),
        },
    }
}

/// Open a capture session, emitting a `VoiceEvent::Error` (and returning `None`) on failure.
/// Extracted so the `PttPress` start can be raced against an incoming release in `select!` and reused for the defensive restart path.
async fn open_session(
    config: &VoiceConfig,
    routes: &SttRoutes,
    events: &SessionEvents,
) -> Option<ActivePtt> {
    match start_capture_session(config, routes, events).await {
        Ok(session) => Some(session),
        Err(e) => {
            let _ = events
                .send(VoiceEvent::Error {
                    message: e.to_string(),
                    hint: None,
                })
                .await;
            None
        }
    }
}

#[cfg(not(feature = "audio"))]
async fn start_capture_session(
    _config: &VoiceConfig,
    _routes: &SttRoutes,
    _events: &SessionEvents,
) -> Result<ActivePtt, VoiceError> {
    Err(VoiceError::Config(
        "voice audio capture disabled (build without `audio` feature)".into(),
    ))
}

/// Hard cap on the pre-connect PCM backlog (memory safety).
/// Sized far above any real connect: the STT connect timeout aborts long before this is reached.
/// In practice it never drops; it only bounds a pathological hang.
#[cfg(feature = "audio")]
const BACKLOG_MAX_CHUNKS: usize = 1024;

/// Until `audio_tx_rx` yields the live STT sender, captured chunks accumulate in a bounded backlog, so the mic never
/// backpressures during connect. Once the sender arrives the backlog is flushed in order and capture streams live.
/// Holding the sender also defers the writer's `audio.done` until the backlog is drained on teardown.
#[cfg(feature = "audio")]
async fn forward_pcm(
    mut mic_rx: mpsc::Receiver<Vec<u8>>,
    mut audio_tx_rx: tokio::sync::oneshot::Receiver<mpsc::Sender<Vec<u8>>>,
) {
    let mut backlog: VecDeque<Vec<u8>> = VecDeque::new();
    let audio_tx = loop {
        tokio::select! {
            chunk = mic_rx.recv() => match chunk {
                // A normal connect stays well under the cap, so the lead-in is kept intact
                // Only a pathologically slow connect (which the connect timeout aborts anyway) drops its oldest chunks
                Some(c) => {
                    if backlog.len() == BACKLOG_MAX_CHUNKS {
                        backlog.pop_front();
                    }
                    backlog.push_back(c);
                }
                None => return, // mic stopped before the socket was ready
            },
            tx = &mut audio_tx_rx => match tx {
                Ok(tx) => break tx,
                Err(_) => return, // connect failed; the sender was dropped
            },
        }
    };
    for chunk in backlog {
        if audio_tx.send(chunk).await.is_err() {
            return;
        }
    }
    while let Some(chunk) = mic_rx.recv().await {
        if audio_tx.send(chunk).await.is_err() {
            break;
        }
    }
}

/// How long a session may run without any transcript before it is torn down (instead of streaming a dead mic until the user gives up).
/// The first transcript disarms it, so long dictation with pauses is unaffected.
#[cfg(any(test, feature = "audio"))]
pub(crate) const NO_SPEECH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Message and permission guidance for a session torn down by [`NO_SPEECH_TIMEOUT`].
/// A denied grant is indistinguishable from not speaking because macOS may return silence instead of an error.
#[cfg(any(test, feature = "audio"))]
pub(crate) fn no_speech_error() -> (String, Option<String>) {
    (
        "No speech was detected. Voice stopped.".to_owned(),
        Some(crate::probe::mic_fix_help().to_owned()),
    )
}

#[cfg(feature = "audio")]
async fn start_capture_session(
    config: &VoiceConfig,
    routes: &SttRoutes,
    events: &SessionEvents,
) -> Result<ActivePtt, VoiceError> {
    // Open the mic concurrently with the bearer fetch and the connect handshake (TLS, WebSocket, `transcript.created`)
    // Both legs take hundreds of ms and used to run in series before any capture, clipping the first word of a hold
    let (mic_tx, mic_rx) = mpsc::channel::<Vec<u8>>(64);
    let sample_rate = config.sample_rate;
    // `spawn_pcm_capture` blocks until the device opens; keep it off the runtime.
    let capture_task =
        tokio::task::spawn_blocking(move || crate::audio::spawn_pcm_capture(sample_rate, mic_tx));

    // Drain mic before connect resolves so capture never backpressures while the socket comes up
    let (audio_tx_tx, audio_tx_rx) = tokio::sync::oneshot::channel::<mpsc::Sender<Vec<u8>>>();
    tokio::spawn(forward_pcm(mic_rx, audio_tx_rx));

    let connect = async {
        match choose_route(routes.auth.bearer().await, routes.clip_transcriber.as_ref())? {
            RouteChoice::Streaming(bearer) => StreamingSttSession::connect(config, &bearer)
                .await
                .map(RouteChoice::Streaming),
            RouteChoice::Clip(transcriber) => Ok(RouteChoice::Clip(transcriber)),
        }
    };
    let (connect_res, capture_res) = tokio::join!(connect, capture_task);

    // Resolve the mic first so a device/permission failure wins over a socket error
    // The `?` on `connect_res` then drops `capture`, releasing the mic
    let capture = match capture_res {
        Ok(Ok(handle)) => handle,
        Ok(Err(e)) => return Err(e),
        Err(join_err) => {
            return Err(VoiceError::Config(format!(
                "voice capture task failed: {join_err}"
            )));
        }
    };
    let route = connect_res?;
    let started = VoiceEvent::CaptureStarted {
        route: route.kind(),
    };
    let mut stt = match route {
        RouteChoice::Streaming(stt) => stt,
        RouteChoice::Clip(transcriber) => {
            let _ = events.send(started).await;
            return Ok(start_clip_session(
                config,
                transcriber,
                capture,
                audio_tx_tx,
                events,
            ));
        }
    };

    // Hand the live sender to the forwarder; it flushes the backlog then streams.
    let audio_tx = stt
        .audio_sender()
        .ok_or_else(|| VoiceError::Stt("STT audio sender unavailable".into()))?;
    let _ = audio_tx_tx.send(audio_tx);
    let _ = events.send(started).await;

    let (finish_tx, mut finish_rx) = mpsc::channel::<()>(1);

    let mut capture = Some(capture);
    let out = events.clone();
    let reader = tokio::spawn(async move {
        // Stop the mic before signalling end-of-utterance so no stray PCM is queued after `audio.done`
        // Stopping releases the device and drops the capture thread's clone of the audio sender. `Option::take` makes it idempotent.
        let stop_capture = |capture: &mut Option<crate::audio::CaptureHandle>| {
            if let Some(handle) = capture.take() {
                handle.stop();
            }
        };
        // Tear down when no transcript arrives within the timeout; the first transcript disarms this
        let no_speech_deadline = tokio::time::Instant::now() + NO_SPEECH_TIMEOUT;
        let mut awaiting_speech = true;
        // Stitch those deltas into the live preview so a long pauseless utterance keeps accumulating instead of resetting to the
        // latest ~3s chunk. The committed prompt text only ever comes from `speech_final`. The server produces that as a clean
        // one-pass re-transcription of the whole turn, better than stitched deltas. The prefix resets on each `speech_final`
        let mut locked_prefix = String::new();
        loop {
            tokio::select! {
                msg = finish_rx.recv() => {
                    if msg.is_some() {
                        // User ended the turn; stop the no-speech watchdog.
                        awaiting_speech = false;
                        stop_capture(&mut capture);
                        stt.finish_audio();
                    } else {
                        return;
                    }
                }
                _ = tokio::time::sleep_until(no_speech_deadline), if awaiting_speech => {
                    // Tear down rather than streaming a dead mic until the user stops.
                    stop_capture(&mut capture);
                    stt.finish_audio();
                    let (message, hint) = no_speech_error();
                    let _ = out.send(VoiceEvent::Error { message, hint }).await;
                    return;
                }
                ev = stt.recv() => {
                    match ev {
                        Some(StreamingSttEvent::Partial(p)) => {
                            let text = p.text.trim();
                            if text.is_empty() {
                                continue;
                            }
                            // Real speech arrived: disarm the no-speech watchdog.
                            awaiting_speech = false;

                            let event = if p.speech_final {
                                locked_prefix.clear();
                                VoiceEvent::UtteranceFinal { text: p.text }
                            } else if p.is_final {
                                // Lock this chunk's delta into the running preview.
                                if !locked_prefix.is_empty() {
                                    locked_prefix.push(' ');
                                }
                                locked_prefix.push_str(text);
                                VoiceEvent::InterimTranscript {
                                    text: locked_prefix.clone(),
                                }
                            } else if locked_prefix.is_empty() {
                                VoiceEvent::InterimTranscript {
                                    text: text.to_owned(),
                                }
                            } else {
                                VoiceEvent::InterimTranscript {
                                    text: format!("{locked_prefix} {text}"),
                                }
                            };
                            // The receiver is gone (the pager dropped the channel), so tear down
                            if out.send(event).await.is_err() {
                                return;
                            }
                        }
                        Some(StreamingSttEvent::Done { text }) => {
                            locked_prefix.clear();
                            if !text.trim().is_empty() {
                                awaiting_speech = false;
                                let _ = out.send(VoiceEvent::UtteranceFinal { text }).await;
                            }
                        }
                        Some(StreamingSttEvent::Error { message }) => {
                            let _ = out.send(VoiceEvent::Error { message, hint: None }).await;
                            return;
                        }
                        Some(StreamingSttEvent::Ready) | None => return,
                    }
                }
            }
        }
    });

    Ok(ActivePtt { finish_tx, reader })
}

/// The capture handle stays owned by the reader task, as on the streaming route, so aborting the task releases the mic.
#[cfg(feature = "audio")]
fn start_clip_session(
    config: &VoiceConfig,
    transcriber: SharedClipTranscriber,
    capture: crate::audio::CaptureHandle,
    audio_tx_tx: tokio::sync::oneshot::Sender<mpsc::Sender<Vec<u8>>>,
    events: &SessionEvents,
) -> ActivePtt {
    use crate::clip::{Interims, max_clip_bytes, run_clip_reader};

    let (clip_tx, mut clip_rx) = mpsc::channel::<Vec<u8>>(64);
    let _ = audio_tx_tx.send(clip_tx);
    let (finish_tx, mut finish_rx) = mpsc::channel::<()>(1);

    let sample_rate = config.sample_rate;
    let language = crate::language::language_for_api(&config.language).to_owned();
    let max_bytes = max_clip_bytes(sample_rate);
    let out = events.clone();
    let reader = tokio::spawn(async move {
        // `stop` joins the capture thread, so it runs off the runtime
        let stop_capture = || async move {
            if let Err(error) = tokio::task::spawn_blocking(move || capture.stop()).await {
                tracing::warn!(%error, "voice: capture stop task failed");
            }
        };
        let interims = Interims::new(transcriber, sample_rate, language, out);
        run_clip_reader(
            &mut clip_rx,
            &mut finish_rx,
            max_bytes,
            stop_capture,
            interims,
        )
        .await;
    });

    ActivePtt { finish_tx, reader }
}

#[cfg(test)]
mod start_race_tests {
    use super::*;

    fn events() -> (SessionEvents, mpsc::Receiver<TaggedVoiceEvent>) {
        let (tx, rx) = mpsc::channel(4);
        (
            SessionEvents {
                session: VoiceSessionId(7),
                tx,
            },
            rx,
        )
    }

    /// Exactly `CaptureCancelled`, stamped with the session; never a `CaptureStarted` the pager would wait on.
    #[tokio::test]
    async fn release_during_start_is_reported_as_cancelled() {
        for cancel in [VoiceCommand::PttRelease, VoiceCommand::Abort] {
            let (cmd_tx, mut cmd_rx) = mpsc::channel(4);
            let (events, mut rx) = events();
            cmd_tx.send(cancel).await.unwrap();

            let outcome =
                race_start_with_next_command(std::future::pending(), &mut cmd_rx, &events).await;

            assert!(matches!(outcome, StartOutcome::Cancelled));
            assert_eq!(
                Some(TaggedVoiceEvent {
                    session: VoiceSessionId(7),
                    event: VoiceEvent::CaptureCancelled,
                }),
                rx.recv().await
            );
            drop(events);
            assert_eq!(None, rx.recv().await, "nothing else is sent for the press");
        }
    }

    #[tokio::test]
    async fn completed_start_is_kept_over_a_queued_release() {
        let (cmd_tx, mut cmd_rx) = mpsc::channel(4);
        let (events, mut rx) = events();
        cmd_tx.send(VoiceCommand::PttRelease).await.unwrap();

        let outcome =
            race_start_with_next_command(std::future::ready(None), &mut cmd_rx, &events).await;

        assert!(matches!(outcome, StartOutcome::Opened(None)));
        assert_eq!(
            Ok(VoiceCommand::PttRelease),
            cmd_rx.try_recv(),
            "the release stays queued for the session that just opened"
        );
        drop(events);
        assert_eq!(None, rx.recv().await);
    }

    /// Repeated releases for one session never block the command loop: the slot the reader stopped draining is left
    /// full and the release is dropped.
    #[tokio::test]
    async fn repeated_releases_do_not_block_the_command_loop() {
        let (finish_tx, _finish_rx) = mpsc::channel::<()>(1);
        let session = ActivePtt {
            finish_tx,
            reader: tokio::spawn(std::future::pending()),
        };
        for _ in 0..3 {
            release(&session);
        }
        session.reader.abort();
    }

    #[tokio::test]
    async fn shutdown_closed_channel_and_repress_end_the_start() {
        let (cmd_tx, mut cmd_rx) = mpsc::channel(4);
        let (events, _rx) = events();
        cmd_tx.send(VoiceCommand::Shutdown).await.unwrap();
        let outcome =
            race_start_with_next_command(std::future::pending(), &mut cmd_rx, &events).await;
        assert!(matches!(outcome, StartOutcome::Shutdown));

        cmd_tx
            .send(VoiceCommand::PttPress {
                session: VoiceSessionId(9),
            })
            .await
            .unwrap();
        let outcome =
            race_start_with_next_command(std::future::pending(), &mut cmd_rx, &events).await;
        assert!(matches!(
            outcome,
            StartOutcome::PressedAgain(VoiceSessionId(9))
        ));

        drop(cmd_tx);
        let outcome =
            race_start_with_next_command(std::future::pending(), &mut cmd_rx, &events).await;
        assert!(matches!(outcome, StartOutcome::Shutdown));
    }
}

#[cfg(test)]
mod route_tests {
    use std::future::{Future, ready};
    use std::pin::Pin;
    use std::sync::Arc;

    use super::*;
    use crate::transcriber::{AudioClip, ClipTranscriber, TranscribeError};

    #[derive(Debug)]
    struct StubTranscriber;

    impl ClipTranscriber for StubTranscriber {
        fn transcribe(
            &self,
            _clip: AudioClip,
        ) -> Pin<Box<dyn Future<Output = Result<String, TranscribeError>> + Send + '_>> {
            Box::pin(ready(Ok(String::new())))
        }
    }

    fn stub() -> SharedClipTranscriber {
        Arc::new(StubTranscriber)
    }

    #[test]
    fn xai_bearer_keeps_the_streaming_route() {
        let route = choose_route(Ok("xai-key".to_owned()), Some(&stub())).unwrap();
        assert_eq!(VoiceRoute::Streaming, route.kind());
        assert!(
            matches!(&route, RouteChoice::Streaming(bearer) if bearer == "xai-key"),
            "{route:?}"
        );
    }

    #[test]
    fn foreign_session_with_a_transcriber_takes_the_clip_route() {
        let transcriber = stub();
        let route = choose_route(Err(VoiceAuthError::ForeignSession), Some(&transcriber)).unwrap();
        assert_eq!(VoiceRoute::Clip, route.kind());
        let RouteChoice::Clip(chosen) = route else {
            panic!("expected the clip route: {route:?}");
        };
        assert!(
            Arc::ptr_eq(&chosen, &transcriber),
            "the host's transcriber is the one handed on"
        );
    }

    #[test]
    fn foreign_session_without_a_transcriber_is_the_credential_error() {
        let error = choose_route(Err(VoiceAuthError::ForeignSession), None).unwrap_err();
        assert_eq!(
            VoiceError::Auth(VoiceAuthError::ForeignSession.to_string()).to_string(),
            error.to_string()
        );
    }

    #[test]
    fn not_signed_in_is_an_error_even_with_a_transcriber() {
        let error = choose_route(Err(VoiceAuthError::NotSignedIn), Some(&stub())).unwrap_err();
        assert_eq!(
            VoiceError::Auth(VoiceAuthError::NotSignedIn.to_string()).to_string(),
            error.to_string()
        );
    }
}

#[cfg(all(test, feature = "audio"))]
mod tests {
    use super::*;

    /// Chunks captured before the STT sender arrives are flushed ahead of the live stream, with nothing reordered or dropped across the handoff.
    #[tokio::test]
    async fn forward_pcm_delivers_buffered_then_live_in_order() {
        let (mic_tx, mic_rx) = mpsc::channel::<Vec<u8>>(8);
        let (tx_tx, tx_rx) = tokio::sync::oneshot::channel();
        let (audio_tx, mut audio_rx) = mpsc::channel::<Vec<u8>>(8);
        let task = tokio::spawn(forward_pcm(mic_rx, tx_rx));

        // These chunks buffer before the live sender is handed over, then flush once it arrives
        // (Keep `mic_tx` open across the handoff: a mic that closes before the socket is ready discards the backlog; see the separate test.)
        mic_tx.send(vec![1]).await.unwrap();
        mic_tx.send(vec![2]).await.unwrap();
        tx_tx.send(audio_tx).unwrap();
        assert_eq!(audio_rx.recv().await, Some(vec![1]));
        assert_eq!(audio_rx.recv().await, Some(vec![2]));

        // Later chunks stream live, still in order
        mic_tx.send(vec![3]).await.unwrap();
        assert_eq!(audio_rx.recv().await, Some(vec![3]));

        drop(mic_tx);
        assert_eq!(audio_rx.recv().await, None, "ends when the mic closes");
        task.await.unwrap();
    }

    /// When the mic stops before the socket is ready, the forwarder exits cleanly.
    #[tokio::test]
    async fn forward_pcm_returns_when_mic_closes_before_connect() {
        let (mic_tx, mic_rx) = mpsc::channel::<Vec<u8>>(8);
        let (_tx_tx, tx_rx) = tokio::sync::oneshot::channel::<mpsc::Sender<Vec<u8>>>();
        let task = tokio::spawn(forward_pcm(mic_rx, tx_rx));
        drop(mic_tx);
        task.await.unwrap();
    }

    /// When connect fails (the oneshot sender is dropped without a value), the forwarder exits and discards the buffered audio.
    #[tokio::test]
    async fn forward_pcm_returns_when_connect_fails() {
        let (mic_tx, mic_rx) = mpsc::channel::<Vec<u8>>(8);
        let (tx_tx, tx_rx) = tokio::sync::oneshot::channel::<mpsc::Sender<Vec<u8>>>();
        let task = tokio::spawn(forward_pcm(mic_rx, tx_rx));
        mic_tx.send(vec![1]).await.unwrap();
        drop(tx_tx);
        task.await.unwrap();
    }

    #[test]
    fn no_speech_error_carries_permission_hint() {
        let (message, hint) = no_speech_error();
        assert_eq!(message, "No speech was detected. Voice stopped.");
        assert!(hint.is_some_and(|hint| hint.contains(crate::probe::mic_fix_help())));
    }
}
