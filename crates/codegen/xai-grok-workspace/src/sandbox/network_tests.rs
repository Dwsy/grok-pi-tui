//! The sandbox with its own egress proxy: the credential `prepare` hands the command, the proxy
//! mapping it back to the call, `finish` revoking it (a background start keeping it for the
//! process it left running), grant rows and the folder's mode reaching
//! the decider, observe decisions landing in the summary, and a held connection settled by the
//! call's bound owner. A resolver that always fails keeps every "allowed" request off the network.

use std::ffi::OsString;
use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use xai_grok_egress_proxy::{
    BlockedRequest, Decider as _, DeciderOutcome, DenySource, EgressProxyOptions, Resolver, WouldBe,
};
use xai_grok_sandbox::command::backend::{
    BackendCapabilities, CallId, CommandTag, OriginalArgv, RenderedPolicy, SandboxBackend,
    SandboxCommandError, WrapReceipt,
};
use xai_grok_sandbox::command::grants::{
    Expiry, FixedClock, Grant, GrantDecision, GrantId, GrantScope, GrantSubject, HostPattern,
};
use xai_grok_sandbox::command::policy::NetworkPolicy;
use xai_grok_sandbox::command::violation::CommandExit;
use xai_grok_sandbox::command::{BackendName, GitConfigEnv, SandboxMode, SandboxPolicy};
use xai_grok_tools::sandbox_launch::SandboxLaunch;
use xai_grok_tools::types::output::{BackgroundTaskStarted, BashOutput, ToolOutput, ToolRunResult};
use xai_tool_runtime::{ToolApprovalPolicy, ToolStream, ToolStreamItem};

use super::network::{NetworkViolationSink, SandboxGrantView};
use super::result_path::run_shell_call_with_replay;
use super::{
    BackendSource, CallOwner, MAX_OPEN_CALLS, NetworkStartError, WorkspaceSandbox,
    WorkspaceSandboxConfig, WorkspaceSandboxError,
};
use crate::handle::WorkspaceHandle;
use crate::permission::{
    HoldAnswer, PermissionHookTransport, SandboxNetworkDecider, SandboxNetworkDeciderConfig,
    ToolApprovalGate, WebFetchDomains,
};

/// Wraps without touching the command, so the environment `prepare` set is what the test reads.
struct Passthrough;

impl SandboxBackend for Passthrough {
    fn name(&self) -> BackendName {
        BackendName::Seatbelt
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities::default()
    }

    fn wrap(
        &self,
        _cmd: &mut tokio::process::Command,
        original: &OriginalArgv,
        _policy: &SandboxPolicy,
        _tag: &CommandTag,
    ) -> Result<WrapReceipt, SandboxCommandError> {
        Ok(WrapReceipt {
            backend: BackendName::Seatbelt,
            rendered: RenderedPolicy::Sbpl {
                profile: "(version 1)".to_owned(),
                params: vec![(
                    "PROGRAM".to_owned(),
                    original.program.to_string_lossy().into_owned(),
                )],
            },
        })
    }
}

/// No request ever leaves the test: an allowed origin fails upstream with 502.
struct NoUpstream;

#[async_trait]
impl Resolver for NoUpstream {
    async fn resolve(&self, _hostname: &str, _port: u16) -> io::Result<Vec<SocketAddr>> {
        Err(io::Error::other("test resolver never resolves"))
    }
}

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    grok_home: PathBuf,
    clock: Arc<FixedClock>,
}

const NOW: i64 = 1_700_000_000;

impl Fixture {
    fn new(mode: &str) -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("ws");
        let grok_home = tmp.path().join("grok-home");
        std::fs::create_dir_all(root.join(".grok")).unwrap();
        std::fs::create_dir_all(&grok_home).unwrap();
        let fx = Fixture {
            _tmp: tmp,
            root,
            grok_home,
            clock: Arc::new(FixedClock::at(NOW)),
        };
        fx.set_mode(mode);
        fx
    }

    /// The user's `<grok_home>/workspaced.toml` layer, so `off` lands too (the trusted workspace's
    /// file may only tighten).
    /// Written as every real writer writes it — atomically, so a new inode each time.
    fn set_mode(&self, mode: &str) {
        xai_grok_config::fs_atomic::write_atomically(
            &crate::sandbox_mode::user_config_path(&self.grok_home),
            &format!("[sandbox]\nmode = \"{mode}\"\n"),
            None,
        )
        .unwrap();
    }

    /// The folder's real `permission.toml`, the one the hub gate keys for a session bound at
    /// the root.
    fn permission_toml(&self) -> PathBuf {
        xai_grok_config::sessions_cwd_dir_in(&self.grok_home, self.root.to_str().unwrap())
            .join("permission.toml")
    }

    /// [`Self::permission_toml`] written; the decider re-reads it when it changes.
    fn write_permission_toml(&self, contents: &str) {
        let file = self.permission_toml();
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, contents).unwrap();
    }

    fn config(&self) -> WorkspaceSandboxConfig {
        WorkspaceSandboxConfig {
            workspace_root: self.root.clone(),
            grok_home: self.grok_home.clone(),
            user_home: Some(self._tmp.path().join("home")),
            git_env: GitConfigEnv::default(),
            control_socket_dir: self.grok_home.join("daemon"),
            remote: None,
            backend: BackendSource::Fixed(Some(Box::new(Passthrough))),
            clock: self.clock.clone(),
        }
    }

    async fn open(&self) -> Arc<WorkspaceSandbox> {
        Arc::new(WorkspaceSandbox::open(self.config()).await)
    }

    /// A sandbox with its proxy started on a loopback port.
    async fn sandbox(&self) -> Arc<WorkspaceSandbox> {
        self.sandbox_with(Box::new(Passthrough)).await
    }

    /// [`Self::sandbox`] with `backend` wrapping the commands.
    async fn sandbox_with(&self, backend: Box<dyn SandboxBackend>) -> Arc<WorkspaceSandbox> {
        let config = WorkspaceSandboxConfig {
            backend: BackendSource::Fixed(Some(backend)),
            ..self.config()
        };
        let sandbox = Arc::new(WorkspaceSandbox::open(config).await);
        sandbox
            .start_network(self.proxy_options())
            .await
            .expect("the proxy binds a loopback port");
        sandbox
    }

    fn proxy_options(&self) -> EgressProxyOptions {
        EgressProxyOptions {
            resolver: Arc::new(NoUpstream),
            hold_timeout: Duration::from_secs(5),
            clock: self.clock.clone(),
            ..EgressProxyOptions::default()
        }
    }

    fn command(&self) -> (tokio::process::Command, OriginalArgv) {
        let mut cmd = tokio::process::Command::new("/bin/sh");
        cmd.arg("-c").arg("true").current_dir(&self.root);
        (
            cmd,
            OriginalArgv {
                program: PathBuf::from("/bin/sh"),
                args: vec![OsString::from("-c"), OsString::from("true")],
                cwd: self.root.clone(),
            },
        )
    }
}

fn env_of(cmd: &tokio::process::Command, name: &str) -> Option<String> {
    cmd.as_std()
        .get_envs()
        .find(|(key, _)| *key == OsString::from(name).as_os_str())
        .and_then(|(_, value)| value.map(|v| v.to_string_lossy().into_owned()))
}

/// `http://grok:<token>@127.0.0.1:<port>` → (`<token>`, the address).
fn credential_of(url: &str) -> (String, SocketAddr) {
    let rest = url.strip_prefix("http://grok:").expect("proxy URL shape");
    let (token, address) = rest.split_once('@').expect("userinfo");
    (token.to_owned(), address.parse().expect("socket address"))
}

/// The status line of one CONNECT sent over `stream` with the given `Proxy-Authorization` value.
async fn connect_over<S: AsyncReadExt + AsyncWriteExt + Unpin>(
    mut stream: S,
    authorization: &str,
    target: &str,
) -> u16 {
    let request = format!(
        "CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Authorization: {authorization}\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    let mut chunk = [0u8; 1024];
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let read = tokio::time::timeout_at(deadline, stream.read(&mut chunk))
            .await
            .expect("the proxy answers")
            .unwrap();
        if read == 0 {
            break;
        }
        response.extend_from_slice(chunk.get(..read).unwrap_or_default());
        if response.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    let line = String::from_utf8_lossy(&response);
    line.split_whitespace()
        .nth(1)
        .and_then(|status| status.parse().ok())
        .unwrap_or_else(|| panic!("no status line in {line:?}"))
}

/// One CONNECT over TCP; returns the status code the proxy answered with.
async fn connect_via(address: SocketAddr, authorization: &str, target: &str) -> u16 {
    let stream = tokio::net::TcpStream::connect(address).await.unwrap();
    connect_over(stream, authorization, target).await
}

/// Whether a proxy of ours still answers on `address`: a CONNECT with the session token gets a
/// status line back. A refused connect is `false`; so is a connect that is accepted but never
/// answered — a `fork()` in flight elsewhere in the test binary holds every parent fd, this
/// listener's included, until its `exec`, so the kernel may still complete a connect into the
/// backlog of a listener the proxy has closed. Bounded by the read deadline.
async fn proxy_answers_at(address: SocketAddr, authorization: &str) -> bool {
    let Ok(mut stream) = tokio::net::TcpStream::connect(address).await else {
        return false;
    };
    let request = format!(
        "CONNECT nobody.example:443 HTTP/1.1\r\nHost: nobody.example:443\r\nProxy-Authorization: {authorization}\r\n\r\n"
    );
    if stream.write_all(request.as_bytes()).await.is_err() {
        return false;
    }
    let mut chunk = [0u8; 64];
    match tokio::time::timeout(Duration::from_secs(2), stream.read(&mut chunk)).await {
        Ok(Ok(read)) => chunk
            .get(..read)
            .is_some_and(|head| head.starts_with(b"HTTP/")),
        _ => false,
    }
}

async fn next_decision(
    requests: &mut tokio::sync::broadcast::Receiver<BlockedRequest>,
) -> BlockedRequest {
    tokio::time::timeout(Duration::from_secs(5), requests.recv())
        .await
        .expect("a decision is published")
        .expect("the channel is open")
}

fn net_grant(host: &str, decision: GrantDecision) -> Grant {
    Grant {
        id: GrantId::new(format!("g-{host}-{decision:?}")),
        subject: GrantSubject::NetHost {
            host: HostPattern::new(host),
            port: None,
        },
        scope: GrantScope::Workspace {
            root: PathBuf::new(),
        },
        expires: Expiry::Never,
        decision,
        granted_at: NOW,
        granted_by: "hub:test".to_owned(),
        via: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prepare_hands_the_command_a_credential_the_proxy_maps_to_the_call_until_finish() {
    let fx = Fixture::new("enforce");
    let sandbox = fx.sandbox().await;
    let info = sandbox.network().expect("started");
    let status = sandbox.status_json();
    assert_eq!(Some(&serde_json::json!("proxy")), status.get("network"));
    assert_eq!(
        Some(&serde_json::json!(info.address.port())),
        status.pointer("/proxy/port")
    );
    assert_eq!(
        Some(info.address.port()),
        sandbox.proxy().map(|proxy| proxy.port),
        "the policy renders the folder's own proxy"
    );
    let mut decisions = sandbox.blocked_requests().unwrap();

    let (mut cmd, original) = fx.command();
    sandbox
        .prepare(&mut cmd, &original, &CallId::tool("call-1"))
        .unwrap();
    let upper = env_of(&cmd, "HTTP_PROXY").expect("HTTP_PROXY set");
    assert_eq!(Some(upper.clone()), env_of(&cmd, "http_proxy"));
    assert_eq!(Some(upper.clone()), env_of(&cmd, "HTTPS_PROXY"));
    assert_eq!(Some(upper.clone()), env_of(&cmd, "ALL_PROXY"));
    assert_eq!(
        Some("localhost,127.0.0.1,::1".to_owned()),
        env_of(&cmd, "no_proxy")
    );
    let (token, address) = credential_of(&upper);
    assert_eq!(info.address, address);

    // No session owner is bound here: the hold fails closed, attributed to the call
    let status = connect_via(address, &format!("Bearer {token}"), "held.example:443").await;
    assert_eq!(403, status);
    let decision = next_decision(&mut decisions).await;
    assert_eq!("held.example", decision.host);
    assert_eq!(443, decision.port);
    assert_eq!(
        Some(CommandTag::for_call(&CallId::tool("call-1"))),
        decision.call
    );
    assert_eq!(DeciderOutcome::Denied, decision.decided);

    // finish revokes: the same token is a 407 before any decision
    assert!(
        sandbox
            .finish(&CallId::tool("call-1"), CommandExit::code(0), b"")
            .await
            .violation()
            .is_none()
    );
    let status = connect_via(address, &format!("Bearer {token}"), "held.example:443").await;
    assert_eq!(407, status);
    assert!(
        tokio::time::timeout(Duration::from_millis(300), decisions.recv())
            .await
            .is_err(),
        "a 407 is not a decision"
    );

    // The replay mints afresh: a different token, and the old one stays dead
    let (mut cmd, original) = fx.command();
    sandbox
        .prepare(&mut cmd, &original, &CallId::tool("call-1"))
        .unwrap();
    let (fresh, _) = credential_of(&env_of(&cmd, "HTTP_PROXY").unwrap());
    assert_ne!(token, fresh);
    assert_eq!(
        403,
        connect_via(address, &format!("Bearer {fresh}"), "held.example:443").await
    );
    sandbox.release_call(&CallId::tool("call-1"));
    assert_eq!(
        407,
        connect_via(address, &format!("Bearer {fresh}"), "held.example:443").await
    );
}

/// Through the folder's real proxy: under `enforce` a revoked credential and an empty one are
/// refused with `407` before any decision; once the folder is under `observe` the same revoked
/// credential is decided as the session's (observe never blocks), not refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enforce_answers_407_to_a_revoked_or_missing_credential_and_observe_does_not() {
    let fx = Fixture::new("enforce");
    let sandbox = fx.sandbox().await;
    let address = sandbox.network().unwrap().address;
    let call = CallId::tool("call-ended");
    let (mut cmd, original) = fx.command();
    sandbox.prepare(&mut cmd, &original, &call).unwrap();
    let (token, _) = credential_of(&env_of(&cmd, "HTTP_PROXY").unwrap());
    sandbox.release_call(&call);
    let revoked = format!("Bearer {token}");
    assert_eq!(
        407,
        connect_via(address, &revoked, "held.example:443").await
    );
    assert_eq!(407, connect_via(address, "", "held.example:443").await);

    fx.set_mode("observe");
    assert_eq!(SandboxMode::Observe, sandbox.mode());
    assert_ne!(
        407,
        connect_via(address, &revoked, "held.example:443").await,
        "observe decides the request instead of refusing it"
    );
    sandbox.stop_network().await;
}

/// The hub's dispatch for a shell call whose one terminal is `output`.
fn dispatching(output: ToolOutput) -> impl FnMut() -> ToolStream<ToolRunResult> + Send + 'static {
    move || {
        let output = output.clone();
        Box::pin(async_stream::stream! {
            yield ToolStreamItem::Terminal(Ok(ToolRunResult {
                output,
                prompt_text: String::new(),
                effective_tool_name: None,
            }));
        })
    }
}

/// `prepare` for `call` as the spawn site does it; the token in the command's `HTTP_PROXY`.
fn prepared_token(fx: &Fixture, sandbox: &WorkspaceSandbox, call: &str) -> (String, SocketAddr) {
    let (mut cmd, original) = fx.command();
    sandbox
        .prepare(&mut cmd, &original, &CallId::tool(call))
        .unwrap();
    credential_of(&env_of(&cmd, "HTTP_PROXY").expect("HTTP_PROXY set"))
}

/// A background start ends the call's stream with its command still running. Its owner goes,
/// but the process keeps its record and the credential in its `HTTP_PROXY` stays live, so a
/// host the folder allows still answers it (502: allowed, no upstream) and a stranger is
/// refused, not 407. A foreground result still revokes at finish
/// without taking a background credential with it; the proxy stopping takes them all.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_background_starts_credential_outlives_its_stream_until_the_proxy_stops() {
    let fx = Fixture::new("enforce");
    let sandbox = fx.sandbox().await;
    sandbox
        .record_grant(
            &CallId::tool("c-grant"),
            None,
            "sess",
            Grant {
                scope: GrantScope::Workspace {
                    root: fx.root.clone(),
                },
                ..net_grant("allowed.example", GrantDecision::Allow)
            },
        )
        .await
        .unwrap();
    let handle =
        WorkspaceHandle::for_test_in_with_sandbox(&fx.root, sandbox.clone(), ToolApprovalGate::Off);
    let session = handle.create_session("main").unwrap();

    let (background, address) = prepared_token(&fx, &sandbox, "bg");
    let items: Vec<_> = run_shell_call_with_replay(
        handle.clone(),
        session.clone(),
        "bg".to_owned(),
        None,
        false,
        dispatching(ToolOutput::BackgroundTaskStarted(BackgroundTaskStarted {
            task_id: "t1".to_owned(),
            task_type: "bash".to_owned(),
            output_file: "/tmp/out".to_owned(),
            status: "running".to_owned(),
            command: "python -m http.server &".to_owned(),
            summary: "running".to_owned(),
            retrieval_hint: String::new(),
            pre_formatted: None,
            pid: None,
        })),
    )
    .collect()
    .await;
    assert!(
        matches!(items.as_slice(), [ToolStreamItem::Terminal(Ok(_))]),
        "{items:?}"
    );
    assert!(sandbox.calls.lock().owner_of(&CallId::tool("bg")).is_none());
    assert_eq!(
        1,
        sandbox.open_calls(),
        "the running process keeps its record"
    );
    let bearer = format!("Bearer {background}");
    assert_eq!(
        502,
        connect_via(address, &bearer, "allowed.example:443").await,
        "the running process still reaches the allowed host"
    );
    assert_eq!(
        403,
        connect_via(address, &bearer, "nobody.example:443").await,
        "still the call's credential: refused, not unauthenticated"
    );

    let (foreground, _) = prepared_token(&fx, &sandbox, "fg");
    let items: Vec<_> = run_shell_call_with_replay(
        handle.clone(),
        session,
        "fg".to_owned(),
        None,
        false,
        dispatching(ToolOutput::Bash(BashOutput {
            output: Vec::new(),
            output_for_prompt: String::new(),
            exit_code: 0,
            command: "true".to_owned(),
            truncated: false,
            signal: None,
            timed_out: false,
            description: None,
            current_dir: fx.root.to_string_lossy().into_owned(),
            output_file: String::new(),
            total_bytes: 0,
            output_delta: None,
            was_bare_echo: false,
        })),
    )
    .collect()
    .await;
    assert!(
        matches!(items.as_slice(), [ToolStreamItem::Terminal(Ok(_))]),
        "{items:?}"
    );
    assert_eq!(
        407,
        connect_via(
            address,
            &format!("Bearer {foreground}"),
            "allowed.example:443"
        )
        .await,
        "a finished foreground call's credential is revoked"
    );
    assert_eq!(
        502,
        connect_via(address, &bearer, "allowed.example:443").await,
        "the foreground finish left the background credential alone"
    );

    sandbox.stop_network().await;
    assert!(
        !proxy_answers_at(address, &bearer).await,
        "the credential goes with the proxy"
    );
}

