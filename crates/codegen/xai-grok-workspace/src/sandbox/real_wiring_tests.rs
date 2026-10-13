//! The command sandbox through the real tool path (`hub.rs` → `result_path.rs` →
//! `sandbox_gate.rs`) with real children; where an OS backend is needed, a stub kernel refuses one
//! specific write the way Seatbelt's deny rule would.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use futures::StreamExt;
use serde_json::{Value, json};
use xai_computer_hub_sdk::ToolServerHandler;
use xai_grok_paths::AbsPathBuf;
use xai_grok_sandbox::command::backend::{
    BackendCapabilities, CommandTag, OriginalArgv, RenderedPolicy, SandboxBackend,
    SandboxCommandError, WrapReceipt,
};
use xai_grok_sandbox::command::grants::FixedClock;
use xai_grok_sandbox::command::{
    BackendName, Blocked, CallId, GitConfigEnv, SandboxMode, SandboxPolicy,
};
use xai_grok_telemetry::events::SandboxCommandOutcome;
use xai_grok_tools::registry::types::ToolServerConfig;
use xai_grok_tools::types::tool::ToolKind;
use xai_tool_runtime::{
    ContentBlock, SessionContext, ToolApprovalPolicy, ToolCallContext, ToolCallId, ToolError,
    ToolErrorKind, ToolStream, ToolStreamItem, TypedToolOutput,
};
use xai_tool_types::ToolDescription;

use super::metrics;
use super::result_path::PIN_LOST_TEXT;
use super::{
    BackendSource, CALL_TABLE_FULL_TEXT, CallOwner, ENFORCE_UNAVAILABLE_TEXT, MAX_OPEN_CALLS,
    WorkspaceSandbox, WorkspaceSandboxConfig,
};
use crate::capability::CapabilityMode;
use crate::handle::WorkspaceHandle;
use crate::handle::tests::{BASH_CCO_STUB_NAME, BASH_CCO_STUB_STDOUT};
use crate::host_kind::WorkspaceHostKind;
use crate::hub::SessionRoutedToolHandler;
use crate::permission::{
    StateFileAccess, ToolApprovalGate, approval_gate_for, grant_store_access, load_state_from_disk,
    persist_state,
};
use crate::session::tool_config::test_support::tc;

/// Everything the stub kernel refuses lives under this tree: outside the workspace, `/tmp` and
/// every build cache.
const REFUSED_TREE: &str = "/srv/grok-w0-real";
/// A refused write whose parent is too shallow to propose (two components): the card offers the
/// one file, confirmed.
const REFUSED_TARGET: &str = "/srv/grok-w0-real/out.txt";
/// A refused write the card can offer a directory for: the highest missing ancestor, `scratch/`.
const GRANTABLE_TARGET: &str = "/srv/grok-w0-real/scratch/out.txt";

/// Stands in for the OS layer. Every spawn is "wrapped" (the receipt says so); a command that
/// names a path under [`REFUSED_TREE`] the policy does not allow is replaced by what the kernel
/// would have made it print.
struct StubKernel {
    wraps: AtomicUsize,
    /// Spawns of a command naming a refused path: the runs the sandbox let through to the OS.
    refused: AtomicUsize,
    /// The tag of every spawn wrapped, in order.
    wrapped: parking_lot::Mutex<Vec<CommandTag>>,
}

impl StubKernel {
    fn new() -> Arc<StubKernel> {
        Arc::new(StubKernel {
            wraps: AtomicUsize::new(0),
            refused: AtomicUsize::new(0),
            wrapped: parking_lot::Mutex::new(Vec::new()),
        })
    }

    /// Whether `call`'s spawn was wrapped.
    fn wrapped(&self, call: &CallId) -> bool {
        self.wrapped.lock().contains(&CommandTag::for_call(call))
    }
}

/// The `Box<dyn SandboxBackend>` the sandbox owns, sharing the counters with the test.
struct KernelHandle(Arc<StubKernel>);

impl SandboxBackend for KernelHandle {
    fn name(&self) -> BackendName {
        BackendName::Seatbelt
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities::default()
    }

    fn wrap(
        &self,
        cmd: &mut tokio::process::Command,
        original: &OriginalArgv,
        policy: &SandboxPolicy,
        tag: &CommandTag,
    ) -> Result<WrapReceipt, SandboxCommandError> {
        self.0.wraps.fetch_add(1, Ordering::SeqCst);
        self.0.wrapped.lock().push(tag.clone());
        // Like the kernel, refuse the first write under the tree the policy does not allow; a
        // path a grant widened the policy to is let through
        let refused_path = original.args.iter().find_map(|arg| {
            arg.to_string_lossy()
                .split_whitespace()
                .filter(|token| token.starts_with(REFUSED_TREE))
                .find(|token| {
                    !policy.would_allow(&Blocked::FsWrite {
                        path: PathBuf::from(token),
                    })
                })
                .map(str::to_owned)
        });
        if let Some(path) = refused_path {
            self.0.refused.fetch_add(1, Ordering::SeqCst);
            let script =
                format!("printf '%s\\n' \"touch: {path}: Operation not permitted\" >&2; exit 1");
            let mut replacement = tokio::process::Command::new("/bin/sh");
            replacement.arg("-c").arg(&script);
            replacement.current_dir(&original.cwd);
            *cmd = replacement;
        }
        let mut argv: Vec<OsString> = vec![original.program.clone().into_os_string()];
        argv.extend(original.args.iter().cloned());
        Ok(WrapReceipt {
            backend: BackendName::Seatbelt,
            rendered: RenderedPolicy::Sbpl {
                profile: "(version 1)".to_owned(),
                params: argv
                    .iter()
                    .map(|arg| ("ARG".to_owned(), arg.to_string_lossy().into_owned()))
                    .collect(),
            },
        })
    }
}

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    grok_home: PathBuf,
}

impl Fixture {
    fn new(mode: &str) -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("ws");
        let grok_home = tmp.path().join("grok-home");
        std::fs::create_dir_all(root.join(".grok")).unwrap();
        std::fs::create_dir_all(&grok_home).unwrap();
        std::fs::write(
            crate::sandbox_mode::workspace_config_path(&root),
            format!("[sandbox]\nmode = \"{mode}\"\n"),
        )
        .unwrap();
        Fixture {
            _tmp: tmp,
            root,
            grok_home,
        }
    }

    fn config(&self, backend: BackendSource) -> WorkspaceSandboxConfig {
        WorkspaceSandboxConfig {
            workspace_root: self.root.clone(),
            grok_home: self.grok_home.clone(),
            user_home: Some(self._tmp.path().join("home")),
            git_env: GitConfigEnv::default(),
            control_socket_dir: self.grok_home.join("workspaced"),
            remote: None,
            backend,
            clock: Arc::new(FixedClock::at(1_800_000_000)),
        }
    }

    async fn sandbox(&self, backend: BackendSource) -> Arc<WorkspaceSandbox> {
        Arc::new(WorkspaceSandbox::open(self.config(backend)).await)
    }

    /// The folder as the daemon serves it: `WorkspaceSandbox::serve`, the one call
    /// `exposures.rs` makes, so the proxy is started or not exactly as it would be in the daemon.
    async fn serve(&self, backend: BackendSource) -> Arc<WorkspaceSandbox> {
        WorkspaceSandbox::serve(self.config(backend)).await
    }

    /// The handle as the daemon builds it: its pre-run approval gate is the one `HOST_KIND_DAEMON`
    /// resolves to, so every test here runs under the real daemon gate.
    fn handle(&self, sandbox: Arc<WorkspaceSandbox>) -> WorkspaceHandle {
        self.handle_with_gate(sandbox, approval_gate_for(WorkspaceHostKind::Daemon))
    }

    fn handle_with_gate(
        &self,
        sandbox: Arc<WorkspaceSandbox>,
        tool_approval: ToolApprovalGate,
    ) -> WorkspaceHandle {
        let handle = WorkspaceHandle::for_test_in_with_sandbox(&self.root, sandbox, tool_approval);
        handle
            .create_session_with_config(
                "main",
                None,
                Some(ToolServerConfig {
                    tools: vec![
                        tc("GrokBuild:run_terminal_cmd", Some(ToolKind::Execute)),
                        tc(
                            "GrokBuild:get_task_output",
                            Some(ToolKind::BackgroundTaskAction),
                        ),
                        tc("GrokBuild:kill_task", Some(ToolKind::KillTaskAction)),
                        tc("GrokBuild:read_file", Some(ToolKind::Read)),
                        tc("GrokBuild:search_replace", Some(ToolKind::Edit)),
                        tc("OpenCode:write", Some(ToolKind::Write)),
                        tc("Codex:apply_patch", Some(ToolKind::Edit)),
                    ],
                    behavior_preset: None,
                }),
                CapabilityMode::All,
                None,
                false,
            )
            .expect("create the session");
        handle
    }
}

