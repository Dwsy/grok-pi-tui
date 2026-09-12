// Per-test-case module for the `pty_e2e` integration test crate.
//
// PTY regression layer for the feedback modal (plan 01-modal PR 1d): the user-visible
// slash/palette/trace/displacement workflows, driven end-to-end with no hidden input owner.
// The minimal-mode bare refusal lives in `minimal/minimal_feedback_session_gate_and_modal.rs`
// (minimal family). Shell-gate request inspection stays in 1c-shell's `mvp_agent` tests; here
// the wire evidence is the local feedback endpoint's POST count, the telemetry events it
// captures, and the mock server's recorded `/v1/storage` uploads.
#[allow(unused_imports)]
use super::common::*;

use anyhow::Context as _;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use xai_grok_pager_pty_harness::{
    InferenceEndpoint, InferenceExpectation, InferenceRequestMatcher,
};

const USER_FEEDBACK_EVENT: &str = "grok-shell-user_feedback";

const MODAL_PLACEHOLDER_SENTINEL: &str = "Tell us what happened";
const EMPTY_SUBMIT_SENTINEL: &str = "Add feedback text";
const THANKS_SENTINEL: &str = "Thanks for the feedback";
const SEND_FAILED_SENTINEL: &str = "Couldn't send feedback";
/// Generic trace-step copy: the slash route supplies no `FeedbackType`.
const TRACE_PROMPT_SENTINEL: &str = "Attach this session's trace to your feedback?";
const TRACE_SEND_CHOICE: &str = "Send this session's trace";
const TRACE_FEEDBACK_ONLY_CHOICE: &str = "No, just the feedback";
const QUESTION_DISPLACED_SENTINEL: &str = "Feedback closed because the agent asked a question";

/// Drive the pager into a live session (welcome, prompt, mock response).
fn enter_session(harness: &mut PtyHarness, content: &ContentController) {
    content.set_response(format!("{MOCK_RESPONSE_SENTINEL} feedback pty ready."));
    harness
        .wait_for_text(WELCOME_SCREEN_SENTINEL, WELCOME_TIMEOUT)
        .expect("welcome text");
    harness
        .inject_keys(format!("{PROMPT}\r").as_bytes())
        .expect("submit prompt");
    harness
        .wait_for_text(MOCK_RESPONSE_SENTINEL, Duration::from_secs(30))
        .expect("session response rendered");
}

/// Open the modal via `/feedback` (paced: a single-shot burst can land Enter
/// before the composer absorbed the slash filter under remote CI load).
fn open_feedback_modal(harness: &mut PtyHarness) {
    inject_keys_paced(harness, b"/feedback");
    harness.inject_keys(b"\r").expect("submit /feedback");
    harness
        .wait_for_text(MODAL_PLACEHOLDER_SENTINEL, Duration::from_secs(15))
        .expect("feedback modal composer should render");
}

// Local feedback endpoint. The shell is the only client of that base URL; only exact `POST
// /feedback` requests are counted and answered with the scripted verdict.

struct ScriptedFeedbackEndpoint {
    url: String,
    posts: Arc<AtomicUsize>,
    fail: Arc<AtomicBool>,
    events: Arc<Mutex<Vec<serde_json::Value>>>,
}

