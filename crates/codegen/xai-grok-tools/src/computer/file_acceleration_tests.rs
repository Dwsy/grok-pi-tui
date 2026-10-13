use std::{
    path::Path,
    sync::atomic::{AtomicUsize, Ordering},
};

use parking_lot::Mutex;
use pretty_assertions::assert_eq;

use super::*;
use crate::computer::local::MockFs;

const ACCELERATED_PATH: &str = "/accelerated/marker";
const ACCELERATED_CONTENT: &[u8] = b"served by the accelerator";

fn empty_slot() -> AcceleratorSlot {
    AcceleratorSlot(OnceLock::new())
}

fn recording_sink() -> (AccelerationSink, Arc<Mutex<Vec<AccelerationNotice>>>) {
    let notices = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&notices);
    let sink: AccelerationSink = Arc::new(move |notice| recorded.lock().push(notice));
    (sink, notices)
}

fn enabled(routes: Option<&str>, sink: AccelerationSink) -> FileAccelerationGate {
    FileAccelerationGate::Enabled(AccelerationContext {
        routes: routes.map(Arc::from),
        sink,
    })
}

/// Counts its calls and wraps nothing: it returns the fs it was given.
fn counting_factory(calls: Arc<AtomicUsize>) -> FileAcceleratorFactory {
    Box::new(move |fs, _context| {
        calls.fetch_add(1, Ordering::SeqCst);
        AcceleratedFs { fs, session: None }
    })
}

fn ended_notice() -> AccelerationNotice {
    AccelerationNotice::SessionEnded {
        arm: AccelerationArm::Treatment,
        duration_ms: 5,
        served: 1,
        declined: BTreeMap::from([("uncovered", 2)]),
        failed: BTreeMap::new(),
        served_latency: [1, 0, 0, 0, 0, 0, 0],
        saturated: 0,
    }
}

/// Reads a real file through `fs`, which only [`LocalFs`] can answer.
async fn reads_the_local_disk(fs: &dyn AsyncFileSystem) -> bool {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("local");
    std::fs::write(&path, b"on disk").expect("write");
    fs.read_file(&path)
        .await
        .is_ok_and(|content| content == b"on disk")
}

#[tokio::test]
async fn disabled_gate_returns_local_fs_without_calling_the_factory() {
    let calls = Arc::new(AtomicUsize::new(0));
    let slot = empty_slot();
    slot.install(counting_factory(Arc::clone(&calls)));

    let accelerated = slot.local_fs(FileAccelerationGate::Disabled);

    assert_eq!(0, calls.load(Ordering::SeqCst));
    assert!(accelerated.session.is_none());
    assert!(reads_the_local_disk(accelerated.fs.as_ref()).await);
}

#[tokio::test]
async fn enabled_gate_without_a_factory_returns_local_fs() {
    let (sink, notices) = recording_sink();

    let accelerated = empty_slot().local_fs(enabled(Some("routes"), sink));

    assert!(accelerated.session.is_none());
    assert!(reads_the_local_disk(accelerated.fs.as_ref()).await);
    assert_eq!(Vec::<AccelerationNotice>::new(), *notices.lock());
}

#[tokio::test]
async fn enabled_gate_hands_local_fs_and_the_context_to_the_factory() {
    let wrapped: Arc<Mutex<Option<Arc<dyn AsyncFileSystem>>>> = Arc::new(Mutex::new(None));
    let routes_seen: Arc<Mutex<Option<Arc<str>>>> = Arc::new(Mutex::new(None));
    let accelerated_fs = Arc::new(MockFs::new());
    accelerated_fs
        .set_file(ACCELERATED_PATH, ACCELERATED_CONTENT)
        .await;
    let slot = empty_slot();
    slot.install({
        let wrapped = Arc::clone(&wrapped);
        let routes_seen = Arc::clone(&routes_seen);
        let accelerated_fs: Arc<dyn AsyncFileSystem> = accelerated_fs;
        Box::new(move |inner, context| {
            *wrapped.lock() = Some(inner);
            *routes_seen.lock() = context.routes;
            (context.sink)(AccelerationNotice::SessionStarted {
                arm: AccelerationArm::Treatment,
            });
            let sink = context.sink;
            AcceleratedFs {
                fs: Arc::clone(&accelerated_fs),
                session: Some(AccelerationSession::new(move || sink(ended_notice()))),
            }
        })
    });
    let (sink, notices) = recording_sink();

    let accelerated = slot.local_fs(enabled(Some("fuse=on"), sink));

    assert_eq!(
        ACCELERATED_CONTENT,
        accelerated
            .fs
            .read_file(Path::new(ACCELERATED_PATH))
            .await
            .expect("the factory's fs answers")
    );
    let inner = wrapped.lock().take().expect("the factory saw the local fs");
    assert!(reads_the_local_disk(inner.as_ref()).await);
    assert_eq!(Some(Arc::from("fuse=on")), routes_seen.lock().take());
    assert_eq!(
        vec![AccelerationNotice::SessionStarted {
            arm: AccelerationArm::Treatment
        }],
        *notices.lock()
    );

    accelerated
        .session
        .expect("the factory's session is handed back")
        .finish();
    assert_eq!(
        vec![
            AccelerationNotice::SessionStarted {
                arm: AccelerationArm::Treatment
            },
            ended_notice(),
        ],
        *notices.lock()
    );
}

#[test]
fn first_installed_factory_wins() {
    let first_calls = Arc::new(AtomicUsize::new(0));
    let later_calls = Arc::new(AtomicUsize::new(0));
    let slot = empty_slot();
    slot.install(counting_factory(Arc::clone(&first_calls)));
    slot.install(counting_factory(Arc::clone(&later_calls)));
    let (sink, _notices) = recording_sink();

    let accelerated = slot.local_fs(enabled(None, sink));

    assert!(accelerated.session.is_none());
    assert_eq!(1, first_calls.load(Ordering::SeqCst));
    assert_eq!(0, later_calls.load(Ordering::SeqCst));
}

#[test]
fn session_summary_runs_exactly_once_whether_finished_or_dropped() {
    let counting_session = |runs: &Arc<AtomicUsize>| {
        let runs = Arc::clone(runs);
        AccelerationSession::new(move || {
            runs.fetch_add(1, Ordering::SeqCst);
        })
    };

    let finished = Arc::new(AtomicUsize::new(0));
    counting_session(&finished).finish();
    assert_eq!(1, finished.load(Ordering::SeqCst));

    let dropped = Arc::new(AtomicUsize::new(0));
    drop(counting_session(&dropped));
    assert_eq!(1, dropped.load(Ordering::SeqCst));
}
