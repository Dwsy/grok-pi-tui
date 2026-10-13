use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use futures::StreamExt;
use serde_json::{Value, json};
use xai_grok_config_types::RemoteSettings;
use xai_grok_egress_proxy::EgressProxyOptions;
use xai_grok_sandbox::command::backend::{
    BackendCapabilities, CallId, CommandTag, OriginalArgv, RenderedPolicy, SandboxBackend,
    SandboxCommandError, WrapReceipt,
};
use xai_grok_sandbox::command::grants::{
    Expiry, FixedClock, Grant, GrantDecision, GrantId, GrantScope, GrantSubject, HostPattern,
};
use xai_grok_sandbox::command::mode::SandboxModeSource;
use xai_grok_sandbox::command::violation::{
    Blocked, CommandExit, Disposition, InformationalReason, Replay, Violation,
};
use xai_grok_sandbox::command::{
    BackendName, GitConfigEnv, SandboxMode, SandboxPolicy, WouldVerdict, WritableLocations,
    canonical_path,
};
use xai_grok_tools::sandbox_launch::SandboxLaunch;
use xai_grok_tools::types::output::{BackgroundTaskStarted, BashOutput, ToolOutput, ToolRunResult};
use xai_tool_runtime::{
    ToolApprovalPolicy, ToolError, ToolErrorKind, ToolProgress, ToolStream, ToolStreamItem,
};

use super::result_path::{AfterRun, PIN_LOST_TEXT, after_shell_run, run_shell_call_with_replay};
use super::{
    BackendSource, CallOwner, ENFORCE_UNAVAILABLE_TEXT, Finished, GrantError, WorkspaceSandbox,
    WorkspaceSandboxConfig, WorkspaceSandboxError,
};
use crate::handle::WorkspaceHandle;
use crate::permission::{
    PermissionHookTransport, SettleContext, ToolApprovalGate, ViolationSettlement, settle_violation,
};
use crate::sandbox_mode::SandboxModeInputs;

/// Wraps like a backend would: a fixed wrapper program in front of the original argv.
struct StubBackend {
    wraps: AtomicUsize,
    reduced: bool,
}

impl StubBackend {
    fn new() -> Arc<StubBackend> {
        Arc::new(StubBackend {
            wraps: AtomicUsize::new(0),
            reduced: false,
        })
    }
}

struct ArcBackend(Arc<StubBackend>);

impl SandboxBackend for ArcBackend {
    fn name(&self) -> BackendName {
        BackendName::Seatbelt
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            reduced: self.0.reduced,
        }
    }

    fn wrap(
        &self,
        cmd: &mut tokio::process::Command,
        original: &OriginalArgv,
        _policy: &SandboxPolicy,
        tag: &CommandTag,
    ) -> Result<WrapReceipt, SandboxCommandError> {
        self.0.wraps.fetch_add(1, Ordering::SeqCst);
        let mut argv: Vec<OsString> = vec![OsString::from(tag.as_ref())];
        argv.push(original.program.clone().into_os_string());
        argv.extend(original.args.iter().cloned());
        *cmd = tokio::process::Command::new("/stub/wrapper");
        cmd.args(&argv);
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
    backend: Arc<StubBackend>,
    clock: Arc<FixedClock>,
}

impl Fixture {
    fn new() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("ws");
        let grok_home = tmp.path().join("grok-home");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&grok_home).unwrap();
        Fixture {
            _tmp: tmp,
            root,
            grok_home,
            backend: StubBackend::new(),
            clock: Arc::new(FixedClock::at(1_700_000_000)),
        }
    }

    /// The workspace's `.grok/workspaced.toml` layer: it may only tighten the user's
    /// mode, so `enforce` lands and `off` is ignored.
    fn set_mode(&self, mode: &str) {
        let path = crate::sandbox_mode::workspace_config_path(&self.root);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        write_mode_file(&path, mode);
    }

    /// The user's `<grok_home>/workspaced.toml` layer: any mode, including `off`.
    fn set_user_mode(&self, mode: &str) {
        write_mode_file(
            &crate::sandbox_mode::user_config_path(&self.grok_home),
            mode,
        );
    }

    /// The organisation's `<grok_home>/managed_config.toml` layer, written as an MDM deploys
    /// it: the whole document, not only a mode.
    fn set_managed_document(&self, document: &str) {
        xai_grok_config::fs_atomic::write_atomically(
            &crate::sandbox_mode::managed_config_path(&self.grok_home),
            document,
            None,
        )
        .unwrap();
    }

    fn config(&self, backend: BackendSource) -> WorkspaceSandboxConfig {
        WorkspaceSandboxConfig {
            workspace_root: self.root.clone(),
            grok_home: self.grok_home.clone(),
            user_home: Some(self._tmp.path().join("home")),
            git_env: GitConfigEnv::default(),
            control_socket_dir: self.grok_home.join("daemon"),
            remote: None,
            backend,
            clock: self.clock.clone(),
        }
    }

    async fn open_with_backend(&self) -> WorkspaceSandbox {
        WorkspaceSandbox::open(self.config(BackendSource::Fixed(Some(Box::new(ArcBackend(
            self.backend.clone(),
        ))))))
        .await
    }

    async fn open_without_backend(&self) -> WorkspaceSandbox {
        WorkspaceSandbox::open(self.config(BackendSource::Fixed(None))).await
    }

    fn command(&self, script: &str) -> (tokio::process::Command, OriginalArgv) {
        let mut cmd = tokio::process::Command::new("/bin/bash");
        cmd.arg("-lc").arg(script).current_dir(&self.root);
        (
            cmd,
            OriginalArgv {
                program: PathBuf::from("/bin/bash"),
                args: vec![OsString::from("-lc"), OsString::from(script)],
                cwd: self.root.clone(),
            },
        )
    }
}

fn program_of(cmd: &tokio::process::Command) -> PathBuf {
    PathBuf::from(cmd.as_std().get_program())
}

/// Written as every real writer writes it — atomically, so a new inode each time and the mode
/// owner's stamp moves whatever the length or the clock tick.
fn write_mode_file(path: &Path, mode: &str) {
    xai_grok_config::fs_atomic::write_atomically(
        path,
        &format!("[sandbox]\nmode = \"{mode}\"\n"),
        None,
    )
    .unwrap();
}

fn grant(subject: GrantSubject, scope: GrantScope) -> Grant {
    Grant {
        id: GrantId::new(format!("g-{}", uuid::Uuid::now_v7())),
        subject,
        scope,
        expires: Expiry::Never,
        decision: GrantDecision::Allow,
        granted_at: 1_700_000_000,
        granted_by: "hub:test".to_owned(),
        via: None,
    }
}

const OUTSIDE_WRITE_STDERR: &[u8] = b"touch: /srv/grok-w0-test/out.txt: Operation not permitted\n";

/// A root outside every default writable root (the workspace, `/tmp`, `$TMPDIR`): the tempdir
/// the fixture lives in is itself writable by default, so grants and violations use this instead.
/// Grant roots need not exist.
fn outside_root() -> PathBuf {
    PathBuf::from("/srv/grok-w0-test/srv-like")
}

/// Everything `std::process::Command` exposes about a spawn, so two commands can be compared
/// field by field.
#[derive(Debug, PartialEq, Eq)]
struct SpawnShape {
    program: OsString,
    args: Vec<OsString>,
    envs: Vec<(OsString, Option<OsString>)>,
    cwd: Option<PathBuf>,
}

fn spawn_shape(cmd: &tokio::process::Command) -> SpawnShape {
    let std = cmd.as_std();
    SpawnShape {
        program: std.get_program().to_os_string(),
        args: std.get_args().map(OsStr::to_os_string).collect(),
        envs: std
            .get_envs()
            .map(|(k, v)| (k.to_os_string(), v.map(OsStr::to_os_string)))
            .collect(),
        cwd: std.get_current_dir().map(Path::to_path_buf),
    }
}

/// Under `off`, serving a folder and a shell call probe, scan, bind and open nothing (no session
/// dir). The first mode that is not `off`, by verb or file edit, engages once with an owner-only
/// session dir; a spawn before that runs unwrapped under `observe` and is refused under `enforce`.
#[tokio::test]
async fn off_engages_nothing_and_the_first_mode_that_is_not_engages_once() {
    type Flip = fn(&Fixture, &WorkspaceSandbox);
    let flips: [(SandboxMode, Flip); 2] = [
        (SandboxMode::Observe, |_, sandbox| {
            sandbox.set_workspace_mode(SandboxMode::Observe).unwrap();
        }),
        (SandboxMode::Enforce, |fx, _| fx.set_mode("enforce")),
    ];
    for (mode, flip) in flips {
        let fx = Fixture::new();
        let own = xai_grok_config::sessions_cwd_dir_in(&fx.grok_home, &fx.root.to_string_lossy());
        let sandbox = Arc::new(fx.open_with_backend().await);
        let handle = WorkspaceHandle::for_test_in_with_sandbox(
            &fx.root,
            sandbox.clone(),
            ToolApprovalGate::Off,
        );
        let session = handle.create_session("main").unwrap();
        let (_, original) = fx.command("echo hi");
        let shell_call = |call_id: &'static str, bound: usize| {
            let (sandbox, original) = (sandbox.clone(), original.clone());
            let dispatch = move || -> ToolStream<ToolRunResult> {
                assert_eq!(bound, sandbox.calls.lock().len(), "{call_id}: bound calls");
                let mut cmd = tokio::process::Command::new(&original.program);
                sandbox
                    .prepare(&mut cmd, &original, &CallId::tool(call_id))
                    .unwrap();
                Box::pin(async_stream::stream! {
                    yield ToolStreamItem::Terminal(Ok(bash_result(0, b"hi\n")));
                })
            };
            run_shell_call_with_replay(
                handle.clone(),
                session.clone(),
                call_id.to_owned(),
                None,
                false,
                dispatch,
            )
            .collect::<Vec<_>>()
        };
        let items = shell_call("off", 0).await;
        assert!(
            matches!(items.as_slice(), [ToolStreamItem::Terminal(Ok(_))]),
            "{mode:?}: {items:?}"
        );
        assert_eq!(0, sandbox.engagements(), "{mode:?}");
        assert!(!own.exists(), "{mode:?}: no grant store was opened");

        flip(&fx, &sandbox);
        let (mut cmd, original) = fx.command("echo hi");
        let early = sandbox.prepare(&mut cmd, &original, &CallId::tool("early"));
        match mode {
            SandboxMode::Enforce => assert!(
                matches!(&early, Err(error) if error.to_string().contains("still starting")),
                "{early:?}"
            ),
            SandboxMode::Observe | SandboxMode::Off => assert!(early.unwrap().is_none()),
        }
        assert_eq!(0, sandbox.engagements(), "{mode:?}: a spawn never engages");

        assert_eq!(mode, sandbox.engage_unless_off().await);
        assert_eq!(mode, sandbox.engage_unless_off().await);
        let items = shell_call("on", 1).await;
        assert!(
            matches!(items.as_slice(), [ToolStreamItem::Terminal(Ok(_))]),
            "{mode:?}: {items:?}"
        );
        assert_eq!(1, sandbox.engagements(), "{mode:?}: engaged exactly once");
        assert!(own.is_dir(), "{mode:?}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                0o700,
                std::fs::metadata(&own).unwrap().permissions().mode() & 0o777
            );
        }
    }
}