/// A background start's result names the connections refused before its stream ended: the
/// call detaches, but its refusal notes reach the model first.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_background_start_reports_the_refusals_met_before_it_detached() {
    let fx = Fixture::new("enforce");
    let sandbox = fx.sandbox().await;
    let handle =
        WorkspaceHandle::for_test_in_with_sandbox(&fx.root, sandbox.clone(), ToolApprovalGate::Off);
    let session = handle.create_session("main").unwrap();
    let call = CallId::tool("bg");
    prepared_token(&fx, &sandbox, "bg");
    sandbox
        .calls
        .lock()
        .push_denial(&call, "refused nobody.example".to_owned());

    let items: Vec<_> = run_shell_call_with_replay(
        handle,
        session,
        "bg".to_owned(),
        None,
        false,
        dispatching(ToolOutput::BackgroundTaskStarted(BackgroundTaskStarted {
            task_id: "t1".to_owned(),
            task_type: "bash".to_owned(),
            output_file: "/tmp/out".to_owned(),
            status: "running".to_owned(),
            command: "curl https://nobody.example/ &".to_owned(),
            summary: "running".to_owned(),
            retrieval_hint: String::new(),
            pre_formatted: None,
            pid: None,
        })),
    )
    .collect()
    .await;
    let [ToolStreamItem::Terminal(Ok(result))] = items.as_slice() else {
        panic!("{items:?}");
    };
    assert!(
        result
            .prompt_text
            .ends_with("\n\n[sandbox] refused nobody.example"),
        "{:?}",
        result.prompt_text
    );
    assert!(sandbox.calls.lock().owner_of(&call).is_none());
    sandbox.stop_network().await;
}

/// The hub's owner for a call, with no channel to raise a card on.
fn cardless_owner() -> CallOwner {
    CallOwner {
        session_id: "sess-1".to_owned(),
        policy: ToolApprovalPolicy::GrantsAllowed,
        transport: None,
        command: None,
    }
}

/// A full call table never makes room by dropping a call that may still run: a background child
/// keeps its entry, its credential and its mode, and a new command is refused with the
/// credential minted for it revoked. After a flip to `observe` both children — the one the hub
/// pinned and the one whose spawn fixed `enforce` on its own — are still decided under
/// `enforce`, the mode they started under; an exit makes the room.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_full_table_keeps_every_live_background_call_and_refuses_the_next_command() {
    let fx = Fixture::new("enforce");
    let sandbox = fx.sandbox().await;
    let pinned = CallId::tool("bg-pinned");
    sandbox.pin_mode(&pinned, SandboxMode::Enforce).unwrap();
    assert!(sandbox.bind_pinned_call(&pinned, cardless_owner()));
    let (pinned_token, address) = prepared_token(&fx, &sandbox, "bg-pinned");
    sandbox.detach_call(&pinned, "sess-1");
    let unpinned = CallId::tool("bg-unpinned");
    sandbox.bind_call(&unpinned, cardless_owner()).unwrap();
    let (unpinned_token, _) = prepared_token(&fx, &sandbox, "bg-unpinned");
    sandbox.detach_call(&unpinned, "sess-1");
    for i in 2..MAX_OPEN_CALLS {
        sandbox
            .bind_call(&CallId::tool(format!("owned-{i}")), cardless_owner())
            .unwrap();
    }
    assert_eq!(MAX_OPEN_CALLS, sandbox.calls.lock().len());

    let newcomer = CallId::tool("newcomer");
    let (mut cmd, original) = fx.command();
    let refused = sandbox.prepare(&mut cmd, &original, &newcomer).unwrap_err();
    assert!(
        matches!(
            refused.downcast_ref::<WorkspaceSandboxError>(),
            Some(WorkspaceSandboxError::CallTableFull)
        ),
        "{refused}"
    );
    let (newcomer_token, _) = credential_of(&env_of(&cmd, "HTTP_PROXY").expect("HTTP_PROXY set"));
    assert_eq!(
        407,
        stranger_status(address, &newcomer_token).await,
        "the refused command's credential went with it"
    );
    assert_eq!(
        MAX_OPEN_CALLS,
        sandbox.calls.lock().len(),
        "nothing was evicted"
    );
    for token in [&pinned_token, &unpinned_token] {
        assert_eq!(
            403,
            stranger_status(address, token).await,
            "kept: refused, not unauthenticated"
        );
    }
    for call in [&pinned, &unpinned] {
        assert_eq!(
            Some(SandboxMode::Enforce),
            sandbox.calls.lock().held_mode(call)
        );
    }

    fx.set_mode("observe");
    sandbox
        .sync_network()
        .await
        .expect("the proxy keeps running");
    assert_eq!(SandboxMode::Observe, sandbox.mode());
    for token in [&pinned_token, &unpinned_token] {
        assert_eq!(
            403,
            stranger_status(address, token).await,
            "both children are still decided under enforce, the mode they started under"
        );
    }
    for call in [&pinned, &unpinned] {
        assert_eq!(
            Some(SandboxMode::Enforce),
            sandbox.calls.lock().held_mode(call)
        );
    }

    // back under `enforce` a revoked token is refused outright rather than admitted as the session's
    fx.set_mode("enforce");
    sandbox
        .sync_network()
        .await
        .expect("the proxy keeps running");
    SandboxLaunch::exited(sandbox.as_ref(), &pinned);
    assert_eq!(407, stranger_status(address, &pinned_token).await);
    assert!(sandbox.calls.lock().held_mode(&pinned).is_none());
    let (mut cmd, original) = fx.command();
    sandbox.prepare(&mut cmd, &original, &newcomer).unwrap();
    assert_eq!(MAX_OPEN_CALLS, sandbox.calls.lock().len());
    sandbox.stop_network().await;
}

/// An entry dropped to make room is released first: the credential of a call nothing of which
/// runs is revoked before its row goes, so no later request authenticates as it. White-box: the
/// stray's spawn record is taken without `finish`, which would have revoked it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_entry_dropped_to_make_room_has_its_credential_revoked_first() {
    let fx = Fixture::new("enforce");
    let sandbox = fx.sandbox().await;
    let stray = CallId::tool("stray");
    let (token, address) = prepared_token(&fx, &sandbox, "stray");
    let (spawn, _left_live) = sandbox.calls.lock().take_prepared(&stray);
    assert!(spawn.is_some());
    assert_eq!(403, stranger_status(address, &token).await);
    for i in 1..MAX_OPEN_CALLS {
        sandbox
            .bind_call(&CallId::tool(format!("owned-{i}")), cardless_owner())
            .unwrap();
    }
    assert_eq!(MAX_OPEN_CALLS, sandbox.calls.lock().len());

    let one_more = CallId::tool("one-more");
    sandbox.bind_call(&one_more, cardless_owner()).unwrap();
    assert_eq!(
        MAX_OPEN_CALLS,
        sandbox.calls.lock().len(),
        "the stray's row made the room"
    );
    assert!(sandbox.calls.lock().owner_of(&one_more).is_some());
    assert_eq!(
        407,
        stranger_status(address, &token).await,
        "its credential went before its row"
    );
    sandbox.stop_network().await;
}

/// A call dropped at its final result takes its credential with it: a background start whose
/// child already exited has nothing to keep one for, and a result that unbinds a call whose spawn
/// record never came back revokes the credential that record was minted with.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_call_dropped_at_its_final_result_takes_its_credential_with_it() {
    let fx = Fixture::new("enforce");
    let sandbox = fx.sandbox().await;
    let early = CallId::tool("bg-early");
    sandbox.bind_call(&early, cardless_owner()).unwrap();
    let (early_token, address) = prepared_token(&fx, &sandbox, "bg-early");
    SandboxLaunch::exited(sandbox.as_ref(), &early);
    sandbox.detach_call(&early, "sess-1");
    assert_eq!(
        407,
        stranger_status(address, &early_token).await,
        "nothing is kept for a child that already exited"
    );
    assert!(sandbox.calls.lock().session_of(&early).is_none());

    let unbound = CallId::tool("unbound");
    sandbox.bind_call(&unbound, cardless_owner()).unwrap();
    let (unbound_token, _) = prepared_token(&fx, &sandbox, "unbound");
    sandbox.release_call(&unbound);
    assert_eq!(
        407,
        stranger_status(address, &unbound_token).await,
        "the open spawn record went with its credential"
    );
    assert_eq!(0, sandbox.calls.lock().len());
    sandbox.stop_network().await;
}

