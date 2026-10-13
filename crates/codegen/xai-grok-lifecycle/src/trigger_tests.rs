use std::sync::Arc;
use std::time::Duration;

use pretty_assertions::assert_eq;

use super::*;
use crate::event::HandlerFuture;
use crate::test_support::{FixedHandler, TestBroker, handler_name, request};

fn report(name: &str, outcome: RunOutcome) -> HandlerReport {
    HandlerReport {
        name: handler_name(name),
        source: HandlerSource::Registered,
        outcome,
        elapsed: Duration::from_millis(10),
    }
}

const OK: RunOutcome = RunOutcome::Finished(HandlerOutcome::Ok);
const FAILED: RunOutcome = RunOutcome::Finished(HandlerOutcome::Failed);
const SELF_TIMED_OUT: RunOutcome = RunOutcome::Finished(HandlerOutcome::TimedOut);

#[test]
fn verdict_follows_the_contract_rules() {
    let cases: Vec<(Verdict, Vec<RunOutcome>)> = vec![
        (Verdict::NoHandlers, vec![]),
        (Verdict::Ok, vec![OK]),
        (Verdict::Ok, vec![OK, OK, OK]),
        (Verdict::Failed, vec![FAILED]),
        (Verdict::Failed, vec![FAILED, RunOutcome::HandlerTimeout]),
        (Verdict::Failed, vec![SELF_TIMED_OUT, FAILED]),
        (Verdict::TimedOut, vec![RunOutcome::DeadlineCut]),
        (Verdict::TimedOut, vec![OK, RunOutcome::DeadlineCut]),
        (Verdict::TimedOut, vec![FAILED, RunOutcome::DeadlineCut]),
        (Verdict::Partial, vec![OK, FAILED]),
        (Verdict::Partial, vec![OK, RunOutcome::HandlerTimeout]),
        (Verdict::Partial, vec![SELF_TIMED_OUT, OK]),
    ];
    for (expected, outcomes) in cases {
        let reports: Vec<HandlerReport> = outcomes
            .iter()
            .enumerate()
            .map(|(index, outcome)| report(&format!("h{index}"), *outcome))
            .collect();
        assert_eq!(expected, verdict(&reports), "{outcomes:?}");
    }
}

#[test]
fn trigger_request_parses_the_contract_example_and_ignores_unknown_fields() {
    let example =
        br#"{"v":1,"reason":"idle_grace","disk":"discarded","deadline_ms":10000,"extra":{"x":1}}"#;
    assert_eq!(
        Ok(request("idle_grace", Disk::Discarded, 10_000)),
        TriggerRequest::parse(example)
    );
}

#[test]
fn trigger_response_matches_the_contract_example() {
    let report = TriggerReport {
        verdict: Verdict::Partial,
        elapsed: Duration::from_millis(640),
        handlers: vec![
            HandlerReport {
                name: handler_name("chrome-cookies"),
                source: HandlerSource::Builtin,
                outcome: OK,
                elapsed: Duration::from_millis(410),
            },
            HandlerReport {
                name: handler_name("myapp-flush"),
                source: HandlerSource::Registered,
                outcome: FAILED,
                elapsed: Duration::from_millis(90),
            },
        ],
    };
    let expected = r#"{"v":1,"verdict":"partial","elapsed_ms":640,"handlers":[
        {"name":"chrome-cookies","source":"builtin","outcome":"ok","elapsed_ms":410},
        {"name":"myapp-flush","source":"registered","outcome":"failed","elapsed_ms":90}]}"#;
    let body = encode_trigger_response(&report).expect("encode");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(expected).expect("fixture"),
        serde_json::from_slice::<serde_json::Value>(&body).expect("body")
    );
}

#[test]
fn response_drops_trailing_entries_to_stay_within_the_cap() {
    let handlers: Vec<HandlerReport> = (0..64)
        .map(|index| {
            report(
                &format!("{index:02}-{}", "n".repeat(45)),
                RunOutcome::DeadlineCut,
            )
        })
        .collect();
    let report = TriggerReport {
        verdict: Verdict::TimedOut,
        elapsed: Duration::from_millis(60_000),
        handlers: handlers.clone(),
    };

    let body = encode_trigger_response(&report).expect("encode");

    assert!(body.len() <= MAX_RESPONSE_BYTES, "{} bytes", body.len());
    let kept = response_names(&body);
    assert!(!kept.is_empty() && kept.len() < handlers.len());
    let expected_prefix: Vec<String> = handlers
        .iter()
        .take(kept.len())
        .map(|handler| handler.name.to_string())
        .collect();
    assert_eq!(expected_prefix, kept);

    let one_more = TriggerReport {
        handlers: handlers.into_iter().take(kept.len() + 1).collect(),
        ..report
    };
    let one_more_body = encode_trigger_response(&one_more).expect("encode");
    assert_eq!(
        kept,
        response_names(&one_more_body),
        "the cut keeps as many entries as fit"
    );
}

