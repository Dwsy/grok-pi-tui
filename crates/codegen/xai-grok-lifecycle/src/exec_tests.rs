#![cfg(unix)]

use pretty_assertions::assert_eq;

use super::*;
use crate::event::Disk;
use crate::test_support::{far_future, handler_name, spec};
use crate::token::ReasonToken;

/// The contract's bound on answering past the trigger deadline.
const DEADLINE_EPSILON: Duration = Duration::from_millis(250);

struct Harness {
    dir: tempfile::TempDir,
    ctx: ExecContext,
}

impl Harness {
    async fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let log = Arc::new(ExecLog::new(dir.path().join("exec.log")));
        log.start_trigger(far_future()).await;
        Harness {
            dir,
            ctx: ExecContext {
                log,
                processes: ProcessScope::new(),
            },
        }
    }

    fn path(&self, name: &str) -> String {
        self.dir.path().join(name).display().to_string()
    }

    async fn run(&self, argv: &[&str], timeout_ms: Option<u64>, budget: Duration) -> RunOutcome {
        let event = PreStopEvent::new(
            ReasonToken::try_from("idle_grace").expect("reason"),
            Disk::Kept,
            Instant::now() + budget,
        );
        run_exec(
            handler_name("test-handler"),
            spec(argv, timeout_ms),
            event,
            self.ctx.clone(),
        )
        .await
    }
}