impl ScriptedFeedbackEndpoint {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind local feedback endpoint");
        let url = format!("http://{}", listener.local_addr().expect("local addr"));
        let posts = Arc::new(AtomicUsize::new(0));
        let fail = Arc::new(AtomicBool::new(false));
        let events = Arc::new(Mutex::new(Vec::new()));
        let (posts_srv, fail_srv, events_srv) = (posts.clone(), fail.clone(), events.clone());
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(Self::serve_connection(
                    stream,
                    posts_srv.clone(),
                    fail_srv.clone(),
                    events_srv.clone(),
                ));
            }
        });
        Self {
            url,
            posts,
            fail,
            events,
        }
    }

    fn post_count(&self) -> usize {
        self.posts.load(Ordering::SeqCst)
    }

    fn set_fail(&self, fail: bool) {
        self.fail.store(fail, Ordering::SeqCst);
    }

    /// Every telemetry event posted so far, one entry of the client's `events` batch each
    /// (`event_name`, `event_value`, `event_metadata`, `timestamp`); an unparseable body is
    /// recorded as `{"unparsed_events_body": ..}` so a wire-shape regression is visible.
    fn captured_events(&self) -> Vec<serde_json::Value> {
        self.events.lock().expect("events lock").clone()
    }

    /// Answer one HTTP request and close. The whole body is drained first so the
    /// client never observes a reset mid-write (which reqwest would surface as a
    /// transport error instead of the scripted status).
    async fn serve_connection(
        mut stream: tokio::net::TcpStream,
        posts: Arc<AtomicUsize>,
        fail: Arc<AtomicBool>,
        events: Arc<Mutex<Vec<serde_json::Value>>>,
    ) {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let mut buf = vec![0u8; 65536];
        let mut seen: Vec<u8> = Vec::new();
        let body_start = loop {
            if let Some(header_end) = seen.windows(4).position(|w| w == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&seen[..header_end]);
                let content_length = headers
                    .lines()
                    .find_map(|l| {
                        let (name, value) = l.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())?
                    })
                    .unwrap_or(0);
                if seen.len() >= header_end + 4 + content_length {
                    break header_end + 4;
                }
            }
            match stream.read(&mut buf).await {
                Ok(0) | Err(_) => break seen.len(),
                Ok(n) => seen.extend_from_slice(&buf[..n]),
            }
        };
        let request_line = String::from_utf8_lossy(&seen)
            .lines()
            .next()
            .unwrap_or_default()
            .to_owned();
        let response = if request_line.starts_with("POST /feedback ") {
            posts.fetch_add(1, Ordering::SeqCst);
            if fail.load(Ordering::SeqCst) {
                let body = r#"{"error":"scripted feedback failure"}"#;
                format!(
                    "HTTP/1.1 500 Internal Server Error\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                )
            } else {
                // Minimal cli-chat-proxy `FeedbackResponse` shape (camelCase).
                let body =
                    r#"{"feedbackId":"pty-feedback-fixture","createdAt":"2026-01-01T00:00:00Z"}"#;
                format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                )
            }
        } else if request_line.starts_with("POST /events ") {
            let body = &seen[body_start..];
            let batch = serde_json::from_slice::<serde_json::Value>(body)
                .ok()
                .and_then(|parsed| parsed.get("events")?.as_array().cloned())
                .unwrap_or_else(|| {
                    vec![json!({ "unparsed_events_body": String::from_utf8_lossy(body) })]
                });
            events.lock().expect("events lock").extend(batch);
            "HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_owned()
        } else {
            "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_owned()
        };
        let _ = stream.write_all(response.as_bytes()).await;
        let _ = stream.shutdown().await;
    }
}

// ── Blocked tool turns ──────────────────────────────────────────────────

/// Register a foreground tool-call turn on both endpoints, held at its terminal barrier
/// ([`ContentController::expect_agent_turn_blocked`] only scripts text).
fn expect_tool_turn_blocked(
    content: &ContentController,
    call_id: &str,
    name: &str,
    args: &str,
) -> [InferenceExpectation; 2] {
    [
        content.expect_response_blocked(
            format!("blocked tool turn {call_id} (responses)"),
            InferenceRequestMatcher::foreground(InferenceEndpoint::Responses),
            ScriptedResponse::sse(responses_api_tool_call_events(call_id, name, args)),
        ),
        content.expect_response_blocked(
            format!("blocked tool turn {call_id} (chat completions)"),
            InferenceRequestMatcher::foreground(InferenceEndpoint::ChatCompletions),
            ScriptedResponse::sse(chat_completions_tool_call_events_with_id(
                call_id, name, args,
            )),
        ),
    ]
}

/// Wait until the active backend's request reached the held terminal barrier.
async fn wait_tool_turn_blocked(expectations: &mut [InferenceExpectation; 2]) {
    let [responses, chat_completions] = expectations;
    let barrier = async {
        tokio::select! {
            _ = responses.wait_blocked() => {}
            _ = chat_completions.wait_blocked() => {}
        }
    };
    tokio::time::timeout(Duration::from_secs(30), barrier)
        .await
        .expect("blocked tool turn reached its terminal barrier");
}

