use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::watch;

use super::*;
use crate::event::TaggedVoiceEvent;
use crate::pipeline::SessionEvents;
use crate::transcriber::{ClipTranscriber, TranscribeError};
use crate::wav::WAV_HEADER_LEN;

const RATE: u32 = 16_000;

fn secs(seconds: f64) -> usize {
    (seconds * f64::from(RATE) * 2.0).round() as usize
}

fn loud(bytes: usize) -> Vec<u8> {
    std::iter::repeat_n(0x1000i16.to_le_bytes(), bytes / 2)
        .flatten()
        .collect()
}

/// Call index `i` blocks while it is in `held` and `release <= i`; `arrived` is the newest held index that started
/// waiting; `cancelled` counts futures dropped before answering.
struct ScriptedTranscriber {
    seen: Mutex<Vec<AudioClip>>,
    script: Box<dyn Fn(usize, usize) -> Result<String, TranscribeError> + Send + Sync>,
    held: Vec<usize>,
    release: watch::Receiver<usize>,
    arrived: watch::Sender<Option<usize>>,
    cancelled: AtomicUsize,
}

impl std::fmt::Debug for ScriptedTranscriber {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ScriptedTranscriber")
    }
}

struct CancelGuard<'a>(&'a AtomicUsize, bool);

impl Drop for CancelGuard<'_> {
    fn drop(&mut self) {
        if !self.1 {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
}

impl ScriptedTranscriber {
    fn shared(
        script: impl Fn(usize, usize) -> Result<String, TranscribeError> + Send + Sync + 'static,
        held: Vec<usize>,
    ) -> (Arc<Self>, watch::Sender<usize>) {
        let (tx, rx) = watch::channel(0);
        let (arrived, _) = watch::channel(None);
        (
            Arc::new(ScriptedTranscriber {
                seen: Mutex::new(Vec::new()),
                script: Box::new(script),
                held,
                release: rx,
                arrived,
                cancelled: AtomicUsize::new(0),
            }),
            tx,
        )
    }

    fn pcm_lens(&self) -> Vec<usize> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .map(|clip| clip.wav.len() - WAV_HEADER_LEN)
            .collect()
    }

    /// Waits for held call `index` to arrive, releases it, then waits for the event it produces.
    async fn release_then_recv(
        &self,
        release: &watch::Sender<usize>,
        events: &mut mpsc::Receiver<TaggedVoiceEvent>,
        index: usize,
    ) -> VoiceEvent {
        let mut arrived = self.arrived.subscribe();
        arrived
            .wait_for(|newest| newest.is_some_and(|newest| newest >= index))
            .await
            .unwrap();
        release.send(index + 1).unwrap();
        events.recv().await.unwrap().event
    }
}

impl ClipTranscriber for ScriptedTranscriber {
    fn transcribe(
        &self,
        clip: AudioClip,
    ) -> Pin<Box<dyn Future<Output = Result<String, TranscribeError>> + Send + '_>> {
        let pcm_len = clip.wav.len() - WAV_HEADER_LEN;
        let index = {
            let mut seen = self.seen.lock().unwrap();
            seen.push(clip);
            seen.len() - 1
        };
        Box::pin(async move {
            let mut guard = CancelGuard(&self.cancelled, false);
            if self.held.contains(&index) {
                self.arrived.send_replace(Some(index));
                let mut release = self.release.clone();
                while *release.borrow() <= index {
                    release.changed().await.unwrap();
                }
            }
            guard.1 = true;
            (self.script)(index, pcm_len)
        })
    }
}

fn indexed(index: usize, _: usize) -> Result<String, TranscribeError> {
    Ok(format!("t{index}"))
}

fn interim(text: &str) -> VoiceEvent {
    VoiceEvent::InterimTranscript {
        text: text.to_owned(),
    }
}

fn final_(text: &str) -> VoiceEvent {
    VoiceEvent::UtteranceFinal {
        text: text.to_owned(),
    }
}

fn error(message: &str) -> VoiceEvent {
    VoiceEvent::Error {
        message: message.to_owned(),
        hint: None,
    }
}

