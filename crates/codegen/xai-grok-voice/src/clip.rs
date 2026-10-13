//! Clip route: buffer the whole recording, transcribe it in one call. Taken when the streaming socket has no usable
//! bearer but the host supplied a [`ClipTranscriber`]. Interims re-transcribe the clip so far; an all-silent clip is
//! refused locally because a denied mic grant records silence on macOS.
//!
//! Every exit ends the session with exactly one terminal event: [`VoiceEvent::Transcribing`] marks the mic closed
//! (the user's stop, the cap, capture ending on its own) and one [`VoiceEvent::UtteranceFinal`] or
//! [`VoiceEvent::Error`] follows; a silent clip, a fatal interim refusal, and no speech by the watchdog emit the
//! `Error` alone. The consumer never waits on a reader that has already gone.

use std::future::Future;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_util::task::AbortOnDropHandle;

use crate::event::VoiceEvent;
use crate::pipeline::SessionEvents;
use crate::transcriber::{AudioClip, FINAL_TIMEOUT, SharedClipTranscriber, TranscribeError};
use crate::wav::encode_pcm16_mono_wav;

/// Longest recording the clip route accepts. At 16 kHz mono PCM16 this is 9.6 MB, and long past any dictated prompt.
pub(crate) const MAX_CLIP_DURATION: Duration = Duration::from_secs(5 * 60);

/// Largest WAV the transcription endpoint accepts (20 MB). At sample rates above 16 kHz this, not the duration, is
/// the binding cap; a clip is cut here rather than recorded in full and refused on upload.
const MAX_CLIP_UPLOAD_BYTES: usize = 20 * 1024 * 1024;

/// Peak |sample| at or below which a PCM16 clip counts as silence. Real speech peaks in the thousands; a denied
/// mic grant yields exact zeros.
const SILENCE_PEAK: i16 = 64;

pub(crate) fn max_clip_bytes(sample_rate: u32) -> usize {
    let bytes_per_second = u64::from(sample_rate) * 2;
    let by_duration =
        usize::try_from(bytes_per_second * MAX_CLIP_DURATION.as_secs()).unwrap_or(usize::MAX);
    by_duration.min(MAX_CLIP_UPLOAD_BYTES - crate::wav::WAV_HEADER_LEN)
}

/// Delay from the mic opening to the first interim request, and the shortest snapshot worth one: a single constant so
/// the first tick can always fire.
pub(crate) const INTERIM_FIRST: Duration = Duration::from_millis(600);

/// Shortest gap between two interim re-transcriptions. The gap grows with the clip ([`Interims::interval`]) so a
/// long dictation is not re-uploaded whole every second.
pub(crate) const INTERIM_INTERVAL: Duration = Duration::from_millis(800);

/// Interims stop past this much audio; re-uploading a longer clip every few seconds costs more than it shows.
pub(crate) const INTERIM_MAX_CLIP: Duration = Duration::from_secs(90);

/// When the audio recorded after an interim's snapshot is shorter than this, that interim's transcript is promoted
/// to the final without another request, so the words settle as soon as the mic closes instead of one round trip
/// later. The tradeoff: a syllable spoken inside that window is lost, the same tradeoff the streaming route makes
/// when its socket closes on release.
pub(crate) const FINAL_REUSE_TAIL: Duration = Duration::from_millis(250);

/// An interim that takes longer than this is abandoned; the next tick tries again with more audio.
pub(crate) const INTERIM_TIMEOUT: Duration = Duration::from_secs(20);

/// How long the final waits for an interim already in flight whose snapshot it could reuse; past this the full
/// request goes out and the interim, if it still answers, lands as an interim.
pub(crate) const REUSE_WAIT: Duration = Duration::from_millis(1500);

/// Bound on closing the mic and draining the audio queued behind a stop. Both normally take milliseconds; a hung
/// recorder process must not hold the user's recording hostage, so on expiry the clip is what arrived so far.
pub(crate) const STOP_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