/// What one real `run_terminal_cmd` call produced, as the model would see it.
#[derive(Debug)]
struct CallResult {
    /// `Ok(value)` is the typed Bash output; `Err(text)` the tool error.
    outcome: Result<Value, String>,
    /// The tool error's kind, `None` for a result.
    error_kind: Option<ToolErrorKind>,
    model_text: String,
}

async fn run_bash(handle: &WorkspaceHandle, command: &str) -> CallResult {
    run_tool(
        handle,
        "run_terminal_cmd",
        json!({ "command": command, "description": "real wiring test" }),
    )
    .await
}

async fn run_tool(handle: &WorkspaceHandle, tool: &str, args: Value) -> CallResult {
    run_tool_as(handle, tool, args, ToolCallId::new_v7()).await
}

async fn run_tool_as(
    handle: &WorkspaceHandle,
    tool: &str,
    args: Value,
    call_id: ToolCallId,
) -> CallResult {
    let handler = SessionRoutedToolHandler::new(
        tool.to_owned(),
        ToolDescription::new(tool.to_owned(), String::new()),
        None,
        handle.clone(),
    )
    .expect("tool id");
    let mut ctx = ToolCallContext::new(call_id);
    ctx.insert(SessionContext("main".to_owned()));
    terminal_of(handler.handle_call(ctx, args).await).await
}

/// The call's terminal item, as the model would see it.
async fn terminal_of(mut stream: ToolStream<TypedToolOutput>) -> CallResult {
    while let Some(item) = stream.next().await {
        match item {
            ToolStreamItem::Progress(_) => {}
            ToolStreamItem::Terminal(Ok(output)) => {
                let model_text = output
                    .model_output
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                return CallResult {
                    outcome: Ok(output.value),
                    error_kind: None,
                    model_text,
                };
            }
            ToolStreamItem::Terminal(Err(error)) => {
                return CallResult {
                    outcome: Err(error.to_string()),
                    error_kind: Some(error.kind),
                    model_text: error.to_string(),
                };
            }
        }
    }
    panic!("the tool stream ended without a terminal item");
}

fn exit_code(value: &Value) -> i64 {
    value
        .get("output")
        .and_then(|output| output.get("exit_code"))
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("bash output has exit_code: {value}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn observe_runs_the_real_child_unwrapped_counts_it_and_never_shows_a_card() {
    let fx = Fixture::new("observe");
    let kernel = StubKernel::new();
    let sandbox = fx
        .sandbox(BackendSource::Fixed(Some(Box::new(KernelHandle(
            kernel.clone(),
        )))))
        .await;
    let handle = fx.handle(sandbox.clone());
    let ran_before = metrics::command_total(
        SandboxMode::Observe,
        Some(BackendName::Seatbelt),
        SandboxCommandOutcome::Ran,
    );

    let marker = fx.root.join("observe-wrote-this");
    let result = run_bash(
        &handle,
        &format!(
            "echo \"sandboxed=${{GROK_SANDBOXED:-unset}}\"; touch {} && echo wrote",
            marker.display()
        ),
    )
    .await;
    let value = result
        .outcome
        .clone()
        .expect("observe never fails a command");
    assert_eq!(0, exit_code(&value), "{result:?}");
    assert!(result.model_text.contains("wrote"), "{result:?}");
    assert!(marker.exists(), "the real child ran and wrote the file");
    assert!(
        !result.model_text.contains("[sandbox]"),
        "observe never appends a denial: {result:?}"
    );
    assert_eq!(
        0,
        kernel.wraps.load(Ordering::SeqCst),
        "observe never calls the enforcing wrap"
    );
    assert_eq!(0, sandbox.open_calls(), "finish consumed the call record");
    let ran_after = metrics::command_total(
        SandboxMode::Observe,
        Some(BackendName::Seatbelt),
        SandboxCommandOutcome::Ran,
    );
    assert!(
        ran_after > ran_before,
        "grok_workspace_sandbox_command_total{{observe,seatbelt,ran}} {ran_before} -> {ran_after}"
    );
    assert!(sandbox.observe_summary().would_block.is_empty());
    let status = sandbox.status_json();
    assert_eq!(Some(&json!("observe")), status.get("mode"));
    // The trusted folder's `observe` tightens the default `off`
    assert_eq!(Some(&json!("workspace_config")), status.get("mode_source"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enforce_without_a_backend_refuses_the_real_command_with_the_documented_text() {
    let fx = Fixture::new("enforce");
    let sandbox = fx.sandbox(BackendSource::Fixed(None)).await;
    let handle = fx.handle(sandbox.clone());

    let marker = fx.root.join("enforce-must-not-write");
    let result = run_bash(&handle, &format!("touch {}", marker.display())).await;
    assert!(
        !marker.exists(),
        "nothing may run: the refusal happens before the spawn"
    );
    assert!(
        result.model_text.contains(ENFORCE_UNAVAILABLE_TEXT),
        "the model sees the documented text: {result:?}"
    );
    assert_eq!(0, sandbox.open_calls());
    let status = sandbox.status_json();
    assert_eq!(Some(&json!("none")), status.get("backend"));
    assert_eq!(Some(&json!("workspace_config")), status.get("mode_source"));
}

/// A full call table evicts nothing and refuses a new command before it spawns — a gated call at
/// the hub's pin (it runs only pinned), a pre-run-prompt call unpinned at its `prepare` — while a
/// shell tool that spawns nothing (stopping a background job) and a non-shell tool run to a result.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_full_call_table_refuses_a_new_command_but_not_a_tool_that_spawns_nothing() {
    for mode in ["enforce", "observe"] {
        let fx = Fixture::new(mode);
        let kernel = StubKernel::new();
        let sandbox = fx
            .sandbox(BackendSource::Fixed(Some(Box::new(KernelHandle(
                kernel.clone(),
            )))))
            .await;
        let handle = fx.handle(sandbox.clone());
        for i in 0..MAX_OPEN_CALLS {
            let owner = CallOwner {
                session_id: "sess-1".to_owned(),
                policy: ToolApprovalPolicy::GrantsAllowed,
                transport: None,
                command: None,
            };
            sandbox
                .bind_call(&CallId::tool(format!("running-{i}")), owner)
                .unwrap();
        }

        let marker = fx.root.join("must-not-exist");
        let shell = run_bash(&handle, &format!("touch {}", marker.display())).await;
        assert!(
            shell.model_text.contains(CALL_TABLE_FULL_TEXT),
            "{mode}: {shell:?}"
        );
        assert!(
            !shell.model_text.contains(PIN_LOST_TEXT),
            "{mode}: refused for the room, not for a lost pin: {shell:?}"
        );
        if mode == "enforce" {
            assert_eq!(
                Some(ToolErrorKind::ConcurrencyLimit),
                shell.error_kind,
                "{mode}: the gated call is refused at the hub's pin: {shell:?}"
            );
        } else {
            assert_ne!(
                Some(ToolErrorKind::ConcurrencyLimit),
                shell.error_kind,
                "{mode}: the pre-run-prompt call got past the hub, refused at prepare: {shell:?}"
            );
        }
        assert!(!marker.exists(), "{mode}: the command never ran");
        assert_eq!(0, kernel.wraps.load(Ordering::SeqCst), "{mode}");
        assert_eq!(MAX_OPEN_CALLS, sandbox.calls.lock().len(), "{mode}");

        // A shell tool that spawns nothing, under any mode a pre-run-prompt call (its output is
        // not one the decoder reads): unpinned and unbound, it runs to its result
        crate::handle::tests::register_bash_cco_stub_on(&handle, "main");
        let stop_id = ToolCallId::new_v7();
        let stop_call = CallId::tool(stop_id.to_string());
        let stop = run_tool_as(&handle, BASH_CCO_STUB_NAME, json!({}), stop_id).await;
        let value = stop
            .outcome
            .clone()
            .unwrap_or_else(|error| panic!("{mode}: the spawn-less call completes: {error}"));
        assert_eq!(0, exit_code(&value), "{mode}: {stop:?}");
        assert!(
            stop.model_text.contains(BASH_CCO_STUB_STDOUT),
            "{mode}: {stop:?}"
        );
        assert_eq!(
            MAX_OPEN_CALLS,
            sandbox.calls.lock().len(),
            "{mode}: nothing was created for it"
        );
        assert_eq!(
            None,
            sandbox.calls.lock().floor_of(&stop_call),
            "{mode}: its result dropped its floor"
        );
        assert_eq!(0, kernel.wraps.load(Ordering::SeqCst), "{mode}");

        let readable = fx.root.join("readable.txt");
        std::fs::write(&readable, "still readable\n").unwrap();
        let read = run_tool(&handle, "read_file", json!({ "target_file": readable })).await;
        assert!(read.outcome.is_ok(), "{mode}: {read:?}");
        assert!(
            read.model_text.contains("still readable"),
            "{mode}: {read:?}"
        );
        assert_eq!(MAX_OPEN_CALLS, sandbox.calls.lock().len(), "{mode}");
    }
}

/// A shell tool of a namespace the decoder does not read (the pre-run-prompt path under every
/// mode) that spawns `command` through the sandbox's launch hook, as the terminal does.
struct SpawningShellTool {
    sandbox: Arc<WorkspaceSandbox>,
    cwd: PathBuf,
}

const SPAWNING_SHELL: &str = "spawning_shell";

impl std::fmt::Debug for SpawningShellTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpawningShellTool")
            .field("cwd", &self.cwd)
            .finish()
    }
}

impl xai_grok_tools::types::tool_metadata::ToolMetadata for SpawningShellTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Execute
    }
    fn tool_namespace(&self) -> xai_grok_tools::types::tool::ToolNamespace {
        xai_grok_tools::types::tool::ToolNamespace::MCP
    }
    fn description_template(&self) -> &str {
        "spawning shell"
    }
}