fn indexed_events(interims: std::ops::Range<usize>, last: usize) -> Vec<VoiceEvent> {
    let mut out: Vec<VoiceEvent> = interims.map(|i| interim(&format!("t{i}"))).collect();
    out.push(final_(&format!("t{last}")));
    out
}

fn interims_for(
    transcriber: &Arc<ScriptedTranscriber>,
) -> (Interims, mpsc::Receiver<TaggedVoiceEvent>) {
    let (tx, events) = mpsc::channel(64);
    let shared: SharedClipTranscriber = transcriber.clone();
    (
        Interims::new(shared, RATE, "en".to_owned(), SessionEvents::for_test(tx)),
        events,
    )
}

fn stop_flag() -> (Arc<AtomicBool>, impl FnOnce() -> std::future::Ready<()>) {
    let flag = Arc::new(AtomicBool::new(false));
    let seen = flag.clone();
    (flag, move || {
        seen.store(true, Ordering::SeqCst);
        std::future::ready(())
    })
}

/// 0.1 s of loud audio per paused-time step; `stop = false` leaves collection running and returns `None`.
async fn record_for(
    interims: &mut Interims,
    seconds: f64,
    stop: bool,
) -> Option<(Vec<u8>, ClipEnd)> {
    let (pcm_tx, mut pcm_rx) = mpsc::channel::<Vec<u8>>(256);
    let (finish_tx, mut finish_rx) = mpsc::channel::<()>(1);
    let collect = collect_clip(
        &mut pcm_rx,
        &mut finish_rx,
        usize::MAX,
        || std::future::ready(()),
        Some(interims),
    );
    let feed = async {
        for _ in 0..(seconds * 10.0).round() as usize {
            pcm_tx.send(loud(secs(0.1))).await.unwrap();
            tokio::time::advance(Duration::from_millis(100)).await;
            tokio::task::yield_now().await;
        }
        if stop {
            finish_tx.send(()).await.unwrap();
            drop(pcm_tx);
        }
    };
    if stop {
        let (out, ()) = tokio::join!(collect, feed);
        Some(out)
    } else {
        tokio::select! {
            out = collect => Some(out),
            () = feed => None,
        }
    }
}

fn events_so_far(events: &mut mpsc::Receiver<TaggedVoiceEvent>) -> Vec<VoiceEvent> {
    let mut out = Vec::new();
    while let Ok(tagged) = events.try_recv() {
        out.push(tagged.event);
    }
    out
}

#[test]
fn caps_and_their_notice() {
    assert_eq!(9_600_000, max_clip_bytes(16_000));
    assert_eq!(
        "Recording stopped at the 5-minute limit; the captured part was transcribed.",
        cap_reached_message(16_000)
    );
    let cap = max_clip_bytes(48_000);
    assert_eq!(20 * 1024 * 1024 - WAV_HEADER_LEN, cap);
    assert_eq!(
        20 * 1024 * 1024,
        crate::wav::encode_pcm16_mono_wav(&vec![0; cap], 48_000).len(),
        "a capped clip's WAV is exactly the endpoint's limit"
    );
    assert_eq!(
        "Recording stopped at the 3-minute upload limit; the captured part was transcribed.",
        cap_reached_message(48_000)
    );
}

#[test]
fn silence_and_the_silent_clip_error() {
    assert!(is_silent(&[]));
    assert!(is_silent(&64i16.to_le_bytes()));
    assert!(is_silent(&(-64i16).to_le_bytes()));
    assert!(!is_silent(&65i16.to_le_bytes()));
    assert!(!is_silent(&i16::MIN.to_le_bytes()));

    for (pcm_len, rate, hint_expected) in [
        (0, 16_000, false),
        (16_000 * 2 - 2, 16_000, false),
        (16_000 * 2, 16_000, true),
        (16_000 * 2, 48_000, false),
        (48_000 * 2, 48_000, true),
    ] {
        let (message, hint) = silent_clip_error(pcm_len, rate);
        assert_eq!("No speech was detected. Voice stopped.", message);
        assert_eq!(
            hint_expected,
            hint.is_some_and(|hint| hint.contains(crate::probe::mic_fix_help())),
            "{pcm_len} bytes at {rate} Hz"
        );
    }
}