struct InFlight {
    snapshot_len: usize,
    task: AbortOnDropHandle<Result<String, TranscribeError>>,
}

type InterimReply = Result<Result<String, TranscribeError>, tokio::task::JoinError>;

struct Delivered {
    snapshot_len: usize,
    text: String,
}

/// Progressive interims for one clip session: every so often the audio buffered so far is uploaded as its own clip
/// and the transcript is emitted as [`VoiceEvent::InterimTranscript`], the same event the streaming route sends.
///
/// One request in flight at a time; a tick that finds one running, or no new audio since the last attempted
/// snapshot, is skipped. Transient failures and empty transcripts are logged and ignored (the final after stop has
/// its own error path); a fatal refusal is kept in `refused` so collection can end the session at once.
/// [`Interims::finish`] reuses an interim whose tail is under [`FINAL_REUSE_TAIL`], else runs a full-clip request.
pub(crate) struct Interims {
    transcriber: SharedClipTranscriber,
    sample_rate: u32,
    language: String,
    out: SessionEvents,
    delivered: Option<Delivered>,
    in_flight: Option<InFlight>,
    attempted_len: Option<usize>,
    refused: Option<TranscribeError>,
    next_at: Instant,
}

/// Which path produced the final; the `Reuse*` paths skipped a request because the audio after that snapshot was
/// under [`FINAL_REUSE_TAIL`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FinalPath {
    ReuseDelivered,
    ReuseInFlight,
    Full,
}

impl Interims {
    pub(crate) fn new(
        transcriber: SharedClipTranscriber,
        sample_rate: u32,
        language: String,
        out: SessionEvents,
    ) -> Self {
        Interims {
            transcriber,
            sample_rate,
            language,
            out,
            delivered: None,
            in_flight: None,
            attempted_len: None,
            refused: None,
            next_at: Instant::now() + INTERIM_FIRST,
        }
    }

    fn duration_of(&self, pcm_len: usize) -> Duration {
        Duration::from_secs_f64(pcm_len as f64 / (f64::from(self.sample_rate) * 2.0))
    }

    pub(crate) fn interval(&self, pcm_len: usize) -> Duration {
        INTERIM_INTERVAL.max(self.duration_of(pcm_len) / 8)
    }

    fn tick_armed(&self, pcm_len: usize) -> bool {
        self.in_flight.is_none() && self.duration_of(pcm_len) <= INTERIM_MAX_CLIP
    }

    fn tick(&mut self, pcm: &[u8]) {
        self.next_at = Instant::now() + self.interval(pcm.len());
        if self.duration_of(pcm.len()) < INTERIM_FIRST
            || self.attempted_len == Some(pcm.len())
            || is_silent(pcm)
        {
            return;
        }
        self.attempted_len = Some(pcm.len());
        let clip = AudioClip {
            wav: encode_pcm16_mono_wav(pcm, self.sample_rate),
            language: self.language.clone(),
        };
        let transcriber = self.transcriber.clone();
        let task = AbortOnDropHandle::new(tokio::spawn(async move {
            match tokio::time::timeout(INTERIM_TIMEOUT, transcriber.transcribe(clip)).await {
                Ok(result) => result,
                Err(_) => Err(TranscribeError::new("interim transcription timed out")),
            }
        }));
        self.in_flight = Some(InFlight {
            snapshot_len: pcm.len(),
            task,
        });
    }

    async fn finished(&mut self, joined: InterimReply) {
        let Some(in_flight) = self.in_flight.take() else {
            return;
        };
        match joined {
            Ok(Ok(text)) if !text.trim().is_empty() => {
                self.delivered = Some(Delivered {
                    snapshot_len: in_flight.snapshot_len,
                    text: text.clone(),
                });
                let _ = self.out.send(VoiceEvent::InterimTranscript { text }).await;
            }
            Ok(Ok(_)) => {}
            Ok(Err(error)) if error.fatal => {
                self.refused = Some(error);
            }
            Ok(Err(error)) => {
                tracing::debug!(%error, "voice: interim transcription failed");
            }
            Err(error) => {
                tracing::debug!(%error, "voice: interim task did not complete");
            }
        }
    }