/// A session directory the sandbox cannot make safe — a link planted where it goes (never
/// followed), or a file in the way of its parent — refuses every command under `enforce` and says
/// why in `sandbox.status`; `observe` and `off` run on. Once the path is fixed, the next command
/// under `enforce` creates the directory and runs.
#[cfg(unix)]
#[tokio::test]
async fn a_session_directory_it_cannot_make_safe_refuses_every_command_under_enforce_only() {
    type Step = fn(&Path, &Path, &Path);
    let cases: [(&str, Step, Step, &str); 2] = [
        (
            "a link where it goes",
            |_, own, elsewhere| {
                std::fs::create_dir_all(own.parent().unwrap()).unwrap();
                std::os::unix::fs::symlink(elsewhere, own).unwrap();
            },
            |_, own, _| std::fs::remove_file(own).unwrap(),
            "is a link or a file, not a directory",
        ),
        (
            "a file in the way of its parent",
            |grok_home, _, _| std::fs::write(grok_home.join("sessions"), "").unwrap(),
            |grok_home, _, _| std::fs::remove_file(grok_home.join("sessions")).unwrap(),
            "could not be created",
        ),
    ];
    for (path, plant, fix, why) in cases {
        let fx = Fixture::new();
        let own = xai_grok_config::sessions_cwd_dir_in(&fx.grok_home, &fx.root.to_string_lossy());
        let elsewhere = fx._tmp.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::set_permissions(
            &elsewhere,
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();
        plant(&fx.grok_home, &own, &elsewhere);
        fx.set_user_mode("enforce");
        let sandbox = fx.open_with_backend().await;
        let status = sandbox.status_json();
        let reason = status
            .get("session_dir_unsafe")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(reason.contains(why), "{path}: {status}");

        let (mut cmd, original) = fx.command("echo hi");
        let error = sandbox
            .prepare(&mut cmd, &original, &CallId::tool("refused"))
            .unwrap_err()
            .to_string();
        assert!(
            error.starts_with(super::SESSION_DIR_UNSAFE_TEXT),
            "{path}: {error}"
        );
        assert!(error.contains(why), "{path}: {error}");
        assert_eq!(0, fx.backend.wraps.load(Ordering::SeqCst), "{path}");
        assert_eq!(
            0o755,
            std::os::unix::fs::PermissionsExt::mode(
                &std::fs::metadata(&elsewhere).unwrap().permissions()
            ) & 0o777,
            "{path}: nothing is changed through a link"
        );
        assert_eq!(0, std::fs::read_dir(&elsewhere).unwrap().count(), "{path}");

        for mode in ["observe", "off"] {
            fx.set_user_mode(mode);
            let (mut cmd, original) = fx.command("echo hi");
            let prepared = sandbox.prepare(&mut cmd, &original, &CallId::tool(mode));
            assert!(prepared.is_ok(), "{path}: {mode} runs on: {prepared:?}");
        }

        fix(&fx.grok_home, &own, &elsewhere);
        fx.set_user_mode("enforce");
        let (mut cmd, original) = fx.command("echo hi");
        let receipt = sandbox
            .prepare(&mut cmd, &original, &CallId::tool("healed"))
            .unwrap()
            .unwrap();
        assert!(receipt.sandboxed, "{path}");
        assert!(std::fs::symlink_metadata(&own).unwrap().is_dir(), "{path}");
        assert_eq!(
            Some(&Value::Null),
            sandbox.status_json().get("session_dir_unsafe"),
            "{path}"
        );
    }
}

/// Checking an unsafe session directory again creates and stats it with no lock held: the check
/// runs while another thread reads the answer, and the new one is kept once that read ends.
#[cfg(unix)]
#[tokio::test]
async fn the_session_directory_is_checked_again_outside_its_lock() {
    let fx = Fixture::new();
    let in_the_way = fx.grok_home.join("sessions");
    std::fs::write(&in_the_way, "").unwrap();
    let sandbox = fx.open_with_backend().await;
    let session_dir = &sandbox.engage().await.session_dir;
    assert!(session_dir.read().is_some());
    std::fs::remove_file(&in_the_way).unwrap();
    let own = xai_grok_config::sessions_cwd_dir_in(&fx.grok_home, &fx.root.to_string_lossy());

    std::thread::scope(|scope| {
        let held = session_dir.read();
        let check = scope.spawn(|| sandbox.session_dir_unsafe());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !own.is_dir() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let created_while_held = own.is_dir();
        drop(held);
        assert!(created_while_held, "the check waited for the lock");
        assert_eq!(None, check.join().unwrap());
    });
    assert_eq!(None, *session_dir.read());
}

#[tokio::test]
async fn off_mode_leaves_the_command_alone_and_records_nothing() {
    let fx = Fixture::new();
    fx.set_user_mode("off");
    let sandbox = fx.open_with_backend().await;
    let (mut cmd, original) = fx.command("echo hi");
    let (pristine, _) = fx.command("echo hi");
    cmd.env("TOOL_API_KEY", "kept");
    let mut pristine = pristine;
    pristine.env("TOOL_API_KEY", "kept");
    let receipt = sandbox
        .prepare(&mut cmd, &original, &CallId::tool("c1"))
        .unwrap();
    assert_eq!(None, receipt);
    assert_eq!(spawn_shape(&pristine), spawn_shape(&cmd));
    assert_eq!(Path::new("/bin/bash"), program_of(&cmd));
    assert_eq!(0, sandbox.open_calls());
    assert_eq!(0, fx.backend.wraps.load(Ordering::SeqCst));
    assert!(
        sandbox
            .finish(
                &CallId::tool("c1"),
                CommandExit::code(1),
                OUTSIDE_WRITE_STDERR
            )
            .await
            .violation()
            .is_none()
    );
    let status = sandbox.status_json();
    assert_eq!(Some(&json!("off")), status.get("mode"));
    assert_eq!(Some(&json!("user_config")), status.get("mode_source"));
}

#[tokio::test]
async fn no_sandbox_table_anywhere_resolves_to_off_and_the_hook_returns_none() {
    let fx = Fixture::new();
    let sandbox = fx.open_with_backend().await;
    assert_eq!(SandboxMode::Off, sandbox.mode());
    assert_eq!(
        Some(&json!("default")),
        sandbox.status_json().get("mode_source")
    );
    let (mut cmd, original) = fx.command("echo hi");
    let (pristine, _) = fx.command("echo hi");
    assert_eq!(
        None,
        sandbox
            .prepare(&mut cmd, &original, &CallId::tool("c1"))
            .unwrap()
    );
    assert_eq!(spawn_shape(&pristine), spawn_shape(&cmd));
    assert_eq!(0, sandbox.open_calls());
    assert_eq!(0, fx.backend.wraps.load(Ordering::SeqCst));
}

/// The organisation's managed config file is the fleet layer, read by the sandbox itself when
/// the config names no explicit `remote`: its `[sandbox] mode` outranks the user's file and the
/// folder's, so the folder runs `enforce` with source `remote` whatever the user file says, and
/// a `sandbox.mode.set off` answers the managed mode, never the requested one.
#[tokio::test]
async fn the_managed_config_is_the_fleet_layer_and_outranks_the_users_file() {
    let fx = Fixture::new();
    fx.set_user_mode("off");
    fx.set_managed_document("[sandbox]\nmode = \"enforce\"\n");
    let sandbox = fx.open_with_backend().await;
    assert_eq!(SandboxMode::Enforce, sandbox.mode());
    let status = sandbox.status_json();
    assert_eq!(Some(&json!("enforce")), status.get("mode"));
    assert_eq!(Some(&json!("remote")), status.get("mode_source"));

    let (_, resolved) = sandbox.set_workspace_mode(SandboxMode::Off).unwrap();
    assert_eq!(SandboxMode::Enforce, resolved.mode);
    assert_eq!(SandboxModeSource::Remote, resolved.source);
}

/// An explicit `remote` in the config is the fleet layer and the managed file is not read for
/// it: a config carrying `observe` opens `observe` over a managed file that says `enforce`.
#[tokio::test]
async fn an_explicit_remote_layer_in_the_config_outranks_the_managed_file() {
    let fx = Fixture::new();
    fx.set_managed_document("[sandbox]\nmode = \"enforce\"\n");
    let mut config = fx.config(BackendSource::Fixed(Some(Box::new(ArcBackend(
        fx.backend.clone(),
    )))));
    config.remote = Some(RemoteSettings {
        sandbox_mode: Some(SandboxMode::Observe),
        ..RemoteSettings::default()
    });
    let sandbox = WorkspaceSandbox::open(config).await;
    assert_eq!(SandboxMode::Observe, sandbox.mode());
    assert_eq!(
        Some(&json!("remote")),
        sandbox.status_json().get("mode_source")
    );
}

/// The managed file is stamped like the other two layers: a deploy that lands while the folder
/// is served is the mode of the next read, and its removal hands the mode back to the user's
/// file — each a resolution of its own, none in between.
#[tokio::test]
async fn a_change_to_the_managed_file_is_the_mode_of_the_next_read() {
    let fx = Fixture::new();
    fx.set_user_mode("observe");
    let sandbox = fx.open_with_backend().await;
    assert_eq!(SandboxMode::Observe, sandbox.mode());
    assert_eq!(
        Some(&json!("user_config")),
        sandbox.status_json().get("mode_source")
    );
    let resolutions = sandbox.mode_resolutions();
    assert_eq!(SandboxMode::Observe, sandbox.mode());
    assert_eq!(resolutions, sandbox.mode_resolutions());

    fx.set_managed_document("[sandbox]\nmode = \"enforce\"\n");
    assert_eq!(SandboxMode::Enforce, sandbox.mode());
    assert_eq!(
        Some(&json!("remote")),
        sandbox.status_json().get("mode_source")
    );
    assert_eq!(resolutions + 1, sandbox.mode_resolutions());

    std::fs::remove_file(crate::sandbox_mode::managed_config_path(&fx.grok_home)).unwrap();
    assert_eq!(SandboxMode::Observe, sandbox.mode());
    assert_eq!(
        Some(&json!("user_config")),
        sandbox.status_json().get("mode_source")
    );
    assert_eq!(resolutions + 2, sandbox.mode_resolutions());
}

/// The rollout switch on a host with no backend: a managed `enforce`, or a managed file that is
/// refused, runs `off` — the folder is exactly as it is with no sandbox: the command runs bare,
/// nothing is wrapped, no proxy variable is set — source `remote`, and `sandbox.status` says why
/// (`degraded`), where the user's own `enforce` is kept and refuses every command instead;
/// `sandbox.mode.set` answers the same degraded mode, never `enforce`.
#[tokio::test]
async fn a_managed_enforce_or_refusal_on_a_host_without_a_backend_runs_off_and_says_so() {
    let fx = Fixture::new();
    for document in ["[sandbox]\nmode = \"enforce\"\n", "not = [toml"] {
        fx.set_managed_document(document);
        let sandbox = fx.open_without_backend().await;
        assert_eq!(SandboxMode::Off, sandbox.mode(), "{document:?}");
        let status = sandbox.status_json();
        assert_eq!(Some(&json!("off")), status.get("mode"), "{document:?}");
        assert_eq!(Some(&json!("remote")), status.get("mode_source"));
        assert!(status.get("degraded").is_some(), "{status}");
        let (mut cmd, original) = fx.command("true");
        let receipt = sandbox
            .prepare(&mut cmd, &original, &CallId::tool("c1"))
            .unwrap();
        assert!(receipt.is_none(), "{document:?}: {receipt:?}");
        assert_eq!(Path::new("/bin/bash"), program_of(&cmd));
        assert!(
            cmd.as_std()
                .get_envs()
                .all(|(key, _)| !key.to_string_lossy().to_lowercase().contains("proxy")),
            "{document:?}"
        );
        assert_eq!(0, fx.backend.wraps.load(Ordering::SeqCst));
        let (_, resolved) = sandbox.set_workspace_mode(SandboxMode::Off).unwrap();
        assert_eq!(SandboxMode::Off, resolved.mode, "{document:?}");
        assert_eq!(SandboxModeSource::Remote, resolved.source);
        assert!(resolved.degraded.is_some());
    }

    std::fs::remove_file(crate::sandbox_mode::managed_config_path(&fx.grok_home)).unwrap();
    fx.set_user_mode("enforce");
    let sandbox = fx.open_without_backend().await;
    assert_eq!(SandboxMode::Enforce, sandbox.mode());
    let status = sandbox.status_json();
    assert_eq!(Some(&json!("user_config")), status.get("mode_source"));
    assert_eq!(Some(&Value::Null), status.get("degraded"));
}

/// A managed file that names no mode adds no layer: an absent file, no `[sandbox]` table, a
/// table without `mode`, and a value that is not a mode all read as `None`, so the user's file
/// decides; the three modes read as themselves, through the same bounded reader as the other
/// layers. The path to it may bend only to where no command writes: the file linked into the
/// user's dotfiles reads, into the workspace it is refused, and a grok home that is itself a
/// symlink is refused whatever it points at, as every daemon-owned file under it is (the grants
/// that say where commands may write cannot be read through it). A file that is there but
/// cannot be read — not TOML, or through a refused link — is refused, never absent: over a
/// user `off` the folder resolves `enforce`, source
/// `remote`, `degraded: config_refused` (`off` with no backend: a refusal enforces nothing
/// there), so a planted or broken managed file cannot fail the mode open.
#[test]
fn a_managed_config_without_a_mode_adds_no_layer() {
    let fx = Fixture::new();
    let writable = WritableLocations::default();
    let inputs = |backend_available| SandboxModeInputs {
        workspace_root: &fx.root,
        grok_home: &fx.grok_home,
        env: None,
        remote: None,
        backend_available,
        writable: &writable,
    };
    let read_at = |grok_home: &Path| {
        crate::sandbox_mode::managed_mode_layer(&SandboxModeInputs {
            grok_home,
            ..inputs(false)
        })
    };
    let read = || crate::sandbox_mode::managed_mode_layer(&inputs(false)).unwrap();
    assert_eq!(None, read(), "absent file");
    for document in [
        "",
        "[telemetry]\nenabled = false\n",
        "[sandbox]\nallowed_web_fetch_domains = []\n",
        "[sandbox]\nmode = \"loud\"\n",
        "[sandbox]\nmode = 3\n",
    ] {
        fx.set_managed_document(document);
        assert_eq!(None, read(), "{document:?}");
    }
    for (document, mode) in [
        ("[sandbox]\nmode = \"off\"\n", SandboxMode::Off),
        ("[sandbox]\nmode = \"observe\"\n", SandboxMode::Observe),
        ("[sandbox]\nmode = \"enforce\"\n", SandboxMode::Enforce),
    ] {
        fx.set_managed_document(document);
        assert_eq!(Some(mode), read(), "{document:?}");
    }

    let managed = crate::sandbox_mode::managed_config_path(&fx.grok_home);
    let dotfiles = fx._tmp.path().join("dotfiles");
    std::fs::create_dir_all(&dotfiles).unwrap();
    std::fs::copy(&managed, dotfiles.join("managed_config.toml")).unwrap();
    let planted = fx.root.join("planted");
    std::fs::create_dir_all(&planted).unwrap();
    std::fs::copy(&managed, planted.join("managed_config.toml")).unwrap();
    let linked_home = fx._tmp.path().join("linked-home");
    std::os::unix::fs::symlink(&dotfiles, &linked_home).unwrap();
    assert!(
        read_at(&linked_home).is_err(),
        "a linked grok home is refused: what its grants let commands write cannot be read through it"
    );
    let bent_home = fx._tmp.path().join("bent-home");
    std::os::unix::fs::symlink(&planted, &bent_home).unwrap();
    assert!(
        read_at(&bent_home).is_err(),
        "a grok home linked into the workspace is refused"
    );
    std::fs::remove_file(&managed).unwrap();
    std::os::unix::fs::symlink(dotfiles.join("managed_config.toml"), &managed).unwrap();
    assert_eq!(
        Some(SandboxMode::Enforce),
        read(),
        "the file linked into the dotfiles reads"
    );
    std::fs::remove_file(&managed).unwrap();
    std::os::unix::fs::symlink(planted.join("managed_config.toml"), &managed).unwrap();
    assert!(
        crate::sandbox_mode::managed_mode_layer(&inputs(false)).is_err(),
        "the file linked into the workspace is refused"
    );
    std::fs::remove_file(&managed).unwrap();

    fx.set_user_mode("off");
    fx.set_managed_document("not = [toml");
    assert!(
        crate::sandbox_mode::managed_mode_layer(&inputs(false)).is_err(),
        "a managed file that is not TOML is refused, not absent"
    );
    for (backend_available, mode) in [(true, "enforce"), (false, "off")] {
        let resolved = crate::sandbox_mode::resolve_sandbox_mode_in(inputs(backend_available));
        let resolved = serde_json::to_value(resolved).unwrap();
        assert_eq!(Some(&json!(mode)), resolved.get("mode"), "{resolved}");
        assert_eq!(Some(&json!("remote")), resolved.get("source"), "{resolved}");
        assert!(resolved.get("degraded").is_some(), "{resolved}");
    }
    fx.set_user_mode("enforce");
    assert_eq!(
        json!({"mode": "enforce", "source": "user_config"}),
        serde_json::to_value(crate::sandbox_mode::resolve_sandbox_mode_in(inputs(false))).unwrap(),
        "the user's own enforce decided; the refusal never lowers it"
    );
}

#[tokio::test]
async fn observe_mode_runs_unwrapped_and_never_decodes_stderr() {
    let fx = Fixture::new();
    fx.set_mode("observe");
    let sandbox = fx.open_with_backend().await;
    let (mut cmd, original) = fx.command("touch /srv/grok-w0-test/out.txt");
    let receipt = sandbox
        .prepare(&mut cmd, &original, &CallId::tool("c1"))
        .unwrap()
        .unwrap();
    assert!(!receipt.sandboxed);
    assert_eq!(Some(BackendName::Seatbelt), receipt.backend);
    assert_eq!(Path::new("/bin/bash"), program_of(&cmd));
    assert_eq!(0, fx.backend.wraps.load(Ordering::SeqCst));
    assert_eq!(1, sandbox.open_calls());
    let finished = sandbox
        .finish(
            &CallId::tool("c1"),
            CommandExit::code(1),
            OUTSIDE_WRITE_STDERR,
        )
        .await;
    assert!(
        finished.violation().is_none(),
        "observe never produces a card"
    );
    assert_eq!(0, sandbox.open_calls());
    assert!(sandbox.observe_summary().would_block.is_empty());
}

/// A shell call the hub let past its pre-run approval because the sandbox enforced keeps that
/// sandbox for its spawn when the folder's mode flips first: it never runs with neither the
/// approval nor an enforcing sandbox. A call dispatched after the flip runs under the new mode,
/// and a spawn nobody dispatched takes the folder's mode as it is at its spawn.
#[tokio::test]
async fn a_call_admitted_under_enforce_stays_enforced_through_a_mode_flip() {
    let fx = Fixture::new();
    fx.set_user_mode("enforce");
    let sandbox = fx.open_with_backend().await;
    assert_eq!(SandboxMode::Enforce, sandbox.mode());
    let admitted = CallId::tool("admitted");
    sandbox.pin_mode(&admitted, SandboxMode::Enforce).unwrap();
    fx.set_user_mode("observe");
    assert_eq!(SandboxMode::Observe, sandbox.mode());

    let (mut cmd, original) = fx.command("true");
    let receipt = sandbox
        .prepare(&mut cmd, &original, &admitted)
        .unwrap()
        .unwrap();
    assert!(receipt.sandboxed, "the admitted call is enforced");
    assert_eq!(1, fx.backend.wraps.load(Ordering::SeqCst));

    let later = CallId::tool("later");
    sandbox.pin_mode(&later, sandbox.mode()).unwrap();
    let (mut cmd, original) = fx.command("true");
    let receipt = sandbox
        .prepare(&mut cmd, &original, &later)
        .unwrap()
        .unwrap();
    assert!(
        !receipt.sandboxed,
        "a call dispatched after the flip runs under observe"
    );
    let (mut cmd, original) = fx.command("true");
    let receipt = sandbox
        .prepare(&mut cmd, &original, &CallId::tool("stray"))
        .unwrap()
        .unwrap();
    assert!(!receipt.sandboxed, "a stray spawn takes the folder's mode");
    assert_eq!(1, fx.backend.wraps.load(Ordering::SeqCst));
    assert_eq!(
        Some(SandboxMode::Observe),
        sandbox.held_mode(&CallId::tool("stray")),
        "and keeps it"
    );
}

/// The call's mode outlives the violation: the replay after the user's answer on the card is
/// enforced too, even when the folder's mode flipped while the card was up.
#[tokio::test]
async fn a_pinned_calls_replay_stays_enforced_through_a_mode_flip() {
    let fx = Fixture::new();
    fx.set_user_mode("enforce");
    let sandbox = fx.open_with_backend().await;
    let admitted = CallId::tool("admitted");
    sandbox.pin_mode(&admitted, SandboxMode::Enforce).unwrap();
    assert!(sandbox.bind_pinned_call(&admitted, owner()));
    let (mut cmd, original) = fx.command("touch /srv/grok-w0-test/out.txt");
    let receipt = sandbox
        .prepare(&mut cmd, &original, &admitted)
        .unwrap()
        .unwrap();
    assert!(receipt.sandboxed);
    let finished = sandbox
        .finish(&admitted, CommandExit::code(1), OUTSIDE_WRITE_STDERR)
        .await;
    assert!(finished.violation().is_some(), "{finished:?}");
    sandbox.mark_replay(
        &admitted,
        GrantSubject::FsWriteRoot {
            root: outside_root(),
        },
    );
    fx.set_user_mode("observe");
    assert_eq!(SandboxMode::Observe, sandbox.mode());

    let (mut cmd, original) = fx.command("touch /srv/grok-w0-test/out.txt");
    let receipt = sandbox
        .prepare(&mut cmd, &original, &admitted)
        .unwrap()
        .unwrap();
    assert!(receipt.sandboxed, "the replay keeps the pin");
    assert_eq!(2, fx.backend.wraps.load(Ordering::SeqCst));
}

/// Under `observe` the proxy's `Observed` decisions are the would-block rows, keyed by host and
/// aggregated with a count.
#[tokio::test]
async fn observe_mode_records_the_proxys_would_block_rows_and_finish_adds_nothing() {
    let fx = Fixture::new();
    fx.set_mode("observe");
    let sandbox = fx.open_with_backend().await;
    let (mut cmd, original) = fx.command("pip install x");
    sandbox
        .prepare(&mut cmd, &original, &CallId::tool("c1"))
        .unwrap();
    sandbox.observe_blocked(
        &Blocked::Net {
            host: Some("pypi.org".to_owned()),
            port: Some(443),
        },
        WouldVerdict::Ask,
    );
    assert!(
        sandbox
            .finish(
                &CallId::tool("c1"),
                CommandExit::code(1),
                OUTSIDE_WRITE_STDERR
            )
            .await
            .violation()
            .is_none()
    );
    let summary = sandbox.observe_summary_json();
    assert_eq!(
        Some(
            &json!([{ "kind": "net", "target": "pypi.org:443", "verdict": "ask", "count": 1, "last_unix": 1_700_000_000 }])
        ),
        summary.get("entries")
    );
    sandbox.observe_blocked(
        &Blocked::Net {
            host: Some("pypi.org".to_owned()),
            port: Some(443),
        },
        WouldVerdict::DenyRow,
    );
    let repeated = sandbox.observe_summary_json();
    assert_eq!(Some(&json!(2)), repeated.pointer("/entries/0/count"));
    assert_eq!(
        Some(&json!("deny_row")),
        repeated.pointer("/entries/0/verdict"),
        "the row reads as its latest verdict"
    );
    assert_eq!(Some(&json!(0)), summary.get("evicted"));
}

/// A developer's own `enforce` on a host with no backend refuses every command and shows in
/// `sandbox.status` undegraded; a refused layer shows `degraded: config_refused` on either host.
#[tokio::test]
async fn enforce_without_a_backend_refuses_every_command_with_the_documented_text() {
    let fx = Fixture::new();
    fx.set_mode("enforce");
    let sandbox = fx.open_without_backend().await;
    let (mut cmd, original) = fx.command("echo hi");
    let error = sandbox
        .prepare(&mut cmd, &original, &CallId::tool("c1"))
        .unwrap_err();
    assert_eq!(ENFORCE_UNAVAILABLE_TEXT, error.to_string());
    assert_eq!(0, sandbox.open_calls());
    assert_eq!(
        json!({
            "mode": "enforce",
            "mode_source": "workspace_config",
            "degraded": null,
            "backend": "none",
            "reduced_sandbox": false,
            "network": "off",
            "proxy": null,
            "open_calls": 0,
            "session_dir_unsafe": null,
        }),
        sandbox.status_json()
    );
    for (backend_available, mode) in [(true, "enforce"), (false, "off")] {
        let fx = Fixture::new();
        std::fs::create_dir_all(crate::sandbox_mode::user_config_path(&fx.grok_home)).unwrap();
        let sandbox = if backend_available {
            fx.open_with_backend().await
        } else {
            fx.open_without_backend().await
        };
        let status = sandbox.status_json();
        assert_eq!(Some(&json!(mode)), status.get("mode"), "{status}");
        let degraded = status.get("degraded");
        assert_eq!(Some(&json!("config_refused")), degraded, "{status}");
    }
}

#[tokio::test]
async fn enforce_wraps_and_finish_decodes_a_write_outside_the_policy() {
    let fx = Fixture::new();
    fx.set_mode("enforce");
    let sandbox = fx.open_with_backend().await;
    let (mut cmd, original) = fx.command("touch /srv/grok-w0-test/out.txt");
    let receipt = sandbox
        .prepare(&mut cmd, &original, &CallId::tool("c1"))
        .unwrap()
        .unwrap();
    assert!(receipt.sandboxed);
    assert_eq!(Path::new("/stub/wrapper"), program_of(&cmd));
    let args: Vec<OsString> = cmd.as_std().get_args().map(OsString::from).collect();
    let [tag, shell, ..] = args.as_slice() else {
        panic!("tag then the original argv, got {args:?}");
    };
    assert_eq!(OsStr::new("grok-c1"), tag);
    assert_eq!(OsStr::new("/bin/bash"), shell);

    let finished = sandbox
        .finish(
            &CallId::tool("c1"),
            CommandExit::code(1),
            OUTSIDE_WRITE_STDERR,
        )
        .await
        .violation()
        .cloned()
        .expect("a write outside every writable root is a violation");
    assert_eq!(SandboxMode::Enforce, finished.mode);
    assert_eq!(Some(BackendName::Seatbelt), finished.backend);
    assert!(!finished.after_replay());
    assert_eq!(
        Blocked::FsWrite {
            path: PathBuf::from("/srv/grok-w0-test/out.txt"),
        },
        finished.violation.blocked
    );
    // The script named the path, so the card offers it (`/srv/<x>` is too
    // broad a directory, so the one file)
    assert_eq!(
        Some(GrantSubject::FsWriteRoot {
            root: PathBuf::from("/srv/grok-w0-test/out.txt"),
        }),
        finished.violation.proposed
    );
    assert_eq!(Disposition::Grantable, finished.violation.disposition);
    assert_eq!(0, sandbox.open_calls(), "finish consumes the record");
}

/// The own-targets rule through the daemon: the call's own argv reaches the decoder, so the same
/// denial from a script that never named the path is informational (`unattributed`) — no grant
/// offered, the user still told.
#[tokio::test]
async fn finish_attributes_the_denial_to_what_the_script_named() {
    let fx = Fixture::new();
    fx.set_mode("enforce");
    let sandbox = fx.open_with_backend().await;
    let (mut cmd, original) = fx.command("cargo test -p xai-grok-sandbox");
    sandbox
        .prepare(&mut cmd, &original, &CallId::tool("c1"))
        .unwrap()
        .unwrap();
    let finished = sandbox
        .finish(
            &CallId::tool("c1"),
            CommandExit::code(101),
            OUTSIDE_WRITE_STDERR,
        )
        .await
        .violation()
        .cloned()
        .expect("the denial is still reported");
    assert_eq!(None, finished.violation.proposed);
    assert_eq!(
        Disposition::informational(InformationalReason::Unattributed),
        finished.violation.disposition
    );
    assert!(finished.violation.produces_card());
}

/// A base in the environment the command runs under attributes a denial beneath it even when
/// `cmd` never sets it: here only the replaying shell's snapshot exports `PYTHONUSERBASE`.
#[tokio::test]
async fn a_base_only_the_replaying_shell_exports_attributes_the_denial() {
    let fx = Fixture::new();
    fx.set_mode("enforce");
    let sandbox = fx.open_with_backend().await;
    let exported = [(
        OsString::from("PYTHONUSERBASE"),
        OsString::from("/srv/grok-w0-test"),
    )];
    for (call, restored, expected) in [
        (
            "unnamed",
            &[][..],
            Disposition::informational(InformationalReason::Unattributed),
        ),
        ("exported", &exported[..], Disposition::Grantable),
    ] {
        let (mut cmd, original) = fx.command("pip install --user x");
        cmd.env_remove("PYTHONUSERBASE").env_remove("XDG_DATA_HOME");
        sandbox
            .prepare_restoring(&mut cmd, &original, restored, &CallId::tool(call))
            .unwrap()
            .unwrap();
        let finished = sandbox
            .finish(
                &CallId::tool(call),
                CommandExit::code(1),
                OUTSIDE_WRITE_STDERR,
            )
            .await
            .violation()
            .cloned()
            .expect("the denial is reported");
        assert_eq!(expected, finished.violation.disposition, "{call}");
    }
}

#[tokio::test]
async fn enforce_exit_zero_and_writes_inside_the_workspace_are_not_violations() {
    let fx = Fixture::new();
    fx.set_mode("enforce");
    let sandbox = fx.open_with_backend().await;
    let (mut cmd, original) = fx.command("echo");
    sandbox
        .prepare(&mut cmd, &original, &CallId::tool("ok"))
        .unwrap();
    assert!(
        sandbox
            .finish(
                &CallId::tool("ok"),
                CommandExit::code(0),
                OUTSIDE_WRITE_STDERR
            )
            .await
            .violation()
            .is_none()
    );
    let (mut cmd, original) = fx.command("touch inside");
    sandbox
        .prepare(&mut cmd, &original, &CallId::tool("inside"))
        .unwrap();
    let inside = format!(
        "touch: cannot touch '{}': Permission denied\n",
        fx.root.join("inside").display()
    );
    assert!(
        sandbox
            .finish(
                &CallId::tool("inside"),
                CommandExit::code(1),
                inside.as_bytes()
            )
            .await
            .violation()
            .is_none(),
        "the policy allows the workspace; the OS refused, not the sandbox"
    );
}

#[tokio::test]
async fn a_call_scoped_grant_applies_to_the_next_spawn_of_that_call_only() {
    let fx = Fixture::new();
    fx.set_mode("enforce");
    let sandbox = fx.open_with_backend().await;
    let outside = outside_root();
    bind(&sandbox, &CallId::tool("c1"));
    sandbox
        .record_grant(
            &CallId::tool("c1"),
            None,
            "s-1",
            grant(
                GrantSubject::FsWriteRoot {
                    root: outside.clone(),
                },
                GrantScope::Call,
            ),
        )
        .await
        .unwrap();
    assert!(
        sandbox.live_grants().await.is_empty(),
        "call grants never reach the store"
    );
    sandbox.mark_replay(
        &CallId::tool("c1"),
        GrantSubject::FsWriteRoot {
            root: outside.clone(),
        },
    );

    let stderr = format!(
        "touch: {}: Operation not permitted\n",
        outside.join("x").display()
    );
    let (mut cmd, original) = fx.command("touch x");
    sandbox
        .prepare(&mut cmd, &original, &CallId::tool("c1"))
        .unwrap();
    let finished = sandbox
        .finish(&CallId::tool("c1"), CommandExit::code(1), stderr.as_bytes())
        .await;
    assert!(
        finished.violation().is_none(),
        "the replay's policy includes the call grant: {finished:?}"
    );
    // The replay was still stopped on the granted target: the backend's limitation is named for
    // the model, not carded and not left as the OS's error
    let Finished::RefusedUnderGrant(refused) = &finished else {
        panic!("a replay stopped on its granted target is refused under the grant: {finished:?}");
    };
    assert_eq!(
        Blocked::FsWrite {
            path: outside.join("x"),
        },
        refused.blocked
    );
    let text = refused.text(None);
    assert!(
        text.starts_with("sandbox: refused again under the grant: ")
            && text.contains("is allowed by the policy but")
            && text.contains("Do not retry the same command"),
        "{text}"
    );

    // The same stderr on a fresh spawn of another call is a violation again, and one of the same
    // call is now a first run (replay consumed)
    let (mut cmd, original) = fx.command("touch x");
    sandbox
        .prepare(&mut cmd, &original, &CallId::tool("c2"))
        .unwrap();
    let finished = sandbox
        .finish(&CallId::tool("c2"), CommandExit::code(1), stderr.as_bytes())
        .await;
    assert!(
        !matches!(finished, Finished::RefusedUnderGrant(_)),
        "c2 is a first run"
    );
    let Finished::Violation(finished) = finished else {
        panic!("no grant for c2: {finished:?}");
    };
    assert!(!finished.after_replay());
    let (mut cmd, original) = fx.command("touch x");
    sandbox
        .prepare(&mut cmd, &original, &CallId::tool("c1"))
        .unwrap();
    let finished = sandbox
        .finish(&CallId::tool("c1"), CommandExit::code(1), stderr.as_bytes())
        .await;
    assert!(
        !matches!(finished, Finished::RefusedUnderGrant(_)),
        "the replay budget is spent; this is a first run again"
    );
    let finished = finished
        .violation()
        .cloned()
        .expect("the call grant was consumed by the replay");
    assert!(!finished.after_replay());
}

/// A first run whose denial the policy already allows is the OS's refusal:
/// no violation and no `refused_under_grant` — that reading is the replay's alone.
#[tokio::test]
async fn a_first_run_stopped_on_an_allowed_target_is_not_refused_under_grant() {
    let fx = Fixture::new();
    fx.set_mode("enforce");
    let sandbox = fx.open_with_backend().await;
    let inside = format!(
        "touch: cannot touch '{}': Permission denied\n",
        fx.root.join("x").display()
    );
    let (mut cmd, original) = fx.command("touch x");
    sandbox
        .prepare(&mut cmd, &original, &CallId::tool("c1"))
        .unwrap();
    let finished = sandbox
        .finish(&CallId::tool("c1"), CommandExit::code(1), inside.as_bytes())
        .await;
    assert!(finished.violation().is_none(), "{finished:?}");
    assert!(
        !matches!(finished, Finished::RefusedUnderGrant(_)),
        "{finished:?}"
    );
}

#[tokio::test]
async fn replay_marks_the_next_finish_as_final() {
    let fx = Fixture::new();
    fx.set_mode("enforce");
    let sandbox = fx.open_with_backend().await;
    let granted = GrantSubject::FsWriteRoot {
        root: outside_root(),
    };
    bind(&sandbox, &CallId::tool("c1"));
    sandbox.mark_replay(&CallId::tool("c1"), granted.clone());
    let (mut cmd, original) = fx.command("touch /srv/grok-w0-test/out.txt");
    sandbox
        .prepare(&mut cmd, &original, &CallId::tool("c1"))
        .unwrap();
    let finished = sandbox
        .finish(
            &CallId::tool("c1"),
            CommandExit::code(1),
            OUTSIDE_WRITE_STDERR,
        )
        .await
        .violation()
        .cloned()
        .unwrap();
    assert!(finished.after_replay());
    assert_eq!(Some(granted), finished.replayed_under);
}

#[tokio::test]
async fn workspace_grants_persist_reload_and_revoke() {
    let fx = Fixture::new();
    fx.set_mode("enforce");
    let sandbox = fx.open_with_backend().await;
    let outside = outside_root();
    let mut row = grant(
        GrantSubject::FsWriteRoot {
            root: outside.clone(),
        },
        GrantScope::Workspace {
            root: fx.root.clone(),
        },
    );
    row.expires = Expiry::Ttl { seconds: 3600 };
    let id = sandbox
        .record_grant(&CallId::tool("c1"), None, "s-1", row)
        .await
        .unwrap();

    let grants = sandbox.grants_json().await;
    let Some([row]) = grants
        .get("grants")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
    else {
        panic!("one grant row, got {grants}");
    };
    assert_eq!(Some(&json!(id.as_ref())), row.get("id"));
    assert_eq!(Some(&json!("fs_write_root")), row.pointer("/subject/kind"));
    assert_eq!(Some(&json!("workspace")), row.pointer("/scope/kind"));
    assert_eq!(Some(&json!(1_700_000_000 + 3600)), row.get("expires_at"));

    // A second sandbox on the same folder sees the row (it is on disk)
    let again = fx.open_with_backend().await;
    assert_eq!(1, again.live_grants().await.len());

    // And a spawn of any call runs under it
    let stderr = format!(
        "touch: {}: Operation not permitted\n",
        outside.join("x").display()
    );
    let (mut cmd, original) = fx.command("touch x");
    again
        .prepare(&mut cmd, &original, &CallId::tool("c9"))
        .unwrap();
    assert!(
        again
            .finish(&CallId::tool("c9"), CommandExit::code(1), stderr.as_bytes())
            .await
            .violation()
            .is_none()
    );

    sandbox.revoke_grant(&id).await.unwrap();
    assert!(sandbox.live_grants().await.is_empty());
    // The other instance picks the revoke up from the file on its next read
    assert!(again.live_grants().await.is_empty());
    let (mut cmd, original) = fx.command("touch x");
    again
        .prepare(&mut cmd, &original, &CallId::tool("c10"))
        .unwrap();
    assert!(
        again
            .finish(
                &CallId::tool("c10"),
                CommandExit::code(1),
                stderr.as_bytes()
            )
            .await
            .violation()
            .is_some(),
        "revoked: the write is a violation again"
    );
    let error = sandbox.revoke_grant(&id).await.unwrap_err();
    assert!(
        matches!(error, WorkspaceSandboxError::Grant(_)),
        "{error:?}"
    );
    assert!(error.to_string().contains("no grant with id"), "{error}");
}

#[tokio::test]
async fn expired_rows_drop_out_of_the_policy() {
    let fx = Fixture::new();
    fx.set_mode("enforce");
    let sandbox = fx.open_with_backend().await;
    let outside = outside_root();
    let mut row = grant(
        GrantSubject::FsWriteRoot {
            root: outside.clone(),
        },
        GrantScope::Workspace {
            root: fx.root.clone(),
        },
    );
    row.expires = Expiry::Ttl { seconds: 60 };
    sandbox
        .record_grant(&CallId::tool("c1"), None, "s-1", row)
        .await
        .unwrap();
    fx.clock.advance(120);
    assert!(sandbox.live_grants().await.is_empty());
    sandbox.refresh_grants().await;
    let stderr = format!(
        "touch: {}: Operation not permitted\n",
        outside.join("x").display()
    );
    let (mut cmd, original) = fx.command("touch x");
    sandbox
        .prepare(&mut cmd, &original, &CallId::tool("c2"))
        .unwrap();
    assert!(
        sandbox
            .finish(&CallId::tool("c2"), CommandExit::code(1), stderr.as_bytes())
            .await
            .violation()
            .is_some()
    );
}

#[tokio::test]
async fn a_protected_grant_is_refused_by_the_store() {
    let fx = Fixture::new();
    fx.set_mode("enforce");
    let sandbox = fx.open_with_backend().await;
    let error = sandbox
        .record_grant(
            &CallId::tool("c1"),
            None,
            "s-1",
            grant(
                GrantSubject::FsWriteRoot {
                    root: fx.root.join(".git").join("hooks"),
                },
                GrantScope::Global,
            ),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("protected"), "{error}");
    assert!(sandbox.live_grants().await.is_empty());
}

/// A grant whose path is not UTF-8 is recorded in no scope, in memory or on disk: the grants file
/// and the Settings list hold UTF-8 only, and a lossy spelling names another folder. A row that
/// does not serialize is left out of the list, never sent as `null`.
#[cfg(unix)]
#[tokio::test]
async fn a_grant_whose_path_is_not_utf8_is_recorded_nowhere() {
    let fx = Fixture::new();
    fx.set_mode("enforce");
    let sandbox = fx.open_with_backend().await;
    let name: &OsStr = std::os::unix::ffi::OsStrExt::from_bytes(b"cache-\xff");
    let root = outside_root().join(name);
    let file = fx.grok_home.join("sandbox_grants.toml");
    let before = std::fs::read(&file).ok();
    let subjects = [
        GrantSubject::FsWriteRoot { root: root.clone() },
        GrantSubject::FsRead { root: root.clone() },
    ];
    let scopes = [
        GrantScope::Call,
        GrantScope::Session,
        GrantScope::Workspace {
            root: fx.root.clone(),
        },
        GrantScope::Global,
    ];
    for subject in &subjects {
        for scope in &scopes {
            let error = sandbox
                .record_grant(
                    &CallId::tool("c1"),
                    None,
                    "s-1",
                    grant(subject.clone(), scope.clone()),
                )
                .await
                .unwrap_err();
            assert!(
                matches!(error, WorkspaceSandboxError::SubjectNotUtf8 { .. }),
                "{scope:?}: {error:?}"
            );
            assert!(error.to_string().contains("cache-\u{fffd}"), "{error}");
        }
    }
    assert!(sandbox.live_grants().await.is_empty());
    assert_eq!(Some(&json!([])), sandbox.grants_json().await.get("grants"));
    assert_eq!(before, std::fs::read(&file).ok(), "nothing is written");

    let rows = super::grants_to_json(&[grant(GrantSubject::FsRead { root }, GrantScope::Global)]);
    assert!(rows.is_empty(), "{rows:?}");
}

/// The folder owner resolves the mode once and every reader (the spawn path,
/// the proxy's decider, `sandbox.status`) gets that; a write to either `workspaced.toml` is
/// resolved on the next read, and `sandbox.mode.set` resolves afresh whatever the files say.
#[tokio::test]
async fn the_mode_is_resolved_once_until_a_layer_file_moves() {
    let fx = Fixture::new();
    fx.set_user_mode("observe");
    let sandbox = fx.open_with_backend().await;
    assert_eq!(
        1,
        sandbox.mode_resolutions(),
        "read once to serve the folder"
    );
    assert_eq!(SandboxMode::Observe, sandbox.mode());
    assert_eq!(SandboxMode::Observe, sandbox.mode());
    assert_eq!(
        SandboxModeSource::UserConfig,
        sandbox.resolved_mode().source
    );
    let _ = sandbox.status_json();
    assert_eq!(
        1,
        sandbox.mode_resolutions(),
        "one read for any number of readers"
    );

    fx.set_mode("enforce");
    assert_eq!(
        SandboxMode::Enforce,
        sandbox.mode(),
        "the workspace file moved"
    );
    assert_eq!(SandboxMode::Enforce, sandbox.mode());
    assert_eq!(2, sandbox.mode_resolutions());

    fx.set_user_mode("off");
    assert_eq!(
        SandboxMode::Enforce,
        sandbox.mode(),
        "the user file moved and was read: the trusted workspace layer still tightens it"
    );
    assert_eq!(3, sandbox.mode_resolutions());

    let (_, effective) = sandbox.set_workspace_mode(SandboxMode::Observe).unwrap();
    assert_eq!(SandboxMode::Observe, effective.mode);
    assert_eq!(4, sandbox.mode_resolutions(), "the verb resolves afresh");
    assert_eq!(SandboxMode::Observe, sandbox.mode());
    assert_eq!(4, sandbox.mode_resolutions());
}

/// Reading the layers holds no lock the other readers wait on: while one reader is stuck
/// reading the layers, another answers at once with the last resolution, and the stuck reader's
/// answer is kept once it lands.
#[tokio::test]
async fn a_slow_mode_resolve_does_not_hold_up_the_other_readers() {
    let fx = Fixture::new();
    fx.set_user_mode("observe");
    let sandbox = fx.open_with_backend().await;
    assert_eq!(SandboxMode::Observe, sandbox.mode());

    std::thread::scope(|scope| {
        let (inside, release) = sandbox.hold_next_mode_read();
        fx.set_user_mode("enforce");
        let stuck = scope.spawn(|| sandbox.mode());
        inside
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("the reader reached the layers");
        let (answered, answer) = std::sync::mpsc::channel();
        let reader = &sandbox;
        scope.spawn(move || answered.send(reader.mode()));
        let other = answer.recv_timeout(std::time::Duration::from_secs(5));
        release.send(()).unwrap();
        assert_eq!(
            Ok(SandboxMode::Observe),
            other,
            "the last resolution, at once"
        );
        assert_eq!(SandboxMode::Enforce, stuck.join().unwrap());
    });
    assert_eq!(Some(SandboxMode::Enforce), kept_mode(&sandbox));
    assert_eq!(SandboxMode::Enforce, sandbox.mode());
}

/// The resolution the cache holds, read without resolving.
fn kept_mode(sandbox: &WorkspaceSandbox) -> Option<SandboxMode> {
    let cache = sandbox.mode.lock();
    cache.resolved.as_ref().map(|(_, resolved)| resolved.mode)
}

/// A resolution begun before a layer was written is answered but not kept, so the next reader
/// reads the layers the write left rather than what the slow reader saw before it.
#[tokio::test]
async fn a_resolve_begun_before_a_mode_write_is_not_kept() {
    let fx = Fixture::new();
    let sandbox = fx.open_with_backend().await;
    let user_file = crate::sandbox_mode::user_config_path(&fx.grok_home);

    std::thread::scope(|scope| {
        let (inside, release) = sandbox.hold_next_mode_read();
        fx.set_user_mode("enforce");
        let stuck = scope.spawn(|| sandbox.mode());
        inside
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("the reader reached the layers");
        sandbox.forget_resolution();
        release.send(()).unwrap();
        assert_eq!(SandboxMode::Enforce, stuck.join().unwrap());
    });
    assert_eq!(
        None,
        kept_mode(&sandbox),
        "the write's reader reads the layers again"
    );
    std::fs::remove_file(&user_file).unwrap();
    assert_eq!(SandboxMode::Off, sandbox.mode());
}

/// A call-scoped grant for a call whose result is already in is refused, not reported as given:
/// no spawn of the call is left to carry it.
#[tokio::test]
async fn a_call_grant_for_a_finished_call_is_refused() {
    let fx = Fixture::new();
    fx.set_mode("enforce");
    let sandbox = fx.open_with_backend().await;
    let call = CallId::tool("c1");
    let once = || {
        grant(
            GrantSubject::FsWriteRoot {
                root: outside_root(),
            },
            GrantScope::Call,
        )
    };
    let error = sandbox
        .record_grant(&call, None, "s-1", once())
        .await
        .unwrap_err();
    assert!(
        matches!(error, WorkspaceSandboxError::CallFinished),
        "never seen: {error:?}"
    );

    let (mut cmd, original) = fx.command("true");
    sandbox.prepare(&mut cmd, &original, &call).unwrap();
    assert!(
        sandbox
            .record_grant(&call, None, "s-1", once())
            .await
            .is_ok()
    );
    sandbox.release_call(&call);
    let error = sandbox
        .record_grant(&call, None, "s-1", once())
        .await
        .unwrap_err();
    assert!(
        matches!(error, WorkspaceSandboxError::CallFinished),
        "its result is in: {error:?}"
    );
    assert_eq!(0, sandbox.calls.lock().len(), "nothing was kept for it");
}

#[tokio::test]
async fn shell_init_spawns_are_wrapped_but_not_remembered() {
    let fx = Fixture::new();
    fx.set_mode("enforce");
    let sandbox = fx.open_with_backend().await;
    let (mut cmd, original) = fx.command("echo");
    let receipt = sandbox
        .prepare(&mut cmd, &original, &CallId::shell_init("login"))
        .unwrap()
        .unwrap();
    assert!(receipt.sandboxed);
    assert_eq!(0, sandbox.open_calls());
}

#[tokio::test]
async fn forget_drops_the_record_and_open_calls_are_bounded() {
    let fx = Fixture::new();
    fx.set_mode("enforce");
    let sandbox = fx.open_with_backend().await;
    let prepare = |call: &str| {
        let (mut cmd, original) = fx.command("echo");
        sandbox.prepare(&mut cmd, &original, &CallId::tool(call))
    };
    for i in 0..super::MAX_OPEN_CALLS {
        prepare(&format!("c{i}")).unwrap();
    }
    let refused = prepare("one-more").unwrap_err();
    assert!(
        matches!(
            refused.downcast_ref::<WorkspaceSandboxError>(),
            Some(WorkspaceSandboxError::CallTableFull)
        ),
        "no record nobody finished is evicted: {refused}"
    );
    assert_eq!(super::MAX_OPEN_CALLS, sandbox.open_calls());
    assert!(
        sandbox
            .finish(
                &CallId::tool("c0"),
                CommandExit::code(1),
                OUTSIDE_WRITE_STDERR
            )
            .await
            .violation()
            .is_some(),
        "the oldest record was kept"
    );
    prepare("one-more").unwrap();
    assert_eq!(super::MAX_OPEN_CALLS, sandbox.open_calls());
    assert_eq!(super::MAX_OPEN_CALLS, sandbox.calls.lock().len());
    sandbox.release_call(&CallId::tool("one-more"));
    assert_eq!(super::MAX_OPEN_CALLS - 1, sandbox.open_calls());
}

#[tokio::test]
async fn reduced_backend_shows_in_status() {
    let fx = Fixture::new();
    fx.set_mode("observe");
    let backend = Arc::new(StubBackend {
        wraps: AtomicUsize::new(0),
        reduced: true,
    });
    let sandbox = WorkspaceSandbox::open(
        fx.config(BackendSource::Fixed(Some(Box::new(ArcBackend(backend))))),
    )
    .await;
    let status = sandbox.status_json();
    assert_eq!(Some(&json!("seatbelt")), status.get("backend"));
    assert_eq!(Some(&json!(true)), status.get("reduced_sandbox"));
    assert_eq!(Some(&json!("observe")), status.get("mode"));
}

/// A "for this conversation" row applies to the calls of the hub session that gave it, the
/// background children they leave included, never another session's, and lives with it.
#[tokio::test]
async fn session_grants_go_with_their_session() {
    let fx = Fixture::new();
    fx.set_mode("enforce");
    let sandbox = fx.open_with_backend().await;
    let outside = outside_root();
    let row = grant(
        GrantSubject::FsWriteRoot {
            root: outside.clone(),
        },
        GrantScope::Session,
    );
    sandbox
        .record_grant(&CallId::tool("c1"), None, "s-1", row)
        .await
        .unwrap();
    let other = grant(
        GrantSubject::NetHost {
            host: xai_grok_sandbox::command::grants::HostPattern::new("pypi.org"),
            port: Some(443),
        },
        GrantScope::Session,
    );
    sandbox
        .record_grant(&CallId::tool("c2"), None, "s-2", other)
        .await
        .unwrap();
    assert_eq!(2, sandbox.live_grants().await.len());

    let stderr = format!(
        "touch: {}: Operation not permitted\n",
        outside.join("x").display()
    );
    // A call is bound to the hub session that dispatched it before it spawns
    let owned_by = |session: &str| super::CallOwner {
        session_id: session.to_owned(),
        policy: xai_tool_runtime::ToolApprovalPolicy::GrantsAllowed,
        transport: None,
        command: None,
    };
    let violates = async |call: &str| {
        let call = CallId::tool(call);
        let (mut cmd, original) = fx.command("touch x");
        sandbox.prepare(&mut cmd, &original, &call).unwrap();
        sandbox
            .finish(&call, CommandExit::code(1), stderr.as_bytes())
            .await
            .violation()
            .is_some()
    };
    let has_pypi = |call: &str| {
        sandbox
            .net_rows_for(Some(&CommandTag::for_call(&CallId::tool(call))))
            .iter()
            .any(|row| row.scope == GrantScope::Session)
    };
    sandbox
        .bind_call(&CallId::tool("c3"), owned_by("s-1"))
        .unwrap();
    assert!(
        !violates("c3").await,
        "the session row is in the policy of its own session's call"
    );
    sandbox
        .bind_call(&CallId::tool("c-other"), owned_by("s-2"))
        .unwrap();
    assert!(
        violates("c-other").await,
        "another session's call is not under that row"
    );
    assert!(
        violates("c-stray").await,
        "a call no session dispatched is under no session's rows"
    );
    assert!(
        has_pypi("c-other"),
        "a session's call is decided under its rows"
    );
    assert!(!has_pypi("c3"), "another session's call is not");
    assert!(!has_pypi("c-stray"), "a call no session dispatched is not");
    assert!(
        !sandbox
            .net_rows(Some("s-1"))
            .iter()
            .any(|row| row.scope == GrantScope::Session)
    );

    let background = CallId::tool("bg");
    sandbox.bind_call(&background, owned_by("s-2")).unwrap();
    let (mut cmd, original) = fx.command("sleep 60 &");
    sandbox.prepare(&mut cmd, &original, &background).unwrap();
    sandbox.detach_call(&background, "s-2");
    assert!(
        has_pypi("bg"),
        "a background child runs on under the rows of the session it ran for"
    );

    sandbox.end_session("s-1").await;
    let live = sandbox.live_grants().await;
    let [survivor] = live.as_slice() else {
        panic!("the other session's row survives, got {live:?}");
    };
    assert_eq!(GrantScope::Session, survivor.scope);
    sandbox
        .bind_call(&CallId::tool("c4"), owned_by("s-1"))
        .unwrap();
    let (mut cmd, original) = fx.command("touch x");
    sandbox
        .prepare(&mut cmd, &original, &CallId::tool("c4"))
        .unwrap();
    assert!(
        sandbox
            .finish(&CallId::tool("c4"), CommandExit::code(1), stderr.as_bytes())
            .await
            .violation()
            .is_some(),
        "the write is a violation again once the session ended"
    );
    sandbox.end_session("s-1").await;
    assert_eq!(1, sandbox.live_grants().await.len());
}

/// The proxy's decider reads the folder's network rows, deny rows included, from the same live
/// snapshot the policy is built from.
#[tokio::test]
async fn net_rows_carry_allow_and_deny_rows_and_no_fs_rows() {
    let fx = Fixture::new();
    let sandbox = fx.open_with_backend().await;
    sandbox
        .record_grant(
            &CallId::tool("c1"),
            None,
            "s-1",
            grant(
                GrantSubject::FsWriteRoot {
                    root: outside_root(),
                },
                GrantScope::Session,
            ),
        )
        .await
        .unwrap();
    sandbox
        .record_grant(
            &CallId::tool("c1"),
            None,
            "s-1",
            grant(
                GrantSubject::NetHost {
                    host: xai_grok_sandbox::command::grants::HostPattern::new("pypi.org"),
                    port: None,
                },
                GrantScope::Session,
            ),
        )
        .await
        .unwrap();
    let mut deny = grant(
        GrantSubject::NetHost {
            host: xai_grok_sandbox::command::grants::HostPattern::new("*.example.com"),
            port: None,
        },
        GrantScope::Session,
    );
    deny.decision = GrantDecision::Deny;
    sandbox
        .record_grant(&CallId::tool("c1"), None, "s-1", deny)
        .await
        .unwrap();

    let mut hosts: Vec<(String, bool)> = sandbox
        .net_rows(Some("s-1"))
        .iter()
        .map(|g| match &g.subject {
            GrantSubject::NetHost { host, .. } => {
                (host.to_string(), g.decision == GrantDecision::Deny)
            }
            other => panic!("{other:?}"),
        })
        .collect();
    hosts.sort();
    assert_eq!(
        vec![
            ("*.example.com".to_owned(), true),
            ("pypi.org".to_owned(), false),
        ],
        hosts
    );
    assert!(
        sandbox
            .net_rows(Some("s-2"))
            .iter()
            .all(|g| g.scope != GrantScope::Session),
        "another session's calls see none of s-1's rows"
    );
}

/// The workspace's `.grok/workspaced.toml` is read whatever the folder's trust: it can only
/// tighten, so an untrusted folder's `enforce` over the user's `observe` holds.
#[tokio::test]
async fn an_untrusted_workspace_can_tighten_the_mode() {
    let fx = Fixture::new();
    fx.set_user_mode("observe");
    fx.set_mode("enforce");
    let sandbox = fx.open_without_backend().await;
    assert_eq!(SandboxMode::Enforce, sandbox.mode());
    assert_eq!(
        SandboxModeSource::WorkspaceConfig,
        sandbox.resolved_mode().source
    );
    let (mut cmd, original) = fx.command("echo hi");
    let error = sandbox
        .prepare(&mut cmd, &original, &CallId::tool("c1"))
        .unwrap_err();
    assert_eq!(ENFORCE_UNAVAILABLE_TEXT, error.to_string());
}

/// With no user layer at all, an untrusted workspace's `enforce` is the mode.
#[tokio::test]
async fn an_untrusted_workspace_with_no_user_layer_resolves_to_its_enforce() {
    let fx = Fixture::new();
    fx.set_mode("enforce");
    let sandbox = fx.open_without_backend().await;
    assert_eq!(SandboxMode::Enforce, sandbox.mode());
    assert_eq!(
        SandboxModeSource::WorkspaceConfig,
        sandbox.resolved_mode().source
    );
}

/// A trusted workspace's layer tightens the user's mode and never loosens it.
#[tokio::test]
async fn a_trusted_workspace_layer_only_tightens() {
    let fx = Fixture::new();
    fx.set_user_mode("enforce");
    fx.set_mode("off");
    let sandbox = fx.open_without_backend().await;
    assert_eq!(SandboxMode::Enforce, sandbox.mode());
    assert_eq!(
        SandboxModeSource::UserConfig,
        sandbox.resolved_mode().source
    );
    fx.set_user_mode("off");
    fx.set_mode("enforce");
    assert_eq!(SandboxMode::Enforce, sandbox.mode());
    assert_eq!(
        SandboxModeSource::WorkspaceConfig,
        sandbox.resolved_mode().source
    );
}

/// `sandbox.mode.set` answers with the effective mode and its layer, not the one asked for: a
/// `mode.set off` on a folder whose user layer is `observe` replies `observe` / `user_config`
/// and a tightening `enforce` replies `enforce` / `workspace_config`.
#[tokio::test]
async fn set_workspace_mode_replies_with_the_effective_mode_and_its_source() {
    let fx = Fixture::new();
    fx.set_user_mode("observe");
    let sandbox = fx.open_without_backend().await;

    let (path, effective) = sandbox.set_workspace_mode(SandboxMode::Off).unwrap();
    assert_eq!(crate::sandbox_mode::workspace_config_path(&fx.root), path);
    assert_eq!(
        "[sandbox]\nmode = \"off\"\n",
        std::fs::read_to_string(&path).unwrap()
    );
    assert_eq!(SandboxMode::Observe, effective.mode);
    assert_eq!(SandboxModeSource::UserConfig, effective.source);
    assert_eq!(effective, sandbox.resolved_mode());

    let (_, effective) = sandbox.set_workspace_mode(SandboxMode::Enforce).unwrap();
    assert_eq!(SandboxMode::Enforce, effective.mode);
    assert_eq!(SandboxModeSource::WorkspaceConfig, effective.source);

    // Folder trust is not consulted: another reader of the folder resolves the same layer
    let reopened = fx.open_without_backend().await;
    let (_, effective) = reopened.set_workspace_mode(SandboxMode::Enforce).unwrap();
    assert_eq!(SandboxMode::Enforce, effective.mode);
    assert_eq!(SandboxModeSource::WorkspaceConfig, effective.source);
}

/// The status reports the folder's proxy while its accept loop runs, with the port the policy
/// routes through; a proxy that stopped on its own is reported as no network, as the next
/// `sync_network` will treat it, not as a bound port nobody answers on.
#[tokio::test]
async fn the_status_reports_the_proxy_only_while_its_accept_loop_runs() {
    let fx = Fixture::new();
    fx.set_mode("enforce");
    let sandbox = Arc::new(WorkspaceSandbox::open(fx.config(BackendSource::Fixed(None))).await);
    assert_eq!(Some(&json!("off")), sandbox.status_json().get("network"));
    let started = sandbox
        .start_network(EgressProxyOptions::default())
        .await
        .unwrap();
    assert_eq!(Some(&json!("proxy")), sandbox.status_json().get("network"));
    assert_eq!(
        Some(started.address.port()),
        sandbox.proxy().map(|proxy| proxy.port)
    );
    sandbox.abort_proxy_for_test().await;
    assert_eq!(None, sandbox.proxy());
    let status = sandbox.status_json();
    assert_eq!(Some(&json!("off")), status.get("network"));
    assert_eq!(Some(&Value::Null), status.get("proxy"));
    sandbox.stop_network().await;
}

/// Every built policy protects the daemon's own control-socket directory, and the
/// guard that checks it refuses a policy that does not. Every build checks the root pinned when
/// the folder engaged: once the link it was opened through moves, no build succeeds again.
#[tokio::test]
async fn the_control_socket_dir_is_protected_by_every_built_policy() {
    let fx = Fixture::new();
    let control_socket_dir = fx.grok_home.join("daemon");
    // The floor lists the directory in its canonical spelling (macOS: `/private/var/…` for a
    // `/var/folders/…` `$TMPDIR`); `covers` is spelling-exact, so ask it with that form.
    let canonical_dir = canonical_path(&control_socket_dir);
    let sandbox = fx.open_with_backend().await;
    let policy = sandbox
        .build_policy(sandbox.engage().await, &[], None)
        .unwrap();
    assert!(
        policy
            .protected
            .iter()
            .any(|protected| protected.covers(&canonical_dir)),
        "{:?}",
        policy.protected
    );
    super::assert_control_socket_protected(&policy, &control_socket_dir).unwrap();
    // The user's mode layer is in the floor too: a command that could write it would switch
    // the sandbox off for every command after it
    let user_layer = crate::sandbox_mode::user_config_path(&fx.grok_home);
    assert!(
        policy.is_protected(&user_layer),
        "{user_layer:?} not in {:?}",
        policy.protected
    );
    assert!(
        policy.is_protected(&fx.root.join(".grok/settings.toml")),
        "the workspace layer lives under the protected .grok/"
    );

    let mut stripped = sandbox
        .build_policy(sandbox.engage().await, &[], None)
        .unwrap();
    stripped
        .protected
        .retain(|protected| !protected.covers(&canonical_dir));
    let error = super::assert_control_socket_protected(&stripped, &control_socket_dir).unwrap_err();
    assert!(
        matches!(&error, WorkspaceSandboxError::ControlSocketUnprotected { dir } if *dir == control_socket_dir),
        "{error:?}"
    );
    assert!(
        error
            .to_string()
            .contains("does not protect the daemon's control-socket directory"),
        "{error}"
    );

    #[cfg(unix)]
    {
        let link = fx._tmp.path().join("served");
        std::os::unix::fs::symlink(&fx.root, &link).unwrap();
        let mut config = fx.config(BackendSource::Fixed(None));
        config.workspace_root = link.clone();
        let linked = WorkspaceSandbox::open(config).await;
        let engaged = linked.engage().await;
        linked.build_policy(engaged, &[], None).unwrap();
        for target in [&fx.grok_home, &fx.root] {
            std::fs::remove_file(&link).unwrap();
            std::os::unix::fs::symlink(target, &link).unwrap();
            let error = linked
                .build_policy(engaged, &[], None)
                .unwrap_err()
                .to_string();
            assert!(error.ends_with("re-open the folder"), "{error}");
        }
    }
}

/// `sandbox.grants.list {}` / `sandbox.grants.revoke {id}` with no folder served read and write
/// `<grok_home>/sandbox_grants.toml` directly; a served folder's sandbox sees the revoke on its
/// next read because the file changed under it.
#[tokio::test]
async fn global_rows_are_listed_and_revoked_without_a_served_folder() {
    let fx = Fixture::new();
    fx.set_mode("enforce");
    let sandbox = fx.open_with_backend().await;
    let mut row = grant(
        GrantSubject::FsWriteRoot {
            root: outside_root(),
        },
        GrantScope::Global,
    );
    row.expires = Expiry::Never;
    let id = sandbox
        .record_grant(&CallId::tool("c1"), None, "s-1", row)
        .await
        .unwrap();
    let mut workspace_row = grant(
        GrantSubject::FsWriteRoot {
            root: outside_root().join("ws-only"),
        },
        GrantScope::Workspace {
            root: fx.root.clone(),
        },
    );
    workspace_row.expires = Expiry::Ttl { seconds: 60 };
    sandbox
        .record_grant(&CallId::tool("c2"), None, "s-1", workspace_row)
        .await
        .unwrap();

    let global = super::global_grants(&fx.grok_home).await;
    let [kept] = global.as_slice() else {
        panic!("workspace rows stay with the folder, got {global:?}");
    };
    assert_eq!(id, kept.id);
    let rows = super::grants_to_json(&global);
    assert_eq!(
        Some(&json!(null)),
        rows.first().and_then(|row| row.get("expires_at"))
    );

    super::revoke_global_grant(&fx.grok_home, &id)
        .await
        .unwrap();
    assert!(super::global_grants(&fx.grok_home).await.is_empty());
    let error = super::revoke_global_grant(&fx.grok_home, &id)
        .await
        .unwrap_err();
    assert!(matches!(error, GrantError::NotFound { .. }), "{error}");
    let live = sandbox.live_grants().await;
    let [reloaded] = live.as_slice() else {
        panic!("the served folder reloaded the changed file, got {live:?}");
    };
    assert_eq!(Some(&json!("workspace")), json!(reloaded.scope).get("kind"));
}

const OUTSIDE_TARGET: &str = "/srv/grok-w0-test/out.txt";

/// The card's answer, as the session owner gives it.
struct AnsweringTransport(Value);

#[async_trait::async_trait]
impl PermissionHookTransport for AnsweringTransport {
    async fn request_permission(&self, _payload: Value) -> Result<Value, String> {
        Ok(self.0.clone())
    }
}

/// "Allow for this call" on the card's own proposal of grant `kind`.
fn allow_once(kind: &str) -> Value {
    json!({
        "outcome": "approve",
        "tool_call_id": "c1",
        "scope": { "kind": kind },
        "duration": { "kind": "call" },
    })
}

fn keep_blocked() -> Value {
    json!({ "outcome": "reject", "tool_call_id": "c1" })
}

/// A connection the proxy holds while the command runs: settled with `Replay::Resume`.
fn held_connection() -> Violation {
    Violation {
        blocked: Blocked::Net {
            host: Some("pypi.org".to_owned()),
            port: Some(443),
        },
        proposed: Some(GrantSubject::NetHost {
            host: HostPattern::new("pypi.org"),
            port: None,
        }),
        disposition: Disposition::Grantable,
        partial_output: None,
        replay: Replay::Resume {
            hold_id: "hold-1".to_owned(),
        },
        exit_code: None,
        stderr_snippet: String::new(),
    }
}

/// The write `finish` decodes from [`OUTSIDE_WRITE_STDERR`]: settled with `Replay::Rerun`.
fn outside_write() -> Violation {
    Violation {
        blocked: Blocked::FsWrite {
            path: PathBuf::from(OUTSIDE_TARGET),
        },
        proposed: Some(GrantSubject::FsWriteRoot {
            root: PathBuf::from(OUTSIDE_TARGET),
        }),
        disposition: Disposition::Grantable,
        partial_output: None,
        replay: Replay::Rerun,
        exit_code: Some(1),
        stderr_snippet: String::new(),
    }
}

/// The gate settling `violation` of `call` with the owner's `reply`.
async fn settle(
    sandbox: &WorkspaceSandbox,
    call: &CallId,
    violation: Violation,
    reply: Value,
) -> ViolationSettlement {
    settle_violation(
        SettleContext {
            sandbox,
            policy: ToolApprovalPolicy::GrantsAllowed,
            call,
            epoch: None,
            session_id: "s-1",
            command: "touch",
            mode: SandboxMode::Enforce,
            backend: Some(BackendName::Seatbelt),
            replayed_under: None,
        },
        violation,
        Some(Arc::new(AnsweringTransport(reply)) as Arc<dyn PermissionHookTransport>),
    )
    .await
}

/// One step of a call's life as the hub, the terminal and the gate drive it.
#[derive(Clone, Copy, Debug)]
enum Step {
    /// The hub dispatched the call (its result path binds it).
    Bind,
    /// The hub skipped the call's pre-run approval because the sandbox enforces.
    Pin,
    Spawn,
    /// The try got no child (a retryable spawn error): the terminal prepares the spawn again.
    Retry,
    /// The owner allowed a connection held while the command runs, for this call.
    HeldAllow,
    /// The owner kept a connection held while the command runs blocked.
    HeldDeny,
    FinishClean,
    /// The command exited on a write outside the policy.
    FinishDenied,
    /// The owner allowed the violation for this call: the gate asks for the replay.
    Replay,
    /// The owner allowed a held connection and a violation for this call once it is gone: both
    /// grants are refused and the denials stand.
    LateAllow,
    /// The call's final result is in, however it ended.
    Release,
    /// The call's final result is a background start from session `s-1`: its child runs on.
    Detach,
    /// The proxy decides the call's connections with the held host allowed.
    ProxyAllows,
    /// The terminal reports the call's backgrounded child exited.
    Exit,
    /// Session `s-1` ended.
    EndSession,
    /// Every other slot of the table is taken by a call the hub dispatched.
    Crowd,
    /// A spawn outside the hub's result path arrives at the table.
    Stray,
    /// Another call arrives at a table full of calls that may still run: the hub's pin, its bind
    /// and its spawn are each refused, and nothing is evicted for it.
    Refused,
    /// The folder's mode flips to `off`.
    FlipOff,
    /// The folder's mode flips to `observe`.
    FlipObserve,
    /// The hub skipped the call's pre-run approval on its pin: its stream binds it.
    BindPinned,
    /// The same bind, refused: the pin is gone, so the stream refuses the call.
    BindPinnedRefused,
    /// The call's open spawn was wrapped under `enforce`.
    Enforced,
}

/// What the table holds for the one call a path drives.
#[derive(Debug, PartialEq, Eq)]
struct Row {
    entries: usize,
    spawns: usize,
    /// The mode the call holds: its open spawn's, else the one kept for its next spawn.
    mode: Option<SandboxMode>,
    once: usize,
}

const fn row(entries: usize, spawns: usize, mode: Option<SandboxMode>, once: usize) -> Row {
    Row {
        entries,
        spawns,
        mode,
        once,
    }
}

/// The call holds `enforce`: pinned by the hub, or fixed by a spawn under the folder's `enforce`.
const E: Option<SandboxMode> = Some(SandboxMode::Enforce);
/// The call holds no mode: its next spawn takes the folder's.
const N: Option<SandboxMode> = None;

const GONE: Row = row(0, 0, N, 0);

fn row_of(sandbox: &WorkspaceSandbox, call: &CallId) -> Row {
    let calls = sandbox.calls.lock();
    let mode = calls.held_mode(call);
    assert_eq!(
        calls.attempt_settling(call).mode,
        mode,
        "the mode its next spawn runs under decides its connections too"
    );
    Row {
        entries: calls.len(),
        spawns: calls.prepared(),
        mode,
        once: calls.once_rows(call).len(),
    }
}

fn spawn(fx: &Fixture, sandbox: &WorkspaceSandbox, call: &CallId) {
    let (mut cmd, original) = fx.command(&format!("touch {OUTSIDE_TARGET}"));
    sandbox.prepare(&mut cmd, &original, call).unwrap();
}

fn owner() -> CallOwner {
    CallOwner {
        session_id: "s-1".to_owned(),
        policy: ToolApprovalPolicy::GrantsAllowed,
        transport: None,
        command: None,
    }
}

/// The hub's dispatch binding `call` to session `s-1`.
fn bind(sandbox: &WorkspaceSandbox, call: &CallId) {
    sandbox.bind_call(call, owner()).unwrap();
}

async fn run_step(fx: &Fixture, sandbox: &WorkspaceSandbox, call: &CallId, step: Step) {
    match step {
        Step::Bind => bind(sandbox, call),
        Step::Pin => sandbox.pin_mode(call, SandboxMode::Enforce).unwrap(),
        Step::Spawn | Step::Retry => spawn(fx, sandbox, call),
        Step::HeldAllow => {
            let settled = settle(sandbox, call, held_connection(), allow_once("net_host")).await;
            let ViolationSettlement::Replay { grant } = settled else {
                panic!("{settled:?}");
            };
            assert_eq!(GrantScope::Call, grant.scope);
        }
        Step::HeldDeny => {
            let settled = settle(sandbox, call, held_connection(), keep_blocked()).await;
            assert!(
                matches!(settled, ViolationSettlement::Denied { .. }),
                "{settled:?}"
            );
        }
        Step::FinishClean => {
            let finished = sandbox.finish(call, CommandExit::code(0), b"").await;
            assert!(finished.violation().is_none(), "{finished:?}");
        }
        Step::FinishDenied => {
            let finished = sandbox
                .finish(call, CommandExit::code(1), OUTSIDE_WRITE_STDERR)
                .await;
            assert!(finished.violation().is_some(), "{finished:?}");
        }
        Step::Replay => {
            let settled = settle(sandbox, call, outside_write(), allow_once("fs_write_root")).await;
            let ViolationSettlement::Replay { grant } = settled else {
                panic!("{settled:?}");
            };
            assert_eq!(GrantScope::Call, grant.scope);
        }
        Step::LateAllow => {
            let late = [
                (held_connection(), allow_once("net_host")),
                (outside_write(), allow_once("fs_write_root")),
            ];
            for (violation, reply) in late {
                let settled = settle(sandbox, call, violation, reply).await;
                let ViolationSettlement::Denied { model_text } = settled else {
                    panic!("{settled:?}");
                };
                assert!(
                    model_text.contains("the command it was for has already finished"),
                    "{model_text}"
                );
            }
        }
        Step::Release => {
            sandbox.release_call(call);
        }
        Step::Detach => {
            sandbox.detach_call(call, "s-1");
        }
        Step::ProxyAllows => {
            let held = held_connection().proposed;
            let rows = sandbox.net_rows_for(Some(&CommandTag::for_call(call)));
            assert!(
                rows.iter()
                    .any(|grant| Some(&grant.subject) == held.as_ref()
                        && grant.decision == GrantDecision::Allow),
                "{rows:?}"
            );
        }
        Step::Exit => SandboxLaunch::exited(sandbox, call),
        Step::EndSession => sandbox.end_session("s-1").await,
        Step::Crowd => {
            for i in 1..super::MAX_OPEN_CALLS {
                bind(sandbox, &CallId::tool(format!("owned-{i}")));
            }
        }
        Step::Stray => spawn(fx, sandbox, &CallId::tool("stray")),
        Step::Refused => {
            let newcomer = CallId::tool("newcomer");
            let refused = [
                sandbox.pin_mode(&newcomer, SandboxMode::Enforce),
                sandbox.bind_call(&newcomer, owner()),
            ];
            for refused in refused {
                assert!(
                    matches!(refused, Err(WorkspaceSandboxError::CallTableFull)),
                    "{refused:?}"
                );
            }
            let (mut cmd, original) = fx.command("echo");
            let refused = sandbox.prepare(&mut cmd, &original, &newcomer).unwrap_err();
            assert!(
                matches!(
                    refused.downcast_ref::<WorkspaceSandboxError>(),
                    Some(WorkspaceSandboxError::CallTableFull)
                ),
                "{refused}"
            );
        }
        Step::FlipOff => {
            fx.set_mode("off");
            assert_eq!(SandboxMode::Off, sandbox.mode());
        }
        Step::FlipObserve => {
            fx.set_mode("observe");
            assert_eq!(SandboxMode::Observe, sandbox.mode());
        }
        Step::BindPinned => assert!(sandbox.bind_pinned_call(call, owner())),
        Step::BindPinnedRefused => assert!(!sandbox.bind_pinned_call(call, owner())),
        Step::Enforced => {
            let calls = sandbox.calls.lock();
            let record = calls.prepared_record(call);
            assert!(
                record
                    .is_some_and(|record| record.mode == SandboxMode::Enforce && record.sandboxed),
                "the spawn ran under the folder's mode"
            );
        }
    }
}

/// A call's entry in the table after every step of every path it can take: a settlement never
/// touches the running spawn, a retry keeps what its first try ran under, a call grant holds for
/// the rest of the call, the mode a spawn fixed is the replay's whatever the folder does, a
/// final result leaves nothing (a background start nothing once its child exits or its session
/// ends), and a later answer never re-creates it. A full table evicts only an entry nothing of
/// which may still run, and with none such refuses a new call; a pinned call whose pin went is
/// refused, not run.
#[tokio::test]
async fn a_calls_entry_follows_every_path_to_its_final_result() {
    let full = super::MAX_OPEN_CALLS;
    let paths: [(&str, &[(Step, Row)]); 15] = [
        (
            "a held connection allowed mid-command, then a violation",
            &[
                (Step::Bind, row(1, 0, N, 0)),
                (Step::Pin, row(1, 0, E, 0)),
                (Step::Spawn, row(1, 1, E, 0)),
                (Step::HeldAllow, row(1, 1, E, 1)),
                (Step::FinishDenied, row(1, 0, E, 1)),
                (Step::Release, GONE),
            ],
        ),
        (
            "a held connection kept blocked mid-command, then a clean exit",
            &[
                (Step::Bind, row(1, 0, N, 0)),
                (Step::Spawn, row(1, 1, E, 0)),
                (Step::HeldDeny, row(1, 1, E, 1)),
                (Step::FinishClean, row(1, 0, N, 0)),
                (Step::Release, GONE),
            ],
        ),
        (
            "a violation replayed once under a call grant, through a flip to observe",
            &[
                (Step::Bind, row(1, 0, N, 0)),
                (Step::Pin, row(1, 0, E, 0)),
                (Step::Spawn, row(1, 1, E, 0)),
                (Step::FinishDenied, row(1, 0, E, 0)),
                (Step::Replay, row(1, 0, E, 1)),
                (Step::FlipObserve, row(1, 0, E, 1)),
                (Step::Spawn, row(1, 1, E, 1)),
                (Step::Enforced, row(1, 1, E, 1)),
                (Step::FinishClean, row(1, 0, N, 0)),
                (Step::Release, GONE),
            ],
        ),
        (
            "a spawn retried after a try with no child, on the first run and on the replay",
            &[
                (Step::Bind, row(1, 0, N, 0)),
                (Step::Pin, row(1, 0, E, 0)),
                (Step::Spawn, row(1, 1, E, 0)),
                (Step::Retry, row(1, 1, E, 0)),
                (Step::FinishDenied, row(1, 0, E, 0)),
                (Step::Replay, row(1, 0, E, 1)),
                (Step::Spawn, row(1, 1, E, 1)),
                (Step::Retry, row(1, 1, E, 1)),
                (Step::FinishClean, row(1, 0, N, 0)),
                (Step::Release, GONE),
            ],
        ),
        (
            "a kept denial or a recorded violation after a pin",
            &[
                (Step::Bind, row(1, 0, N, 0)),
                (Step::Pin, row(1, 0, E, 0)),
                (Step::Spawn, row(1, 1, E, 0)),
                (Step::FinishDenied, row(1, 0, E, 0)),
                (Step::Release, GONE),
            ],
        ),
        (
            "a background start whose child is allowed a held connection, then exits",
            &[
                (Step::Bind, row(1, 0, N, 0)),
                (Step::Pin, row(1, 0, E, 0)),
                (Step::Spawn, row(1, 1, E, 0)),
                (Step::Detach, row(1, 1, E, 0)),
                (Step::HeldAllow, row(1, 1, E, 1)),
                (Step::ProxyAllows, row(1, 1, E, 1)),
                (Step::Exit, GONE),
                (Step::LateAllow, GONE),
            ],
        ),
        (
            "a background start whose child is kept blocked until its session ends",
            &[
                (Step::Bind, row(1, 0, N, 0)),
                (Step::Spawn, row(1, 1, E, 0)),
                (Step::Detach, row(1, 1, E, 0)),
                (Step::HeldDeny, row(1, 1, E, 1)),
                (Step::EndSession, GONE),
                (Step::HeldDeny, GONE),
            ],
        ),
        (
            "a background child that exits before its start result",
            &[
                (Step::Bind, row(1, 0, N, 0)),
                (Step::Spawn, row(1, 1, E, 0)),
                (Step::Exit, row(1, 0, N, 0)),
                (Step::Detach, GONE),
            ],
        ),
        (
            "answers after the call's final result",
            &[
                (Step::Bind, row(1, 0, N, 0)),
                (Step::Spawn, row(1, 1, E, 0)),
                (Step::FinishDenied, row(1, 0, E, 0)),
                (Step::Release, GONE),
                (Step::LateAllow, GONE),
                (Step::HeldDeny, GONE),
            ],
        ),
        (
            "a spawn outside the hub's result path",
            &[(Step::Spawn, row(1, 1, E, 0)), (Step::FinishClean, GONE)],
        ),
        (
            "a call cancelled before its spawn",
            &[
                (Step::Bind, row(1, 0, N, 0)),
                (Step::Pin, row(1, 0, E, 0)),
                (Step::Release, GONE),
            ],
        ),
        (
            "a pin in a full table, then the folder flips to off before its spawn",
            &[
                (Step::Crowd, row(full - 1, 0, N, 0)),
                (Step::Pin, row(full, 0, E, 0)),
                (Step::Refused, row(full, 0, E, 0)),
                (Step::FlipOff, row(full, 0, E, 0)),
                (Step::BindPinned, row(full, 0, E, 0)),
                (Step::Spawn, row(full, 1, E, 0)),
                (Step::Enforced, row(full, 1, E, 0)),
                (Step::FinishClean, row(full, 0, N, 0)),
                (Step::Release, row(full - 1, 0, N, 0)),
            ],
        ),
        (
            "a pinned background child in a full table, through a flip to observe, until it exits",
            &[
                (Step::Bind, row(1, 0, N, 0)),
                (Step::Pin, row(1, 0, E, 0)),
                (Step::Spawn, row(1, 1, E, 0)),
                (Step::Detach, row(1, 1, E, 0)),
                (Step::Crowd, row(full, 1, E, 0)),
                (Step::Refused, row(full, 1, E, 0)),
                (Step::FlipObserve, row(full, 1, E, 0)),
                (Step::Exit, row(full - 1, 0, N, 0)),
                (Step::Stray, row(full, 1, N, 0)),
            ],
        ),
        (
            "a finished spawn's leftover makes room in a full table, and answers nothing after",
            &[
                (Step::Spawn, row(1, 1, E, 0)),
                (Step::FinishDenied, row(1, 0, N, 0)),
                (Step::HeldAllow, row(1, 0, N, 1)),
                (Step::Crowd, row(full, 0, N, 1)),
                (Step::Stray, row(full, 1, N, 0)),
                (Step::LateAllow, row(full, 1, N, 0)),
            ],
        ),
        (
            "a pin whose entry went before its owner bound",
            &[
                (Step::Pin, row(1, 0, E, 0)),
                (Step::Release, GONE),
                (Step::BindPinnedRefused, GONE),
            ],
        ),
    ];
    let call = CallId::tool("c1");
    for (path, steps) in paths {
        let fx = Fixture::new();
        fx.set_mode("enforce");
        let sandbox = fx.open_with_backend().await;
        for (step, expected) in steps {
            run_step(&fx, &sandbox, &call, *step).await;
            assert_eq!(*expected, row_of(&sandbox, &call), "{path}: after {step:?}");
        }
    }

    // At the cap only the oldest entry nothing of which may still run goes: never a bound call,
    // a spawn nobody finished or a background child; with none such the next call is refused
    let fx = Fixture::new();
    fx.set_mode("enforce");
    let sandbox = fx.open_with_backend().await;
    let running = CallId::tool("running");
    bind(&sandbox, &running);
    spawn(&fx, &sandbox, &running);
    let background = CallId::tool("background");
    bind(&sandbox, &background);
    sandbox.pin_mode(&background, SandboxMode::Enforce).unwrap();
    spawn(&fx, &sandbox, &background);
    sandbox.detach_call(&background, "s-1");
    let stray = CallId::tool("stray");
    spawn(&fx, &sandbox, &stray);
    let leftover = CallId::tool("leftover");
    sandbox.open_settlement(&leftover).unwrap();
    let host = GrantSubject::NetHost {
        host: HostPattern::new("pypi.org"),
        port: None,
    };
    assert!(
        sandbox
            .calls
            .lock()
            .stash_once(&leftover, None, grant(host, GrantScope::Call))
    );
    for i in 4..super::MAX_OPEN_CALLS {
        bind(&sandbox, &CallId::tool(format!("b{i}")));
    }
    assert_eq!(super::MAX_OPEN_CALLS, sandbox.calls.lock().len());
    let held = |call: &CallId| sandbox.calls.lock().held_mode(call);
    bind(&sandbox, &CallId::tool("one-more"));
    assert_eq!(super::MAX_OPEN_CALLS, sandbox.calls.lock().len());
    assert!(
        sandbox.calls.lock().once_rows(&leftover).is_empty(),
        "the leftover made room"
    );
    let refused = sandbox.bind_call(&CallId::tool("then-one-more"), owner());
    assert!(
        matches!(refused, Err(WorkspaceSandboxError::CallTableFull)),
        "{refused:?}"
    );
    assert_eq!(super::MAX_OPEN_CALLS, sandbox.calls.lock().len());
    assert_eq!(
        (Some(SandboxMode::Enforce), Some(SandboxMode::Enforce)),
        (held(&background), held(&stray)),
        "the background child and the stray spawn were kept, the child still pinned"
    );
    assert_eq!(
        Some(SandboxMode::Enforce),
        sandbox.calls.lock().held_mode(&background)
    );
    assert_eq!(3, sandbox.open_calls());
    let finished = sandbox
        .finish(&running, CommandExit::code(1), OUTSIDE_WRITE_STDERR)
        .await;
    assert!(
        finished.violation().is_some(),
        "the oldest call kept its running spawn: {finished:?}"
    );
}

/// A connection held while the command runs, answered "allow for this call": the running
/// spawn's record stays, so its later denial still decodes, and the grant covers the call's
/// next connections — never another call's — until the call's result is in.
#[tokio::test]
async fn a_held_connection_answered_mid_command_keeps_the_spawn_and_its_call_grant() {
    let fx = Fixture::new();
    fx.set_mode("enforce");
    let sandbox = fx.open_with_backend().await;
    let call = CallId::tool("c1");
    let tag = CommandTag::for_call(&call);
    let hosts = |tag: Option<&CommandTag>| -> Vec<GrantSubject> {
        sandbox
            .net_rows_for(tag)
            .into_iter()
            .map(|grant| grant.subject)
            .collect()
    };
    bind(&sandbox, &call);
    spawn(&fx, &sandbox, &call);
    let settled = settle(&sandbox, &call, held_connection(), allow_once("net_host")).await;
    let ViolationSettlement::Replay { grant } = settled else {
        panic!("{settled:?}");
    };
    assert_eq!(GrantScope::Call, grant.scope);

    assert_eq!(1, sandbox.open_calls(), "the running spawn's record stays");
    assert_eq!(vec![grant.subject.clone()], hosts(Some(&tag)));
    assert!(hosts(Some(&CommandTag::for_call(&CallId::tool("c2")))).is_empty());
    assert!(
        hosts(None).is_empty(),
        "a call grant never reaches the store"
    );

    let finished = sandbox
        .finish(&call, CommandExit::code(1), OUTSIDE_WRITE_STDERR)
        .await;
    let violation = finished
        .violation()
        .cloned()
        .expect("the spawn's later denial decodes");
    assert!(!violation.after_replay());
    assert_eq!(
        vec![grant.subject.clone()],
        hosts(Some(&tag)),
        "still in effect while the gate settles the denial"
    );
    sandbox.release_call(&call);
    assert!(hosts(Some(&tag)).is_empty());
}

/// A spawn whose first try got no child (a retryable spawn error) is prepared again: the retry
/// is the same spawn, so it runs pinned, as the replay and under the call grant whatever the
/// folder's mode is by then, and the call's final result still leaves nothing.
#[tokio::test]
async fn a_retried_spawn_keeps_the_pin_the_replay_mark_and_the_call_grant() {
    let fx = Fixture::new();
    fx.set_user_mode("enforce");
    let sandbox = fx.open_with_backend().await;
    let call = CallId::tool("c1");
    bind(&sandbox, &call);
    sandbox.pin_mode(&call, SandboxMode::Enforce).unwrap();
    spawn(&fx, &sandbox, &call);
    let finished = sandbox
        .finish(&call, CommandExit::code(1), OUTSIDE_WRITE_STDERR)
        .await;
    assert!(finished.violation().is_some(), "{finished:?}");
    let settled = settle(
        &sandbox,
        &call,
        outside_write(),
        allow_once("fs_write_root"),
    )
    .await;
    let ViolationSettlement::Replay { grant } = settled else {
        panic!("{settled:?}");
    };
    fx.set_user_mode("off");
    assert_eq!(SandboxMode::Off, sandbox.mode());

    for attempt in ["the replay's first try", "its retry"] {
        let (mut cmd, original) = fx.command(&format!("touch {OUTSIDE_TARGET}"));
        let receipt = sandbox.prepare(&mut cmd, &original, &call).unwrap();
        assert!(
            receipt.is_some_and(|receipt| receipt.sandboxed),
            "{attempt} keeps the pin"
        );
        let once: Vec<_> = sandbox
            .calls
            .lock()
            .once_rows(&call)
            .into_iter()
            .map(|row| row.id)
            .collect();
        assert_eq!(vec![grant.id.clone()], once, "{attempt}");
    }
    assert_eq!(3, fx.backend.wraps.load(Ordering::SeqCst));
    assert_eq!(
        1,
        sandbox.open_calls(),
        "the retry's record replaced the first try's"
    );

    let finished = sandbox
        .finish(&call, CommandExit::code(1), OUTSIDE_WRITE_STDERR)
        .await;
    assert!(
        matches!(finished, Finished::RefusedUnderGrant(_)),
        "the retry ran under the call grant and is the replay: {finished:?}"
    );
    sandbox.release_call(&call);
    assert_eq!(0, sandbox.calls.lock().len());
}

fn bash_result(exit_code: i32, output: &[u8]) -> ToolRunResult {
    ToolRunResult {
        output: ToolOutput::Bash(BashOutput {
            output: output.to_vec(),
            output_for_prompt: String::from_utf8_lossy(output).into_owned(),
            exit_code,
            command: format!("touch {OUTSIDE_TARGET}"),
            truncated: false,
            signal: None,
            timed_out: false,
            description: None,
            current_dir: "/ws".to_owned(),
            output_file: String::new(),
            total_bytes: 0,
            output_delta: None,
            was_bare_echo: false,
        }),
        prompt_text: "exit 1".to_owned(),
        effective_tool_name: None,
    }
}

#[derive(Clone, Copy, Debug)]
enum Ending {
    ToolError,
    /// Every try of the spawn got no child.
    SpawnFailed,
    NoTerminal,
    Cancelled,
    NeverPolled,
}

/// Every final result of a call pinned to `enforce` leaves no entry for it: a kept denial and a
/// recorded violation from the result path, and a stream that ends on a tool error, a spawn no
/// try of which started or no terminal, or is dropped mid-command or before it ran.
#[tokio::test]
async fn a_final_result_after_a_pin_leaves_no_entry_for_the_call() {
    let fx = Fixture::new();
    fx.set_mode("enforce");
    let sandbox = Arc::new(fx.open_with_backend().await);
    let handle =
        WorkspaceHandle::for_test_in_with_sandbox(&fx.root, sandbox.clone(), ToolApprovalGate::Off);
    let session = handle.create_session("main").unwrap();
    let call = CallId::tool("c1");

    for (case, output, noted) in [
        ("a kept denial", OUTSIDE_WRITE_STDERR, true),
        (
            "a recorded violation",
            b"Sandbox: bash(1) deny(1) file-write-create\n".as_slice(),
            false,
        ),
    ] {
        bind(&sandbox, &call);
        sandbox.pin_mode(&call, SandboxMode::Enforce).unwrap();
        spawn(&fx, &sandbox, &call);
        let AfterRun::Keep(result) =
            after_shell_run(&handle, &session, &call, bash_result(1, output)).await
        else {
            panic!("{case}: the result is kept");
        };
        assert_eq!(
            noted,
            result
                .prompt_text
                .contains("no channel to the session owner"),
            "{case}: {}",
            result.prompt_text
        );
        assert_eq!(0, sandbox.calls.lock().len(), "{case}");
        assert_eq!(0, sandbox.open_calls(), "{case}");
    }

    let (_, original) = fx.command(&format!("touch {OUTSIDE_TARGET}"));
    let dispatch = |ending: Ending| {
        let sandbox = sandbox.clone();
        let original = original.clone();
        move || -> ToolStream<ToolRunResult> {
            let tries = if matches!(ending, Ending::SpawnFailed) {
                2
            } else {
                1
            };
            for _ in 0..tries {
                let mut cmd = tokio::process::Command::new(&original.program);
                sandbox
                    .prepare(&mut cmd, &original, &CallId::tool("c1"))
                    .unwrap();
            }
            Box::pin(async_stream::stream! {
                yield ToolStreamItem::Progress(ToolProgress::Text {
                    text: "running".to_owned(),
                });
                match ending {
                    Ending::ToolError | Ending::SpawnFailed => {
                        yield ToolStreamItem::Terminal(Err(ToolError::new(
                            ToolErrorKind::TerminalError,
                            "terminal died",
                        )));
                    }
                    Ending::NoTerminal => {}
                    Ending::Cancelled | Ending::NeverPolled => {
                        futures::future::pending::<()>().await;
                    }
                }
            })
        }
    };
    for ending in [
        Ending::ToolError,
        Ending::SpawnFailed,
        Ending::NoTerminal,
        Ending::Cancelled,
        Ending::NeverPolled,
    ] {
        sandbox.pin_mode(&call, SandboxMode::Enforce).unwrap();
        let mut stream = run_shell_call_with_replay(
            handle.clone(),
            session.clone(),
            "c1".to_owned(),
            None,
            true,
            dispatch(ending),
        );
        match ending {
            Ending::ToolError | Ending::SpawnFailed | Ending::NoTerminal => {
                let items: Vec<_> = stream.by_ref().collect().await;
                assert!(
                    matches!(items.last(), Some(ToolStreamItem::Terminal(Err(_)))),
                    "{ending:?}: {items:?}"
                );
            }
            Ending::Cancelled => {
                let first = stream.next().await;
                assert!(
                    matches!(first, Some(ToolStreamItem::Progress(_))),
                    "{first:?}"
                );
                assert_eq!(1, sandbox.open_calls(), "the command is running");
            }
            Ending::NeverPolled => {}
        }
        drop(stream);
        assert_eq!(0, sandbox.calls.lock().len(), "{ending:?}");
        assert_eq!(0, sandbox.open_calls(), "{ending:?}");
    }
}

/// A call the hub let past its pre-run approval on the enforce pin runs enforced or not at all:
/// a full table refuses a newcomer rather than evict the pin, so a folder flipped to `off`
/// before the spawn still runs it wrapped, and a stream that finds the pin gone refuses the call
/// without dispatching it.
#[tokio::test]
async fn a_pinned_call_runs_enforced_or_not_at_all() {
    let fx = Fixture::new();
    fx.set_mode("enforce");
    let sandbox = Arc::new(fx.open_with_backend().await);
    let handle =
        WorkspaceHandle::for_test_in_with_sandbox(&fx.root, sandbox.clone(), ToolApprovalGate::Off);
    let session = handle.create_session("main").unwrap();
    let (_, original) = fx.command(&format!("touch {OUTSIDE_TARGET}"));
    let wrapped = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let dispatch = |call_id: &str| {
        let sandbox = sandbox.clone();
        let original = original.clone();
        let wrapped = wrapped.clone();
        let call = CallId::tool(call_id);
        move || -> ToolStream<ToolRunResult> {
            let mut cmd = tokio::process::Command::new(&original.program);
            let receipt = sandbox.prepare(&mut cmd, &original, &call).unwrap();
            wrapped
                .lock()
                .push(receipt.is_some_and(|receipt| receipt.sandboxed));
            Box::pin(async_stream::stream! {
                yield ToolStreamItem::Terminal(Ok(bash_result(0, b"")));
            })
        }
    };

    for i in 1..super::MAX_OPEN_CALLS {
        bind(&sandbox, &CallId::tool(format!("owned-{i}")));
    }
    sandbox
        .pin_mode(&CallId::tool("pinned"), SandboxMode::Enforce)
        .unwrap();
    let (mut cmd, original_stray) = fx.command("echo");
    let refused = sandbox
        .prepare(&mut cmd, &original_stray, &CallId::tool("stray"))
        .unwrap_err();
    assert!(
        matches!(
            refused.downcast_ref::<WorkspaceSandboxError>(),
            Some(WorkspaceSandboxError::CallTableFull)
        ),
        "the pin is not evicted for a stray: {refused}"
    );
    fx.set_mode("off");
    assert_eq!(SandboxMode::Off, sandbox.mode());
    let items: Vec<_> = run_shell_call_with_replay(
        handle.clone(),
        session.clone(),
        "pinned".to_owned(),
        None,
        true,
        dispatch("pinned"),
    )
    .collect()
    .await;
    assert!(
        matches!(items.as_slice(), [ToolStreamItem::Terminal(Ok(_))]),
        "{items:?}"
    );
    assert_eq!(vec![true], *wrapped.lock(), "the spawn ran under its pin");

    let lost = CallId::tool("lost");
    sandbox.pin_mode(&lost, SandboxMode::Enforce).unwrap();
    sandbox.release_call(&lost);
    let entries = sandbox.calls.lock().len();
    let items: Vec<_> = run_shell_call_with_replay(
        handle,
        session,
        "lost".to_owned(),
        None,
        true,
        dispatch("lost"),
    )
    .collect()
    .await;
    let [ToolStreamItem::Terminal(Err(error))] = items.as_slice() else {
        panic!("{items:?}");
    };
    assert!(
        matches!(error.kind, ToolErrorKind::PermissionDenied) && error.detail == PIN_LOST_TEXT,
        "{error:?}"
    );
    assert_eq!(1, wrapped.lock().len(), "the refused call never spawned");
    assert_eq!(
        entries,
        sandbox.calls.lock().len(),
        "the refusal created nothing"
    );
}

fn background_started() -> ToolRunResult {
    ToolRunResult {
        output: ToolOutput::BackgroundTaskStarted(BackgroundTaskStarted {
            task_id: "t1".to_owned(),
            task_type: "bash".to_owned(),
            output_file: "/tmp/out".to_owned(),
            status: "running".to_owned(),
            command: "while :; do curl https://pypi.org; done".to_owned(),
            summary: "running".to_owned(),
            retrieval_hint: String::new(),
            pre_formatted: None,
            pid: None,
        }),
        prompt_text: "started in the background".to_owned(),
        effective_tool_name: None,
    }
}

/// A call whose stream ends on a background start leaves its child running under the spawn's
/// record: a connection held while it runs and answered "allow for this call" afterwards is in
/// effect for that child's connections, until the child exits or its session ends.
#[tokio::test]
async fn a_background_start_keeps_its_childs_call_answers_until_the_child_is_gone() {
    let fx = Fixture::new();
    fx.set_mode("enforce");
    let sandbox = Arc::new(fx.open_with_backend().await);
    let handle =
        WorkspaceHandle::for_test_in_with_sandbox(&fx.root, sandbox.clone(), ToolApprovalGate::Off);
    let session = handle.create_session("main").unwrap();
    let (_, original) = fx.command(&format!("touch {OUTSIDE_TARGET}"));
    let held = held_connection().proposed;

    for (call_id, gone) in [
        ("bg-exit", "the child exits"),
        ("bg-end", "the session ends"),
    ] {
        let call = CallId::tool(call_id);
        let tag = CommandTag::for_call(&call);
        let allowed = |tag: &CommandTag| {
            sandbox
                .net_rows_for(Some(tag))
                .iter()
                .any(|grant| Some(&grant.subject) == held.as_ref())
        };
        let dispatch = {
            let sandbox = sandbox.clone();
            let original = original.clone();
            let call = call.clone();
            move || -> ToolStream<ToolRunResult> {
                let mut cmd = tokio::process::Command::new(&original.program);
                sandbox.prepare(&mut cmd, &original, &call).unwrap();
                Box::pin(async_stream::stream! {
                    yield ToolStreamItem::Terminal(Ok(background_started()));
                })
            }
        };
        let stream = run_shell_call_with_replay(
            handle.clone(),
            session.clone(),
            call_id.to_owned(),
            None,
            false,
            dispatch,
        );
        let items: Vec<_> = stream.collect().await;
        assert!(
            matches!(items.as_slice(), [ToolStreamItem::Terminal(Ok(_))]),
            "{items:?}"
        );

        let settled = settle(&sandbox, &call, held_connection(), allow_once("net_host")).await;
        assert!(
            matches!(settled, ViolationSettlement::Replay { .. }),
            "{settled:?}"
        );
        assert!(
            allowed(&tag),
            "{gone}: the answer is in effect for the child"
        );
        assert_eq!(1, sandbox.open_calls(), "{gone}: the child runs on");
        assert!(!allowed(&CommandTag::for_call(&CallId::tool("other"))));

        sandbox.end_session("another-session").await;
        assert!(allowed(&tag), "{gone}: another session's end leaves it");
        match call_id {
            "bg-exit" => SandboxLaunch::exited(sandbox.as_ref(), &call),
            _ => sandbox.end_session(session.session_id()).await,
        }
        assert!(!allowed(&tag), "{gone}");
        assert_eq!(0, sandbox.calls.lock().len(), "{gone}");

        let late = settle(&sandbox, &call, held_connection(), allow_once("net_host")).await;
        assert!(
            matches!(late, ViolationSettlement::Denied { .. }),
            "{late:?}"
        );
        assert!(!allowed(&tag), "{gone}: a later answer is refused");
        assert_eq!(0, sandbox.calls.lock().len(), "{gone}");
    }
}