/// Slash open/type/Enter sends and closes; empty Enter refuses first. Inline `/feedback <text>`
/// runs as a skill turn without opening the modal. The palette entry opens the modal without
/// touching the main composer draft.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run the owning pty_e2e_* Cargo test with --ignored (see Cargo.toml)"]
async fn feedback_modal_send_prefill_edit_and_palette_pty() {
    const SENT_REPORT: &str = "pty-modal-send-report-alpha";
    const INLINE_REPORT: &str = "pty-modal-inline-send-beta";
    const MAIN_DRAFT: &str = "pty-main-draft-preserved-xyz";

    let content = ContentController::start().await.expect("start content");
    let binary = pager_binary().expect("resolve pager binary");
    let mut harness = PtyHarness::spawn_with_content_env_ops_in_dir(
        &binary,
        DEFAULT_ROWS,
        DEFAULT_COLS,
        &content,
        &["--yolo", "--trust"],
        // The sandbox baseline disables feedback; the send path needs the shell gate open.
        &[EnvOp::set("GROK_FEEDBACK_ENABLED", "true")],
        Some(content.home()),
    )
    .expect("spawn pager with content");
    enter_session(&mut harness, &content);

    // Slash open, empty refusal, type, Enter: the success closes the modal and thanks.
    open_feedback_modal(&mut harness);
    harness.inject_keys(b"\r").expect("empty enter");
    harness
        .wait_for_text(EMPTY_SUBMIT_SENTINEL, Duration::from_secs(15))
        .expect("empty Enter must show the validation message");
    assert!(
        harness.contains_text(MODAL_PLACEHOLDER_SENTINEL),
        "empty Enter must keep the modal open\nscreen:\n{}",
        harness.screen_contents()
    );
    assert!(
        !harness.contains_text(THANKS_SENTINEL),
        "empty Enter must not thank the user\nscreen:\n{}",
        harness.screen_contents()
    );
    inject_keys_paced(&mut harness, SENT_REPORT.as_bytes());
    harness
        .wait_for_text(SENT_REPORT, Duration::from_secs(15))
        .expect("typed report renders in the modal composer");
    harness.inject_keys(b"\r").expect("submit report");
    harness
        .wait_until("modal closed at send", Duration::from_secs(30), |h| {
            !h.contains_text(SENT_REPORT)
        })
        .expect("the committed report must close the modal immediately");
    harness
        .wait_for_text(THANKS_SENTINEL, Duration::from_secs(15))
        .expect("the send thanks the user at close time");

    // Inline `/feedback <text>` runs a skill turn and does not open the modal.
    const INLINE_RESPONSE: &str = "MOCKRESPONSE inline feedback skill ready";
    content.set_response(INLINE_RESPONSE);
    inject_keys_paced(
        &mut harness,
        format!("/feedback {INLINE_REPORT}").as_bytes(),
    );
    harness.inject_keys(b"\r").expect("submit inline /feedback");
    harness
        .wait_for_text(INLINE_RESPONSE, Duration::from_secs(30))
        .expect("inline feedback must run a model turn");
    assert!(
        !harness.contains_text(MODAL_PLACEHOLDER_SENTINEL),
        "inline /feedback must not open the modal\nscreen:\n{}",
        harness.screen_contents()
    );

    // Palette open: the inline send above left a predraft, so the modal opens on Drafts.
    // The main composer draft stays byte-for-byte where it was.
    inject_keys_paced(&mut harness, MAIN_DRAFT.as_bytes());
    harness
        .wait_until("main draft rendered", Duration::from_secs(15), |h| {
            composer_holds(h, MAIN_DRAFT)
        })
        .expect("main composer holds the draft");
    inject_keys_paced(&mut harness, b"\x10"); // Ctrl+P opens the command palette
    inject_keys_paced(&mut harness, b"send feedback");
    harness
        .wait_for_text("Send Feedback", Duration::from_secs(15))
        .expect("palette filter shows the Send Feedback entry");
    harness.inject_keys(b"\r").expect("pick Send Feedback");
    let inline_draft_row = format!("Unclassified · Other · {INLINE_REPORT}");
    harness
        .wait_for_text(&inline_draft_row, Duration::from_secs(15))
        .expect("palette must open the feedback modal on the saved inline draft");
    harness.inject_keys(b"\x1b").expect("esc cancels the modal");
    harness
        .wait_until(
            "palette-opened modal closed",
            Duration::from_secs(15),
            |h| !h.contains_text(&inline_draft_row),
        )
        .expect("Esc should close the palette-opened modal");
    assert!(
        composer_holds(&harness, MAIN_DRAFT),
        "the main composer draft must survive the palette round trip untouched\nscreen:\n{}",
        harness.screen_contents()
    );
    assert!(
        !harness.contains_text("panicked"),
        "pager panicked\nscreen:\n{}",
        harness.screen_contents()
    );
    harness.quit().expect("clean quit");
}