    fn tail_after(&self, snapshot_len: usize, pcm_len: usize) -> Duration {
        self.duration_of(pcm_len.saturating_sub(snapshot_len))
    }

    /// The mic has closed and `pcm` is the whole clip: emits the session's one final (or its error). `None` means
    /// the error was emitted, so a caller's follow-up notice never papers over a failure.
    pub(crate) async fn finish(mut self, pcm: &[u8]) -> Option<FinalPath> {
        let stopped = Instant::now();
        let path = self.finish_inner(pcm).await;
        tracing::debug!(
            ?path,
            release_to_final_ms = stopped.elapsed().as_millis(),
            "voice: clip final"
        );
        path
    }

    async fn finish_inner(&mut self, pcm: &[u8]) -> Option<FinalPath> {
        if let Some(delivered) = &self.delivered
            && self.tail_after(delivered.snapshot_len, pcm.len()) < FINAL_REUSE_TAIL
        {
            let text = delivered.text.clone();
            let _ = self.out.send(VoiceEvent::UtteranceFinal { text }).await;
            return Some(FinalPath::ReuseDelivered);
        }
        if let Some(mut in_flight) = self.in_flight.take() {
            if self.tail_after(in_flight.snapshot_len, pcm.len()) < FINAL_REUSE_TAIL {
                match tokio::time::timeout(REUSE_WAIT, &mut in_flight.task).await {
                    Ok(Ok(Ok(text))) if !text.trim().is_empty() => {
                        let _ = self.out.send(VoiceEvent::UtteranceFinal { text }).await;
                        return Some(FinalPath::ReuseInFlight);
                    }
                    // Failed or empty reply: fall through to the full final (in_flight already taken)
                    Ok(_) => {}
                    // Still out after REUSE_WAIT: the full request goes ahead, and the interim may still land first
                    Err(_) => self.in_flight = Some(in_flight),
                }
            } else {
                self.in_flight = Some(in_flight);
            }
        }
        // The interim still in flight is delivered while the full final runs, so the words jump ahead before the
        // final corrects them; `biased` so the final wins and an interim unanswered by then is dropped
        let transcriber = self.transcriber.clone();
        let out = self.out.clone();
        let full = transcribe_and_emit(
            &transcriber,
            pcm,
            self.sample_rate,
            self.language.clone(),
            &out,
        );
        let mut full = std::pin::pin!(full);
        loop {
            let in_flight = self.in_flight.as_mut().map(|f| &mut f.task);
            tokio::select! {
                biased;
                landed = &mut full => {
                    self.in_flight = None;
                    return landed.then_some(FinalPath::Full);
                }
                joined = in_flight_finished(in_flight) => self.finished(joined).await,
            }
        }
    }
}