#[tokio::test]
async fn collect_clip_drains_after_the_stop_and_stops_at_the_cap() {
    for (stop_first, max_bytes, expected) in [
        (true, 1024, (vec![1, 2, 3, 4, 5, 6], ClipEnd::Stopped)),
        (false, 4, (vec![1, 2, 3, 4], ClipEnd::CapReached)),
        (true, 4, (vec![1, 2, 3, 4], ClipEnd::Stopped)),
        (false, 1024, (vec![1, 2, 3, 4, 5, 6], ClipEnd::CaptureEnded)),
    ] {
        let (pcm_tx, mut pcm_rx) = mpsc::channel::<Vec<u8>>(8);
        let (finish_tx, mut finish_rx) = mpsc::channel::<()>(1);
        let (stopped, stop) = stop_flag();
        if stop_first {
            finish_tx.send(()).await.unwrap();
        }
        pcm_tx.send(vec![1, 2, 3]).await.unwrap();
        pcm_tx.send(vec![4, 5, 6]).await.unwrap();
        drop(pcm_tx);
        let out = collect_clip(&mut pcm_rx, &mut finish_rx, max_bytes, stop, None).await;
        assert_eq!(expected, out, "stop_first={stop_first} max={max_bytes}");
        assert!(
            stopped.load(Ordering::SeqCst),
            "the mic is closed in every case"
        );
    }
}

/// A mic close that never returns, and a forwarder that never drops its sender after the stop, each cost at most
/// [`STOP_DRAIN_TIMEOUT`]; the clip is what arrived by then.
#[tokio::test(start_paused = true)]
async fn stop_and_drain_are_bounded() {
    let (pcm_tx, mut pcm_rx) = mpsc::channel::<Vec<u8>>(8);
    let (finish_tx, mut finish_rx) = mpsc::channel::<()>(1);
    finish_tx.send(()).await.unwrap();
    pcm_tx.send(vec![1, 2]).await.unwrap();
    let started = Instant::now();
    let out = tokio::time::timeout(
        STOP_DRAIN_TIMEOUT * 4,
        collect_clip(
            &mut pcm_rx,
            &mut finish_rx,
            1024,
            std::future::pending::<()>,
            None,
        ),
    )
    .await
    .expect("a hung mic close does not hang the clip");
    assert_eq!((vec![1, 2], ClipEnd::Stopped), out);
    assert_eq!(
        STOP_DRAIN_TIMEOUT,
        started.elapsed(),
        "one deadline covers the close and the drain"
    );

    let (pcm_tx, mut pcm_rx) = mpsc::channel::<Vec<u8>>(8);
    let (finish_tx, mut finish_rx) = mpsc::channel::<()>(1);
    finish_tx.send(()).await.unwrap();
    pcm_tx.send(vec![3]).await.unwrap();
    let started = Instant::now();
    let out = tokio::time::timeout(
        STOP_DRAIN_TIMEOUT * 4,
        collect_clip(
            &mut pcm_rx,
            &mut finish_rx,
            1024,
            || std::future::ready(()),
            None,
        ),
    )
    .await
    .expect("a sender that is never dropped does not hang the drain");
    assert_eq!((vec![3], ClipEnd::Stopped), out);
    assert_eq!(STOP_DRAIN_TIMEOUT, started.elapsed());
    drop(pcm_tx);
}

#[tokio::test]
async fn transcribe_and_emit_uploads_wav_and_emits_one_terminal_event() {
    for (reply, expected, landed) in [
        (Ok("hello world".to_owned()), final_("hello world"), true),
        (
            Ok("  ".to_owned()),
            error("No speech was detected. Voice stopped."),
            false,
        ),
        (
            Err(TranscribeError::new("Upgrade to Pro.")),
            error("Upgrade to Pro."),
            false,
        ),
    ] {
        let (transcriber, _release) =
            ScriptedTranscriber::shared(move |_, _| reply.clone(), vec![]);
        let shared: SharedClipTranscriber = transcriber.clone();
        let (tx, mut events) = mpsc::channel(4);
        let out = SessionEvents::for_test(tx);
        let pcm = loud(32);

        assert_eq!(
            landed,
            transcribe_and_emit(&shared, &pcm, RATE, "en".to_owned(), &out).await,
            "{expected:?}"
        );
        let seen = transcriber.seen.lock().unwrap();
        let [clip] = seen.as_slice() else {
            panic!("one upload expected: {seen:?}");
        };
        assert_eq!("en", clip.language);
        assert_eq!(pcm.as_slice(), clip.wav.get(WAV_HEADER_LEN..).unwrap());
        drop(out);
        assert_eq!(vec![expected], events_so_far(&mut events));
    }
}