/// `release_call` is the one way a call ends, and it revokes the call's credential whether or not
/// the table still holds the call. White-box: a stray's spawn record is taken without `finish` and
/// its entry goes empty, leaving a live credential with no entry behind it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn release_call_revokes_the_credential_of_a_call_the_table_no_longer_holds() {
    let fx = Fixture::new("enforce");
    let sandbox = fx.sandbox().await;
    let stray = CallId::tool("stray");
    let (token, address) = prepared_token(&fx, &sandbox, "stray");
    {
        let mut calls = sandbox.calls.lock();
        let (spawn, _left_live) = calls.take_prepared(&stray);
        assert!(spawn.is_some());
        calls.close_settlement(&stray);
    }
    assert_eq!(
        0,
        sandbox.calls.lock().len(),
        "the table holds nothing of it"
    );
    assert_eq!(
        403,
        stranger_status(address, &token).await,
        "its credential still authenticates"
    );

    sandbox.release_call(&stray);
    assert_eq!(
        407,
        stranger_status(address, &token).await,
        "released with the call"
    );
    sandbox.stop_network().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grant_rows_and_the_folder_mode_reach_the_decider() {
    let fx = Fixture::new("enforce");
    let sandbox = fx.sandbox().await;
    let address = sandbox.network().unwrap().address;
    let session = sandbox.session_proxy_authorization().unwrap();
    let mut decisions = sandbox.blocked_requests().unwrap();

    // The session-wide token carries no call: an unknown host is refused without a card
    assert_eq!(
        403,
        connect_via(address, &session, "nobody.example:443").await
    );
    let refused = next_decision(&mut decisions).await;
    assert_eq!(
        (None, DeciderOutcome::Denied),
        (refused.call, refused.decided)
    );

    // An allow row recorded through the sandbox is live for the proxy at once (502: allowed, no upstream)
    sandbox
        .record_grant(
            &CallId::tool("c-grant"),
            None,
            "sess",
            Grant {
                scope: GrantScope::Workspace {
                    root: fx.root.clone(),
                },
                ..net_grant("allowed.example", GrantDecision::Allow)
            },
        )
        .await
        .unwrap();
    assert_eq!(
        502,
        connect_via(address, &session, "allowed.example:443").await
    );

    // A deny row wins and is a decision
    let denied_id = sandbox
        .record_grant(
            &CallId::tool("c-deny"),
            None,
            "sess",
            Grant {
                scope: GrantScope::Workspace {
                    root: fx.root.clone(),
                },
                ..net_grant("denied.example", GrantDecision::Deny)
            },
        )
        .await
        .unwrap();
    assert_eq!(
        403,
        connect_via(address, &session, "denied.example:443").await
    );
    assert_eq!(
        DeciderOutcome::Denied,
        next_decision(&mut decisions).await.decided
    );

    // Revoking the deny row puts the host back to "ask", which the session token cannot
    sandbox.revoke_grant(&denied_id).await.unwrap();
    assert_eq!(
        403,
        connect_via(address, &session, "denied.example:443").await
    );
    assert_eq!(
        DeciderOutcome::Denied,
        next_decision(&mut decisions).await.decided
    );

    // sandbox.mode.set → observe: the decider reads the folder's mode at its next decision and
    // now observes. Observe never blocks: the unknown host, a deny-row host and an organisation-list
    // host all go through (502: allowed, no upstream); the summary shows enforce's verdict for each.
    fx.set_mode("observe");
    assert_eq!(SandboxMode::Observe, sandbox.mode());
    fx.write_permission_toml("disallowed_web_fetch_domains = [\"tracker.example\"]\n");
    sandbox
        .record_grant(
            &CallId::tool("c-deny-again"),
            None,
            "sess",
            Grant {
                scope: GrantScope::Workspace {
                    root: fx.root.clone(),
                },
                ..net_grant("denied.example", GrantDecision::Deny)
            },
        )
        .await
        .unwrap();
    for host in [
        "nobody.example:443",
        "denied.example:443",
        "tracker.example:443",
    ] {
        assert_eq!(502, connect_via(address, &session, host).await, "{host}");
    }
    let observed: Vec<DeciderOutcome> = [
        next_decision(&mut decisions).await,
        next_decision(&mut decisions).await,
        next_decision(&mut decisions).await,
    ]
    .into_iter()
    .map(|decision| decision.decided)
    .collect();
    assert_eq!(
        vec![
            DeciderOutcome::Observed {
                would: WouldBe::Asked
            },
            DeciderOutcome::Observed {
                would: WouldBe::Denied(DenySource::DenyRow)
            },
            DeciderOutcome::Observed {
                would: WouldBe::Denied(DenySource::WebFetchDenylist)
            },
        ],
        observed
    );
    let summary = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let summary = sandbox.observe_summary();
            if summary.would_block.len() == 3 {
                return summary;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the observe feed records the decisions");
    let mut rows: Vec<(&str, &str, u64)> = summary
        .would_block
        .iter()
        .map(|row| (row.target.as_str(), <&str>::from(row.verdict), row.count))
        .collect();
    rows.sort_unstable();
    assert_eq!(
        vec![
            ("denied.example:443", "deny_row", 1),
            ("nobody.example:443", "ask", 1),
            ("tracker.example:443", "policy_denylist", 1),
        ],
        rows
    );
    assert!(
        summary
            .would_block
            .iter()
            .all(|row| <&str>::from(row.kind) == "net")
    );
}

/// Shell calls dispatched at once each sync the folder's network before spawning: they all get
/// the one proxy, none sees "already started", and no second listener is bound and dropped.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_syncs_share_one_proxy() {
    let fx = Fixture::new("enforce");
    let sandbox = fx.open().await;
    let mut syncs = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let sandbox = sandbox.clone();
        syncs.spawn(async move { sandbox.sync_network().await.map_err(|e| e.to_string()) });
    }
    let mut addresses = Vec::new();
    while let Some(result) = syncs.join_next().await {
        let info = result
            .unwrap()
            .expect("every sync succeeds")
            .expect("enforce runs a proxy");
        addresses.push(info.address);
    }
    let running = sandbox.network().expect("one proxy runs").address;
    assert!(
        addresses.iter().all(|address| *address == running),
        "{addresses:?} vs {running}"
    );
    sandbox.stop_network().await;
}

/// The daemon's serve step: nothing under `off`, a loopback port otherwise
/// (TCP only in v1), idempotent so the verb and every dispatch can call it again; the unserve
/// stops the listener.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sync_network_follows_the_mode_and_listens_on_loopback() {
    let fx = Fixture::new("off");
    let sandbox = fx.open().await;
    assert!(matches!(sandbox.sync_network().await, Ok(None)));
    assert!(sandbox.network().is_none(), "off starts no proxy");
    assert_eq!(
        Some(&serde_json::json!("off")),
        sandbox.status_json().get("network")
    );
    assert!(sandbox.proxy().is_none());

    fx.set_mode("enforce");
    let info = sandbox
        .sync_network()
        .await
        .expect("the proxy binds")
        .expect("enforce starts one");
    assert!(info.address.ip().is_loopback(), "{}", info.address);
    assert_eq!(
        Ok(Some(info.clone())),
        sandbox
            .sync_network()
            .await
            .map_err(|error| error.to_string()),
        "a second sync keeps the running proxy"
    );
    assert_eq!(
        Some(info.address.port()),
        sandbox.proxy().map(|proxy| proxy.port),
        "the policy renders the port the command is pointed at"
    );

    // A CONNECT with the session token and no call is refused (403)
    let session = sandbox.session_proxy_authorization().unwrap();
    assert_eq!(
        403,
        connect_via(info.address, &session, "nobody.example:443").await
    );

    // Unserve: the listener goes even though the sandbox lives on, and nothing starts it again
    // (a call dispatched during the teardown syncs too); a re-serve opens a fresh sandbox
    sandbox.stop_network().await;
    assert!(sandbox.network().is_none());
    assert!(sandbox.proxy().is_none());
    assert!(
        !proxy_answers_at(info.address, &session).await,
        "the stopped proxy still answers on {}",
        info.address
    );
    assert_eq!(
        Ok(None),
        sandbox
            .sync_network()
            .await
            .map_err(|error| error.to_string()),
        "an unserved folder's sync starts no proxy"
    );
    assert!(matches!(
        sandbox.start_network(fx.proxy_options()).await,
        Err(NetworkStartError::Closed)
    ));
    assert!(sandbox.network().is_none());
}

/// A proxy whose accept loop gave up is not kept: the next sync starts another, so the folder
/// is never pointed at a closed port.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_proxy_that_stopped_on_its_own_is_replaced_on_the_next_sync() {
    let fx = Fixture::new("enforce");
    let sandbox = fx.open().await;
    let first = sandbox
        .sync_network()
        .await
        .expect("the proxy binds")
        .expect("enforce starts one");
    sandbox.abort_proxy_for_test().await;
    let session = sandbox.session_proxy_authorization().unwrap();
    assert!(!proxy_answers_at(first.address, &session).await);
    let replaced = sandbox
        .sync_network()
        .await
        .expect("rebinds")
        .expect("enforce");
    let session = sandbox.session_proxy_authorization().unwrap();
    assert!(
        proxy_answers_at(replaced.address, &session).await,
        "the replacement answers on {}",
        replaced.address
    );
    sandbox.stop_network().await;
}

/// A mode flipped by editing `workspaced.toml` moves the proxy exactly as the verb does — `off`
/// stops the running listener, `observe` leaves it stopped and points no command at one (observe
/// watches the file system only), `enforce` starts one again — on the owner's next sync (the next
/// dispatched command); the mode itself is re-resolved from the file only when the file moved.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mode_flip_through_the_file_stops_and_starts_the_proxy_on_the_next_sync() {
    let fx = Fixture::new("enforce");
    let sandbox = fx.open().await;
    let first = sandbox
        .sync_network()
        .await
        .expect("binds")
        .expect("enforce starts one");
    let session = sandbox.session_proxy_authorization().unwrap();
    assert_eq!(SandboxMode::Enforce, sandbox.mode());
    assert!(proxy_answers_at(first.address, &session).await);

    fx.set_mode("off");
    assert_eq!(
        SandboxMode::Off,
        sandbox.mode(),
        "the file moved, so the mode is resolved again"
    );
    assert!(matches!(sandbox.sync_network().await, Ok(None)));
    assert!(sandbox.network().is_none(), "off stopped the proxy");
    assert!(
        !proxy_answers_at(first.address, &session).await,
        "the listener is closed"
    );
    assert_eq!(
        Some(&serde_json::json!("off")),
        sandbox.status_json().get("network")
    );

    fx.set_mode("observe");
    assert_eq!(SandboxMode::Observe, sandbox.mode());
    assert!(matches!(sandbox.sync_network().await, Ok(None)));
    assert!(sandbox.network().is_none(), "observe binds no listener");
    let (mut cmd, original) = fx.command();
    sandbox
        .prepare(&mut cmd, &original, &CallId::tool("observed"))
        .unwrap();
    assert!(
        cmd.as_std()
            .get_envs()
            .all(|(key, _)| !key.to_string_lossy().to_ascii_lowercase().contains("proxy")),
        "observe points the command at no proxy"
    );
    assert!(
        !proxy_answers_at(first.address, &session).await,
        "still nothing listens"
    );

    fx.set_mode("enforce");
    assert_eq!(SandboxMode::Enforce, sandbox.mode());
    let second = sandbox
        .sync_network()
        .await
        .expect("binds again")
        .expect("enforce starts one");
    assert!(second.address.ip().is_loopback());
    assert!(sandbox.network().is_some());
    assert_eq!(
        Some(second.address.port()),
        sandbox.proxy().map(|proxy| proxy.port)
    );
    sandbox.stop_network().await;
}

/// The unserve awaits the proxy's shutdown even while a reader still
/// holds the network side mid-use — the listener is closed when `stop_network` returns, not when
/// the last reader lets go.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_network_closes_the_listener_while_a_reader_holds_the_side() {
    let fx = Fixture::new("enforce");
    let sandbox = fx.sandbox().await;
    let info = sandbox.network().expect("started");
    let session = sandbox.session_proxy_authorization().unwrap();
    assert_eq!(
        403,
        connect_via(info.address, &session, "nobody.example:443").await
    );

    let held = sandbox.network.load_full().expect("the side is installed");
    sandbox.stop_network().await;
    assert!(
        !proxy_answers_at(info.address, &session).await,
        "the proxy still answers on {} while a reader holds the side",
        info.address
    );
    assert!(
        sandbox.session_proxy_authorization().is_none(),
        "a reader racing the stop gets no credential"
    );
    drop(held);
}

/// The web-fetch domain sets reach the decider from the folder's real `permission.toml`, re-read
/// when the file changes: an allow lands on the next decision and a disallow beats it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_folders_permission_toml_domains_reach_the_decider_when_the_file_changes() {
    let fx = Fixture::new("enforce");
    let sandbox = fx.sandbox().await;
    let address = sandbox.network().unwrap().address;
    let session = sandbox.session_proxy_authorization().unwrap();
    let mut decisions = sandbox.blocked_requests().unwrap();

    // Nothing remembered yet: the session token cannot ask, so the host is refused
    assert_eq!(
        403,
        connect_via(address, &session, "docs.example:443").await
    );
    assert_eq!(
        DeciderOutcome::Denied,
        next_decision(&mut decisions).await.decided
    );

    fx.write_permission_toml("allowed_web_fetch_domains = [\"docs.example\"]\n");
    assert_eq!(
        502,
        connect_via(address, &session, "docs.example:443").await
    );
    assert_eq!(
        502,
        connect_via(address, &session, "www.docs.example:443").await
    );

    fx.write_permission_toml(
        "allowed_web_fetch_domains = [\"docs.example\"]\ndisallowed_web_fetch_domains = [\"docs.example\"]\n",
    );
    assert_eq!(
        403,
        connect_via(address, &session, "docs.example:443").await
    );
    assert_eq!(
        DeciderOutcome::Denied,
        next_decision(&mut decisions).await.decided
    );

    // A command's plant: the file replaced by a link to one it wrote in the workspace, then the
    // session directory itself. The daemon's own file is read through no link: both allow no host.
    // Each phase's file has its own length: the store's change check is `(mtime, len)`
    let allow = "allowed_web_fetch_domains = [\"docs.example\"]\n";
    let file = fx.permission_toml();
    let dir = file.parent().unwrap().to_path_buf();
    let planted = fx.root.join("planted.toml");
    std::fs::write(&planted, format!("{allow}# planted file\n")).unwrap();
    std::fs::remove_file(&file).unwrap();
    std::os::unix::fs::symlink(&planted, &file).unwrap();
    assert_eq!(
        403,
        connect_via(address, &session, "docs.example:443").await,
        "a linked permission.toml allows no host"
    );
    let planted_dir = fx.root.join("planted-session");
    std::fs::create_dir_all(&planted_dir).unwrap();
    std::fs::write(
        planted_dir.join("permission.toml"),
        format!("{allow}# planted through a linked directory\n"),
    )
    .unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    std::os::unix::fs::symlink(&planted_dir, &dir).unwrap();
    assert_eq!(
        403,
        connect_via(address, &session, "docs.example:443").await,
        "a linked session directory allows no host"
    );
    std::fs::remove_file(&dir).unwrap();
    fx.write_permission_toml(allow);
    assert_eq!(
        502,
        connect_via(address, &session, "docs.example:443").await,
        "the daemon's own file again"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_start_is_refused_and_a_dropped_sandbox_takes_its_proxy_down() {
    let fx = Fixture::new("enforce");
    let sandbox = fx.sandbox().await;
    let address = sandbox.network().unwrap().address;
    assert!(matches!(
        sandbox.start_network(fx.proxy_options()).await,
        Err(NetworkStartError::AlreadyStarted)
    ));
    drop(sandbox);
    let refused = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if tokio::net::TcpStream::connect(address).await.is_err() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    assert_eq!(Ok(true), refused, "the listener closes with the sandbox");
}

/// The session owner, scripted: each card takes the next reply; every payload is kept.
struct ScriptedOwner {
    replies: parking_lot::Mutex<std::collections::VecDeque<serde_json::Value>>,
    seen: parking_lot::Mutex<Vec<serde_json::Value>>,
}

impl ScriptedOwner {
    fn new(replies: impl IntoIterator<Item = serde_json::Value>) -> Arc<ScriptedOwner> {
        Arc::new(ScriptedOwner {
            replies: parking_lot::Mutex::new(replies.into_iter().collect()),
            seen: parking_lot::Mutex::new(Vec::new()),
        })
    }
}

#[async_trait]
impl PermissionHookTransport for ScriptedOwner {
    async fn request_permission(
        &self,
        payload: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        self.seen.lock().push(payload);
        self.replies
            .lock()
            .pop_front()
            .ok_or_else(|| "no scripted answer left".to_owned())
    }
}

fn owner(
    session_id: &str,
    policy: ToolApprovalPolicy,
    transport: &Arc<ScriptedOwner>,
) -> CallOwner {
    CallOwner {
        session_id: session_id.to_owned(),
        policy,
        transport: Some(transport.clone() as Arc<dyn PermissionHookTransport>),
        command: None,
    }
}

/// A session owner who never answers; `waiting` is how many cards it is sitting on right now,
/// `withdrawn` the holds whose cards it was told are stale, in order. `slow_withdrawal` makes
/// being told take 200 ms, so a stop racing the withdrawal lands inside it.
#[derive(Default)]
struct SilentOwner {
    waiting: AtomicUsize,
    asked: tokio::sync::Notify,
    withdrawn: parking_lot::Mutex<Vec<String>>,
    slow_withdrawal: std::sync::atomic::AtomicBool,
}

struct Waiting<'a>(&'a AtomicUsize);

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[async_trait]
impl PermissionHookTransport for SilentOwner {
    async fn request_permission(
        &self,
        _payload: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        self.waiting.fetch_add(1, Ordering::SeqCst);
        let _waiting = Waiting(&self.waiting);
        self.asked.notify_one();
        std::future::pending().await
    }