impl xai_tool_runtime::Tool for SpawningShellTool {
    type Args = Value;
    type Output = String;

    fn id(&self) -> xai_tool_protocol::ToolId {
        xai_tool_protocol::ToolId::new(SPAWNING_SHELL).expect("valid id")
    }
    fn description(&self, _ctx: &xai_tool_runtime::ListToolsContext) -> ToolDescription {
        ToolDescription::new(SPAWNING_SHELL, "spawning shell")
    }
    async fn run(&self, ctx: ToolCallContext, input: Value) -> Result<String, ToolError> {
        let command = input
            .get("command")
            .and_then(Value::as_str)
            .expect("a command")
            .to_owned();
        let mut cmd = tokio::process::Command::new("/bin/sh");
        cmd.arg("-c").arg(&command).current_dir(&self.cwd);
        let call = CallId::tool(ctx.call_id.to_string());
        let terminal = |error: String| ToolError::new(ToolErrorKind::TerminalError, error);
        let bound = match self.sandbox.calls.lock().session_of(&call) {
            Some(_) => "bound",
            None => "unbound",
        };
        xai_grok_tools::sandbox_launch::prepare(Some(&*self.sandbox), &mut cmd, &call)
            .map_err(|error| terminal(error.to_string()))?;
        let status = cmd
            .status()
            .await
            .map_err(|error| terminal(error.to_string()))?;
        Ok(format!("spawning shell exited {status}, {bound}"))
    }
}

