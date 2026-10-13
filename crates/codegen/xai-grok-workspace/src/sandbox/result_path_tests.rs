use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use futures::StreamExt;
use xai_grok_sandbox::command::grants::FixedClock;
use xai_grok_sandbox::command::{CallId, GitConfigEnv, SandboxMode};
use xai_grok_tools::types::output::{BashOutput, TextOutput, ToolOutput, ToolRunResult};
use xai_tool_runtime::{ToolError, ToolErrorKind, ToolProgress, ToolStream, ToolStreamItem};

use super::{PIN_LOST_TEXT, ReplayBudget, run_shell_call_with_replay, signal_number, with_notes};
use crate::handle::WorkspaceHandle;
use crate::permission::ToolApprovalGate;
use crate::sandbox::{BackendSource, WorkspaceSandbox, WorkspaceSandboxConfig};

#[test]
fn signal_strings_from_the_terminal_backend_parse() {
    assert_eq!(Some(9), signal_number("signal 9"));
    assert_eq!(Some(31), signal_number("signal 31"));
    assert_eq!(None, signal_number("timeout"));
    assert_eq!(None, signal_number("killed"));
    assert_eq!(None, signal_number("signal x"));
}

#[test]
fn the_replay_budget_is_one() {
    let mut budget = ReplayBudget::One;
    assert!(budget.spend());
    assert_eq!(ReplayBudget::Spent, budget);
    assert!(!budget.spend());
    assert!(!budget.spend());
}

fn bash(exit_code: i32) -> BashOutput {
    BashOutput {
        output: Vec::new(),
        output_for_prompt: String::new(),
        exit_code,
        command: "make".to_owned(),
        truncated: false,
        signal: None,
        timed_out: false,
        description: None,
        current_dir: "/ws".to_owned(),
        output_file: String::new(),
        total_bytes: 0,
        output_delta: None,
        was_bare_echo: false,
    }
}

fn run_result(exit_code: i32) -> ToolRunResult {
    ToolRunResult {
        output: ToolOutput::Bash(bash(exit_code)),
        prompt_text: "exit 0".to_owned(),
        effective_tool_name: None,
    }
}

/// The sandbox's notes on the run render after the command's output, in order; with none the
/// result is left as the tool reported it.
#[test]
fn a_note_renders_after_the_output() {
    assert_eq!("exit 0", with_notes(run_result(0), &[]).prompt_text);
    let noted = with_notes(run_result(0), &["a".to_owned(), "b".to_owned()]);
    assert_eq!("exit 0\n\n[sandbox] a\n\n[sandbox] b", noted.prompt_text);
}

struct Dispatches {
    calls: Arc<AtomicUsize>,
}