    async fn withdraw_permission(&self, hold_id: &str) {
        if self.slow_withdrawal.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        self.withdrawn.lock().push(hold_id.to_owned());
    }
}

/// A session owner who answers every card with `reply`, each only once the test adds a permit;
/// `withdrawn` the holds whose cards it was told are stale, in order.
struct GatedOwner {
    reply: serde_json::Value,
    asked: AtomicUsize,
    card: tokio::sync::Notify,
    answers: tokio::sync::Semaphore,
    withdrawn: parking_lot::Mutex<Vec<String>>,
}

impl GatedOwner {
    fn new(reply: serde_json::Value) -> Arc<GatedOwner> {
        Arc::new(GatedOwner {
            reply,
            asked: AtomicUsize::new(0),
            card: tokio::sync::Notify::new(),
            answers: tokio::sync::Semaphore::new(0),
            withdrawn: parking_lot::Mutex::new(Vec::new()),
        })
    }
}

#[async_trait]
impl PermissionHookTransport for GatedOwner {
    async fn request_permission(
        &self,
        _payload: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        self.card.notify_one();
        self.answers
            .acquire()
            .await
            .map_err(|closed| closed.to_string())?
            .forget();
        Ok(self.reply.clone())
    }

    async fn withdraw_permission(&self, hold_id: &str) {
        self.withdrawn.lock().push(hold_id.to_owned());
    }
}

/// A hold a covering row released before its card's task ran raises no card: the owner is never
/// asked about a connection that is already answered (a later answer on such a stale card would
/// record a row nobody needs).
#[tokio::test]
async fn a_hold_released_before_its_card_runs_raises_no_card() {
    let fx = Fixture::new("enforce");
    let sandbox = fx.open().await;
    let transport = ScriptedOwner::new([]);
    let call = CallId::tool("call-stale");
    sandbox
        .bind_call(
            &call,
            owner("sess-1", ToolApprovalPolicy::GrantsAllowed, &transport),
        )
        .unwrap();
    // A spawn record: a hold is parked only for a call something still runs for
    let (mut cmd, original) = fx.command();
    sandbox.prepare(&mut cmd, &original, &call).unwrap();
    let sink = Arc::new(NetworkViolationSink {
        sandbox: Arc::downgrade(&sandbox),
        decider: std::sync::OnceLock::new(),
    });
    let decider = Arc::new(SandboxNetworkDecider::new(SandboxNetworkDeciderConfig {
        clock: fx.clock.clone(),
        hold_timeout: Duration::from_secs(600),
        sink: Some(sink.clone()),
        view: Arc::new(SandboxGrantView(Arc::downgrade(&sandbox))),
    }));
    let _ = sink.decider.set(Arc::downgrade(&decider));
    let tag = CommandTag::for_call(&call);
    let origin = xai_grok_sandbox::WebsiteOrigin::parse("https://stale.example").unwrap();
    // The card's task is queued, not run: this runtime has one thread and nothing yielded yet
    let answer = decider.hold(&origin, Some(&tag)).await;
    let mut allow = net_grant("stale.example", GrantDecision::Allow);
    allow.scope = GrantScope::Session;
    sandbox
        .record_grant(&call, None, "sess-1", allow)
        .await
        .unwrap();
    assert!(
        decider
            .settle_hold(
                "another-card",
                HoldAnswer::Allow,
                &WebFetchDomains::default()
            )
            .is_none()
    );
    let answered = tokio::time::timeout(Duration::from_secs(5), answer).await;
    assert_eq!(Ok(Ok(true)), answered);
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    assert!(
        transport.seen.lock().is_empty(),
        "no card for an answered connection"
    );
}

/// A connection tagged for an owned call the table holds no open spawn for (its child exited
/// before the park, its `finish` ran) is refused at the park — no card, no note for the model;
/// once a spawn of the call is on record its connection is parked and asked about as before.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hold_for_a_call_with_no_spawn_record_is_refused_without_a_card() {
    let fx = Fixture::new("enforce");
    let sandbox = fx.open().await;
    assert_eq!(SandboxMode::Enforce, sandbox.mode());
    let transport = ScriptedOwner::new([serde_json::json!({
        "outcome": "reject",
        "tool_call_id": "call-gone",
    })]);
    let call = CallId::tool("call-gone");
    sandbox
        .bind_call(
            &call,
            owner("sess-1", ToolApprovalPolicy::GrantsAllowed, &transport),
        )
        .unwrap();
    let sink = Arc::new(NetworkViolationSink {
        sandbox: Arc::downgrade(&sandbox),
        decider: std::sync::OnceLock::new(),
    });
    let decider = Arc::new(SandboxNetworkDecider::new(SandboxNetworkDeciderConfig {
        clock: fx.clock.clone(),
        hold_timeout: Duration::from_secs(600),
        sink: Some(sink.clone()),
        view: Arc::new(SandboxGrantView(Arc::downgrade(&sandbox))),
    }));
    let _ = sink.decider.set(Arc::downgrade(&decider));
    let tag = CommandTag::for_call(&call);
    let origin = xai_grok_sandbox::WebsiteOrigin::parse("https://gone.example").unwrap();
    let refused = decider.hold(&origin, Some(&tag)).await;
    assert!(
        matches!(
            tokio::time::timeout(Duration::from_secs(5), refused).await,
            Ok(Err(_))
        ),
        "refused: the receiver is closed, not left waiting on a card"
    );
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    assert!(transport.seen.lock().is_empty(), "no card");
    assert!(sandbox.take_network_denials(&call).is_empty(), "no note");
    assert!(
        sandbox.calls.lock().owner_of(&call).is_some(),
        "the entry stays"
    );

    let (mut cmd, original) = fx.command();
    sandbox.prepare(&mut cmd, &original, &call).unwrap();
    let parked = decider.hold(&origin, Some(&tag)).await;
    let answered = tokio::time::timeout(Duration::from_secs(5), parked).await;
    assert_eq!(
        Ok(Ok(false)),
        answered,
        "asked, and the owner kept it blocked"
    );
    assert_eq!(1, transport.seen.lock().len(), "one card");
    assert_eq!(1, sandbox.take_network_denials(&call).len());
}

/// Unserving the folder ends a hold settlement still waiting on its card: nothing is left
/// asking the owner, and nothing keeps the sandbox alive.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stopping_the_network_ends_a_hold_settlement_still_waiting_on_its_card() {
    let fx = Fixture::new("enforce");
    let sandbox = fx.open().await;
    // The proxy's shutdown waits out a parked connection's hold; keep that short here
    let info = sandbox
        .start_network(EgressProxyOptions {
            hold_timeout: Duration::from_secs(1),
            ..fx.proxy_options()
        })
        .await
        .expect("the proxy binds a loopback port");
    let address = info.address;
    let silent = Arc::new(SilentOwner::default());
    let call = CallId::tool("call-net");
    sandbox
        .bind_call(
            &call,
            CallOwner {
                session_id: "sess-1".to_owned(),
                policy: ToolApprovalPolicy::GrantsAllowed,
                transport: Some(silent.clone() as Arc<dyn PermissionHookTransport>),
                command: None,
            },
        )
        .unwrap();
    let (mut cmd, original) = fx.command();
    sandbox.prepare(&mut cmd, &original, &call).unwrap();
    let (token, _) = credential_of(&env_of(&cmd, "HTTP_PROXY").unwrap());
    let bearer = format!("Bearer {token}");
    let client =
        tokio::spawn(async move { connect_via(address, &bearer, "held.example:443").await });
    tokio::time::timeout(Duration::from_secs(5), silent.asked.notified())
        .await
        .expect("the held connection raised a card");
    assert_eq!(1, silent.waiting.load(Ordering::SeqCst));

    sandbox.stop_network().await;
    let released = tokio::time::timeout(Duration::from_secs(5), async {
        while silent.waiting.load(Ordering::SeqCst) != 0 || Arc::strong_count(&sandbox) != 1 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(
        released.is_ok(),
        "waiting on {} card(s), {} strong refs to the sandbox",
        silent.waiting.load(Ordering::SeqCst),
        Arc::strong_count(&sandbox)
    );
    let _ = client.await;
}

/// A background start keeps its session's "for this conversation" network rows for the process
/// it left running: the session goes with the kept credential, not with the unbound owner.
#[tokio::test]
async fn a_detached_background_call_keeps_its_sessions_network_rows() {
    let fx = Fixture::new("enforce");
    let sandbox = fx.sandbox().await;
    let call = CallId::tool("call-bg");
    sandbox
        .bind_call(
            &call,
            CallOwner {
                session_id: "sess-1".to_owned(),
                policy: ToolApprovalPolicy::GrantsAllowed,
                transport: None,
                command: None,
            },
        )
        .unwrap();
    prepared_token(&fx, &sandbox, "call-bg");
    let mut allow = net_grant("session.example", GrantDecision::Allow);
    allow.scope = GrantScope::Session;
    sandbox
        .record_grant(&call, None, "sess-1", allow)
        .await
        .unwrap();
    let covered = |tag: &CommandTag| {
        sandbox.net_rows_for(Some(tag)).iter().any(|grant| {
            matches!(&grant.subject, GrantSubject::NetHost { host, .. } if host.to_string() == "session.example")
        })
    };
    let tag = CommandTag::for_call(&call);
    assert!(covered(&tag));
    sandbox.detach_call(&call, "sess-1");
    assert!(
        covered(&tag),
        "the detached process keeps its session's row"
    );
    assert!(
        !covered(&CommandTag::for_call(&CallId::tool("unowned"))),
        "a call no session owns has none"
    );
    sandbox.stop_network().await;
}

/// The call table is the one record of a background start: the session its process runs for is
/// the table's, not a copy on the network side, so it lasts as long as the call's entry and not
/// as long as one proxy — a proxy that died and was replaced leaves it in place.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_background_calls_session_is_the_tables_and_outlives_a_replaced_proxy() {
    let fx = Fixture::new("enforce");
    let sandbox = fx.sandbox().await;
    let call = CallId::tool("call-bg");
    sandbox.bind_call(&call, cardless_owner()).unwrap();
    prepared_token(&fx, &sandbox, "call-bg");
    let mut allow = net_grant("session.example", GrantDecision::Allow);
    allow.scope = GrantScope::Session;
    sandbox
        .record_grant(&call, None, "sess-1", allow)
        .await
        .unwrap();
    sandbox.detach_call(&call, "sess-1");

    sandbox.abort_proxy_for_test().await;
    assert!(
        sandbox
            .sync_network()
            .await
            .expect("a replacement proxy starts")
            .is_some()
    );
    assert_eq!(
        Some("sess-1".to_owned()),
        sandbox.calls.lock().session_of(&call)
    );
    assert!(
        sandbox
            .net_rows_for(Some(&CommandTag::for_call(&call)))
            .iter()
            .any(|grant| matches!(
                &grant.subject,
                GrantSubject::NetHost { host, .. } if host.to_string() == "session.example"
            )),
        "the process still runs for its session"
    );
    sandbox.stop_network().await;
}

/// A background start's credential goes with the process it was kept for: the child's exit and
/// the end of the hub session that started it each revoke it (407 under `enforce`), so a
/// grandchild the job daemonized keeps no authenticated egress for the life of the folder's
/// proxy. Another session's background start is left alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_detached_credential_goes_with_its_childs_exit_or_its_sessions_end() {
    let fx = Fixture::new("enforce");
    let sandbox = fx.sandbox().await;
    let (exiting, address) = prepared_token(&fx, &sandbox, "bg-exit");
    sandbox.detach_call(&CallId::tool("bg-exit"), "sess-1");
    let (ending, _) = prepared_token(&fx, &sandbox, "bg-end");
    sandbox.detach_call(&CallId::tool("bg-end"), "sess-1");
    let (kept, _) = prepared_token(&fx, &sandbox, "bg-kept");
    sandbox.detach_call(&CallId::tool("bg-kept"), "sess-2");
    for token in [&exiting, &ending, &kept] {
        assert_eq!(
            403,
            stranger_status(address, token).await,
            "kept: refused, not unauthenticated"
        );
    }

    SandboxLaunch::exited(sandbox.as_ref(), &CallId::tool("bg-exit"));
    assert_eq!(
        407,
        stranger_status(address, &exiting).await,
        "the child's exit revoked its credential"
    );
    assert!(
        sandbox
            .calls
            .lock()
            .session_of(&CallId::tool("bg-exit"))
            .is_none(),
        "and it is no longer kept"
    );
    assert_eq!(403, stranger_status(address, &ending).await);

    sandbox.end_session("sess-1").await;
    assert_eq!(
        407,
        stranger_status(address, &ending).await,
        "the session's end revoked its credential"
    );
    assert!(
        sandbox
            .calls
            .lock()
            .session_of(&CallId::tool("bg-end"))
            .is_none()
    );
    assert_eq!(
        403,
        stranger_status(address, &kept).await,
        "another session's background start keeps its credential"
    );
    assert_eq!(1, sandbox.open_calls());
    sandbox.stop_network().await;
}

/// One CONNECT to a host nobody allowed, with `token` as the call's credential: 403 while the
/// credential authenticates, 407 once it is revoked.
async fn stranger_status(address: SocketAddr, token: &str) -> u16 {
    connect_via(address, &format!("Bearer {token}"), "nobody.example:443").await
}