/// The in-modal trace step, end to end against real POST verdicts: Esc backs out without sending;
/// FeedbackOnly sends the report alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run the owning pty_e2e_* Cargo test with --ignored (see Cargo.toml)"]
async fn feedback_modal_trace_step_and_upload_ordering_pty() {
    const REPORT_FEEDBACK_ONLY: &str = "pty-trace-feedback-only-report";
    const REPORT_SEND_SESSION: &str = "pty-trace-send-session-report";
    const REPORT_POST_FAILURE: &str = "pty-trace-post-failure-report";

    let trace_upload_count = |content: &ContentController| {
        content
            .storage_uploads()
            .iter()
            .filter(|u| u.path.contains("feedback_trace.tar.gz"))
            .count()
    };

    let content = ContentController::start().await.expect("start content");
    let endpoint = ScriptedFeedbackEndpoint::start().await;
    seed_fake_oauth(&content, "feedback-trace-pty");
    let mut overrides = Vec::from(oauth_credential_ops());
    overrides.push(EnvOp::set("GROK_FEEDBACK_ENABLED", "true"));
    overrides.push(EnvOp::set("GROK_FEEDBACK_TRACE_CARD", "true"));
    // TestSandbox pins DISABLE_TELEMETRY=1; this test needs product telemetry on.
    overrides.push(EnvOp::remove("DISABLE_TELEMETRY"));
    overrides.push(EnvOp::set("GROK_TELEMETRY_ENABLED", "true"));
    // Telemetry is on for the trace offer gate; the events sink must be this endpoint, never the baked production one.
    let events_url = format!("{}/events", endpoint.url);
    overrides.push(EnvOp::set("GROK_TELEMETRY_EVENTS_URL", events_url.as_str()));
    overrides.push(EnvOp::set("GROK_TELEMETRY_EVENTS_API_KEY", "pty-capture"));
    overrides.push(EnvOp::set("GROK_FEEDBACK_BASE_URL", endpoint.url.as_str()));
    overrides.push(EnvOp::remove("GROK_TRACE_UPLOAD_URL"));

    let binary = pager_binary().expect("resolve pager binary");
    let mut harness = PtyHarness::spawn_with_content_env_ops_in_dir(
        &binary,
        DEFAULT_ROWS,
        DEFAULT_COLS,
        &content,
        &["--yolo", "--trust"],
        &overrides,
        Some(content.home()),
    )
    .expect("spawn pager with content");
    enter_session(&mut harness, &content);

    // Submit reaches the in-modal trace question instead of sending.
    open_feedback_modal(&mut harness);
    inject_keys_paced(&mut harness, REPORT_FEEDBACK_ONLY.as_bytes());
    harness
        .wait_for_text(REPORT_FEEDBACK_ONLY, Duration::from_secs(15))
        .expect("report renders in the modal composer");
    harness.inject_keys(b"\r").expect("submit into trace step");
    harness
        .wait_for_text(TRACE_PROMPT_SENTINEL, Duration::from_secs(15))
        .expect("the trace question renders inside the modal");
    harness
        .wait_for_text(TRACE_SEND_CHOICE, Duration::from_secs(15))
        .expect("trace choices render");

    // Esc backs out to Write without sending anything.
    harness
        .inject_keys(b"\x1b")
        .expect("esc backs out of trace");
    harness
        .wait_until("trace step dismissed", Duration::from_secs(15), |h| {
            !h.contains_text(TRACE_PROMPT_SENTINEL)
        })
        .expect("Esc must return to the Write step");
    harness
        .wait_for_text(REPORT_FEEDBACK_ONLY, Duration::from_secs(15))
        .expect("the draft survives backing out of the trace step");
    assert_eq!(
        endpoint.post_count(),
        0,
        "backing out of the trace step must not POST"
    );

    // FeedbackOnly: the report posts alone; no archive follows.
    harness.inject_keys(b"\r").expect("re-enter trace step");
    harness
        .wait_for_text(TRACE_FEEDBACK_ONLY_CHOICE, Duration::from_secs(15))
        .expect("trace step re-rendered");
    harness.inject_keys(b"2").expect("select FeedbackOnly");
    harness.inject_keys(b"\r").expect("confirm FeedbackOnly");
    harness
        .wait_until("feedback-only modal closed", Duration::from_secs(30), |h| {
            !h.contains_text(REPORT_FEEDBACK_ONLY)
        })
        .expect("FeedbackOnly must send and close the modal");
    harness
        .wait_until(
            "feedback-only POST recorded",
            Duration::from_secs(15),
            |_| endpoint.post_count() >= 1,
        )
        .expect("the FeedbackOnly report reaches the feedback endpoint");
    assert_eq!(
        trace_upload_count(&content),
        0,
        "FeedbackOnly must never upload a trace archive: {:?}",
        content.storage_uploads()
    );

    // SendThisSession: exactly one archive lands, strictly after the successful POST.
    open_feedback_modal(&mut harness);
    inject_keys_paced(&mut harness, REPORT_SEND_SESSION.as_bytes());
    harness.inject_keys(b"\r").expect("submit into trace step");
    harness
        .wait_for_text(TRACE_PROMPT_SENTINEL, Duration::from_secs(15))
        .expect("trace question for the second report");
    harness.inject_keys(b"1").expect("select SendThisSession");
    harness.inject_keys(b"\r").expect("confirm SendThisSession");
    harness
        .wait_until(
            "send-this-session modal closed",
            Duration::from_secs(30),
            |h| !h.contains_text(REPORT_SEND_SESSION),
        )
        .expect("SendThisSession must send and close the modal");
    harness
        .wait_until("second POST recorded", Duration::from_secs(15), |_| {
            endpoint.post_count() >= 2
        })
        .expect("the SendThisSession report reaches the feedback endpoint");
    harness
        .wait_until("one-shot archive uploaded", Duration::from_secs(60), |_| {
            trace_upload_count(&content) >= 1
        })
        .expect("the consented archive must upload after the POST succeeded");
    assert_eq!(
        endpoint.post_count(),
        2,
        "the archive must not ride a duplicate feedback POST"
    );
    assert_eq!(
        trace_upload_count(&content),
        1,
        "exactly one consented archive: {:?}",
        content.storage_uploads()
    );

    // POST failure: the modal already closed at the committed send, the error lands in the
    // transcript, and no upload happens even though the user consented to SendThisSession.
    endpoint.set_fail(true);
    open_feedback_modal(&mut harness);
    inject_keys_paced(&mut harness, REPORT_POST_FAILURE.as_bytes());
    harness.inject_keys(b"\r").expect("submit into trace step");
    harness
        .wait_for_text(TRACE_PROMPT_SENTINEL, Duration::from_secs(15))
        .expect("trace question for the failing report");
    harness.inject_keys(b"1").expect("select SendThisSession");
    harness.inject_keys(b"\r").expect("confirm SendThisSession");
    harness
        .wait_until(
            "failing modal closed at send",
            Duration::from_secs(30),
            |h| !h.contains_text(REPORT_POST_FAILURE),
        )
        .expect("the committed send closes the modal before the POST verdict");
    harness
        .wait_for_text(SEND_FAILED_SENTINEL, Duration::from_secs(30))
        .expect("a failed POST must surface its error in the transcript");
    harness
        .wait_until("failed POST recorded", Duration::from_secs(15), |_| {
            endpoint.post_count() >= 3
        })
        .expect("the failing report reached the feedback endpoint");
    assert_eq!(
        trace_upload_count(&content),
        1,
        "a failed POST must never be followed by an upload: {:?}",
        content.storage_uploads()
    );

    // The sends emitted their telemetry to this endpoint (fire-and-forget, so the last one may still be in flight).
    harness
        .wait_until(
            "user_feedback telemetry captured",
            Duration::from_secs(15),
            |_| {
                endpoint
                    .captured_events()
                    .iter()
                    .any(|event| event["event_name"] == USER_FEEDBACK_EVENT)
            },
        )
        .with_context(|| format!("captured events: {:?}", endpoint.captured_events()))
        .expect("the sends' telemetry must reach the scripted events sink");
    assert!(
        !harness.contains_text("panicked"),
        "pager panicked\nscreen:\n{}",
        harness.screen_contents()
    );
    harness.quit().expect("clean quit");
}