/// A pre-run-prompt shell call the full table had no room to pin spawns no weaker than the mode
/// it was dispatched under — enforced once the table has room, refused unrun while it has none —
/// and gets the pinned treatment meanwhile: counted, the proxy kept up, bound at its spawn.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unpinned_shell_call_spawns_no_weaker_than_its_dispatch_mode_after_a_flip() {
    for (flip, drained) in [
        (SandboxMode::Off, true),
        (SandboxMode::Observe, true),
        (SandboxMode::Off, false),
    ] {
        let fx = Fixture::new("enforce");
        let kernel = StubKernel::new();
        let sandbox = fx
            .sandbox(BackendSource::Fixed(Some(Box::new(KernelHandle(
                kernel.clone(),
            )))))
            .await;
        let handle = fx.handle(sandbox.clone());
        let running: Vec<CallId> = (0..MAX_OPEN_CALLS)
            .map(|i| CallId::tool(format!("running-{i}")))
            .collect();
        for call in &running {
            let owner = CallOwner {
                session_id: "sess-1".to_owned(),
                policy: ToolApprovalPolicy::GrantsAllowed,
                transport: None,
                command: None,
            };
            sandbox.bind_call(call, owner).unwrap();
        }
        handle
            .session("main")
            .expect("the fixture's session")
            .toolset()
            .register_tool(
                SPAWNING_SHELL.to_owned(),
                SpawningShellTool {
                    sandbox: sandbox.clone(),
                    cwd: fx.root.clone(),
                },
                Some(json!({ "type": "object", "properties": {} })),
            )
            .expect("register the spawning shell");

        let marker = fx.root.join("floored");
        let handler = SessionRoutedToolHandler::new(
            SPAWNING_SHELL.to_owned(),
            ToolDescription::new(SPAWNING_SHELL, String::new()),
            None,
            handle.clone(),
        )
        .expect("tool id");
        let call_id = ToolCallId::new_v7();
        let call = CallId::tool(call_id.to_string());
        let mut ctx = ToolCallContext::new(call_id);
        ctx.insert(SessionContext("main".to_owned()));
        let stream = handler
            .handle_call(
                ctx,
                json!({ "command": format!("touch {}", marker.display()) }),
            )
            .await;
        assert_eq!(None, sandbox.held_mode(&call), "{flip:?}: no room to pin");
        assert_eq!(
            Some(SandboxMode::Enforce),
            sandbox.calls.lock().floor_of(&call),
            "{flip:?}: the dispatch mode is the call's floor"
        );
        sandbox
            .sync_network()
            .await
            .expect("the proxy binds")
            .expect("enforce starts one");

        let (_, effective) = sandbox
            .set_workspace_mode(flip)
            .expect("the folder's .grok/ is writable");
        assert_eq!(flip, effective.mode);
        assert_eq!(flip, sandbox.mode());
        assert_eq!(
            1,
            sandbox.running_under_another_mode(flip),
            "{flip:?}: the floored call counts in the mode verb's reply"
        );
        if drained {
            for call in &running {
                sandbox.release_call(call);
            }
            assert_eq!(
                0,
                sandbox.calls.lock().len(),
                "{flip:?}: the table has room"
            );
        }

        let result = terminal_of(stream).await;
        if drained {
            let output = result
                .outcome
                .clone()
                .unwrap_or_else(|error| panic!("{flip:?}: enforced once there is room: {error}"));
            assert!(marker.exists(), "{flip:?}: the command ran: {result:?}");
            assert!(
                kernel.wrapped(&call),
                "{flip:?}: the spawn was wrapped, not run under the folder's weaker mode"
            );
            assert!(
                output
                    .get("prompt_text")
                    .and_then(Value::as_str)
                    .is_some_and(|text| text.ends_with(", bound")),
                "{flip:?}: bound to its session at the spawn, as a pinned call is: {output}"
            );
            // Observe kept the proxy through the run (the call's end found it up and scheduled
            // its stop, the call being the last under enforce); off had stopped it at the dispatch
            assert_eq!(
                usize::from(flip == SandboxMode::Observe),
                sandbox.proxy_resyncs(),
                "{flip:?}: observe keeps the proxy up for the floored enforce call; off stops it"
            );
            if flip == SandboxMode::Observe {
                sandbox.proxy_resynced().await;
                assert!(
                    sandbox.network().is_none(),
                    "{flip:?}: the proxy goes with the last enforce call"
                );
            }
        } else {
            assert!(
                result.model_text.contains(CALL_TABLE_FULL_TEXT),
                "{flip:?}: refused for the room at prepare: {result:?}"
            );
            assert_ne!(
                Some(ToolErrorKind::ConcurrencyLimit),
                result.error_kind,
                "{flip:?}: past the hub: {result:?}"
            );
            assert!(
                !marker.exists(),
                "{flip:?}: never run unsandboxed while the table is full"
            );
            assert_eq!(MAX_OPEN_CALLS, sandbox.calls.lock().len(), "{flip:?}");
            assert_eq!(
                0,
                sandbox.proxy_resyncs(),
                "{flip:?}: off stopped the proxy at dispatch"
            );
        }
        assert_eq!(
            None,
            sandbox.calls.lock().floor_of(&call),
            "{flip:?}: the result dropped the floor"
        );
        sandbox.stop_network().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enforce_decodes_a_refused_write_and_keeps_the_denial_without_a_hub() {
    let fx = Fixture::new("enforce");
    let kernel = StubKernel::new();
    let sandbox = fx
        .sandbox(BackendSource::Fixed(Some(Box::new(KernelHandle(
            kernel.clone(),
        )))))
        .await;
    let handle = fx.handle(sandbox.clone());
    let violations_before = metrics::command_total(
        SandboxMode::Enforce,
        Some(BackendName::Seatbelt),
        SandboxCommandOutcome::Violation,
    );

    // Inside the workspace: wrapped, allowed, runs for real
    let inside = fx.root.join("inside.txt");
    let ok = run_bash(
        &handle,
        &format!("echo hi > {} && cat {}", inside.display(), inside.display()),
    )
    .await;
    let value = ok.outcome.clone().expect("an allowed command succeeds");
    assert_eq!(0, exit_code(&value), "{ok:?}");
    assert!(ok.model_text.contains("hi"), "{ok:?}");
    assert!(!ok.model_text.contains("[sandbox]"), "{ok:?}");
    assert!(
        kernel.wraps.load(Ordering::SeqCst) >= 1,
        "enforce wraps every spawn"
    );

    // Outside: the stub kernel refuses; the result path decodes it and, with no hub transport,
    // keeps the denial and tells the model
    let denied = run_bash(&handle, &format!("touch {REFUSED_TARGET}")).await;
    let value = denied
        .outcome
        .clone()
        .expect("a denial is a tool result, not a tool error");
    assert_eq!(1, exit_code(&value), "{denied:?}");
    assert!(
        denied
            .model_text
            .contains("[sandbox] sandbox denied: write to /srv/grok-w0-real/out.txt"),
        "{denied:?}"
    );
    assert!(
        denied.model_text.contains("outside workspace"),
        "{denied:?}"
    );
    assert!(
        denied
            .model_text
            .contains("no channel to the session owner"),
        "fail closed without a hub: {denied:?}"
    );
    assert!(!Path::new(REFUSED_TARGET).exists());
    assert_eq!(0, sandbox.open_calls());
    assert!(
        sandbox.live_grants().await.is_empty(),
        "no grant without the owner's answer"
    );
    let violations_after = metrics::command_total(
        SandboxMode::Enforce,
        Some(BackendName::Seatbelt),
        SandboxCommandOutcome::Violation,
    );
    assert!(
        violations_after > violations_before,
        "grok_workspace_sandbox_command_total{{enforce,seatbelt,violation}} {violations_before} -> {violations_after}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn off_leaves_the_real_command_alone() {
    let fx = Fixture::new("off");
    let kernel = StubKernel::new();
    let sandbox = fx
        .sandbox(BackendSource::Fixed(Some(Box::new(KernelHandle(
            kernel.clone(),
        )))))
        .await;
    let handle = fx.handle(sandbox.clone());
    let result = run_bash(&handle, "echo off-mode").await;
    let value = result.outcome.clone().expect("off runs everything");
    assert_eq!(0, exit_code(&value));
    assert!(result.model_text.contains("off-mode"));
    assert_eq!(0, kernel.wraps.load(Ordering::SeqCst));
    assert_eq!(0, sandbox.open_calls());
}

/// A `HOST_KIND_DAEMON` handle with `[sandbox] mode = off` runs a shell call with zero permission
/// hooks: the daemon's pre-run gate is `Off` and under `off` the sandbox gate has nothing to
/// settle. Were the daemon arm `Enforced`, this handle (no hub) would refuse the call.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn daemon_host_runs_a_shell_call_under_off_with_zero_permission_hooks() {
    let fx = Fixture::new("off");
    let sandbox = fx.sandbox(BackendSource::Fixed(None)).await;
    let owner = Arc::new(ScriptedOwner::silent());
    sandbox.set_card_transport(owner.clone());
    assert_eq!(
        ToolApprovalGate::Off,
        approval_gate_for(WorkspaceHostKind::Daemon)
    );
    let handle = fx.handle(sandbox.clone());

    let result = run_bash(&handle, "echo unasked").await;
    let value = result.outcome.clone().expect("the call ran unasked");
    assert_eq!(0, exit_code(&value), "{result:?}");
    assert!(result.model_text.contains("unasked"), "{result:?}");
    assert!(
        !result.model_text.contains("permission"),
        "no pre-run denial text: {result:?}"
    );
    assert_eq!(
        Vec::<Value>::new(),
        owner.seen.lock().clone(),
        "no card of any kind was posted"
    );
}

/// Where the pre-run approval gate is enforced, a shell call on a folder whose
/// sandbox is on is the sandbox gate's alone — with no hub transport `approve_hub_call` would fail
/// closed, and the command runs anyway; a non-shell mutating tool on the same handle still meets
/// the pre-run gate.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enforced_pre_run_gate_leaves_shell_calls_to_the_sandbox_gate() {
    let fx = Fixture::new("enforce");
    let kernel = StubKernel::new();
    let sandbox = fx
        .sandbox(BackendSource::Fixed(Some(Box::new(KernelHandle(
            kernel.clone(),
        )))))
        .await;
    let handle = fx.handle_with_gate(sandbox.clone(), ToolApprovalGate::Enforced);

    // `rm` is never let through unasked by the pre-run classifier (`echo` alone would be), so
    // the call running at all shows the pre-run gate never saw it
    let marker = fx.root.join("scratch.txt");
    std::fs::write(&marker, "x\n").unwrap();
    let shell = run_bash(
        &handle,
        &format!("rm -f {} && echo sandbox-gated", marker.display()),
    )
    .await;
    let value = shell
        .outcome
        .clone()
        .expect("the shell call never met the pre-run gate");
    assert_eq!(0, exit_code(&value), "{shell:?}");
    assert!(shell.model_text.contains("sandbox-gated"), "{shell:?}");
    assert!(!marker.exists(), "the command ran");
    assert!(
        kernel.wraps.load(Ordering::SeqCst) >= 1,
        "the sandbox ran it: enforce wraps every spawn"
    );

    let target = fx.root.join("edited.txt");
    std::fs::write(&target, "before\n").unwrap();
    let edit = run_tool(
        &handle,
        "search_replace",
        json!({ "file_path": target, "old_string": "before", "new_string": "after" }),
    )
    .await;
    assert_eq!(
        Err("tool permission unavailable (no hub transport)".to_owned()),
        edit.outcome,
        "a non-shell mutating tool still waits for the owner: {edit:?}"
    );
    assert_eq!("before\n", std::fs::read_to_string(&target).unwrap());
}

/// A run requested in the background never reaches the decoder, so the sandbox card cannot be its
/// gate: where the pre-run approval is enforced it meets the pre-run gate (with no hub transport
/// it fails closed unrun), while the same command in the foreground is the sandbox gate's alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enforced_pre_run_gate_still_asks_for_a_run_requested_in_the_background() {
    let fx = Fixture::new("enforce");
    let kernel = StubKernel::new();
    let sandbox = fx
        .sandbox(BackendSource::Fixed(Some(Box::new(KernelHandle(
            kernel.clone(),
        )))))
        .await;
    let handle = fx.handle_with_gate(sandbox.clone(), ToolApprovalGate::Enforced);
    let marker = fx.root.join("keep.txt");
    std::fs::write(&marker, "kept\n").unwrap();
    let command = format!("rm -f {}", marker.display());
    for request in [
        json!({ "is_background": true }),
        json!({ "block_until_ms": 0 }),
        json!({ "timeout": 0 }),
    ] {
        let mut args = json!({ "command": command, "description": "real wiring test" });
        args.as_object_mut()
            .unwrap()
            .extend(request.as_object().unwrap().clone());
        let shell = run_tool(&handle, "run_terminal_cmd", args).await;
        assert_eq!(
            Err("tool permission unavailable (no hub transport)".to_owned()),
            shell.outcome,
            "{request}: {shell:?}"
        );
        assert!(marker.exists(), "the call never ran: {request}");
        assert_eq!(0, kernel.wraps.load(Ordering::SeqCst), "{request}");
        assert_eq!(0, sandbox.open_calls(), "nothing left pinned: {request}");
    }

    let shell = run_bash(&handle, &command).await;
    let value = shell
        .outcome
        .clone()
        .expect("the foreground run never met the pre-run gate");
    assert_eq!(0, exit_code(&value), "{shell:?}");
    assert!(!marker.exists(), "the foreground command ran");
    assert!(
        kernel.wraps.load(Ordering::SeqCst) >= 1,
        "the sandbox ran it: enforce wraps every spawn"
    );
}