/// The child exits before the hub has the call's start (the exit lands with the call still
/// owned): its credential is revoked (407), the connection it held on a card is refused and the
/// card withdrawn; the hub's late start then finds nothing left to keep and releases it once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_owned_childs_exit_revokes_its_credential_and_refuses_its_parked_hold() {
    let fx = Fixture::new("enforce");
    let sandbox = fx.open().await;
    // Only the exit may answer the hold within the 5 s the test waits, not the proxy's timeout
    sandbox
        .start_network(EgressProxyOptions {
            hold_timeout: Duration::from_secs(600),
            ..fx.proxy_options()
        })
        .await
        .expect("the proxy binds a loopback port");
    let silent = Arc::new(SilentOwner::default());
    let call = CallId::tool("bg-early-exit");
    sandbox
        .bind_call(
            &call,
            CallOwner {
                session_id: "sess-1".to_owned(),
                policy: ToolApprovalPolicy::GrantsAllowed,
                transport: Some(silent.clone() as Arc<dyn PermissionHookTransport>),
                command: None,
            },
        )
        .unwrap();
    let (token, address) = prepared_token(&fx, &sandbox, "bg-early-exit");
    let held = {
        let bearer = format!("Bearer {token}");
        tokio::spawn(async move { connect_via(address, &bearer, "held.example:443").await })
    };
    tokio::time::timeout(Duration::from_secs(5), silent.asked.notified())
        .await
        .expect("the child's connection is held on a card");
    assert_eq!(1, silent.waiting.load(Ordering::SeqCst));

    SandboxLaunch::exited(sandbox.as_ref(), &call);
    let status = tokio::time::timeout(Duration::from_secs(5), held)
        .await
        .expect("the exit answers the parked hold")
        .unwrap();
    assert_eq!(403, status, "the held connection is refused");
    assert_eq!(
        407,
        stranger_status(address, &token).await,
        "the credential went with the child, start or no start"
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        while silent.withdrawn.lock().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the refused hold's card is withdrawn");
    assert!(
        sandbox.calls.lock().owner_of(&call).is_some(),
        "the entry waits for the hub's result"
    );

    // The hub's late background start: no spawn is left, so the call is released — once
    sandbox.detach_call(&call, "sess-1");
    assert_eq!(0, sandbox.calls.lock().len());
    sandbox.release_call(&call);
    sandbox.stop_network().await;
    let withdrawn = silent.withdrawn.lock().clone();
    assert_eq!(1, withdrawn.len(), "told once: {withdrawn:?}");
}

/// The exit's other half, on the wire: a connection that authenticated before the spawn record
/// went and reaches the park after it is refused there — 403, no card, nothing for the model —
/// and the hub's late start still releases the call once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_connection_authenticated_before_the_exit_is_refused_at_the_park_without_a_card() {
    let fx = Fixture::new("enforce");
    let sandbox = fx.open().await;
    // A card the owner never answers must not be timed out into a deny within the test
    sandbox
        .start_network(EgressProxyOptions {
            hold_timeout: Duration::from_secs(600),
            ..fx.proxy_options()
        })
        .await
        .expect("the proxy binds a loopback port");
    let owner = GatedOwner::new(serde_json::json!({
        "outcome": "reject",
        "tool_call_id": "bg-late",
    }));
    let call = CallId::tool("bg-late");
    sandbox
        .bind_call(
            &call,
            CallOwner {
                session_id: "sess-1".to_owned(),
                policy: ToolApprovalPolicy::GrantsAllowed,
                transport: Some(owner.clone() as Arc<dyn PermissionHookTransport>),
                command: None,
            },
        )
        .unwrap();
    let (token, address) = prepared_token(&fx, &sandbox, "bg-late");
    // The race's state, held still: the table let the spawn record go (the exit's table half)
    // while this connection's credential had already authenticated, before the revoke
    let (record, _released) = sandbox.calls.lock().take_prepared(&call);
    assert!(record.is_some());

    assert_eq!(
        403,
        connect_via(address, &format!("Bearer {token}"), "late.example:443").await,
        "refused at the park: nothing runs for an answer to reach"
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(0, owner.asked.load(Ordering::SeqCst), "no card");
    assert!(sandbox.take_network_denials(&call).is_empty(), "no note");
    assert!(sandbox.calls.lock().owner_of(&call).is_some());

    // The hub's late background start: no spawn is left, so the call is released — once
    sandbox.detach_call(&call, "sess-1");
    assert_eq!(0, sandbox.calls.lock().len());
    assert_eq!(
        407,
        stranger_status(address, &token).await,
        "released: the credential went with it"
    );
    assert_eq!(0, owner.asked.load(Ordering::SeqCst));
    sandbox.stop_network().await;
}

/// A call is decided under the mode its spawn fixed, whatever the folder does next: once the
/// folder flips to `observe`, both a call the hub pinned to `enforce` and one whose spawn read
/// `enforce` from the folder still have their connections held on cards, the sync keeps the
/// proxy for them and releases neither hold, and only the stop refuses them. A flip never gives
/// a command that started under `enforce` the network unasked; a command dispatched after the
/// flip is pointed at no proxy at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enforce_calls_connections_stay_asked_after_the_folder_leaves_enforce() {
    let fx = Fixture::new("enforce");
    let sandbox = fx.sandbox().await;
    let address = sandbox.network().unwrap().address;
    let silent = Arc::new(SilentOwner::default());
    // Two sessions: a session is shown one card at a time, and both holds must be on a card
    let owner = |session: &str| CallOwner {
        session_id: session.to_owned(),
        policy: ToolApprovalPolicy::GrantsAllowed,
        transport: Some(silent.clone() as Arc<dyn PermissionHookTransport>),
        command: None,
    };
    let pinned = CallId::tool("pinned");
    sandbox.pin_mode(&pinned, SandboxMode::Enforce).unwrap();
    assert!(sandbox.bind_pinned_call(&pinned, owner("sess-1")));
    let (pinned_token, _) = prepared_token(&fx, &sandbox, "pinned");
    let unpinned = CallId::tool("unpinned");
    sandbox.bind_call(&unpinned, owner("sess-2")).unwrap();
    let (unpinned_token, _) = prepared_token(&fx, &sandbox, "unpinned");
    assert_eq!(
        Some(SandboxMode::Enforce),
        sandbox.calls.lock().held_mode(&unpinned),
        "the spawn fixed the folder's mode on the call"
    );

    fx.set_mode("observe");
    sandbox
        .sync_network()
        .await
        .expect("the proxy keeps running");
    assert_eq!(SandboxMode::Observe, sandbox.mode());

    let later = CallId::tool("later");
    sandbox.bind_call(&later, owner("sess-3")).unwrap();
    let (mut cmd, original) = fx.command();
    sandbox.prepare(&mut cmd, &original, &later).unwrap();
    assert!(
        env_of(&cmd, "HTTP_PROXY").is_none(),
        "a command dispatched after the flip is pointed at no proxy"
    );
    assert_eq!(
        Some(SandboxMode::Observe),
        sandbox.calls.lock().held_mode(&later)
    );
    assert!(sandbox.network().is_some(), "kept for the enforce calls");
    assert_eq!(0, silent.waiting.load(Ordering::SeqCst));

    let clients: Vec<_> = [pinned_token, unpinned_token]
        .into_iter()
        .map(|token| {
            let bearer = format!("Bearer {token}");
            tokio::spawn(async move { connect_via(address, &bearer, "flip.example:443").await })
        })
        .collect();
    tokio::time::timeout(Duration::from_secs(5), async {
        while silent.waiting.load(Ordering::SeqCst) != 2 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("both enforce calls' connections raised a card under observe");

    sandbox
        .sync_network()
        .await
        .expect("the proxy keeps running");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        2,
        silent.waiting.load(Ordering::SeqCst),
        "a sync releases no hold: the calls keep the mode they started under"
    );
    assert!(
        clients.iter().all(|client| !client.is_finished()),
        "still parked"
    );

    sandbox.stop_network().await;
    for client in clients {
        let status = tokio::time::timeout(Duration::from_secs(5), client)
            .await
            .expect("the parked client is answered")
            .unwrap();
        assert_eq!(
            403, status,
            "the stop refuses what an enforce call had parked"
        );
    }
}

/// The status the looping child reports for its next connection.
async fn next_status(statuses: &mut tokio::sync::mpsc::UnboundedReceiver<u16>) -> u16 {
    tokio::time::timeout(Duration::from_secs(5), statuses.recv())
        .await
        .expect("the loop reports")
        .expect("the loop runs")
}