/// A host transcriber that never answers the final is cut off at [`FINAL_TIMEOUT`] with an error, not left hanging.
#[tokio::test(start_paused = true)]
async fn final_request_is_bounded_when_the_transcriber_never_answers() {
    let (transcriber, _release) = ScriptedTranscriber::shared(indexed, vec![0]);
    let shared: SharedClipTranscriber = transcriber.clone();
    let (tx, mut events) = mpsc::channel(4);
    let out = SessionEvents::for_test(tx);
    let started = Instant::now();
    assert!(!transcribe_and_emit(&shared, &loud(32), RATE, "en".to_owned(), &out).await);
    assert_eq!(FINAL_TIMEOUT, started.elapsed());
    assert_eq!(
        vec![error("Transcription timed out. Try again in a moment.")],
        events_so_far(&mut events)
    );
}

/// A five-second clip: the first interim goes out 0.6 s in, then every 0.8 s (0.6, 1.4, 2.2, 3.0, 3.8, 4.6 s), each
/// carrying the clip so far; the final after stop re-transcribes the whole clip because 0.4 s of audio followed the
/// last snapshot.
#[tokio::test(start_paused = true)]
async fn interims_grow_with_the_clip_and_the_final_follows() {
    let (transcriber, _release) = ScriptedTranscriber::shared(indexed, vec![]);
    let (mut interims, mut events) = interims_for(&transcriber);
    assert_eq!(Duration::from_millis(800), interims.interval(secs(6.4)));
    assert_eq!(Duration::from_millis(7500), interims.interval(secs(60.0)));

    let (pcm, end) = record_for(&mut interims, 5.0, true).await.unwrap();
    assert_eq!((secs(5.0), ClipEnd::Stopped), (pcm.len(), end));
    let snapshots: Vec<usize> = [0.6, 1.4, 2.2, 3.0, 3.8, 4.6].map(secs).to_vec();
    assert_eq!(snapshots, transcriber.pcm_lens());

    assert_eq!(Some(FinalPath::Full), interims.finish(&pcm).await);
    assert_eq!(
        Some(&secs(5.0)),
        transcriber.pcm_lens().last(),
        "full-clip final"
    );
    assert_eq!(indexed_events(0..6, 6), events_so_far(&mut events));
}

#[tokio::test(start_paused = true)]
async fn ticks_are_skipped_while_an_interim_is_in_flight() {
    let (transcriber, release) = ScriptedTranscriber::shared(indexed, vec![0]);
    let (mut interims, mut events) = interims_for(&transcriber);
    let (pcm_tx, mut pcm_rx) = mpsc::channel::<Vec<u8>>(256);
    let (finish_tx, mut finish_rx) = mpsc::channel::<()>(1);
    let collect = collect_clip(
        &mut pcm_rx,
        &mut finish_rx,
        usize::MAX,
        || std::future::ready(()),
        Some(&mut interims),
    );
    let feed = async {
        let step = || async {
            pcm_tx.send(loud(secs(0.1))).await.unwrap();
            tokio::time::advance(Duration::from_millis(100)).await;
            tokio::task::yield_now().await;
        };
        for _ in 0..70 {
            step().await;
        }
        assert_eq!(
            vec![secs(0.6)],
            transcriber.pcm_lens(),
            "held: no second request in 7 s"
        );
        release.send(1).unwrap();
        // The reply lands and the overdue tick fires once; the one after it is an eighth of 7 s out
        for _ in 0..7 {
            step().await;
        }
        assert_eq!(
            2,
            transcriber.pcm_lens().len(),
            "{:?}",
            transcriber.pcm_lens()
        );
        finish_tx.send(()).await.unwrap();
        drop(pcm_tx);
    };
    let ((_, end), ()) = tokio::join!(collect, feed);
    assert_eq!(ClipEnd::Stopped, end);
    assert_eq!(
        vec![interim("t0"), interim("t1")],
        events_so_far(&mut events)
    );
}