/// Pends forever when nothing is in flight, so it can sit in a `select!`.
async fn in_flight_finished(
    in_flight: Option<&mut AbortOnDropHandle<Result<String, TranscribeError>>>,
) -> InterimReply {
    match in_flight {
        Some(task) => task.await,
        None => std::future::pending().await,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClipEnd {
    Stopped,
    CapReached,
    /// The capture side closed before any stop request.
    CaptureEnded,
    /// An interim came back with a fatal refusal; the clip is not worth uploading again.
    Refused,
    /// Nothing but silence by [`crate::pipeline::NO_SPEECH_TIMEOUT`], as on the streaming route.
    NoSpeech,
}

/// On stop the mic is closed first and the channel drained to its end, so audio still queued behind the stop (the
/// last word) is kept. Closing the mic ends the forwarder, which drops the sender; [`STOP_DRAIN_TIMEOUT`] bounds
/// both steps in case it does not.
pub(crate) async fn collect_clip<F: Future<Output = ()>>(
    pcm_rx: &mut mpsc::Receiver<Vec<u8>>,
    finish_rx: &mut mpsc::Receiver<()>,
    max_bytes: usize,
    stop_capture: impl FnOnce() -> F,
    mut interims: Option<&mut Interims>,
) -> (Vec<u8>, ClipEnd) {
    let mut pcm: Vec<u8> = Vec::new();
    let push = |pcm: &mut Vec<u8>, chunk: &[u8]| {
        let room = max_bytes.saturating_sub(pcm.len());
        pcm.extend_from_slice(chunk.split_at(chunk.len().min(room)).0);
        pcm.len() >= max_bytes
    };
    // Checked once, at the deadline: a denied mic grant records zeros with no other symptom
    let no_speech_deadline = Instant::now() + crate::pipeline::NO_SPEECH_TIMEOUT;
    let mut awaiting_speech = true;
    let end = loop {
        let tick_armed = interims
            .as_deref()
            .is_some_and(|interims| interims.tick_armed(pcm.len()));
        let next_at = interims
            .as_deref()
            .map_or_else(Instant::now, |interims| interims.next_at);
        let in_flight = interims
            .as_deref_mut()
            .and_then(|interims| interims.in_flight.as_mut().map(|f| &mut f.task));
        tokio::select! {
            // Stop first: audio queued behind it is recovered by the drain below, and a stop that raced the mic
            // closing must still read as the user's stop, not as a dead capture
            biased;
            stop = finish_rx.recv() => {
                break if stop.is_some() { ClipEnd::Stopped } else { ClipEnd::CaptureEnded };
            }
            chunk = pcm_rx.recv() => match chunk {
                Some(chunk) => {
                    if push(&mut pcm, &chunk) {
                        break ClipEnd::CapReached;
                    }
                }
                None => break ClipEnd::CaptureEnded,
            },
            joined = in_flight_finished(in_flight) => {
                if let Some(interims) = interims.as_deref_mut() {
                    interims.finished(joined).await;
                    if interims.refused.is_some() {
                        break ClipEnd::Refused;
                    }
                }
            }
            _ = tokio::time::sleep_until(next_at), if tick_armed => {
                if let Some(interims) = interims.as_deref_mut() {
                    interims.tick(&pcm);
                }
            }
            _ = tokio::time::sleep_until(no_speech_deadline), if awaiting_speech => {
                if is_silent(&pcm) {
                    break ClipEnd::NoSpeech;
                }
                awaiting_speech = false;
            }
        }
    };
    let deadline = Instant::now() + STOP_DRAIN_TIMEOUT;
    if tokio::time::timeout_at(deadline, stop_capture())
        .await
        .is_err()
    {
        tracing::warn!("voice: closing the microphone timed out; transcribing what was captured");
    }
    if end == ClipEnd::Stopped {
        let drain = async {
            while let Some(chunk) = pcm_rx.recv().await {
                if push(&mut pcm, &chunk) {
                    break;
                }
            }
        };
        if tokio::time::timeout_at(deadline, drain).await.is_err() {
            tracing::warn!("voice: draining audio after the stop timed out");
        }
    }
    (pcm, end)
}

/// Records until the mic closes, then emits [`VoiceEvent::Transcribing`] and the session's one final or error; a
/// silent clip, a fatal interim refusal, and the no-speech watchdog emit their `Error` alone. On the cap the final
/// lands first and the notice after it, so the captured part is in the prompt when the notice shows; a `Notice`,
/// not an `Error`, because the final already ended the session and the user may be recording again by the time it
/// is applied.
pub(crate) async fn run_clip_reader<F: Future<Output = ()>>(
    pcm_rx: &mut mpsc::Receiver<Vec<u8>>,
    finish_rx: &mut mpsc::Receiver<()>,
    max_bytes: usize,
    stop_capture: impl FnOnce() -> F,
    mut interims: Interims,
) {
    let sample_rate = interims.sample_rate;
    let out = interims.out.clone();
    let (pcm, end) = collect_clip(
        pcm_rx,
        finish_rx,
        max_bytes,
        stop_capture,
        Some(&mut interims),
    )
    .await;
    if let Some(refused) = interims.refused.take() {
        let _ = out
            .send(VoiceEvent::Error {
                message: refused.message,
                hint: None,
            })
            .await;
        return;
    }
    if is_silent(&pcm) {
        let (message, hint) = silent_clip_error(pcm.len(), sample_rate);
        let _ = out.send(VoiceEvent::Error { message, hint }).await;
        return;
    }
    if out.send(VoiceEvent::Transcribing).await.is_err() {
        return;
    }
    let landed = interims.finish(&pcm).await;
    if landed.is_some() && end == ClipEnd::CapReached {
        let _ = out
            .send(VoiceEvent::Notice {
                message: cap_reached_message(sample_rate),
            })
            .await;
    }
}

pub(crate) fn is_silent(pcm: &[u8]) -> bool {
    pcm.as_chunks::<2>()
        .0
        .iter()
        .all(|&pair| i16::from_le_bytes(pair).saturating_abs() <= SILENCE_PEAK)
}

/// Shortest silent clip that counts as evidence of a microphone problem rather than a tap on the capture chord.
const SILENT_CLIP_HINT_AFTER: Duration = Duration::from_secs(1);

/// Same message as the streaming no-speech watchdog; the microphone hint only once the clip is long enough that
/// silence is suspicious, so a quick tap costs a toast, not a permissions walkthrough.
pub(crate) fn silent_clip_error(pcm_len: usize, sample_rate: u32) -> (String, Option<String>) {
    let (message, hint) = crate::pipeline::no_speech_error();
    let hint_after_bytes = u64::from(sample_rate) * 2 * SILENT_CLIP_HINT_AFTER.as_secs();
    let hint = (pcm_len as u64 >= hint_after_bytes)
        .then_some(hint)
        .flatten();
    (message, hint)
}

/// Names the duration only when that was the binding cap at `sample_rate`.
pub(crate) fn cap_reached_message(sample_rate: u32) -> String {
    let seconds = max_clip_bytes(sample_rate) as u64 / (u64::from(sample_rate) * 2).max(1);
    if seconds >= MAX_CLIP_DURATION.as_secs() {
        format!(
            "Recording stopped at the {}-minute limit; the captured part was transcribed.",
            MAX_CLIP_DURATION.as_secs() / 60
        )
    } else {
        format!(
            "Recording stopped at the {}-minute upload limit; the captured part was transcribed.",
            seconds / 60
        )
    }
}

/// Emits exactly one terminal event: a non-empty final, or an error. Returns whether it was the final, so a
/// caller's follow-up notice never papers over a failure.
pub(crate) async fn transcribe_and_emit(
    transcriber: &SharedClipTranscriber,
    pcm: &[u8],
    sample_rate: u32,
    language: String,
    out: &SessionEvents,
) -> bool {
    let clip = AudioClip {
        wav: encode_pcm16_mono_wav(pcm, sample_rate),
        language,
    };
    let result = match tokio::time::timeout(FINAL_TIMEOUT, transcriber.transcribe(clip)).await {
        Ok(result) => result,
        Err(_) => Err(TranscribeError::new(
            "Transcription timed out. Try again in a moment.",
        )),
    };
    let event = match result {
        Ok(text) if !text.trim().is_empty() => VoiceEvent::UtteranceFinal { text },
        Ok(_) => VoiceEvent::Error {
            message: "No speech was detected. Voice stopped.".to_owned(),
            hint: None,
        },
        Err(error) => VoiceEvent::Error {
            message: error.message,
            hint: None,
        },
    };
    let landed = matches!(event, VoiceEvent::UtteranceFinal { .. });
    let _ = out.send(event).await;
    landed
}

#[cfg(test)]
#[path = "clip_tests.rs"]
mod tests;