/// A background start whose command loops over the network: the connection the proxy held while
/// the call's stream still ran is refused once the stream ended on the start — there is no
/// stream left to ask on — and its card goes with it, so the answer the user gives later records
/// nothing. The child keeps its mode and its credential: its next connection is refused without
/// a card, and only its exit revokes the credential.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn background_start_refuses_the_childs_held_connection_and_withdraws_its_card() {
    let fx = Fixture::new("enforce");
    let sandbox = fx.open().await;
    // A hold the proxy would time out on its own proves nothing: only the start's drain may
    // answer it within the 5 s `next_status` waits
    sandbox
        .start_network(EgressProxyOptions {
            hold_timeout: Duration::from_secs(600),
            ..fx.proxy_options()
        })
        .await
        .expect("the proxy binds a loopback port");
    let owner = GatedOwner::new(serde_json::json!({
        "outcome": "approve",
        "tool_call_id": "bg-loop",
        "scope": { "kind": "net_host" },
        "duration": { "kind": "call" },
    }));
    sandbox.set_card_transport(owner.clone());
    let handle =
        WorkspaceHandle::for_test_in_with_sandbox(&fx.root, sandbox.clone(), ToolApprovalGate::Off);
    let session = handle.create_session("main").unwrap();
    let call = CallId::tool("bg-loop");
    let (token, address) = prepared_token(&fx, &sandbox, "bg-loop");

    // The dispatch starts the command's loop, one connection per tick; the stream ends on the
    // background start once the loop's first connection is held on a card
    let (statuses_tx, mut statuses) = tokio::sync::mpsc::unbounded_channel();
    let (ticks, ticks_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    let mut command = Some((statuses_tx, ticks_rx));
    let dispatch = {
        let owner = owner.clone();
        move || -> ToolStream<ToolRunResult> {
            let (statuses_tx, mut ticks_rx) = command.take().expect("one dispatch");
            let bearer = format!("Bearer {token}");
            tokio::spawn(async move {
                loop {
                    let status = connect_via(address, &bearer, "loop.example:443").await;
                    if statuses_tx.send(status).is_err() || ticks_rx.recv().await.is_none() {
                        break;
                    }
                }
            });
            let owner = owner.clone();
            Box::pin(async_stream::stream! {
                owner.card.notified().await;
                yield ToolStreamItem::Terminal(Ok(ToolRunResult {
                    output: ToolOutput::BackgroundTaskStarted(BackgroundTaskStarted {
                        task_id: "t1".to_owned(),
                        task_type: "bash".to_owned(),
                        output_file: "/tmp/out".to_owned(),
                        status: "running".to_owned(),
                        command: "while :; do curl https://loop.example/; sleep 1; done &"
                            .to_owned(),
                        summary: "running".to_owned(),
                        retrieval_hint: String::new(),
                        pre_formatted: None,
                        pid: None,
                    }),
                    prompt_text: String::new(),
                    effective_tool_name: None,
                }));
            })
        }
    };
    let items: Vec<_> = tokio::time::timeout(
        Duration::from_secs(5),
        run_shell_call_with_replay(handle, session, "bg-loop".to_owned(), None, false, dispatch)
            .collect(),
    )
    .await
    .expect("the stream ends on the background start");
    assert!(
        matches!(items.as_slice(), [ToolStreamItem::Terminal(Ok(_))]),
        "{items:?}"
    );
    assert!(sandbox.calls.lock().owner_of(&call).is_none());
    assert_eq!(1, sandbox.open_calls(), "the child runs on");
    assert_eq!(
        Some(SandboxMode::Enforce),
        sandbox.calls.lock().held_mode(&call),
        "under the mode it started under"
    );

    assert_eq!(
        403,
        next_status(&mut statuses).await,
        "the held connection is refused: no stream is left to ask on"
    );
    assert_eq!(1, owner.asked.load(Ordering::SeqCst));
    let withdrawn = tokio::time::timeout(Duration::from_secs(5), async {
        while owner.withdrawn.lock().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(
        withdrawn.is_ok(),
        "the refused hold's card was not withdrawn"
    );
    let withdrawn = owner.withdrawn.lock().clone();
    assert_eq!(1, withdrawn.len(), "told exactly once: {withdrawn:?}");
    assert!(
        !sandbox.hold_pending_for_test(withdrawn.first().expect("one withdrawal")),
        "the withdrawn card names a hold the decider still has"
    );
    owner.answers.add_permits(1);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        sandbox.calls.lock().once_rows(&call).is_empty(),
        "the answer to a withdrawn card records nothing for the child"
    );
    ticks.send(()).unwrap();
    assert_eq!(
        403,
        next_status(&mut statuses).await,
        "the child's next connection is refused without a card"
    );
    assert_eq!(
        1,
        owner.asked.load(Ordering::SeqCst),
        "nobody is asked for it"
    );

    SandboxLaunch::exited(sandbox.as_ref(), &call);
    ticks.send(()).unwrap();
    assert_eq!(
        407,
        next_status(&mut statuses).await,
        "the credential went with the child"
    );
    assert_eq!(1, owner.asked.load(Ordering::SeqCst));
    drop(ticks);
    // The stop finds nothing parked for the child: no second withdrawal
    sandbox.stop_network().await;
    assert_eq!(1, owner.withdrawn.lock().len());
}

/// Flipping the folder to `off` stops its proxy at once even while connections wait on cards, so
/// the shutdown (and the lifecycle lock every later sync takes) never waits out a hold. Every
/// parked connection is refused: both calls started under `enforce`, a stop answers nothing with
/// an allow, and `off` reaches the commands dispatched after it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_flip_to_off_stops_the_proxy_without_waiting_out_a_parked_hold() {
    let fx = Fixture::new("enforce");
    let sandbox = fx.open().await;
    let address = sandbox
        .start_network(EgressProxyOptions {
            hold_timeout: Duration::from_secs(120),
            ..fx.proxy_options()
        })
        .await
        .expect("the proxy binds a loopback port")
        .address;
    let silent = Arc::new(SilentOwner::default());
    // Two sessions: a session is shown one card at a time, and both holds must be on a card
    let owner = |session: &str| CallOwner {
        session_id: session.to_owned(),
        policy: ToolApprovalPolicy::GrantsAllowed,
        transport: Some(silent.clone() as Arc<dyn PermissionHookTransport>),
        command: None,
    };
    sandbox
        .bind_call(&CallId::tool("call-off"), owner("sess-1"))
        .unwrap();
    let (unpinned_token, _) = prepared_token(&fx, &sandbox, "call-off");
    let pinned = CallId::tool("call-pinned");
    sandbox.pin_mode(&pinned, SandboxMode::Enforce).unwrap();
    assert!(sandbox.bind_pinned_call(&pinned, owner("sess-2")));
    let (pinned_token, _) = prepared_token(&fx, &sandbox, "call-pinned");
    let unpinned_client = tokio::spawn(async move {
        connect_via(
            address,
            &format!("Bearer {unpinned_token}"),
            "held.example:443",
        )
        .await
    });
    let pinned_client = tokio::spawn(async move {
        connect_via(
            address,
            &format!("Bearer {pinned_token}"),
            "held.example:443",
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while silent.waiting.load(Ordering::SeqCst) != 2 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("both held connections raised a card");

    fx.set_mode("off");
    let synced = tokio::time::timeout(Duration::from_secs(10), sandbox.sync_network()).await;
    assert!(synced.is_ok(), "the stop waited out a parked hold");
    assert!(sandbox.network().is_none());
    for client in [unpinned_client, pinned_client] {
        let status = tokio::time::timeout(Duration::from_secs(5), client)
            .await
            .expect("the parked client is answered")
            .unwrap();
        assert_eq!(
            403, status,
            "a hold parked under enforce is refused by the stop, not let through"
        );
    }
}

/// A connection parked under `enforce` whose card nobody answers stays parked, its card up, when
/// the folder flips to `observe`: the call keeps the mode it started under, so the sync keeps
/// the proxy for it, releases nothing and withdraws nothing. The command dispatched after the
/// flip is pointed at no proxy and raises no card. The call's final result is what
/// refuses the parked connection; its card goes with it, deterministically and not at its
/// deadline, and the owner is told exactly once which card is stale — a hold the decider no
/// longer has — although that result, the last enforce call's, also stops the proxy.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn card_raised_under_enforce_stays_up_after_the_folder_flips_to_observe() {
    let fx = Fixture::new("enforce");
    let sandbox = fx.sandbox().await;
    let address = sandbox.network().unwrap().address;
    // Told slowly: the stop the release schedules lands inside the withdrawal
    let silent = Arc::new(SilentOwner {
        slow_withdrawal: std::sync::atomic::AtomicBool::new(true),
        ..SilentOwner::default()
    });
    let owner = || CallOwner {
        session_id: "sess-1".to_owned(),
        policy: ToolApprovalPolicy::GrantsAllowed,
        transport: Some(silent.clone() as Arc<dyn PermissionHookTransport>),
        command: None,
    };
    let call = CallId::tool("call-flip");
    sandbox.bind_call(&call, owner()).unwrap();
    let (mut cmd, original) = fx.command();
    sandbox.prepare(&mut cmd, &original, &call).unwrap();
    let (token, _) = credential_of(&env_of(&cmd, "HTTP_PROXY").unwrap());
    let bearer = format!("Bearer {token}");
    let client =
        tokio::spawn(async move { connect_via(address, &bearer, "held.example:443").await });
    tokio::time::timeout(Duration::from_secs(5), silent.asked.notified())
        .await
        .expect("the held connection raised a card");

    fx.set_mode("observe");
    assert_eq!(1, silent.waiting.load(Ordering::SeqCst));
    sandbox
        .sync_network()
        .await
        .expect("the proxy keeps running");
    assert_eq!(SandboxMode::Observe, sandbox.mode());
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        1,
        silent.waiting.load(Ordering::SeqCst),
        "the card raised under enforce is still up"
    );
    assert!(silent.withdrawn.lock().is_empty(), "and was not withdrawn");
    assert!(!client.is_finished(), "the connection is still parked");
    assert_eq!(
        Some(SandboxMode::Enforce),
        sandbox.calls.lock().held_mode(&call)
    );

    let next = CallId::tool("call-next");
    sandbox.bind_call(&next, owner()).unwrap();
    let (mut cmd, original) = fx.command();
    sandbox.prepare(&mut cmd, &original, &next).unwrap();
    assert!(
        env_of(&cmd, "HTTP_PROXY").is_none(),
        "the next command is pointed at no proxy"
    );
    assert_eq!(1, silent.waiting.load(Ordering::SeqCst));

    sandbox.release_call(&call);
    assert_eq!(
        1,
        sandbox.proxy_resyncs(),
        "the last enforce call's end schedules the stop"
    );
    let status = tokio::time::timeout(Duration::from_secs(5), client)
        .await
        .expect("the parked client is answered")
        .unwrap();
    assert_eq!(403, status, "the call's result refuses what it had parked");
    let withdrawn = tokio::time::timeout(Duration::from_secs(5), async {
        while silent.waiting.load(Ordering::SeqCst) != 0 || silent.withdrawn.lock().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(
        withdrawn.is_ok(),
        "the refused hold's card is still waiting on the owner, or the owner was not told"
    );
    let withdrawn = silent.withdrawn.lock().clone();
    assert_eq!(1, withdrawn.len(), "told exactly once: {withdrawn:?}");
    let withdrawn_id = withdrawn.first().expect("one withdrawal");
    assert!(
        !sandbox.hold_pending_for_test(withdrawn_id),
        "the withdrawn card names a hold the decider still has"
    );
    // The stop the end scheduled raced the withdrawal, which is the sandbox's task and not the
    // proxy's; it found nothing parked for the call: no second withdrawal
    sandbox.proxy_resynced().await;
    assert!(
        sandbox.network().is_none(),
        "stopped with the last enforce call"
    );
    sandbox.stop_network().await;
    assert_eq!(1, silent.withdrawn.lock().len());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_held_request_is_settled_by_the_bound_owner_and_an_allow_resumes_it() {
    let fx = Fixture::new("enforce");
    let sandbox = fx.sandbox().await;
    let address = sandbox.network().unwrap().address;
    let mut decisions = sandbox.blocked_requests().unwrap();
    let scripted = ScriptedOwner::new([
        serde_json::json!({
            "outcome": "approve",
            "tool_call_id": "call-net",
            "scope": { "kind": "net_host" },
            "duration": { "kind": "workspace" },
            "expires": { "kind": "ttl", "seconds": 3600 },
        }),
        serde_json::json!({
            "outcome": "reject",
            "tool_call_id": "call-net",
            "followup_message": "not that host",
        }),
    ]);
    // The hub binds the tool's `command` argument with the owner: the card shows that, not the
    // spawned argv (the terminal's bootstrap script)
    sandbox
        .bind_call(
            &CallId::tool("call-net"),
            CallOwner {
                command: Some("curl https://held.example/".to_owned()),
                ..owner("sess-1", ToolApprovalPolicy::GrantsAllowed, &scripted)
            },
        )
        .unwrap();
    let (mut cmd, original) = fx.command();
    sandbox
        .prepare(&mut cmd, &original, &CallId::tool("call-net"))
        .unwrap();
    let (token, _) = credential_of(&env_of(&cmd, "HTTP_PROXY").unwrap());
    let bearer = format!("Bearer {token}");

    // Held at the proxy until the card is answered; the allow records the row and resumes the
    // connection, which then fails upstream (502) because nothing resolves in the test
    let started = std::time::Instant::now();
    assert_eq!(502, connect_via(address, &bearer, "held.example:443").await);
    let resumed = next_decision(&mut decisions).await;
    assert_eq!(DeciderOutcome::Allowed, resumed.decided);
    assert_eq!(
        Some(CommandTag::for_call(&CallId::tool("call-net"))),
        resumed.call
    );
    assert!(
        started.elapsed() >= Duration::from_millis(resumed.hold_ms),
        "hold_ms {} is the time the connection waited",
        resumed.hold_ms
    );
    let payloads = scripted.seen.lock().clone();
    assert_eq!(1, payloads.len());
    let payload = payloads.first().expect("one card");
    assert_eq!(
        Some(&serde_json::json!("call-net")),
        payload.get("tool_call_id")
    );
    assert_eq!(
        Some(&serde_json::json!("curl https://held.example/")),
        payload.get("bash_command")
    );
    assert_eq!(Some(&serde_json::json!("resume")), payload.get("replay"));
    assert_eq!(
        Some(&serde_json::json!("held.example")),
        payload.pointer("/blocked/host")
    );
    assert_eq!(
        Some(&serde_json::json!(["call", "session", "workspace"])),
        payload.get("offered_scopes")
    );
    assert!(
        payload
            .get("deadline_unix")
            .and_then(serde_json::Value::as_i64)
            .is_some_and(|at| at > NOW),
        "{payload}"
    );
    let grants = sandbox.live_grants().await;
    assert_eq!(1, grants.len(), "{grants:?}");
    let grant = grants.first().expect("one live row");
    assert_eq!(
        GrantSubject::NetHost {
            host: HostPattern::new("held.example"),
            port: None,
        },
        grant.subject
    );
    assert_eq!("hub:sess-1", grant.granted_by);
    assert_eq!(
        Some("curl https://held.example/"),
        grant.via.as_ref().map(|via| via.command.as_str())
    );

    // The row is live for the decider at once: no second card for the same host
    assert_eq!(
        502,
        connect_via(address, &bearer, "held.example:8443").await
    );
    assert_eq!(1, scripted.seen.lock().len());

    // A refusal keeps the hold denied (403) and leaves the model its text
    assert_eq!(
        403,
        connect_via(address, &bearer, "other.example:443").await
    );
    let denied = next_decision(&mut decisions).await;
    assert_eq!(DeciderOutcome::Denied, denied.decided);
    assert!(denied.hold_ms < 5_000, "answered, not timed out");
    let texts = sandbox.take_network_denials(&CallId::tool("call-net"));
    assert_eq!(1, texts.len(), "{texts:?}");
    let text = texts.first().expect("one denial");
    assert!(
        text.contains("sandbox denied: connection to other.example:443")
            && text.contains("User says: not that host"),
        "{texts:?}"
    );
    assert!(
        sandbox
            .take_network_denials(&CallId::tool("call-net"))
            .is_empty(),
        "consumed"
    );
    assert_eq!(
        1,
        sandbox.live_grants().await.len(),
        "a reject records nothing"
    );

    // Its owner gone (the stream ended on a background start): the next hold has nobody to ask
    // and fails closed without a card
    sandbox.detach_call(&CallId::tool("call-net"), "sess-1");
    assert_eq!(
        403,
        connect_via(address, &bearer, "third.example:443").await
    );
    assert_eq!(
        DeciderOutcome::Denied,
        next_decision(&mut decisions).await.decided
    );
    assert_eq!(2, scripted.seen.lock().len());
}

/// Under `always_prompt` the card offers the call scope only and a workspace answer is clamped
/// to it; the call-scoped row covers the command's next connections while it runs
/// and is gone with the call; a transport that fails keeps the hold refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn always_prompt_offers_call_only_and_a_failing_transport_refuses_the_hold() {
    let fx = Fixture::new("enforce");
    let sandbox = fx.sandbox().await;
    let address = sandbox.network().unwrap().address;
    let scripted = ScriptedOwner::new([serde_json::json!({
        "outcome": "approve",
        "tool_call_id": "call-ap",
        "scope": { "kind": "net_host" },
        "duration": { "kind": "workspace" },
    })]);
    sandbox
        .bind_call(
            &CallId::tool("call-ap"),
            owner("sess-2", ToolApprovalPolicy::AlwaysPrompt, &scripted),
        )
        .unwrap();
    let (mut cmd, original) = fx.command();
    sandbox
        .prepare(&mut cmd, &original, &CallId::tool("call-ap"))
        .unwrap();
    let (token, _) = credential_of(&env_of(&cmd, "HTTP_PROXY").unwrap());
    let bearer = format!("Bearer {token}");

    assert_eq!(502, connect_via(address, &bearer, "once.example:443").await);
    let payloads = scripted.seen.lock().clone();
    let payload = payloads.first().expect("one card");
    assert_eq!(
        Some(&serde_json::json!(["call"])),
        payload.get("offered_scopes")
    );
    assert_eq!(
        Some(&serde_json::json!("always_prompt")),
        payload.get("tool_approval_policy")
    );
    // The workspace answer was clamped to the call: nothing persisted...
    assert!(sandbox.live_grants().await.is_empty());
    // ...but the same command's next connection to that host is covered without a card
    assert_eq!(502, connect_via(address, &bearer, "once.example:443").await);
    assert_eq!(1, scripted.seen.lock().len());
    // ...and with the script exhausted the transport fails, which keeps another host refused
    assert_eq!(
        403,
        connect_via(address, &bearer, "elsewhere.example:443").await
    );
    let texts = sandbox.take_network_denials(&CallId::tool("call-ap"));
    assert!(
        texts
            .iter()
            .any(|t| t.contains("the permission prompt failed")),
        "{texts:?}"
    );

    // The call ends: its once row goes with it, so a new call for the same host is asked again
    // (and refused: nobody is bound for it)
    assert!(
        sandbox
            .finish(&CallId::tool("call-ap"), CommandExit::code(0), b"")
            .await
            .violation()
            .is_none()
    );
    sandbox.release_call(&CallId::tool("call-ap"));
    let (mut cmd, original) = fx.command();
    sandbox
        .prepare(&mut cmd, &original, &CallId::tool("call-next"))
        .unwrap();
    let (token, _) = credential_of(&env_of(&cmd, "HTTP_PROXY").unwrap());
    assert_eq!(
        403,
        connect_via(address, &format!("Bearer {token}"), "once.example:443").await
    );
}

/// Through the folder's real proxy: while any call started under `enforce` is live — one the hub
/// pinned, one whose spawn fixed `enforce` on its own, or a child that call left running in the
/// background — the flip to `observe` does not open the proxy to a request with no credential
/// (that could be the enforce command itself with its token dropped), so it is a `407` like any
/// unknown token; and once the last of them is gone the proxy goes with it — the exit schedules
/// the sync that stops it — rather than staying to decide credential-less requests as the
/// session's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enforce_spawns_keep_the_proxy_authenticating_after_the_folder_leaves_enforce() {
    let fx = Fixture::new("enforce");
    let sandbox = fx.sandbox().await;
    let address = sandbox.network().unwrap().address;
    let silent = Arc::new(SilentOwner::default());
    let owner = || CallOwner {
        session_id: "sess-1".to_owned(),
        policy: ToolApprovalPolicy::GrantsAllowed,
        transport: Some(silent.clone() as Arc<dyn PermissionHookTransport>),
        command: None,
    };
    let pinned = CallId::tool("pinned");
    sandbox.pin_mode(&pinned, SandboxMode::Enforce).unwrap();
    assert!(sandbox.bind_pinned_call(&pinned, owner()));
    let (_pinned_token, _) = prepared_token(&fx, &sandbox, "pinned");
    let unpinned = CallId::tool("unpinned");
    sandbox.bind_call(&unpinned, owner()).unwrap();
    let (_unpinned_token, _) = prepared_token(&fx, &sandbox, "unpinned");

    fx.set_mode("observe");
    sandbox
        .sync_network()
        .await
        .expect("the proxy keeps running");
    assert_eq!(SandboxMode::Observe, sandbox.mode());
    assert!(!sandbox.admits_unauthenticated_for_test());
    assert_eq!(
        407,
        connect_via(address, "", "dropped.example:443").await,
        "no credential while an enforce call runs: refused, not decided as the session's"
    );
    assert_eq!(
        407,
        connect_via(address, "Bearer not-a-token", "dropped.example:443").await
    );
    assert_eq!(
        0,
        silent.waiting.load(Ordering::SeqCst),
        "no card was raised"
    );

    sandbox.release_call(&pinned);
    assert!(
        !sandbox.admits_unauthenticated_for_test(),
        "the unpinned enforce spawn still runs"
    );
    assert_eq!(
        0,
        sandbox.proxy_resyncs(),
        "not the last: no sync scheduled"
    );
    assert_eq!(407, connect_via(address, "", "dropped.example:443").await);
    sandbox.detach_call(&unpinned, "sess-1");
    assert!(
        !sandbox.admits_unauthenticated_for_test(),
        "its background child still runs under enforce"
    );
    assert_eq!(
        0,
        sandbox.proxy_resyncs(),
        "the child runs on: no sync scheduled"
    );
    assert_eq!(407, connect_via(address, "", "dropped.example:443").await);

    // The last enforce spawn is gone: its exit schedules the sync that stops the proxy the
    // observing folder kept for it
    SandboxLaunch::exited(sandbox.as_ref(), &unpinned);
    assert_eq!(1, sandbox.proxy_resyncs());
    sandbox.proxy_resynced().await;
    assert!(
        sandbox.network().is_none(),
        "stopped with the last enforce spawn"
    );
    assert!(
        !proxy_answers_at(address, "").await,
        "nothing listens for a credential-less request once nothing runs under enforce"
    );
    sandbox.stop_network().await;
}

/// How the last call running under `enforce` ends: its final result, a background start whose
/// child then exits or whose session then ends, or a background start of a call with no open
/// spawn (a pin the table still held; `detach` releases such a call).
#[derive(Clone, Copy, Debug)]
enum LastEnd {
    Release,
    DetachThenExit,
    SessionEnd,
    DetachWithNoSpawn,
}

/// After the flip to `observe` the proxy stays for the calls still running under `enforce` and
/// goes with the last of them however it ends: the end schedules a sync off every lock, which
/// stops the listener and reports `network: "off"`. A non-last call or a detach alone: nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_last_enforce_calls_end_stops_the_proxy_the_observing_folder_kept_for_it() {
    for end in [
        LastEnd::Release,
        LastEnd::DetachThenExit,
        LastEnd::SessionEnd,
        LastEnd::DetachWithNoSpawn,
    ] {
        let fx = Fixture::new("enforce");
        let sandbox = fx.sandbox().await;
        let address = sandbox.network().unwrap().address;
        let session = sandbox.session_proxy_authorization().unwrap();
        let owner = || CallOwner {
            session_id: "sess-1".to_owned(),
            policy: ToolApprovalPolicy::GrantsAllowed,
            transport: None,
            command: None,
        };
        let first = CallId::tool("first");
        let last = CallId::tool("last");
        sandbox.bind_call(&first, owner()).unwrap();
        prepared_token(&fx, &sandbox, "first");
        if matches!(end, LastEnd::DetachWithNoSpawn) {
            sandbox.pin_mode(&last, SandboxMode::Enforce).unwrap();
            assert!(sandbox.bind_pinned_call(&last, owner()));
        } else {
            sandbox.bind_call(&last, owner()).unwrap();
            prepared_token(&fx, &sandbox, "last");
        }

        fx.set_mode("observe");
        sandbox
            .sync_network()
            .await
            .expect("the proxy keeps running for the enforce calls");
        assert_eq!(SandboxMode::Observe, sandbox.published_mode());
        assert!(
            sandbox.network().is_some(),
            "{end:?}: kept for the enforce calls"
        );
        assert_eq!(
            Some(&serde_json::json!("proxy")),
            sandbox.status_json().get("network"),
            "{end:?}"
        );

        sandbox.release_call(&first);
        assert_eq!(
            0,
            sandbox.proxy_resyncs(),
            "{end:?}: a call that is not the last schedules no sync"
        );
        assert!(sandbox.network().is_some(), "{end:?}: still kept");
        assert!(
            proxy_answers_at(address, &session).await,
            "{end:?}: still listening"
        );

        match end {
            LastEnd::Release => {
                sandbox.release_call(&last);
            }
            LastEnd::DetachThenExit => {
                sandbox.detach_call(&last, "sess-1");
                assert_eq!(
                    0,
                    sandbox.proxy_resyncs(),
                    "{end:?}: the background child still runs under enforce"
                );
                assert!(sandbox.network().is_some(), "{end:?}: kept for the child");
                SandboxLaunch::exited(sandbox.as_ref(), &last);
            }
            LastEnd::SessionEnd => {
                sandbox.detach_call(&last, "sess-1");
                assert_eq!(0, sandbox.proxy_resyncs(), "{end:?}");
                sandbox.end_session("sess-1").await;
            }
            LastEnd::DetachWithNoSpawn => {
                sandbox.detach_call(&last, "sess-1");
            }
        }
        assert_eq!(
            1,
            sandbox.proxy_resyncs(),
            "{end:?}: the last enforce call's end schedules one sync"
        );
        sandbox.proxy_resynced().await;
        assert!(
            sandbox.network().is_none(),
            "{end:?}: the sync stopped the proxy"
        );
        assert_eq!(
            Some(&serde_json::json!("off")),
            sandbox.status_json().get("network"),
            "{end:?}"
        );
        assert!(
            !proxy_answers_at(address, &session).await,
            "{end:?}: the listener is closed"
        );
        sandbox.stop_network().await;
    }
}

/// The proxy is the folder's under `enforce`, not its calls': a folder back at `enforce` by the
/// time its last enforce call ends keeps it — no sync is scheduled, and the one that runs next
/// keeps it too.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_last_enforce_calls_end_keeps_the_proxy_of_a_folder_back_at_enforce() {
    let fx = Fixture::new("enforce");
    let sandbox = fx.sandbox().await;
    let address = sandbox.network().unwrap().address;
    let session = sandbox.session_proxy_authorization().unwrap();
    prepared_token(&fx, &sandbox, "only");
    fx.set_mode("observe");
    sandbox
        .sync_network()
        .await
        .expect("the proxy keeps running for the enforce call");
    assert!(sandbox.network().is_some());
    fx.set_mode("enforce");
    sandbox
        .sync_network()
        .await
        .expect("the folder is back at enforce");
    assert_eq!(SandboxMode::Enforce, sandbox.published_mode());

    sandbox.release_call(&CallId::tool("only"));
    assert_eq!(
        0,
        sandbox.proxy_resyncs(),
        "under enforce the proxy is the folder's: nothing to sync"
    );
    assert!(sandbox.network().is_some(), "kept");
    assert_eq!(
        Some(&serde_json::json!("proxy")),
        sandbox.status_json().get("network")
    );
    assert!(proxy_answers_at(address, &session).await);
    sandbox
        .sync_network()
        .await
        .expect("the proxy keeps running")
        .expect("enforce keeps one");
    assert!(sandbox.network().is_some());
    sandbox.stop_network().await;
}