/// No request goes out for a clip past the interim ceiling, for silence, for a snapshot under 0.6 s, or for a
/// buffer already attempted (a stalled mic), whether that attempt delivered, came back empty, or failed.
#[tokio::test(start_paused = true)]
async fn snapshots_not_worth_a_request_are_skipped() {
    type Script = fn(usize, usize) -> Result<String, TranscribeError>;
    let empty: Script = |_, _| Ok(String::new());
    let failing: Script = |_, _| Err(TranscribeError::new("boom"));
    let cases: [(Vec<u8>, Script, usize, usize, &str); 6] = [
        (
            loud(secs(INTERIM_MAX_CLIP.as_secs_f64() + 1.0)),
            indexed,
            0,
            0,
            "past the ceiling",
        ),
        (vec![0; secs(3.0)], indexed, 0, 0, "silent"),
        (loud(secs(0.5)), indexed, 0, 0, "too short"),
        (
            loud(secs(1.0)),
            indexed,
            1,
            1,
            "unchanged since the last snapshot",
        ),
        (
            loud(secs(1.0)),
            empty,
            1,
            0,
            "unchanged after an empty reply",
        ),
        (
            loud(secs(1.0)),
            failing,
            1,
            0,
            "unchanged after a failed reply",
        ),
    ];
    for (pcm, script, requests, expected_events, why) in cases {
        let (transcriber, _release) = ScriptedTranscriber::shared(script, vec![]);
        let (mut interims, mut events) = interims_for(&transcriber);
        let (pcm_tx, mut pcm_rx) = mpsc::channel::<Vec<u8>>(8);
        let (finish_tx, mut finish_rx) = mpsc::channel::<()>(1);
        pcm_tx.send(pcm).await.unwrap();
        let collect = collect_clip(
            &mut pcm_rx,
            &mut finish_rx,
            usize::MAX,
            || std::future::ready(()),
            Some(&mut interims),
        );
        let feed = async {
            for _ in 0..5 {
                tokio::time::advance(Duration::from_secs(1)).await;
                tokio::task::yield_now().await;
            }
            finish_tx.send(()).await.unwrap();
            drop(pcm_tx);
        };
        let _ = tokio::join!(collect, feed);
        assert_eq!(
            requests,
            transcriber.pcm_lens().len(),
            "{why}: {:?}",
            transcriber.pcm_lens()
        );
        assert_eq!(expected_events, events_so_far(&mut events).len(), "{why}");
    }
}

/// A fatal refusal on an interim (plan gate, sign-in) ends the session on that tick: the mic closes, the one `Error`
/// carries the refusal, and no `Transcribing` or final follows. A transient failure keeps recording.
#[tokio::test(start_paused = true)]
async fn fatal_interim_refusal_ends_the_session_at_once() {
    let (transcriber, _release) = ScriptedTranscriber::shared(
        |_, _| Err(TranscribeError::fatal("Upgrade to Pro.")),
        vec![],
    );
    let (interims, mut events) = interims_for(&transcriber);
    let (pcm_tx, mut pcm_rx) = mpsc::channel::<Vec<u8>>(256);
    let (_finish_tx, mut finish_rx) = mpsc::channel::<()>(1);
    let (stopped, stop) = stop_flag();
    let started = Instant::now();
    let reader = run_clip_reader(&mut pcm_rx, &mut finish_rx, usize::MAX, stop, interims);
    let feed = async {
        for _ in 0..30 {
            pcm_tx.send(loud(secs(0.1))).await.unwrap();
            tokio::time::advance(Duration::from_millis(100)).await;
            tokio::task::yield_now().await;
        }
    };
    tokio::select! {
        () = reader => {}
        () = feed => panic!("the refusal did not end the session"),
    }
    assert!(stopped.load(Ordering::SeqCst), "the mic is closed");
    assert_eq!(vec![secs(0.6)], transcriber.pcm_lens(), "one request");
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(vec![error("Upgrade to Pro.")], events_so_far(&mut events));

    let (transcriber, _release) =
        ScriptedTranscriber::shared(|_, _| Err(TranscribeError::new("upstream timeout")), vec![]);
    let (mut interims, mut events) = interims_for(&transcriber);
    let (pcm, end) = record_for(&mut interims, 3.0, true).await.unwrap();
    assert_eq!((secs(3.0), ClipEnd::Stopped), (pcm.len(), end));
    assert!(
        transcriber.pcm_lens().len() > 1,
        "transient failures keep recording"
    );
    assert!(events_so_far(&mut events).is_empty());
}