impl Dispatches {
    fn new() -> Dispatches {
        Dispatches {
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// A dispatch that streams one progress item and then `terminal`.
    fn streaming(
        &self,
        terminal: impl Fn() -> Option<Result<ToolRunResult, ToolError>> + Send + Sync + 'static,
    ) -> impl FnMut() -> ToolStream<ToolRunResult> + Send + 'static {
        let calls = self.calls.clone();
        let terminal = Arc::new(terminal);
        move || {
            calls.fetch_add(1, Ordering::SeqCst);
            let terminal = terminal.clone();
            Box::pin(async_stream::stream! {
                yield ToolStreamItem::Progress(ToolProgress::Text {
                    text: "working".to_owned(),
                });
                if let Some(terminal) = terminal() {
                    yield ToolStreamItem::Terminal(terminal);
                }
            })
        }
    }
}

async fn collect(stream: ToolStream<ToolRunResult>) -> Vec<ToolStreamItem<ToolRunResult>> {
    stream.collect().await
}

/// Without a sandbox the driver is a pass-through: one dispatch, every item forwarded, the
/// terminal last. A call the hub pinned finds no sandbox to hold its pin and is refused unrun.
#[tokio::test]
async fn without_a_sandbox_the_call_runs_once_and_passes_through() {
    let tmp = tempfile::tempdir().unwrap();
    let handle = WorkspaceHandle::for_test_in(tmp.path());
    let session = handle.create_session("main").unwrap();
    let dispatches = Dispatches::new();
    let items = collect(run_shell_call_with_replay(
        handle.clone(),
        session.clone(),
        "call-1".to_owned(),
        None,
        false,
        dispatches.streaming(|| Some(Ok(run_result(0)))),
    ))
    .await;
    assert_eq!(1, dispatches.calls.load(Ordering::SeqCst));
    let [
        ToolStreamItem::Progress(_),
        ToolStreamItem::Terminal(Ok(result)),
    ] = items.as_slice()
    else {
        panic!("{items:?}");
    };
    assert_eq!("exit 0", result.prompt_text);

    let items = collect(run_shell_call_with_replay(
        handle,
        session,
        "call-2".to_owned(),
        None,
        true,
        dispatches.streaming(|| Some(Ok(run_result(0)))),
    ))
    .await;
    assert_eq!(
        1,
        dispatches.calls.load(Ordering::SeqCst),
        "never dispatched"
    );
    let [ToolStreamItem::Terminal(Err(error))] = items.as_slice() else {
        panic!("{items:?}");
    };
    assert!(error.to_string().contains(PIN_LOST_TEXT), "{error}");
}

#[tokio::test]
async fn a_tool_error_is_forwarded_and_ends_the_stream() {
    let tmp = tempfile::tempdir().unwrap();
    let handle = WorkspaceHandle::for_test_in(tmp.path());
    let session = handle.create_session("main").unwrap();
    let dispatches = Dispatches::new();
    let items = collect(run_shell_call_with_replay(
        handle.clone(),
        session,
        "call-1".to_owned(),
        None,
        false,
        dispatches.streaming(|| {
            Some(Err(ToolError::new(
                ToolErrorKind::TerminalError,
                "terminal died",
            )))
        }),
    ))
    .await;
    assert_eq!(1, dispatches.calls.load(Ordering::SeqCst));
    let [_, ToolStreamItem::Terminal(Err(error))] = items.as_slice() else {
        panic!("{items:?}");
    };
    assert!(error.to_string().contains("terminal died"), "{error}");
}

/// A handle whose folder sandbox is in `mode`, with no backend: nothing here spawns.
async fn handle_with_sandbox(
    root: &std::path::Path,
    mode: &str,
) -> (WorkspaceHandle, Arc<WorkspaceSandbox>) {
    let grok_home = root.join("grok-home");
    let workspace = root.join("ws");
    std::fs::create_dir_all(workspace.join(".grok")).unwrap();
    std::fs::create_dir_all(&grok_home).unwrap();
    std::fs::write(
        crate::sandbox_mode::workspace_config_path(&workspace),
        format!("[sandbox]\nmode = \"{mode}\"\n"),
    )
    .unwrap();
    let sandbox = Arc::new(
        WorkspaceSandbox::open(WorkspaceSandboxConfig {
            workspace_root: workspace.clone(),
            control_socket_dir: grok_home.join("workspaced"),
            grok_home,
            user_home: Some(root.join("home")),
            remote: None,
            backend: BackendSource::Fixed(None),
            clock: Arc::new(FixedClock::at(1_800_000_000)),
            git_env: GitConfigEnv::default(),
        })
        .await,
    );
    let handle = WorkspaceHandle::for_test_in_with_sandbox(
        &workspace,
        sandbox.clone(),
        ToolApprovalGate::Off,
    );
    (handle, sandbox)
}

/// A shell call whose result is not the `Bash` output the decoder reads is kept unchanged, and
/// under `enforce` — where a refusal inside that run gets no card — the result path says so at
/// `warn`; under `observe` there is nothing to miss, so nothing is logged.
#[test]
fn a_shell_result_the_decoder_cannot_read_is_kept_and_warned_about_under_enforce() {
    for (mode, sandbox_mode, warns) in [
        ("enforce", SandboxMode::Enforce, true),
        ("observe", SandboxMode::Observe, false),
    ] {
        let ((), logged) = crate::capturing_warn_logs(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async {
                let tmp = tempfile::tempdir().unwrap();
                let (handle, sandbox) = handle_with_sandbox(tmp.path(), mode).await;
                assert_eq!(sandbox_mode, sandbox.mode(), "{mode}");
                // The hub fixes the call's mode at dispatch; the warning is about that mode
                sandbox
                    .pin_mode(&CallId::tool("call-1"), sandbox_mode)
                    .unwrap();
                let session = handle.create_session("main").unwrap();
                let dispatches = Dispatches::new();
                let items = collect(run_shell_call_with_replay(
                    handle.clone(),
                    session,
                    "call-1".to_owned(),
                    None,
                    false,
                    dispatches.streaming(|| {
                        Some(Ok(ToolRunResult {
                            output: ToolOutput::Text(TextOutput::from("pi ran it")),
                            prompt_text: "pi ran it".to_owned(),
                            effective_tool_name: None,
                        }))
                    }),
                ))
                .await;
                assert_eq!(1, dispatches.calls.load(Ordering::SeqCst), "{mode}");
                let [_, ToolStreamItem::Terminal(Ok(result))] = items.as_slice() else {
                    panic!("{mode}: {items:?}");
                };
                assert_eq!("pi ran it", result.prompt_text, "{mode}: kept unchanged");
                assert_eq!(0, sandbox.open_calls(), "{mode}: the call was forgotten");
            });
        });
        assert_eq!(
            warns,
            logged.contains("shell call yielded no decodable output under enforce"),
            "{mode}: {logged}"
        );
        if warns {
            assert!(logged.contains("call-1"), "{logged}");
        }
    }
}