/// `observe` never raises a card, so it is no gate: where the pre-run approval is enforced, a
/// shell call under `observe` still meets it (with no hub transport, it fails closed unrun).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enforced_pre_run_gate_still_asks_for_shell_calls_under_observe() {
    let fx = Fixture::new("observe");
    let kernel = StubKernel::new();
    let sandbox = fx
        .sandbox(BackendSource::Fixed(Some(Box::new(KernelHandle(
            kernel.clone(),
        )))))
        .await;
    let handle = fx.handle_with_gate(sandbox.clone(), ToolApprovalGate::Enforced);
    let marker = fx.root.join("keep.txt");
    std::fs::write(&marker, "kept\n").unwrap();
    let shell = run_bash(&handle, &format!("rm -f {}", marker.display())).await;
    assert_eq!(
        Err("tool permission unavailable (no hub transport)".to_owned()),
        shell.outcome,
        "{shell:?}"
    );
    assert!(marker.exists(), "the call never ran");
    assert_eq!(0, kernel.wraps.load(Ordering::SeqCst));
}

/// The folder's persisted `permission.toml` denies stand ahead of the sandbox gate: under
/// `enforce`, a shell call a deny row names is refused before anything is spawned — no wrap, no
/// child, and nothing left pinned in the call table — while a sibling command the folder has no
/// answer for still runs under the sandbox's card.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enforced_pre_run_gate_refuses_a_persisted_deny_before_the_spawn() {
    let fx = Fixture::new("enforce");
    let kernel = StubKernel::new();
    let sandbox = fx
        .sandbox(BackendSource::Fixed(Some(Box::new(KernelHandle(
            kernel.clone(),
        )))))
        .await;
    let handle = fx.handle_with_gate(sandbox.clone(), ToolApprovalGate::Enforced);
    let session = handle.session("main").expect("the fixture's session");
    let cwd = AbsPathBuf::new(session.cwd().to_path_buf()).expect("absolute session cwd");
    let mut state = load_state_from_disk(&cwd, None).await;
    state.disallowed_bash_commands.insert("curl".to_owned());
    persist_state(&cwd, &state, None).await;

    let denied = run_bash(&handle, "curl http://127.0.0.1:9/ && echo ran-anyway").await;
    let error = denied
        .outcome
        .clone()
        .expect_err("the folder's deny row refuses the call");
    assert!(error.contains("previously rejected"), "{denied:?}");
    assert_eq!(
        0,
        kernel.wraps.load(Ordering::SeqCst),
        "the refusal happened before the spawn: nothing was wrapped, nothing ran"
    );
    assert_eq!(
        0,
        sandbox.calls.lock().len(),
        "a refused call leaves nothing pinned in the call table"
    );

    // `rm` is never let through unasked by the pre-run classifier: with no deny row for it, the
    // call is the sandbox card's and runs
    let marker = fx.root.join("scratch.txt");
    std::fs::write(&marker, "x\n").unwrap();
    let ran = run_bash(&handle, &format!("rm -f {} && echo ran", marker.display())).await;
    let value = ran
        .outcome
        .clone()
        .expect("a command the folder has no answer for is the sandbox card's");
    assert_eq!(0, exit_code(&value), "{ran:?}");
    assert!(
        !marker.exists(),
        "the sibling command ran under the sandbox"
    );
    assert!(
        kernel.wraps.load(Ordering::SeqCst) >= 1,
        "enforce wrapped it"
    );
}

/// An `always_prompt` tenant has no persisted answer a sandbox card could stand in for, so under
/// `enforce` it is still asked before the run: with no hub transport the call fails closed, unrun.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enforced_pre_run_gate_still_asks_always_prompt_tenants_under_enforce() {
    let fx = Fixture::new("enforce");
    let kernel = StubKernel::new();
    let sandbox = fx
        .sandbox(BackendSource::Fixed(Some(Box::new(KernelHandle(
            kernel.clone(),
        )))))
        .await;
    let handle = fx.handle_with_gate(sandbox.clone(), ToolApprovalGate::Enforced);
    handle
        .session("main")
        .expect("the fixture's session")
        .approval
        .set_policy(ToolApprovalPolicy::AlwaysPrompt);

    let marker = fx.root.join("kept-for-the-owner.txt");
    std::fs::write(&marker, "kept\n").unwrap();
    let shell = run_bash(&handle, &format!("rm -f {}", marker.display())).await;
    assert_eq!(
        Err("tool permission unavailable (no hub transport)".to_owned()),
        shell.outcome,
        "{shell:?}"
    );
    assert!(marker.exists(), "the call never ran");
    assert_eq!(0, kernel.wraps.load(Ordering::SeqCst));
    assert_eq!(0, sandbox.calls.lock().len());
}

/// On the daemon, whose pre-run gate asks nothing, no file tool writes a mode layer under
/// `enforce`, however the path is spelled, nor a path that cannot be resolved: the call is refused
/// unrun, while the same tools still write any other file. Under `observe` nothing is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_file_tool_never_writes_a_mode_layer_under_enforce() {
    let fx = Fixture::new("enforce");
    let enforce = "[sandbox]\nmode = \"enforce\"\n";
    let lowered = "[sandbox]\nmode = \"off\"\n";
    let user_layer = crate::sandbox_mode::user_config_path(&fx.grok_home);
    let workspace_layer = crate::sandbox_mode::workspace_config_path(&fx.root);
    std::fs::write(&user_layer, enforce).unwrap();
    std::os::unix::fs::symlink(&fx.grok_home, fx.root.join("grok-link")).unwrap();
    let handle = fx.handle(fx.sandbox(BackendSource::Fixed(None)).await);
    let notes = fx.root.join("notes.txt");
    let patch = "*** Begin Patch\n*** Add File: notes.txt\n+kept out\n\
                 *** Delete File: .grok/workspaced.toml\n*** End Patch";
    for (tool, args) in [
        (
            "search_replace",
            json!({ "file_path": user_layer, "old_string": "enforce", "new_string": "off" }),
        ),
        (
            "write",
            json!({ "file_path": fx.root.join("grok-link/workspaced.toml"), "content": lowered }),
        ),
        (
            "write",
            json!({ "file_path": ".grok/./workspaced.toml", "content": lowered }),
        ),
        (
            "write",
            json!({ "file_path": "missing/../.grok/workspaced.toml", "content": lowered }),
        ),
        (
            "write",
            json!({ "file_path": ".grok/workspaced.toml/workspaced.toml", "content": lowered }),
        ),
        ("apply_patch", json!({ "patch": patch })),
    ] {
        let call = run_tool(&handle, tool, args).await;
        assert_eq!(
            Err(super::MODE_LAYER_WRITE_TEXT.to_owned()),
            call.outcome,
            "{tool}: {call:?}"
        );
    }
    assert_eq!(enforce, std::fs::read_to_string(&user_layer).unwrap());
    assert_eq!(enforce, std::fs::read_to_string(&workspace_layer).unwrap());
    assert!(!notes.exists(), "no hunk of the refused patch ran");
    assert!(!fx.root.join("missing").exists(), "the refused write ran");

    let wrote = run_tool(
        &handle,
        "write",
        json!({ "file_path": notes, "content": "draft\n" }),
    )
    .await;
    wrote
        .outcome
        .unwrap_or_else(|error| panic!("write: {error}"));
    let edited = run_tool(
        &handle,
        "search_replace",
        json!({ "file_path": notes, "old_string": "draft", "new_string": "final" }),
    )
    .await;
    edited
        .outcome
        .unwrap_or_else(|error| panic!("search_replace: {error}"));
    assert_eq!("final\n", std::fs::read_to_string(&notes).unwrap());
    let patched = run_tool(
        &handle,
        "apply_patch",
        json!({ "patch": "*** Begin Patch\n*** Delete File: notes.txt\n*** End Patch" }),
    )
    .await;
    patched
        .outcome
        .unwrap_or_else(|error| panic!("apply_patch: {error}"));
    assert!(!notes.exists(), "the patch ran");

    let observed = Fixture::new("observe");
    let handle = observed.handle(observed.sandbox(BackendSource::Fixed(None)).await);
    let layer = crate::sandbox_mode::workspace_config_path(&observed.root);
    let raised = run_tool(
        &handle,
        "write",
        json!({ "file_path": layer, "content": enforce }),
    )
    .await;
    raised
        .outcome
        .unwrap_or_else(|error| panic!("write under observe: {error}"));
    assert_eq!(enforce, std::fs::read_to_string(&layer).unwrap());
}