/// Ten seconds of nothing but silence ends the session with the no-speech error and its microphone hint, as the
/// streaming watchdog does; any speech before the deadline disarms it.
#[tokio::test(start_paused = true)]
async fn no_speech_watchdog_ends_a_silent_recording() {
    for (chunk, ends) in [(vec![0u8; secs(0.1)], true), (loud(secs(0.1)), false)] {
        let (transcriber, _release) = ScriptedTranscriber::shared(indexed, vec![]);
        let (interims, mut events) = interims_for(&transcriber);
        let (pcm_tx, mut pcm_rx) = mpsc::channel::<Vec<u8>>(256);
        let (finish_tx, mut finish_rx) = mpsc::channel::<()>(1);
        let (stopped, stop) = stop_flag();
        let started = Instant::now();
        let reader = run_clip_reader(&mut pcm_rx, &mut finish_rx, usize::MAX, stop, interims);
        let feed = async {
            for _ in 0..150 {
                pcm_tx.send(chunk.clone()).await.unwrap();
                tokio::time::advance(Duration::from_millis(100)).await;
                tokio::task::yield_now().await;
            }
            finish_tx.send(()).await.unwrap();
            drop(pcm_tx);
        };
        let (ended, ()) = tokio::join!(
            async {
                reader.await;
                started.elapsed()
            },
            feed
        );
        assert!(stopped.load(Ordering::SeqCst));
        let events = events_so_far(&mut events);
        if ends {
            assert_eq!(
                crate::pipeline::NO_SPEECH_TIMEOUT,
                ended,
                "cut at the deadline"
            );
            let [VoiceEvent::Error { message, hint }] = events.as_slice() else {
                panic!("{events:?}");
            };
            assert_eq!("No speech was detected. Voice stopped.", message);
            assert!(
                hint.is_some(),
                "ten seconds of silence is a microphone problem"
            );
        } else {
            assert!(
                events.contains(&VoiceEvent::Transcribing),
                "the watchdog disarmed and the stop ran its course: {events:?}"
            );
            assert!(matches!(
                events.last(),
                Some(VoiceEvent::UtteranceFinal { .. })
            ));
        }
    }
}

#[tokio::test(start_paused = true)]
async fn final_reuses_the_delivered_interim_only_when_the_tail_is_short() {
    // Snapshots at 0.6 … 4.6 s
    for (stop_at, requests, path, final_text) in [
        (4.7, 6, FinalPath::ReuseDelivered, "t5"),
        (4.9, 7, FinalPath::Full, "t6"),
    ] {
        let (transcriber, _release) = ScriptedTranscriber::shared(indexed, vec![]);
        let (mut interims, mut events) = interims_for(&transcriber);
        let (pcm, _) = record_for(&mut interims, stop_at, true).await.unwrap();
        assert_eq!(6, transcriber.pcm_lens().len());
        assert_eq!(Some(path), interims.finish(&pcm).await, "stop at {stop_at}");
        assert_eq!(requests, transcriber.pcm_lens().len(), "stop at {stop_at}");
        assert_eq!(
            Some(final_(final_text)),
            events_so_far(&mut events).pop(),
            "stop at {stop_at}"
        );
    }
}