/// Handler names in a trigger response that must carry `"truncated":true`.
fn response_names(body: &[u8]) -> Vec<String> {
    let parsed: serde_json::Value = serde_json::from_slice(body).expect("json");
    assert_eq!(
        Some(true),
        parsed.get("truncated").and_then(serde_json::Value::as_bool)
    );
    parsed
        .get("handlers")
        .and_then(serde_json::Value::as_array)
        .expect("handlers")
        .iter()
        .filter_map(|entry| entry.get("name").and_then(serde_json::Value::as_str))
        .map(str::to_owned)
        .collect()
}

#[test]
fn earlier_sources_shadow_later_ones_with_the_same_name() {
    let builtin: Arc<dyn PreStopHandler> = Arc::new(FixedHandler::ok());
    let spec = crate::test_support::spec(&["/bin/true"], None);
    let table = handler_table(
        vec![(handler_name("shared"), builtin)],
        vec![
            ImageHandler {
                name: handler_name("shared"),
                spec: spec.clone(),
            },
            ImageHandler {
                name: handler_name("image-only"),
                spec: spec.clone(),
            },
        ],
        vec![
            (handler_name("image-only"), spec.clone()),
            (handler_name("registered-only"), spec),
        ],
    );
    let listed: Vec<(&str, HandlerSource)> = table
        .iter()
        .map(|entry| (entry.name.as_ref(), entry.source))
        .collect();
    assert_eq!(
        vec![
            ("shared", HandlerSource::Builtin),
            ("image-only", HandlerSource::Image),
            ("registered-only", HandlerSource::Registered),
        ],
        listed
    );
}

#[tokio::test(start_paused = true)]
async fn trigger_without_handlers_reports_no_handlers() {
    let test = TestBroker::new();
    let received = Instant::now();
    let report = test
        .broker
        .trigger(request("manual", Disk::Kept, 10_000), received)
        .await
        .expect("not busy");
    assert_eq!(
        TriggerReport {
            verdict: Verdict::NoHandlers,
            elapsed: Duration::ZERO,
            handlers: Vec::new(),
        },
        report
    );
}

#[tokio::test(start_paused = true)]
async fn deadline_cuts_a_slow_builtin() {
    let test = TestBroker::new();
    test.add_builtin("slow", FixedHandler::slow(Duration::from_secs(3600)));
    test.add_builtin("quick", FixedHandler::failed());

    let report = test
        .broker
        .trigger(request("pre_ttl", Disk::Discarded, 100), Instant::now())
        .await
        .expect("not busy");

    assert_eq!(
        TriggerReport {
            verdict: Verdict::TimedOut,
            elapsed: Duration::from_millis(100),
            handlers: vec![
                HandlerReport {
                    name: handler_name("quick"),
                    source: HandlerSource::Builtin,
                    outcome: FAILED,
                    elapsed: Duration::ZERO,
                },
                HandlerReport {
                    name: handler_name("slow"),
                    source: HandlerSource::Builtin,
                    outcome: RunOutcome::DeadlineCut,
                    elapsed: Duration::from_millis(100),
                },
            ],
        },
        report
    );
}

struct PanickingHandler;

impl PreStopHandler for PanickingHandler {
    fn pre_stop<'a>(&'a self, _event: &'a PreStopEvent) -> HandlerFuture<'a> {
        Box::pin(async { panic!("handler bug") })
    }
}

#[tokio::test(start_paused = true)]
async fn panicking_builtin_is_reported_failed() {
    let test = TestBroker::new();
    test.add_builtin("buggy", PanickingHandler);
    test.add_builtin("fine", FixedHandler::ok());

    let report = test
        .broker
        .trigger(request("manual", Disk::Kept, 1_000), Instant::now())
        .await
        .expect("not busy");

    let outcomes: Vec<(&str, RunOutcome)> = report
        .handlers
        .iter()
        .map(|handler| (handler.name.as_ref(), handler.outcome))
        .collect();
    assert_eq!(vec![("buggy", FAILED), ("fine", OK)], outcomes);
    assert_eq!(Verdict::Partial, report.verdict);
}

