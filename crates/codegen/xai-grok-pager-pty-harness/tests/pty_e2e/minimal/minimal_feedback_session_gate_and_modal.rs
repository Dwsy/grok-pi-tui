// Per-test-case module for the `pty_e2e` integration test crate.
#[allow(unused_imports)]
use crate::common::*;

const MODAL_PLACEHOLDER_SENTINEL: &str = "Tell us what happened";
const BARE_REFUSAL_SENTINEL: &str = "in minimal mode";
const INLINE_RESPONSE_SENTINEL: &str = "MOCKRESPONSE feedback skill ready";
const INLINE_FEEDBACK: &str = "minimal-pty-feedback-report-xyz";

/// Minimal: bare `/feedback` refuses visibly and inline `/feedback <text>` runs as a skill turn.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run the owning pty_e2e_* Cargo test with --ignored (see Cargo.toml)"]
async fn minimal_feedback_session_gate_and_modal() {
    let content = ContentController::start().await.expect("start content");
    content.set_response(format!("{MOCK_RESPONSE_SENTINEL} minimal ready."));

    let mut harness = spawn_minimal(&content);
    wait_minimal_ready(&mut harness);

    // Under remote CI a single-shot `/feedback\r` can land Enter before the composer has absorbed the slash filter
    inject_keys_paced(&mut harness, b"/feedback");
    harness.inject_keys(b"\r").expect("submit bare /feedback");
    harness
        .wait_for_full_text(BARE_REFUSAL_SENTINEL, Duration::from_secs(15))
        .expect("bare minimal /feedback must show the visible refusal notice");
    assert!(
        !harness.contains_text(MODAL_PLACEHOLDER_SENTINEL),
        "minimal must never render (or invisibly open) the feedback modal\nscreen:\n{}",
        harness.screen_contents()
    );

    // Bind a session so the inline send has a session_id.
    harness
        .inject_keys(format!("{PROMPT}\r").as_bytes())
        .expect("submit prompt");
    harness
        .wait_for_text(MOCK_RESPONSE_SENTINEL, Duration::from_secs(30))
        .expect("response rendered");

    content.set_response(INLINE_RESPONSE_SENTINEL);
    inject_keys_paced(
        &mut harness,
        format!("/feedback {INLINE_FEEDBACK}").as_bytes(),
    );
    harness
        .inject_keys(b"\r")
        .expect("submit inline /feedback with session");
    harness
        .wait_for_full_text(INLINE_RESPONSE_SENTINEL, Duration::from_secs(30))
        .expect("minimal inline feedback should run a model turn");
    assert!(
        !harness.contains_text("panicked"),
        "pager panicked\nscreen:\n{}",
        harness.screen_contents()
    );

    quit_minimal(&mut harness);
}