/// An in-flight interim the final could reuse is waited on for at most [`REUSE_WAIT`]; past that the full request
/// goes out and the words settle from it, so a hung interim never adds its whole timeout to release-to-final.
#[tokio::test(start_paused = true)]
async fn hung_reusable_interim_is_overtaken_by_the_full_request() {
    let (transcriber, _release) = ScriptedTranscriber::shared(indexed, vec![5]);
    let (mut interims, mut events) = interims_for(&transcriber);
    let (pcm, _) = record_for(&mut interims, 4.7, true).await.unwrap();
    assert_eq!(6, transcriber.pcm_lens().len());
    let _ = events_so_far(&mut events);

    let stopped = Instant::now();
    assert_eq!(Some(FinalPath::Full), interims.finish(&pcm).await);
    assert_eq!(REUSE_WAIT, stopped.elapsed());
    assert_eq!(7, transcriber.pcm_lens().len(), "the full request went out");
    assert_eq!(vec![final_("t6")], events_so_far(&mut events));
}

#[tokio::test(start_paused = true)]
async fn final_reuses_the_in_flight_interim_when_its_tail_is_short() {
    let (transcriber, release) = ScriptedTranscriber::shared(indexed, vec![5]);
    let (mut interims, mut events) = interims_for(&transcriber);
    let (pcm, _) = record_for(&mut interims, 4.7, true).await.unwrap();
    assert_eq!(6, transcriber.pcm_lens().len());
    assert_eq!(
        5,
        events_so_far(&mut events).len(),
        "the sixth request is still out"
    );

    let (path, event) = tokio::join!(
        interims.finish(&pcm),
        transcriber.release_then_recv(&release, &mut events, 5)
    );
    assert_eq!(Some(FinalPath::ReuseInFlight), path);
    assert_eq!(6, transcriber.pcm_lens().len(), "no final request");
    assert_eq!(final_("t5"), event);
    assert!(events_so_far(&mut events).is_empty());
}

/// The interim in flight at the stop whose tail is too long still lands while the full final runs; when the final
/// answers first the interim is dropped instead.
#[tokio::test(start_paused = true)]
async fn in_flight_interim_at_stop_lands_before_the_final_or_is_dropped() {
    // Request 2 (snapshot 2.2 s) is held, so the later ticks are skipped; the final is request 3
    let (transcriber, release) = ScriptedTranscriber::shared(indexed, vec![2, 3]);
    let (mut interims, mut events) = interims_for(&transcriber);
    let (pcm, _) = record_for(&mut interims, 5.0, true).await.unwrap();
    assert_eq!([0.6, 1.4, 2.2].map(secs).to_vec(), transcriber.pcm_lens());
    assert_eq!(
        vec![interim("t0"), interim("t1")],
        events_so_far(&mut events)
    );
    let (path, ordered) = tokio::join!(interims.finish(&pcm), async {
        let interim = transcriber
            .release_then_recv(&release, &mut events, 2)
            .await;
        let final_ = transcriber
            .release_then_recv(&release, &mut events, 3)
            .await;
        vec![interim, final_]
    });
    assert_eq!(Some(FinalPath::Full), path);
    assert_eq!(Some(&secs(5.0)), transcriber.pcm_lens().last());
    assert_eq!(vec![interim("t2"), final_("t3")], ordered);
    assert!(events_so_far(&mut events).is_empty());

    let (transcriber, release) = ScriptedTranscriber::shared(indexed, vec![2]);
    let (mut interims, mut events) = interims_for(&transcriber);
    let (pcm, _) = record_for(&mut interims, 5.0, true).await.unwrap();
    let _ = events_so_far(&mut events);
    assert_eq!(Some(FinalPath::Full), interims.finish(&pcm).await);
    tokio::task::yield_now().await;
    assert_eq!(vec![final_("t3")], events_so_far(&mut events));
    assert_eq!(1, transcriber.cancelled.load(Ordering::SeqCst));
    release.send(3).unwrap();
    tokio::task::yield_now().await;
    assert!(events_so_far(&mut events).is_empty());
}