/// Reads one line per exec handler from a FIFO the exec handlers announce themselves on, then reports that all of
/// them started while it was still running.
#[cfg(unix)]
struct RendezvousHandler {
    started_fifo: std::path::PathBuf,
    expected: Vec<&'static str>,
    all_started: tokio::sync::mpsc::Sender<()>,
}

#[cfg(unix)]
impl PreStopHandler for RendezvousHandler {
    fn pre_stop<'a>(&'a self, _event: &'a PreStopEvent) -> HandlerFuture<'a> {
        use tokio::io::AsyncBufReadExt;
        use tokio::net::unix::pipe;
        Box::pin(async move {
            let receiver = pipe::OpenOptions::new()
                .open_receiver(&self.started_fifo)
                .expect("open started fifo");
            // Our own writer end keeps the FIFO from reading as EOF before the exec handlers open it.
            let _writer = pipe::OpenOptions::new()
                .open_sender(&self.started_fifo)
                .expect("hold started fifo");
            let mut lines = tokio::io::BufReader::new(receiver).lines();
            let mut seen = Vec::new();
            while seen.len() < self.expected.len() {
                let line = lines.next_line().await.expect("read").expect("line");
                seen.push(line);
            }
            seen.sort();
            assert_eq!(self.expected, seen);
            self.all_started.send(()).await.expect("test alive");
            HandlerOutcome::Ok
        })
    }
}

/// Each exec handler announces itself, then blocks until the test opens the gate FIFO, which it only does once the
/// built-in has heard from both. Run one after another, the first handler would wait forever and be cut.
#[cfg(unix)]
#[tokio::test]
async fn builtin_image_and_registered_handlers_run_concurrently() {
    let test = TestBroker::new();
    let dir = test.path("rendezvous");
    std::fs::create_dir_all(&dir).expect("rendezvous dir");
    let (started_fifo, gate_fifo) = (dir.join("started.fifo"), dir.join("gate.fifo"));
    for fifo in [&started_fifo, &gate_fifo] {
        nix::unistd::mkfifo(fifo, nix::sys::stat::Mode::S_IRWXU).expect("mkfifo");
    }
    let (all_started_tx, mut all_started_rx) = tokio::sync::mpsc::channel(1);
    test.add_builtin(
        "builtin-rendezvous",
        RendezvousHandler {
            started_fifo: started_fifo.clone(),
            expected: vec!["image", "registered"],
            all_started: all_started_tx,
        },
    );
    let dir_arg = dir.display().to_string();
    test.write_image_manifest(
        "image-wait.json",
        &serde_json::json!({"v": 1, "argv": [
            "/bin/sh", "-c", r#"echo image > "$0/started.fifo"; : > "$0/gate.fifo""#, dir_arg,
        ]})
        .to_string(),
    );
    test.register(
        "registered-fail",
        &[
            "/bin/sh",
            "-c",
            r#"echo registered > "$0/started.fifo"; : > "$0/gate.fifo"; exit 7"#,
            &dir_arg,
        ],
    )
    .await;

    let broker = test.broker.clone();
    let trigger = tokio::spawn(async move {
        broker
            .trigger(request("idle_grace", Disk::Kept, 20_000), Instant::now())
            .await
    });
    tokio::time::timeout(Duration::from_secs(20), all_started_rx.recv())
        .await
        .expect("every handler started")
        .expect("rendezvous alive");
    let gate = tokio::net::unix::pipe::OpenOptions::new()
        .open_receiver(&gate_fifo)
        .expect("open gate");
    let report = tokio::time::timeout(Duration::from_secs(20), trigger)
        .await
        .expect("trigger finished")
        .expect("task")
        .expect("not busy");
    drop(gate);

    let outcomes: Vec<(&str, HandlerSource, RunOutcome)> = report
        .handlers
        .iter()
        .map(|handler| (handler.name.as_ref(), handler.source, handler.outcome))
        .collect();
    assert_eq!(
        vec![
            ("builtin-rendezvous", HandlerSource::Builtin, OK),
            ("image-wait", HandlerSource::Image, OK),
            ("registered-fail", HandlerSource::Registered, FAILED),
        ],
        outcomes
    );
    assert_eq!(Verdict::Partial, report.verdict);
}