/// A shell result the decoder cannot read still carries the refusals the owner answered while
/// the call ran: they come out of the one release that ends the call, whatever the result's
/// shape, so a held connection the owner refused is not dropped with a `Text` result.
#[tokio::test]
async fn non_bash_result_carries_the_refusals_answered_while_the_call_ran() {
    let tmp = tempfile::tempdir().unwrap();
    let (handle, sandbox) = handle_with_sandbox(tmp.path(), "enforce").await;
    let session = handle.create_session("main").unwrap();
    let call = CallId::tool("call-1");
    let dispatches = Dispatches::new();
    let refused_in = sandbox.clone();
    let items = collect(run_shell_call_with_replay(
        handle.clone(),
        session,
        "call-1".to_owned(),
        None,
        false,
        dispatches.streaming(move || {
            // The owner refused a held connection while the command ran
            refused_in.calls.lock().push_denial(
                &CallId::tool("call-1"),
                "refused api.example:443".to_owned(),
            );
            Some(Ok(ToolRunResult {
                output: ToolOutput::Text(TextOutput::from("pi ran it")),
                prompt_text: "pi ran it".to_owned(),
                effective_tool_name: None,
            }))
        }),
    ))
    .await;
    let [_, ToolStreamItem::Terminal(Ok(result))] = items.as_slice() else {
        panic!("{items:?}");
    };
    assert_eq!(
        "pi ran it\n\n[sandbox] refused api.example:443",
        result.prompt_text
    );
    assert_eq!(0, sandbox.open_calls(), "released with its result");
    assert!(
        sandbox.take_network_denials(&call).is_empty(),
        "drained by the release, not left behind"
    );
}

/// A call whose stream ends without a terminal, or is dropped by its consumer mid-command, does
/// not leave its owner bound (nor, with it, its proxy credential live). Under `observe`: a
/// folder that is `off` binds no call to begin with.
#[tokio::test]
async fn the_calls_owner_is_released_however_the_stream_ends() {
    let tmp = tempfile::tempdir().unwrap();
    let (handle, sandbox) = handle_with_sandbox(tmp.path(), "observe").await;
    let session = handle.create_session("main").unwrap();
    let call = CallId::tool("call-1");

    let dispatches = Dispatches::new();
    collect(run_shell_call_with_replay(
        handle.clone(),
        session.clone(),
        "call-1".to_owned(),
        None,
        false,
        dispatches.streaming(|| None),
    ))
    .await;
    assert!(sandbox.calls.lock().owner_of(&call).is_none());

    let mut stream = run_shell_call_with_replay(
        handle.clone(),
        session,
        "call-1".to_owned(),
        None,
        false,
        || -> ToolStream<ToolRunResult> {
            Box::pin(async_stream::stream! {
                yield ToolStreamItem::Progress(ToolProgress::Text {
                    text: "working".to_owned(),
                });
                futures::future::pending::<()>().await;
            })
        },
    );
    assert!(matches!(
        stream.next().await,
        Some(ToolStreamItem::Progress(_))
    ));
    assert!(
        sandbox.calls.lock().owner_of(&call).is_some(),
        "bound while the command runs"
    );
    drop(stream);
    assert!(sandbox.calls.lock().owner_of(&call).is_none());
}

#[tokio::test]
async fn a_stream_without_a_terminal_is_reported_not_redispatched() {
    let tmp = tempfile::tempdir().unwrap();
    let handle = WorkspaceHandle::for_test_in(tmp.path());
    let session = handle.create_session("main").unwrap();
    let dispatches = Dispatches::new();
    let items = collect(run_shell_call_with_replay(
        handle.clone(),
        session,
        "call-1".to_owned(),
        None,
        false,
        dispatches.streaming(|| None),
    ))
    .await;
    assert_eq!(1, dispatches.calls.load(Ordering::SeqCst));
    let ToolStreamItem::Terminal(Err(error)) = items.last().unwrap() else {
        panic!("{items:?}");
    };
    assert!(error.to_string().contains("without a terminal"), "{error}");
}