/// A mode layer that does not exist yet is guarded as one: under `enforce` no file tool creates
/// the user's (here through a link) while the folder's sets the mode, nor the folder's while the
/// user's does.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_file_tool_never_creates_a_missing_mode_layer_under_enforce() {
    let fx = Fixture::new("enforce");
    let enforce = "[sandbox]\nmode = \"enforce\"\n";
    let lowered = "[sandbox]\nmode = \"off\"\n";
    let user_layer = crate::sandbox_mode::user_config_path(&fx.grok_home);
    let workspace_layer = crate::sandbox_mode::workspace_config_path(&fx.root);
    std::os::unix::fs::symlink(&fx.grok_home, fx.root.join("grok-link")).unwrap();
    let handle = fx.handle(fx.sandbox(BackendSource::Fixed(None)).await);

    let call = run_tool(
        &handle,
        "write",
        json!({ "file_path": fx.root.join("grok-link/workspaced.toml"), "content": lowered }),
    )
    .await;
    assert_eq!(
        Err(super::MODE_LAYER_WRITE_TEXT.to_owned()),
        call.outcome,
        "{call:?}"
    );
    assert!(!user_layer.exists(), "the user layer was created");

    std::fs::write(&user_layer, enforce).unwrap();
    std::fs::remove_file(&workspace_layer).unwrap();
    let patch = "*** Begin Patch\n*** Add File: .grok/workspaced.toml\n+[sandbox]\n\
                 +mode = \"off\"\n*** End Patch";
    for (tool, args) in [
        (
            "write",
            json!({ "file_path": workspace_layer, "content": lowered }),
        ),
        ("apply_patch", json!({ "patch": patch })),
    ] {
        let call = run_tool(&handle, tool, args).await;
        assert_eq!(
            Err(super::MODE_LAYER_WRITE_TEXT.to_owned()),
            call.outcome,
            "{tool}: {call:?}"
        );
    }
    assert!(!workspace_layer.exists(), "the folder's layer was created");
    assert_eq!(enforce, std::fs::read_to_string(&user_layer).unwrap());
}

/// A tool that holds its run until the test lets it go, so the call table can be read mid-call.
#[derive(Debug)]
struct HeldTool {
    kind: ToolKind,
    id: &'static str,
    started: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

impl xai_grok_tools::types::tool_metadata::ToolMetadata for HeldTool {
    fn kind(&self) -> ToolKind {
        self.kind
    }
    fn tool_namespace(&self) -> xai_grok_tools::types::tool::ToolNamespace {
        xai_grok_tools::types::tool::ToolNamespace::MCP
    }
    fn description_template(&self) -> &str {
        "held tool"
    }
}

impl xai_tool_runtime::Tool for HeldTool {
    type Args = Value;
    type Output = String;

    fn id(&self) -> xai_tool_protocol::ToolId {
        xai_tool_protocol::ToolId::new(self.id).expect("valid id")
    }
    fn description(&self, _ctx: &xai_tool_runtime::ListToolsContext) -> ToolDescription {
        ToolDescription::new(self.id, "held tool")
    }
    async fn run(
        &self,
        _ctx: ToolCallContext,
        _input: Value,
    ) -> Result<String, xai_tool_runtime::ToolError> {
        self.started.notify_one();
        self.release.notified().await;
        Ok("held tool ran".to_owned())
    }
}

/// A tool that is no shell never meets the sandbox: while it runs, under `enforce`, the call
/// table holds nothing for it — no mode, no bound call, nothing for the decoder — where a shell
/// tool of any namespace is bound for its run under the mode the hub dispatched it with, and one
/// whose output the decoder does not read (an MCP shell here) keeps the pre-run prompt and gets
/// no card.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tool_that_is_no_shell_never_meets_the_sandbox() {
    let fx = Fixture::new("enforce");
    let sandbox = fx.sandbox(BackendSource::Fixed(None)).await;
    let handle = fx.handle_with_gate(sandbox.clone(), ToolApprovalGate::Off);
    let toolset = handle
        .session("main")
        .expect("the fixture's session")
        .toolset();
    for (name, kind, id, bound, mode) in [
        ("held_read", ToolKind::Read, "held_read", 0, None),
        (
            "held_shell",
            ToolKind::Execute,
            "held_shell",
            1,
            Some(SandboxMode::Enforce),
        ),
    ] {
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        toolset
            .register_tool(
                name.to_owned(),
                HeldTool {
                    kind,
                    id,
                    started: started.clone(),
                    release: release.clone(),
                },
                Some(json!({ "type": "object", "properties": {} })),
            )
            .expect("register the held tool");
        let call_id = ToolCallId::new_v7();
        let call = CallId::tool(call_id.to_string());
        let observe = async {
            started.notified().await;
            let calls = sandbox.calls.lock();
            assert_eq!(
                bound,
                calls.len(),
                "{name}: bound in the call table mid-run"
            );
            assert_eq!(
                mode,
                calls.held_mode(&call),
                "{name}: a shell runs under the mode it was dispatched with, nothing else has one"
            );
            drop(calls);
            release.notify_one();
        };
        let (result, ()) = tokio::join!(run_tool_as(&handle, name, json!({}), call_id), observe);
        result
            .outcome
            .clone()
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert!(result.model_text.contains("held tool ran"), "{result:?}");
        assert_eq!(0, sandbox.calls.lock().len(), "{name}: nothing left behind");
    }
}

/// The hub gate's grant store is the daemon's own file only while the folder's sandbox is on;
/// under `off`, and with no sandbox wired at all, it stays the CLI's plain file.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_hub_gate_reads_grants_as_the_daemons_own_file_only_while_the_sandbox_is_on() {
    let daemon_owned = StateFileAccess::DaemonOwned {
        grok_home: xai_grok_config::grok_home(),
    };
    for (mode, expected) in [
        ("off", StateFileAccess::Plain),
        ("observe", daemon_owned.clone()),
        ("enforce", daemon_owned),
    ] {
        let fx = Fixture::new(mode);
        let sandbox = fx.sandbox(BackendSource::Fixed(None)).await;
        let handle = fx.handle(sandbox);
        assert_eq!(expected, grant_store_access(&handle), "{mode}");
    }
    let unwired = WorkspaceHandle::for_test();
    assert_eq!(StateFileAccess::Plain, grant_store_access(&unwired));
}

/// The hub lets a shell call past the enforced pre-run gate on the sandbox's enforce pin and its
/// stream runs the call only under that pin: one whose pin is gone by the time its stream first
/// runs, with the folder flipped to `off` meanwhile, is refused unrun rather than run unwrapped.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_shell_call_whose_enforce_pin_is_gone_is_refused_unrun() {
    let fx = Fixture::new("enforce");
    let kernel = StubKernel::new();
    let sandbox = fx
        .sandbox(BackendSource::Fixed(Some(Box::new(KernelHandle(
            kernel.clone(),
        )))))
        .await;
    let handle = fx.handle_with_gate(sandbox.clone(), ToolApprovalGate::Enforced);
    let marker = fx.root.join("keep.txt");
    std::fs::write(&marker, "kept\n").unwrap();
    let handler = SessionRoutedToolHandler::new(
        "run_terminal_cmd".to_owned(),
        ToolDescription::new("run_terminal_cmd".to_owned(), String::new()),
        None,
        handle.clone(),
    )
    .expect("tool id");
    let call_id = ToolCallId::new_v7();
    let mut ctx = ToolCallContext::new(call_id.clone());
    ctx.insert(SessionContext("main".to_owned()));
    let mut stream = handler
        .handle_call(
            ctx,
            json!({ "command": format!("rm -f {}", marker.display()), "description": "pin" }),
        )
        .await;

    sandbox.release_call(&CallId::tool(call_id.to_string()));
    std::fs::write(
        crate::sandbox_mode::workspace_config_path(&fx.root),
        "[sandbox]\nmode = \"off\"\n",
    )
    .unwrap();
    assert_eq!(SandboxMode::Off, sandbox.mode());
    let Some(ToolStreamItem::Terminal(Err(error))) = stream.next().await else {
        panic!("the call is refused");
    };
    assert!(error.to_string().contains(PIN_LOST_TEXT), "{error}");
    assert!(marker.exists(), "the call never ran");
    assert_eq!(0, kernel.wraps.load(Ordering::SeqCst));
}