/// Process death is not an event this process can await for a grandchild, so poll it under a generous bound.
async fn is_gone_within(pid: u32, bound: Duration) -> bool {
    tokio::time::timeout(bound, async {
        while !xai_tty_utils::process_not_running(pid) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .is_ok()
}

#[tokio::test]
async fn exec_env_carries_event_reason_disk_and_remaining_deadline() {
    let harness = Harness::new().await;
    let out = harness.path("env.txt");
    let script = r#"printf '%s|%s|%s|%s' "$GROK_LIFECYCLE_EVENT" "$GROK_LIFECYCLE_REASON" "$GROK_LIFECYCLE_DISK" "$GROK_LIFECYCLE_DEADLINE_MS" > "$0""#;

    let outcome = harness
        .run(
            &["/bin/sh", "-c", script, &out],
            None,
            Duration::from_secs(5),
        )
        .await;

    assert_eq!(RunOutcome::Finished(HandlerOutcome::Ok), outcome);
    let env = std::fs::read_to_string(&out).expect("env file");
    let fields: Vec<&str> = env.split('|').collect();
    let [event, reason, disk, remaining_ms] = fields.as_slice() else {
        panic!("unexpected env line {env:?}");
    };
    assert_eq!(("pre-stop", "idle_grace", "kept"), (*event, *reason, *disk));
    let remaining_ms: u64 = remaining_ms.parse().expect("deadline ms");
    assert!(
        (4_000..=5_000).contains(&remaining_ms),
        "remaining deadline at spawn: {remaining_ms}"
    );
}

#[tokio::test]
async fn deadline_env_is_the_handlers_own_timeout_when_that_is_shorter() {
    let harness = Harness::new().await;
    let out = harness.path("deadline.txt");
    let script = r#"printf '%s' "$GROK_LIFECYCLE_DEADLINE_MS" > "$0""#;

    let outcome = harness
        .run(
            &["/bin/sh", "-c", script, &out],
            Some(20_000),
            Duration::from_secs(60),
        )
        .await;

    assert_eq!(RunOutcome::Finished(HandlerOutcome::Ok), outcome);
    assert_eq!(
        "20000",
        std::fs::read_to_string(&out).expect("deadline file")
    );
}

#[tokio::test]
async fn exit_status_decides_ok_or_failed() {
    let harness = Harness::new().await;
    let budget = Duration::from_secs(5);
    assert_eq!(
        RunOutcome::Finished(HandlerOutcome::Ok),
        harness
            .run(&["/bin/sh", "-c", "exit 0"], None, budget)
            .await
    );
    assert_eq!(
        RunOutcome::Finished(HandlerOutcome::Failed),
        harness
            .run(&["/bin/sh", "-c", "exit 3"], None, budget)
            .await
    );
    assert_eq!(
        RunOutcome::Finished(HandlerOutcome::Failed),
        harness.run(&["/nonexistent/handler"], None, budget).await
    );
}

#[tokio::test]
async fn handler_ignoring_sigterm_is_killed_with_its_group_by_the_deadline() {
    let harness = Harness::new().await;
    let pid_file = harness.path("grandchild.pid");
    let script = r#"trap '' TERM; /bin/sleep 30 & echo $! > "$0"; wait"#;
    // Generous so that even a heavily loaded machine runs the `trap` before SIGTERM lands.
    let budget = Duration::from_secs(5);

    let started = Instant::now();
    let outcome = harness
        .run(&["/bin/sh", "-c", script, &pid_file], None, budget)
        .await;
    let elapsed = started.elapsed();

    assert_eq!(RunOutcome::DeadlineCut, outcome);
    assert!(elapsed >= budget, "cut before the deadline: {elapsed:?}");
    assert!(
        elapsed < budget + DEADLINE_EPSILON,
        "cut too late: {elapsed:?}"
    );
    let grandchild: u32 = std::fs::read_to_string(&pid_file)
        .expect("pid file: the shell never reached its trap before the deadline")
        .trim()
        .parse()
        .expect("pid");
    assert!(
        is_gone_within(grandchild, Duration::from_secs(5)).await,
        "grandchild {grandchild} survived the group kill"
    );
    assert_eq!(0, harness.ctx.processes.live_count());
}

#[tokio::test]
async fn run_dropped_at_its_first_await_kills_the_whole_group() {
    let harness = Harness::new().await;
    let pid_file = harness.path("grandchild.pid");
    let script = r#"/bin/sleep 30 & echo $! > "$0"; wait"#;
    let argv = ["/bin/sh", "-c", script, &pid_file];
    let mut run = Box::pin(harness.run(&argv, None, Duration::from_secs(30)));

    tokio::select! {
        biased;
        outcome = &mut run => panic!("handler finished on its first poll: {outcome:?}"),
        () = std::future::ready(()) => {}
    }
    let grandchild = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(pid) = std::fs::read_to_string(&pid_file)
                && let Some(pid) = pid.strip_suffix('\n')
            {
                return pid.parse::<u32>().expect("pid");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the shell never forked its grandchild");
    drop(run);

    assert!(
        is_gone_within(grandchild, Duration::from_secs(5)).await,
        "grandchild {grandchild} survived the dropped run"
    );
    assert_eq!(0, harness.ctx.processes.live_count());
}

#[tokio::test]
async fn background_child_of_a_handler_that_exited_is_killed() {
    let harness = Harness::new().await;
    let pid_file = harness.path("background.pid");
    let script = r#"/bin/sleep 30 > /dev/null 2>&1 & echo $! > "$0""#;

    let outcome = harness
        .run(
            &["/bin/sh", "-c", script, &pid_file],
            None,
            Duration::from_secs(30),
        )
        .await;

    assert_eq!(RunOutcome::Finished(HandlerOutcome::Ok), outcome);
    let background: u32 = std::fs::read_to_string(&pid_file)
        .expect("pid file")
        .trim()
        .parse()
        .expect("pid");
    assert!(
        is_gone_within(background, Duration::from_secs(5)).await,
        "background child {background} outlived its handler"
    );
    assert_eq!(0, harness.ctx.processes.live_count());
}

#[tokio::test]
async fn own_timeout_cuts_before_the_trigger_deadline() {
    let harness = Harness::new().await;
    let started = Instant::now();
    let outcome = harness
        .run(&["/bin/sleep", "30"], Some(200), Duration::from_secs(10))
        .await;
    let elapsed = started.elapsed();

    assert_eq!(RunOutcome::HandlerTimeout, outcome);
    assert!(
        elapsed >= Duration::from_millis(200) && elapsed < Duration::from_secs(2),
        "{elapsed:?}"
    );
}

#[tokio::test]
async fn output_goes_to_the_log_not_the_outcome() {
    let harness = Harness::new().await;
    let outcome = harness
        .run(
            &["/bin/sh", "-c", "echo to-stdout; echo to-stderr >&2"],
            None,
            Duration::from_secs(5),
        )
        .await;
    harness.ctx.log.finish_trigger();

    assert_eq!(RunOutcome::Finished(HandlerOutcome::Ok), outcome);
    let log = std::fs::read_to_string(harness.dir.path().join("exec.log")).expect("log");
    for needle in [
        "test-handler: started",
        "to-stdout",
        "to-stderr",
        "test-handler: ok after",
    ] {
        assert!(log.contains(needle), "{needle:?} missing from {log:?}");
    }
}