/// A call ended with no runtime current — a release guard dropped as a runtime shuts down —
/// schedules nothing and panics nowhere: the proxy waits for the next sync, which stops it.
#[test]
fn a_call_ended_with_no_runtime_current_leaves_the_proxy_to_the_next_sync() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let fx = Fixture::new("enforce");
    let sandbox = runtime.block_on(async {
        let sandbox = fx.sandbox().await;
        prepared_token(&fx, &sandbox, "only");
        fx.set_mode("observe");
        sandbox
            .sync_network()
            .await
            .expect("the proxy keeps running for the enforce call");
        sandbox
    });
    assert!(tokio::runtime::Handle::try_current().is_err());

    sandbox.release_call(&CallId::tool("only"));
    assert_eq!(0, sandbox.proxy_resyncs(), "no runtime to run a sync on");
    assert!(
        sandbox.network().is_some(),
        "the proxy waits for the next sync"
    );
    runtime.block_on(async {
        assert!(
            matches!(sandbox.sync_network().await, Ok(None)),
            "the next sync stops it"
        );
        assert!(sandbox.network().is_none());
        sandbox.stop_network().await;
    });
}

/// The proxy decides a connection carrying no call's credential — the session's — under the
/// mode the sandbox last resolved, from memory: a burst of connections adds no stat of either
/// `workspaced.toml` — the daemon's own reads (before each command, on each sync) are what
/// refresh it — and a flip made by editing the file reaches the proxy with the next sync, which
/// keeps the proxy for the call still running under `enforce`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_proxy_reads_the_mode_from_memory_and_the_sync_is_what_refreshes_it() {
    let fx = Fixture::new("enforce");
    let sandbox = fx.sandbox().await;
    let address = sandbox.network().unwrap().address;
    let (_token, _) = prepared_token(&fx, &sandbox, "call-burst");
    let stamps = sandbox.mode_stamps();
    assert!(stamps > 0, "the prepare resolved the mode");
    for n in 0..5 {
        assert_eq!(
            407,
            connect_via(address, "", &format!("h{n}.burst.example:443")).await
        );
    }
    assert_eq!(
        stamps,
        sandbox.mode_stamps(),
        "five decisions stat-ed the mode's files"
    );

    // Edited on disk: the proxy still decides under `enforce` until the daemon's next sync...
    fx.set_mode("observe");
    assert_eq!(407, connect_via(address, "", "edited.example:443").await);
    assert_eq!(stamps, sandbox.mode_stamps());
    assert_eq!(SandboxMode::Enforce, sandbox.published_mode());
    // ...which reads the files once and hands the proxy `observe`, kept for the enforce call
    sandbox
        .sync_network()
        .await
        .expect("the proxy keeps running for the call");
    assert_eq!(stamps + 1, sandbox.mode_stamps());
    assert_eq!(SandboxMode::Observe, sandbox.published_mode());
    assert!(sandbox.network().is_some());
    assert_eq!(
        407,
        connect_via(address, "", "edited.example:443").await,
        "an enforce call still runs: nothing unauthenticated passes"
    );
    assert_eq!(
        stamps + 1,
        sandbox.mode_stamps(),
        "without a read of the folder"
    );
    sandbox.stop_network().await;
}

/// Every spelling of the proxy pointer `EnvPolicy::proxy_vars_at` writes.
const PROXY_VARS: [&str; 8] = [
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
    "no_proxy",
];

/// The proxy pointer variables set on `cmd`, by name.
fn proxy_vars_set(cmd: &tokio::process::Command) -> Vec<&'static str> {
    PROXY_VARS
        .iter()
        .copied()
        .filter(|name| env_of(cmd, name).is_some())
        .collect()
}

/// The terminal's own spawns — the login shell, the state snapshot, the static shell, the
/// `.envrc` evaluator — are pointed at no proxy under either mode: no `HTTP_PROXY` of any
/// spelling, so nothing of theirs reaches the port to be answered `407`. A tool call in the
/// same folder gets the pointer with its own credential under `enforce` only: under `observe`
/// it is pointed at nothing even while a proxy listens (observe watches the file system only).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn init_shell_spawns_get_no_proxy_pointer_while_tool_calls_get_a_credentialed_one() {
    for mode in ["enforce", "observe"] {
        let fx = Fixture::new(mode);
        let sandbox = fx.sandbox().await;
        for label in ["login-env", "shell-state", "static-shell", "envrc-source"] {
            let (mut cmd, original) = fx.command();
            sandbox
                .prepare(&mut cmd, &original, &CallId::shell_init(label))
                .unwrap()
                .expect("wrapped under an active mode");
            let set = proxy_vars_set(&cmd);
            assert!(
                set.is_empty(),
                "{mode}/{label} was pointed at a proxy: {set:?}"
            );
        }
        if mode == "observe" {
            let (mut cmd, original) = fx.command();
            sandbox
                .prepare(&mut cmd, &original, &CallId::tool("tool-call"))
                .unwrap()
                .expect("runs bare under observe");
            let set = proxy_vars_set(&cmd);
            assert!(
                set.is_empty(),
                "observe pointed a tool call at a proxy: {set:?}"
            );
        } else {
            let (token, proxied_at) = prepared_token(&fx, &sandbox, "tool-call");
            assert!(!token.is_empty());
            assert_eq!(sandbox.network().unwrap().address, proxied_at);
        }
        sandbox.stop_network().await;
    }
}

/// A tool call spawned while the folder's proxy handle is already taken (the stop is under way)
/// finds no running proxy: the one reading that builds its policy and mints its pointer sees
/// none, so under `enforce` the kernel policy has no network and the command gets no pointer of
/// any spelling — never a bare one at a port nobody answers on — and it is remembered and runs.
/// With the proxy back, the next spawn is pointed at it with a credential.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_finding_the_proxy_stopping_gets_no_network_and_no_pointer_under_enforce() {
    let fx = Fixture::new("enforce");
    let sandbox = fx.sandbox().await;
    let proxy = sandbox.take_proxy_for_test().expect("the proxy is running");
    let call = CallId::tool("no-proxy");
    let (mut cmd, original) = fx.command();
    sandbox
        .prepare(&mut cmd, &original, &call)
        .unwrap()
        .expect("wrapped under enforce");
    let set = proxy_vars_set(&cmd);
    assert!(set.is_empty(), "pointed at a stopping proxy: {set:?}");
    assert_eq!(
        NetworkPolicy::Off,
        sandbox
            .calls
            .lock()
            .prepared_record(&call)
            .expect("remembered")
            .policy
            .network,
        "no proxy, no network"
    );
    assert_eq!(1, sandbox.open_calls());
    sandbox.restore_proxy_for_test(proxy);
    let (token, _) = prepared_token(&fx, &sandbox, "after-the-stop");
    assert!(!token.is_empty());
    sandbox.stop_network().await;
}

/// A backend whose wrap refuses every command.
struct RefusingWrap;

impl SandboxBackend for RefusingWrap {
    fn name(&self) -> BackendName {
        BackendName::Seatbelt
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities::default()
    }

    fn wrap(
        &self,
        _cmd: &mut tokio::process::Command,
        _original: &OriginalArgv,
        _policy: &SandboxPolicy,
        _tag: &CommandTag,
    ) -> Result<WrapReceipt, SandboxCommandError> {
        Err(SandboxCommandError::Unavailable {
            reason: "refused for the test".to_owned(),
        })
    }
}

/// A spawn whose wrap fails after its credential was minted leaves no credential behind: the
/// mint is kept only by the spawn's record, and a refused spawn has none, so the token its
/// environment was handed is a `407` at the proxy — nothing can present it as the call's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refused_wrap_revokes_the_credential_minted_for_the_spawn() {
    let fx = Fixture::new("enforce");
    let sandbox = fx.sandbox_with(Box::new(RefusingWrap)).await;
    let address = sandbox.network().unwrap().address;
    let call = CallId::tool("refused-wrap");
    let (mut cmd, original) = fx.command();
    let refused = sandbox
        .prepare(&mut cmd, &original, &call)
        .expect_err("the wrap refuses");
    assert!(
        refused.to_string().contains("refused for the test"),
        "{refused}"
    );
    assert_eq!(0, sandbox.open_calls(), "nothing was remembered for it");
    let (token, _) = credential_of(&env_of(&cmd, "HTTP_PROXY").expect("the pointer was written"));
    assert_eq!(
        407,
        stranger_status(address, &token).await,
        "the credential went with the refused spawn"
    );
    let proxy = sandbox.take_proxy_for_test().expect("the proxy is running");
    assert!(
        !proxy.revoke_call_credential(&CommandTag::for_call(&call)),
        "the proxy still held a credential for the refused spawn"
    );
    sandbox.restore_proxy_for_test(proxy);
    sandbox.stop_network().await;
}

