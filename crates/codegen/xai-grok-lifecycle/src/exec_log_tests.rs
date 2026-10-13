use pretty_assertions::assert_eq;

use super::*;
use crate::test_support::far_future;

#[tokio::test]
async fn log_never_exceeds_the_cap_and_marks_the_cut() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("exec.log");
    let log = ExecLog::new(path.clone());
    log.start_trigger(far_future()).await;
    let chunk = vec![b'x'; 8 * 1024];
    for _ in 0..16 {
        log.append(&chunk).await;
    }
    log.finish_trigger();

    let contents = std::fs::read(&path).expect("read log");
    assert_eq!(EXEC_LOG_CAP_BYTES, contents.len());
    assert!(contents.ends_with(TRUNCATION_MARKER));
}

#[tokio::test]
async fn finish_while_an_append_holds_the_sink_still_closes_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("exec.log");
    let log = ExecLog::new(path.clone());
    log.start_trigger(far_future()).await;
    log.append(b"first trigger\n").await;

    let in_flight = log.sink.lock().await;
    log.finish_trigger();
    drop(in_flight);
    log.append(b"late output\n").await;

    assert_eq!(
        b"first trigger\n".to_vec(),
        std::fs::read(&path).expect("read")
    );
    assert!(log.sink.lock().await.is_none(), "the sink is still open");
}

#[tokio::test]
async fn each_trigger_starts_an_empty_log_and_output_after_finish_is_dropped() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("exec.log");
    let log = ExecLog::new(path.clone());

    log.start_trigger(far_future()).await;
    log.append(b"first trigger\n").await;
    log.finish_trigger();
    log.append(b"late output\n").await;
    assert_eq!(
        b"first trigger\n".to_vec(),
        std::fs::read(&path).expect("read")
    );

    log.start_trigger(far_future()).await;
    log.append(b"second trigger\n").await;
    log.finish_trigger();
    assert_eq!(
        b"second trigger\n".to_vec(),
        std::fs::read(&path).expect("read")
    );
}