/// Dropping the session (the pipeline aborting its reader) cancels the interim in flight, and during the upload the
/// final too; nothing is emitted.
#[tokio::test(start_paused = true)]
async fn abort_cancels_the_in_flight_interim_and_the_final() {
    let (transcriber, _release) = ScriptedTranscriber::shared(indexed, vec![0]);
    let (mut interims, mut events) = interims_for(&transcriber);
    assert!(record_for(&mut interims, 3.0, false).await.is_none());
    assert_eq!(1, transcriber.pcm_lens().len());
    drop(interims);
    tokio::task::yield_now().await;
    assert_eq!(1, transcriber.cancelled.load(Ordering::SeqCst));
    assert!(events_so_far(&mut events).is_empty());

    let (transcriber, _release) = ScriptedTranscriber::shared(indexed, vec![2, 3]);
    let (mut interims, mut events) = interims_for(&transcriber);
    let (pcm, _) = record_for(&mut interims, 5.0, true).await.unwrap();
    let _ = events_so_far(&mut events);
    let finish = tokio::spawn(async move { interims.finish(&pcm).await });
    tokio::task::yield_now().await;
    assert_eq!(
        4,
        transcriber.pcm_lens().len(),
        "the final request went out"
    );
    finish.abort();
    let _ = finish.await;
    tokio::task::yield_now().await;
    assert_eq!(2, transcriber.cancelled.load(Ordering::SeqCst));
    assert!(events_so_far(&mut events).is_empty());
}

/// Failed and empty interim replies are ignored (no toast, no event); later interims and the final proceed.
#[tokio::test(start_paused = true)]
async fn interim_failures_and_empty_replies_are_ignored() {
    let (transcriber, _release) = ScriptedTranscriber::shared(
        |index, len| match index {
            0 => Err(TranscribeError::new("Upgrade to Pro.")),
            1 => Ok("   ".to_owned()),
            _ => indexed(index, len),
        },
        vec![],
    );
    let (mut interims, mut events) = interims_for(&transcriber);
    let (pcm, _) = record_for(&mut interims, 5.0, true).await.unwrap();
    assert_eq!(Some(FinalPath::Full), interims.finish(&pcm).await);
    assert_eq!(indexed_events(2..6, 6), events_so_far(&mut events));
}

/// Every way the mic closes ends the session with `Transcribing` and then exactly one final or error, so the pager
/// never waits on a reader that has already gone: the user's stop, the cap (final first, then the notice), and the
/// capture ending on its own. A silent clip is the error alone.
#[tokio::test(start_paused = true)]
async fn reader_ends_every_session_with_transcribing_then_one_terminal_event() {
    enum Close {
        Stop,
        Cap,
        CaptureEnded,
        SilentStop,
    }
    for (close, expected) in [
        (Close::Stop, vec![VoiceEvent::Transcribing, final_("t0")]),
        (
            Close::Cap,
            vec![
                VoiceEvent::Transcribing,
                final_("t0"),
                VoiceEvent::Notice {
                    message: cap_reached_message(RATE),
                },
            ],
        ),
        (
            Close::CaptureEnded,
            vec![VoiceEvent::Transcribing, final_("t0")],
        ),
        (
            Close::SilentStop,
            vec![error("No speech was detected. Voice stopped.")],
        ),
    ] {
        let (transcriber, _release) = ScriptedTranscriber::shared(indexed, vec![]);
        let (interims, mut events) = interims_for(&transcriber);
        let (pcm_tx, mut pcm_rx) = mpsc::channel::<Vec<u8>>(8);
        let (finish_tx, mut finish_rx) = mpsc::channel::<()>(1);
        let (stopped, stop) = stop_flag();
        let chunk = match close {
            Close::SilentStop => vec![0; secs(0.2)],
            Close::Stop | Close::Cap | Close::CaptureEnded => loud(secs(0.2)),
        };
        let max_bytes = match close {
            Close::Cap => chunk.len(),
            Close::Stop | Close::CaptureEnded | Close::SilentStop => usize::MAX,
        };
        pcm_tx.send(chunk).await.unwrap();
        if matches!(close, Close::Stop | Close::SilentStop) {
            finish_tx.send(()).await.unwrap();
        }
        // Dropping the sender is how the forwarder reports a mic that died; the stop channel stays open
        drop(pcm_tx);
        run_clip_reader(&mut pcm_rx, &mut finish_rx, max_bytes, stop, interims).await;
        assert!(stopped.load(Ordering::SeqCst), "the mic is closed");
        assert_eq!(expected, events_so_far(&mut events));
    }
}