/// An `ask_user_question` ingress visibly displaces the open feedback modal: the eviction notice
/// and the question card render, the unsent draft is discarded (not sent, not moved into the main
/// composer), and the question remains answerable.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run the owning pty_e2e_* Cargo test with --ignored (see Cargo.toml)"]
async fn feedback_modal_displaced_by_acp_question_pty() {
    const QUESTION_TEXT: &str = "Which displacement detail matters most?";
    const FIRST_OPTION: &str = "Modal eviction";
    const DRAFT: &str = "pty-question-displaced-draft";
    const DONE_SENTINEL: &str = "QUESTIONDISPLACEDONE";

    let content = ContentController::start().await.expect("start content");
    let args = json!({
        "questions": [{
            "question": QUESTION_TEXT,
            "options": [
                { "label": FIRST_OPTION, "description": "The modal must be gone" },
                { "label": "Draft handling", "description": "The draft must not leak" },
            ],
        }]
    })
    .to_string();
    let mut turn = expect_tool_turn_blocked(
        &content,
        "call_displace_question",
        "ask_user_question",
        &args,
    );
    content.set_response(DONE_SENTINEL);

    let binary = pager_binary().expect("resolve pager binary");
    let mut harness = PtyHarness::spawn_with_content_in_dir(
        &binary,
        DEFAULT_ROWS,
        DEFAULT_COLS,
        &content,
        &["--yolo", "--trust"],
        Some(content.home()),
    )
    .expect("spawn pager with content");
    harness
        .wait_for_text(WELCOME_SCREEN_SENTINEL, WELCOME_TIMEOUT)
        .expect("welcome text");
    harness
        .inject_keys(format!("{PROMPT}\r").as_bytes())
        .expect("submit prompt");
    wait_tool_turn_blocked(&mut turn).await;

    // Open the modal mid-turn, while the question is provably still held.
    open_feedback_modal(&mut harness);
    inject_keys_paced(&mut harness, DRAFT.as_bytes());
    harness
        .wait_for_text(DRAFT, Duration::from_secs(15))
        .expect("draft renders in the modal composer");
    assert!(
        !harness.contains_text(QUESTION_TEXT),
        "the held question must not have landed yet\nscreen:\n{}",
        harness.screen_contents()
    );

    for expectation in &turn {
        expectation.release();
    }
    harness
        .wait_for_text(QUESTION_DISPLACED_SENTINEL, Duration::from_secs(30))
        .expect("the eviction notice must be visible");
    harness
        .wait_for_text(QUESTION_TEXT, Duration::from_secs(15))
        .expect("the question card renders after displacing feedback");
    assert!(
        !harness.contains_text(MODAL_PLACEHOLDER_SENTINEL),
        "the feedback modal must be gone\nscreen:\n{}",
        harness.screen_contents()
    );
    assert!(
        !harness.contains_text(DRAFT),
        "the discarded draft must not survive anywhere (composer included)\nscreen:\n{}",
        harness.screen_contents()
    );
    assert!(
        !harness.contains_text(THANKS_SENTINEL),
        "a displaced draft must never send\nscreen:\n{}",
        harness.screen_contents()
    );

    // The question stayed the sole input owner: answer it and let the turn settle.
    harness.inject_keys(b" ").expect("mark the first answer");
    harness.inject_keys(b"\r").expect("submit answers");
    harness
        .wait_for_text(DONE_SENTINEL, Duration::from_secs(30))
        .expect("agent turn resumes after the answers");
    assert!(
        !harness.contains_text("panicked"),
        "pager panicked\nscreen:\n{}",
        harness.screen_contents()
    );
    harness.quit().expect("clean quit");
}