/// The hub fixes a shell call's mode as it dispatches it: a call dispatched under `observe` runs
/// unwrapped when its command spawns after the folder flipped to `enforce`, and the call
/// dispatched next runs enforced. A mode change reaches the next command, never one already
/// dispatched — the mode change reply counts that one as still running under the old mode.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_call_dispatched_under_observe_runs_observed_after_a_flip_to_enforce() {
    let fx = Fixture::new("observe");
    let kernel = StubKernel::new();
    let sandbox = fx
        .sandbox(BackendSource::Fixed(Some(Box::new(KernelHandle(
            kernel.clone(),
        )))))
        .await;
    let handle = fx.handle(sandbox.clone());
    let handler = SessionRoutedToolHandler::new(
        "run_terminal_cmd".to_owned(),
        ToolDescription::new("run_terminal_cmd".to_owned(), String::new()),
        None,
        handle.clone(),
    )
    .expect("tool id");
    let call_id = ToolCallId::new_v7();
    let call = CallId::tool(call_id.to_string());
    let mut ctx = ToolCallContext::new(call_id);
    ctx.insert(SessionContext("main".to_owned()));
    let mut stream = handler
        .handle_call(
            ctx,
            json!({ "command": "echo observed", "description": "dispatched under observe" }),
        )
        .await;
    assert_eq!(
        Some(SandboxMode::Observe),
        sandbox.held_mode(&call),
        "the dispatch fixed the call's mode"
    );

    // The flip goes the way Desktop's does: `sandbox.mode.set` stores the mode it resolves before
    // it replies, so the read that follows — and the next dispatch's pin — is the new one. A file
    // edited by hand reaches the next command once the layers are re-read, not on the instant
    let (_, effective) = sandbox
        .set_workspace_mode(SandboxMode::Enforce)
        .expect("the folder's .grok/ is writable");
    assert_eq!(SandboxMode::Enforce, effective.mode);
    assert_eq!(SandboxMode::Enforce, sandbox.mode());
    assert_eq!(
        1,
        sandbox.running_under_another_mode(SandboxMode::Enforce),
        "the dispatched call still runs under observe"
    );

    let mut model_text = None;
    while let Some(item) = stream.next().await {
        match item {
            ToolStreamItem::Progress(_) => {}
            ToolStreamItem::Terminal(Ok(output)) => {
                model_text = Some(
                    output
                        .model_output
                        .iter()
                        .filter_map(|block| match block {
                            ContentBlock::Text { text } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n"),
                );
            }
            ToolStreamItem::Terminal(Err(error)) => panic!("observe runs it: {error}"),
        }
    }
    let model_text = model_text.expect("the call ran to its result");
    assert!(model_text.contains("observed"), "{model_text}");
    assert!(
        !kernel.wrapped(&call),
        "the call dispatched under observe met the enforcing wrap"
    );
    assert_eq!(0, sandbox.open_calls());
    assert_eq!(0, sandbox.running_under_another_mode(SandboxMode::Enforce));

    // The terminal's own spawns hold no mode and take the folder's at each spawn; a call
    // dispatched now is enforced
    let next_id = ToolCallId::new_v7();
    let next = CallId::tool(next_id.to_string());
    let result = run_tool_as(
        &handle,
        "run_terminal_cmd",
        json!({ "command": "echo next", "description": "dispatched under enforce" }),
        next_id,
    )
    .await;
    result.outcome.expect("enforce with a backend runs it");
    assert!(
        kernel.wrapped(&next),
        "the call dispatched after the flip is enforced"
    );
}

/// The `off` pin at daemon level: the folder is served as the daemon serves it
/// (`WorkspaceSandbox::serve`) and the real command runs with nothing
/// around it — no proxy process or listener for the folder, no proxy or sandbox variables in the
/// child's environment, and (on Linux, read from `/proc`) the shell's argv is exactly what the
/// tool built and its parent is this process, so no wrapper stands between the two. Its
/// `enforce` twin is [`enforce_serves_the_folder_with_a_proxy_the_real_child_is_pointed_at`].
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn off_starts_no_proxy_and_the_real_child_sees_no_sandbox() {
    let fx = Fixture::new("off");
    let kernel = StubKernel::new();
    let sandbox = fx
        .serve(BackendSource::Fixed(Some(Box::new(KernelHandle(
            kernel.clone(),
        )))))
        .await;
    assert!(
        sandbox.network().is_none(),
        "serving an off folder starts no proxy"
    );
    let status = sandbox.status_json();
    assert_eq!(Some(&json!("off")), status.get("network"));
    assert_eq!(Some(&Value::Null), status.get("proxy"));
    let handle = fx.handle(sandbox.clone());

    let command = "echo PPID=$PPID; tr '\\0' ' ' </proc/$$/cmdline; echo; env";
    let result = run_bash(&handle, command).await;
    let value = result.outcome.clone().expect("off runs everything");
    assert_eq!(0, exit_code(&value), "{result:?}");
    let text = &result.model_text;
    for var in [
        "HTTP_PROXY=",
        "HTTPS_PROXY=",
        "http_proxy=",
        "https_proxy=",
        "ALL_PROXY=",
        "GROK_SANDBOX",
    ] {
        assert!(
            !text.contains(var),
            "{var} reached the child under off: {text}"
        );
    }
    #[cfg(target_os = "linux")]
    {
        assert!(
            text.contains(&format!("PPID={}", std::process::id())),
            "the shell's parent is this process, no wrapper in between: {text}"
        );
        let cmdline = text
            .lines()
            .find(|line| line.contains("-O extglob -c "))
            .unwrap_or_else(|| panic!("the shell's own argv is in the output: {text}"));
        let argv0 = cmdline.split(' ').next().unwrap_or_default();
        assert!(
            argv0.ends_with("bash"),
            "argv[0] is the shell itself, not a wrapper: {cmdline}"
        );
        assert!(
            cmdline.trim_end().ends_with(&format!("-- {command}")),
            "the tool's `-- <command>` closes the argv: {cmdline}"
        );
    }
    assert_eq!(0, kernel.wraps.load(Ordering::SeqCst));
    assert_eq!(0, sandbox.open_calls());
    assert!(sandbox.observe_summary().would_block.is_empty());
    assert!(sandbox.network().is_none(), "still no proxy after the run");
}

/// Serving an `off` folder as the daemon does (`WorkspaceSandbox::serve`) builds its shell only:
/// no probe, no protected-path scan, no grant store (the session directory is not created) and
/// no proxy; a real shell call through the daemon gate binds nothing and pins nothing, so nothing
/// re-reads the grant files either. The mode verb's steps (`set_workspace_mode`, then
/// `sync_network`, as `sandbox.mode.set` runs them) engage the folder exactly once on the first
/// mode that is not `off` — `observe`, which still starts no proxy — and the step on to
/// `enforce` starts the proxy without engaging again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn off_serves_a_shell_and_the_first_mode_verb_that_is_not_off_engages_it_once() {
    let fx = Fixture::new("off");
    let kernel = StubKernel::new();
    let sandbox = fx
        .serve(BackendSource::Fixed(Some(Box::new(KernelHandle(
            kernel.clone(),
        )))))
        .await;
    let own = xai_grok_config::sessions_cwd_dir_in(&fx.grok_home, &fx.root.to_string_lossy());
    assert_eq!(
        0,
        sandbox.engagements(),
        "serving an off folder engages nothing"
    );
    assert!(!own.exists(), "no grant store was opened");
    assert!(sandbox.network().is_none(), "and no proxy started");
    assert_eq!(Some(&json!("none")), sandbox.status_json().get("backend"));

    let handle = fx.handle(sandbox.clone());
    // The call is looked at while its child runs: a binding or a pin is released with the call
    let started = fx.root.join("off-call-started");
    let hold = fx.root.join("off-call-hold");
    std::fs::write(&hold, "").unwrap();
    let command = format!(
        "touch {} && while [ -e {} ]; do sleep 0.05; done",
        started.display(),
        hold.display()
    );
    let running = tokio::spawn({
        let handle = handle.clone();
        async move { run_bash(&handle, &command).await }
    });
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while !started.exists() {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the child started");
    assert_eq!(
        0,
        sandbox.engagements(),
        "a shell call under off engages nothing"
    );
    assert_eq!(
        0,
        sandbox.calls.lock().len(),
        "and is neither bound nor pinned while it runs"
    );
    assert!(!own.exists());
    std::fs::remove_file(&hold).unwrap();
    let result = running.await.unwrap();
    assert_eq!(0, exit_code(&result.outcome.clone().unwrap()), "{result:?}");
    assert_eq!(0, kernel.wraps.load(Ordering::SeqCst));

    sandbox.set_workspace_mode(SandboxMode::Observe).unwrap();
    assert!(matches!(sandbox.sync_network().await, Ok(None)));
    assert_eq!(
        1,
        sandbox.engagements(),
        "the first mode that is not off engaged it"
    );
    assert!(own.is_dir(), "the grant store is open");
    assert!(sandbox.network().is_none(), "observe starts no proxy");
    assert_eq!(
        Some(&json!(<&str>::from(BackendName::Seatbelt))),
        sandbox.status_json().get("backend"),
        "the backend shows once the folder engaged"
    );

    sandbox.set_workspace_mode(SandboxMode::Enforce).unwrap();
    let info = sandbox
        .sync_network()
        .await
        .expect("the proxy binds")
        .expect("enforce starts one");
    assert!(info.address.ip().is_loopback());
    assert_eq!(
        1,
        sandbox.engagements(),
        "engaged once, whatever the mode does next"
    );
    let result = run_bash(&handle, "echo hi").await;
    assert_eq!(0, exit_code(&result.outcome.clone().unwrap()), "{result:?}");
    assert_eq!(
        1,
        kernel.wraps.load(Ordering::SeqCst),
        "the call ran wrapped"
    );
    assert_eq!(1, sandbox.engagements());
    sandbox.stop_network().await;
}

/// The `enforce` twin of the daemon-level `off` pin: serving the folder
/// starts its egress proxy on a loopback port, and the real child is pointed at it — the proxy
/// variables in its environment name that port with a credential minted for the call; the
/// listener answers while the folder is served and is gone once the daemon unserves it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enforce_serves_the_folder_with_a_proxy_the_real_child_is_pointed_at() {
    let fx = Fixture::new("enforce");
    let kernel = StubKernel::new();
    let sandbox = fx
        .serve(BackendSource::Fixed(Some(Box::new(KernelHandle(
            kernel.clone(),
        )))))
        .await;
    let network = sandbox
        .network()
        .expect("serving an enforce folder starts the proxy");
    assert!(network.address.ip().is_loopback(), "{}", network.address);
    assert_eq!(
        Some(network.address.port()),
        sandbox.proxy().map(|proxy| proxy.port)
    );
    assert!(
        tokio::net::TcpStream::connect(network.address)
            .await
            .is_ok(),
        "the listener answers on {}",
        network.address
    );
    let handle = fx.handle(sandbox.clone());

    let result = run_bash(&handle, "env").await;
    let value = result.outcome.clone().expect("enforce runs a plain env");
    assert_eq!(0, exit_code(&value), "{result:?}");
    let text = &result.model_text;
    let port = network.address.port().to_string();
    for var in ["HTTP_PROXY=", "HTTPS_PROXY=", "http_proxy=", "https_proxy="] {
        let line = text
            .lines()
            .find(|line| line.starts_with(var))
            .unwrap_or_else(|| panic!("{var} reached the child under enforce: {text}"));
        assert!(
            line.contains("@127.0.0.1:") && line.ends_with(&port),
            "the child is pointed at the folder's proxy with a call credential: {line}"
        );
    }
    assert!(
        kernel.wraps.load(Ordering::SeqCst) >= 1,
        "enforce wraps the spawn (and the terminal's own init shells)"
    );
    assert_eq!(0, sandbox.open_calls());

    sandbox.stop_network().await;
    assert!(sandbox.network().is_none(), "unserve stops the proxy");
    // Real children are forking here; a `fork()` in flight holds every parent fd until its
    // `exec`, this listener's included, so the kernel may still complete a connect into the
    // backlog of a listener the proxy has closed. Bounded: the child execs in milliseconds.
    let refused = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if tokio::net::TcpStream::connect(network.address)
                .await
                .is_err()
            {
                return true;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await;
    assert_eq!(
        Ok(true),
        refused,
        "the listener is closed on {}",
        network.address
    );
}

/// The session owner, scripted through the sandbox's own card transport (no hub on the VM).
struct ScriptedOwner {
    replies: parking_lot::Mutex<std::collections::VecDeque<Value>>,
    seen: parking_lot::Mutex<Vec<Value>>,
}

impl ScriptedOwner {
    /// An owner with no answers: any card posted to it is recorded and fails the settle.
    fn silent() -> ScriptedOwner {
        ScriptedOwner {
            replies: parking_lot::Mutex::new(std::collections::VecDeque::new()),
            seen: parking_lot::Mutex::new(Vec::new()),
        }
    }
}

#[async_trait::async_trait]
impl crate::permission::PermissionHookTransport for ScriptedOwner {
    async fn request_permission(&self, payload: Value) -> Result<Value, String> {
        self.seen.lock().push(payload);
        self.replies
            .lock()
            .pop_front()
            .ok_or_else(|| "no scripted answer left".to_owned())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_card_answered_over_the_installed_transport_replays_the_real_command_once() {
    let fx = Fixture::new("enforce");
    let kernel = StubKernel::new();
    let sandbox = fx
        .sandbox(BackendSource::Fixed(Some(Box::new(KernelHandle(
            kernel.clone(),
        )))))
        .await;
    let owner = Arc::new(ScriptedOwner {
        replies: parking_lot::Mutex::new(std::collections::VecDeque::from([json!({
            "outcome": "approve",
            "scope": { "kind": "fs_write_root" },
            "duration": { "kind": "call" },
        })])),
        seen: parking_lot::Mutex::new(Vec::new()),
    });
    sandbox.set_card_transport(owner.clone());
    let handle = fx.handle(sandbox.clone());
    // The first target is granted for the call; the replay gets past it and the kernel refuses
    // the second, which the widened policy still excludes: one card, two real spawns, and the
    // second violation is final ("already run again", never a third run or a second card)
    let command = format!("touch {GRANTABLE_TARGET} {REFUSED_TARGET}");
    let denied = run_bash(&handle, &command).await;
    eprintln!("[enforce/replayed] {denied:#?}");
    let payloads = owner.seen.lock().clone();
    assert_eq!(1, payloads.len(), "one card per call: {payloads:?}");
    let payload = payloads.first().expect("one card");
    eprintln!("[enforce/replayed payload] {payload}");
    assert_eq!(Some(&json!("sandbox_violation")), payload.get("kind"));
    assert_eq!(Some(&json!(command)), payload.get("bash_command"));
    assert_eq!(Some(&json!("rerun")), payload.get("replay"));
    assert_eq!(
        Some(&json!("/srv/grok-w0-real/scratch")),
        payload.pointer("/proposed_grant/subject/root"),
        "the card offers the highest missing ancestor: {payload}"
    );
    assert!(
        payload
            .get("offered_scopes")
            .and_then(Value::as_array)
            .is_some_and(|scopes| scopes.contains(&json!("call"))),
        "{payload}"
    );
    assert_eq!(
        2,
        kernel.refused.load(Ordering::SeqCst),
        "the command reached the OS twice: the run and its one replay"
    );
    let value = denied.outcome.clone().expect("a kept denial is a result");
    assert_eq!(1, exit_code(&value));
    assert!(
        denied
            .model_text
            .contains(&format!("write to {REFUSED_TARGET}")),
        "the replay's denial names the second target: {denied:?}"
    );
    assert!(
        denied.model_text.contains(
            "already run again under the user's grant for writes under /srv/grok-w0-real/scratch"
        ),
        "{denied:?}"
    );
    assert!(
        !denied
            .model_text
            .contains("no channel to the session owner")
    );
    assert_eq!(0, sandbox.open_calls());
    assert!(
        sandbox.live_grants().await.is_empty(),
        "a call-scoped grant never reaches the store"
    );
}