/// A folder's stop racing a spawn under `enforce` — the proxy was there when the policy was
/// built, gone when the credential was to be minted (a flip to `off`, an unserve) — leaves the
/// command with no pointer of any spelling, never a bare one, and the spawn runs; what the mint
/// produced on the side that went is revoked with the spawn's guard. With the proxy back, the
/// next spawn gets every pointer together with its credential, in one step.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_racing_the_proxys_stop_gets_no_pointer_rather_than_a_bare_one() {
    let mode = "enforce";
    let fx = Fixture::new(mode);
    let sandbox = fx.sandbox().await;
    let taken = Arc::new(parking_lot::Mutex::new(None));
    sandbox.stage_before_next_mint_for_test({
        let taken = taken.clone();
        move |sandbox| *taken.lock() = sandbox.network.swap(None)
    });
    let racing = CallId::tool("racing-the-stop");
    let (mut cmd, original) = fx.command();
    sandbox
        .prepare(&mut cmd, &original, &racing)
        .unwrap()
        .expect("wrapped under an active mode");
    let set = proxy_vars_set(&cmd);
    assert!(
        set.is_empty(),
        "{mode}: pointed bare at a stopping proxy: {set:?}"
    );
    let side = taken
        .lock()
        .take()
        .expect("the stop ran between build and mint");
    sandbox.network.store(Some(side));
    let proxy = sandbox.take_proxy_for_test().expect("the proxy is running");
    assert!(
        !proxy.revoke_call_credential(&CommandTag::for_call(&racing)),
        "{mode}: the credential minted on the side that went was kept"
    );
    sandbox.restore_proxy_for_test(proxy);

    let (mut cmd, original) = fx.command();
    sandbox
        .prepare(&mut cmd, &original, &CallId::tool("after-the-race"))
        .unwrap()
        .expect("wrapped under an active mode");
    assert_eq!(
        PROXY_VARS.to_vec(),
        proxy_vars_set(&cmd),
        "{mode}: every spelling"
    );
    let (token, at) = credential_of(&env_of(&cmd, "HTTP_PROXY").unwrap());
    assert!(!token.is_empty());
    assert_eq!(sandbox.network().unwrap().address, at);
    for name in [
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        assert_eq!(
            env_of(&cmd, "HTTP_PROXY"),
            env_of(&cmd, name),
            "{mode}: {name} carries the same credential"
        );
    }
    sandbox.stop_network().await;
}

/// The policy build points the kernel at the port of the running proxy it was handed and writes
/// no pointer: the pointer is `point_at_proxy`'s to write, with the credential minted on that
/// same side, so no path can hand out the port bare. Handed no side, it opens no network.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn policy_build_carries_the_proxy_port_and_no_pointer() {
    let fx = Fixture::new("enforce");
    let sandbox = fx.sandbox().await;
    let port = sandbox.network().unwrap().address.port();
    let side = sandbox.running_network_side().expect("the proxy runs");
    let engaged = sandbox.engage().await;
    let policy = sandbox.build_policy(engaged, &[], Some(&side)).unwrap();
    assert_eq!(NetworkPolicy::Proxy { port }, policy.network);
    assert!(policy.env.set.is_empty(), "{:?}", policy.env.set);
    let policy = sandbox.build_policy(engaged, &[], None).unwrap();
    assert_eq!(NetworkPolicy::Off, policy.network);
    assert!(policy.env.set.is_empty(), "{:?}", policy.env.set);
    sandbox.stop_network().await;
}

/// A background start keeps its token but not its "allow once" rows: before the detach the
/// call-scoped row answers the command's next connection to that host; after it, the same token
/// at the same host is refused (no owner is left to ask, and the row is gone), while a host the
/// workspace allows still answers it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_detached_call_loses_its_once_rows_and_keeps_the_workspace_ones() {
    let fx = Fixture::new("enforce");
    let sandbox = fx.sandbox().await;
    let address = sandbox.network().unwrap().address;
    sandbox
        .record_grant(
            &CallId::tool("c-grant"),
            None,
            "sess-2",
            Grant {
                scope: GrantScope::Workspace {
                    root: fx.root.clone(),
                },
                ..net_grant("shared.example", GrantDecision::Allow)
            },
        )
        .await
        .unwrap();
    let scripted = ScriptedOwner::new([serde_json::json!({
        "outcome": "approve",
        "tool_call_id": "call-bg",
        "scope": { "kind": "net_host" },
        "duration": { "kind": "workspace" },
    })]);
    let call = CallId::tool("call-bg");
    sandbox
        .bind_call(
            &call,
            owner("sess-2", ToolApprovalPolicy::AlwaysPrompt, &scripted),
        )
        .unwrap();
    let (mut cmd, original) = fx.command();
    sandbox.prepare(&mut cmd, &original, &call).unwrap();
    let (token, _) = credential_of(&env_of(&cmd, "HTTP_PROXY").unwrap());
    let bearer = format!("Bearer {token}");

    // The card's workspace answer is clamped to the call under `always_prompt`: a once row
    assert_eq!(502, connect_via(address, &bearer, "once.example:443").await);
    assert_eq!(502, connect_via(address, &bearer, "once.example:443").await);
    assert_eq!(
        1,
        scripted.seen.lock().len(),
        "the once row covered the retry"
    );

    sandbox.detach_call(&call, "sess-2");
    assert_eq!(
        403,
        connect_via(address, &bearer, "once.example:443").await,
        "the once row went with the owner"
    );
    assert_eq!(1, scripted.seen.lock().len(), "nobody left to ask, no card");
    assert_eq!(
        502,
        connect_via(address, &bearer, "shared.example:443").await,
        "the workspace row still answers the token"
    );
    sandbox.stop_network().await;
}

/// A spawn that lands after the call's final result — the stream ended and the hub released the
/// call, then the tool's retry spawned anyway — is refused rather than re-creating the call as a
/// stray with a live credential: nothing is remembered for it, and the credential minted for the
/// late spawn is revoked with it. The hub's next dispatch under the same id starts afresh.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn late_spawn_for_a_released_call_is_refused_and_leaves_no_credential() {
    let fx = Fixture::new("enforce");
    let sandbox = fx.sandbox().await;
    let address = sandbox.network().unwrap().address;
    let call = CallId::tool("call-late");
    sandbox.bind_call(&call, cardless_owner()).unwrap();
    let (first_token, _) = prepared_token(&fx, &sandbox, "call-late");
    sandbox.release_call(&call);
    assert_eq!(407, stranger_status(address, &first_token).await);

    let (mut cmd, original) = fx.command();
    let refused = sandbox
        .prepare(&mut cmd, &original, &call)
        .expect_err("the call is over");
    assert!(
        matches!(
            refused.downcast_ref::<WorkspaceSandboxError>(),
            Some(WorkspaceSandboxError::CallReleased)
        ),
        "{refused}"
    );
    assert_eq!(0, sandbox.open_calls(), "nothing was re-created for it");
    let (late_token, _) =
        credential_of(&env_of(&cmd, "HTTP_PROXY").expect("the pointer was written"));
    assert_eq!(
        407,
        stranger_status(address, &late_token).await,
        "the late spawn's credential went with the refusal"
    );

    sandbox.bind_call(&call, cardless_owner()).unwrap();
    let (again, _) = prepared_token(&fx, &sandbox, "call-late");
    assert_eq!(
        403,
        stranger_status(address, &again).await,
        "a new call under the id runs"
    );
    sandbox.stop_network().await;
}

/// An allow answered on a card is judged again as it lands: a deny row that reached the folder
/// while the card was up (another session's "deny for this workspace") refuses the connection
/// instead of the allow letting it through — the grant is not recorded
/// (`WorkspaceSandboxError::DeniedByRow`) — and the command's result says so.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn allow_answered_after_a_deny_row_landed_is_applied_as_a_deny_and_noted() {
    let fx = Fixture::new("enforce");
    let sandbox = fx.sandbox().await;
    let address = sandbox.network().unwrap().address;
    let mut decisions = sandbox.blocked_requests().unwrap();
    let owner = GatedOwner::new(serde_json::json!({
        "outcome": "approve",
        "tool_call_id": "call-rejudged",
        "scope": { "kind": "net_host" },
        "duration": { "kind": "call" },
    }));
    let call = CallId::tool("call-rejudged");
    sandbox
        .bind_call(
            &call,
            CallOwner {
                session_id: "sess-1".to_owned(),
                policy: ToolApprovalPolicy::GrantsAllowed,
                transport: Some(owner.clone() as Arc<dyn PermissionHookTransport>),
                command: None,
            },
        )
        .unwrap();
    let (token, _) = prepared_token(&fx, &sandbox, "call-rejudged");
    let bearer = format!("Bearer {token}");
    let client =
        tokio::spawn(async move { connect_via(address, &bearer, "late-deny.example:443").await });
    tokio::time::timeout(Duration::from_secs(5), owner.card.notified())
        .await
        .expect("the held connection raised a card");

    // Another session denies the host for the workspace while the card is up
    sandbox
        .record_grant(
            &CallId::tool("c-other"),
            None,
            "sess-2",
            Grant {
                scope: GrantScope::Workspace {
                    root: fx.root.clone(),
                },
                ..net_grant("late-deny.example", GrantDecision::Deny)
            },
        )
        .await
        .unwrap();
    assert!(
        !client.is_finished(),
        "the card is still up: a deny row releases nothing by itself"
    );

    owner.answers.add_permits(1);
    let status = tokio::time::timeout(Duration::from_secs(5), client)
        .await
        .expect("the answer settles the hold")
        .unwrap();
    assert_eq!(403, status, "the allow was applied as a deny");
    let decided = next_decision(&mut decisions).await;
    assert_eq!(DeciderOutcome::Denied, decided.decided, "{decided:?}");
    let notes = sandbox.take_network_denials(&call);
    assert!(
        notes
            .iter()
            .any(|note| note.contains("an \"Always reject\" row covers it")),
        "{notes:?}"
    );
    assert_eq!(
        403,
        connect_via(address, &format!("Bearer {token}"), "late-deny.example:443").await,
        "the deny row decides the next one, no card"
    );
    assert_eq!(1, owner.asked.load(Ordering::SeqCst));
    sandbox.stop_network().await;
}

/// The same allow, overtaken by the folder's lists instead of a row: a `disallowed_web_fetch_domains`
/// entry written to `permission.toml` while the card was up refuses the connection when the answer
/// lands (the decider re-judges it against the lists as they stand) — the call's allow was recorded,
/// the deny list outranks it, and the command's result says so.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn allow_answered_after_a_deny_list_entry_landed_is_applied_as_a_deny_and_noted() {
    let fx = Fixture::new("enforce");
    let sandbox = fx.sandbox().await;
    let address = sandbox.network().unwrap().address;
    let mut decisions = sandbox.blocked_requests().unwrap();
    let owner = GatedOwner::new(serde_json::json!({
        "outcome": "approve",
        "tool_call_id": "call-relisted",
        "scope": { "kind": "net_host" },
        "duration": { "kind": "call" },
    }));
    let call = CallId::tool("call-relisted");
    sandbox
        .bind_call(
            &call,
            CallOwner {
                session_id: "sess-1".to_owned(),
                policy: ToolApprovalPolicy::GrantsAllowed,
                transport: Some(owner.clone() as Arc<dyn PermissionHookTransport>),
                command: None,
            },
        )
        .unwrap();
    let (token, _) = prepared_token(&fx, &sandbox, "call-relisted");
    let bearer = format!("Bearer {token}");
    let client =
        tokio::spawn(async move { connect_via(address, &bearer, "late-list.example:443").await });
    tokio::time::timeout(Duration::from_secs(5), owner.card.notified())
        .await
        .expect("the held connection raised a card");

    // The folder's permission.toml disallows the host while the card is up
    fx.write_permission_toml("disallowed_web_fetch_domains = [\"late-list.example\"]\n");
    assert!(
        !client.is_finished(),
        "the card is still up: a list entry on disk releases nothing by itself"
    );

    owner.answers.add_permits(1);
    let status = tokio::time::timeout(Duration::from_secs(5), client)
        .await
        .expect("the answer settles the hold")
        .unwrap();
    assert_eq!(403, status, "the allow was applied as a deny");
    let decided = next_decision(&mut decisions).await;
    assert_eq!(DeciderOutcome::Denied, decided.decided, "{decided:?}");
    let notes = sandbox.take_network_denials(&call);
    assert!(
        notes
            .iter()
            .any(|note| note.contains("a deny rule added while the card was up covers it")),
        "{notes:?}"
    );
    assert_eq!(
        403,
        connect_via(address, &format!("Bearer {token}"), "late-list.example:443").await,
        "the list outranks the call's recorded allow on the next one: refused before anyone answers"
    );
    // A list refusal tells the user why, once, on a card nothing is parked under
    tokio::time::timeout(Duration::from_secs(5), owner.card.notified())
        .await
        .expect("the refusal posted its informational card");
    assert_eq!(2, owner.asked.load(Ordering::SeqCst));
    assert_eq!(
        DeciderOutcome::Denied,
        next_decision(&mut decisions).await.decided
    );
    sandbox.stop_network().await;
}

/// A hold whose card is up is released by a row another instance of the folder wrote to the
/// grants file: the reload that picks the row up (the daemon's read before its next command)
/// sweeps the parked holds under the rows and the web-fetch lists as they stand, so the
/// connection goes through with an allow and its card is withdrawn — nobody waits out a card
/// for a host the folder has since allowed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reloaded_rows_release_the_holds_they_now_cover_and_withdraw_their_cards() {
    let fx = Fixture::new("enforce");
    let sandbox = fx.sandbox().await;
    let address = sandbox.network().unwrap().address;
    let mut decisions = sandbox.blocked_requests().unwrap();
    let silent = Arc::new(SilentOwner::default());
    let call = CallId::tool("call-swept");
    sandbox
        .bind_call(
            &call,
            CallOwner {
                session_id: "sess-1".to_owned(),
                policy: ToolApprovalPolicy::GrantsAllowed,
                transport: Some(silent.clone() as Arc<dyn PermissionHookTransport>),
                command: None,
            },
        )
        .unwrap();
    let (token, _) = prepared_token(&fx, &sandbox, "call-swept");
    let bearer = format!("Bearer {token}");
    let client =
        tokio::spawn(async move { connect_via(address, &bearer, "swept.example:443").await });
    tokio::time::timeout(Duration::from_secs(5), silent.asked.notified())
        .await
        .expect("the held connection raised a card");

    // Another instance over the same folder allows the host for the workspace, on disk
    let other = fx.open().await;
    other
        .record_grant(
            &CallId::tool("c-other"),
            None,
            "sess-2",
            Grant {
                scope: GrantScope::Workspace {
                    root: fx.root.clone(),
                },
                ..net_grant("swept.example", GrantDecision::Allow)
            },
        )
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !client.is_finished(),
        "nothing read the file yet: still parked"
    );

    sandbox.refresh_grants().await;
    let status = tokio::time::timeout(Duration::from_secs(5), client)
        .await
        .expect("the reload releases the hold")
        .unwrap();
    assert_eq!(
        502, status,
        "allowed by the row read from disk, no upstream"
    );
    let decided = next_decision(&mut decisions).await;
    assert_eq!(DeciderOutcome::Allowed, decided.decided, "{decided:?}");
    let withdrawn = tokio::time::timeout(Duration::from_secs(5), async {
        while silent.waiting.load(Ordering::SeqCst) != 0 || silent.withdrawn.lock().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(
        withdrawn.is_ok(),
        "the released hold's card was not withdrawn"
    );
    assert_eq!(1, silent.withdrawn.lock().len());
    sandbox.stop_network().await;
}
