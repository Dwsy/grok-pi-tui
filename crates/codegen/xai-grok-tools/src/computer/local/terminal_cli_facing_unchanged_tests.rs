//! Pin: with no [`SandboxLaunch`](crate::sandbox_launch::SandboxLaunch)
//! installed — which is every `grok` CLI process — each shell spawn site execs exactly the
//! process its golden execs: the site's assembly of the command without the seam, against the
//! live site with the hook `None`. What is compared is what the kernel received: the child's
//! argv (`/proc/$$/cmdline`), its working directory and its environment block, byte for byte.
//! The rc-sourcing captures (login env, static snapshot, persistent state) are compared on what
//! they return, since the child's script is theirs and its output is a function of the exec'd
//! argv, cwd and env.
//!
//! Linux-only: the probe reads `/proc`.

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;

use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{
    ActorSettings, LocalTerminalActor, SpawnSandbox, apply_child_env, capture_login_env,
    shell_state, spawn_shell_command,
};
use crate::computer::local::SearchShadowConfig;
use crate::computer::local::static_shell::StaticShellSnapshot;
use crate::util::{EnvironmentVariablePattern, ShellEnvironmentPolicy};

/// Prints the shell's own argv, cwd and exec-time environment, each NUL-delimited, separated
/// by `\x1f`. `$$` is the shell the site spawned (the wrapper scripts `eval` the user command in
/// that same process).
const PROBE: &str = "cat /proc/$$/cmdline; printf '\\x1f'; readlink /proc/$$/cwd; printf '\\x1f'; cat /proc/$$/environ";

async fn read_to_end(child: &mut tokio::process::Child) -> Vec<u8> {
    let mut out = Vec::new();
    let mut err = Vec::new();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let (stdout_read, stderr_read) = tokio::join!(
        async {
            if let Some(mut stdout) = stdout {
                stdout.read_to_end(&mut out).await.unwrap();
            }
        },
        async {
            if let Some(mut stderr) = stderr {
                stderr.read_to_end(&mut err).await.unwrap();
            }
        }
    );
    let _ = (stdout_read, stderr_read);
    let status = child.wait().await.unwrap();
    assert!(
        status.success(),
        "probe exited {status:?}; stderr: {}; stdout: {}",
        String::from_utf8_lossy(&err),
        String::from_utf8_lossy(&out)
    );
    out
}

/// The three probe fields, for a readable diff when the pin fails.
fn split_probe(bytes: &[u8]) -> (Vec<String>, String, Vec<String>) {
    let mut parts = bytes.split(|b| *b == 0x1f);
    let argv = parts
        .next()
        .unwrap()
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect();
    let cwd = String::from_utf8_lossy(parts.next().unwrap())
        .trim()
        .to_owned();
    let env = parts
        .next()
        .unwrap()
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect();
    (argv, cwd, env)
}

fn assert_same_exec(golden: &[u8], live: &[u8]) {
    let (golden_argv, golden_cwd, golden_env) = split_probe(golden);
    let (live_argv, live_cwd, live_env) = split_probe(live);
    assert_eq!(golden_argv, live_argv, "argv");
    assert_eq!(golden_cwd, live_cwd, "cwd");
    assert_eq!(golden_env, live_env, "environment block");
    assert_eq!(golden, live, "raw probe bytes");
}

fn filtering_policy() -> ShellEnvironmentPolicy {
    ShellEnvironmentPolicy {
        exclude: vec![EnvironmentVariablePattern::new_case_insensitive(
            "GROK_PIN_SECRET*",
        )],
        set: HashMap::from([("GROK_PIN_BASE".to_owned(), "base".to_owned())]),
        ..Default::default()
    }
}

fn request_env() -> HashMap<String, String> {
    HashMap::from([
        ("GROK_PIN_REQ".to_owned(), "req".to_owned()),
        ("GROK_PIN_SECRET_TOKEN".to_owned(), "leak".to_owned()),
    ])
}

fn login_env() -> HashMap<String, String> {
    HashMap::from([
        ("GROK_PIN_LOGIN".to_owned(), "login".to_owned()),
        ("PATH".to_owned(), "/login/bin:/usr/bin:/bin".to_owned()),
    ])
}

/// `spawn_shell_command` (unix) without the seam.
fn main_spawn_shell_command(
    command: &str,
    cwd: &Path,
    env: &HashMap<String, String>,
    login_env: Option<&HashMap<String, String>>,
    search_shadows: SearchShadowConfig,
    shell_env_policy: Option<&ShellEnvironmentPolicy>,
) -> std::io::Result<tokio::process::Child> {
    let shell = shell_state::ShellKind::detect();
    let wrapped_command = {
        let inject =
            crate::computer::local::embedded_search_tools::search_injection(search_shadows);
        if inject.is_empty() {
            command.to_string()
        } else {
            format!("{inject}{command}")
        }
    };
    let mut cmd = tokio::process::Command::new(shell.binary_path());
    if matches!(shell, shell_state::ShellKind::Zsh) {
        cmd.arg("-o").arg("nonomatch");
    }
    cmd.arg("-c")
        .arg(&wrapped_command)
        .current_dir(cwd)
        .stdin(xai_tty_utils::null_stdio())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    apply_child_env(&mut cmd, shell_env_policy, login_env, env);
    crate::util::detach_command(&mut cmd);
    xai_grok_sandbox::child_net::restrict_child_network(&mut cmd);
    #[allow(clippy::disallowed_methods)] // the golden for the pin; waited on by the test
    cmd.spawn()
}

#[tokio::test]
async fn cli_facing_spawn_shell_command_unchanged_without_hook() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = dunce::canonicalize(tmp.path()).unwrap();
    for (env, login, policy) in [
        (HashMap::new(), None, None),
        (request_env(), Some(login_env()), Some(filtering_policy())),
    ] {
        let (mut live, _group) = spawn_shell_command(
            PROBE,
            &cwd,
            &env,
            login.as_ref(),
            SearchShadowConfig::default(),
            policy.as_ref(),
            SpawnSandbox {
                hook: None,
                tool_call_id: "pin-call",
            },
        )
        .unwrap();
        let live = read_to_end(&mut live).await;
        let mut golden = main_spawn_shell_command(
            PROBE,
            &cwd,
            &env,
            login.as_ref(),
            SearchShadowConfig::default(),
            policy.as_ref(),
        )
        .unwrap();
        let golden = read_to_end(&mut golden).await;
        assert_same_exec(&golden, &live);
        let (argv, probe_cwd, probe_env) = split_probe(&live);
        assert_eq!(cwd, Path::new(&probe_cwd));
        assert!(
            argv.last().is_some_and(|last| last.ends_with(PROBE)),
            "{argv:?}"
        );
        assert!(
            probe_env.contains(&format!(
                "{}={}",
                crate::util::GROK_AGENT_ENV,
                crate::util::GROK_AGENT_ENV_VALUE
            )),
            "{probe_env:?}"
        );
    }
}

/// An actor with no hook, as `LocalTerminalBackend::new_local_with_*` builds one.
fn actor(
    persistent_shell: bool,
    login_shell_capture: bool,
    shell_env_policy: Option<ShellEnvironmentPolicy>,
) -> LocalTerminalActor {
    let (cmd_tx, cmd_rx) = mpsc::channel(4);
    LocalTerminalActor::new(
        cmd_rx,
        cmd_tx.downgrade(),
        CancellationToken::new(),
        super::CgroupGuard::noop(),
        super::MemoryMonitor::noop(),
        persistent_shell,
        login_shell_capture,
        SearchShadowConfig::default(),
        ActorSettings::from_env(),
        crate::util::global_process_scope().clone(),
        None,
        shell_env_policy,
        None,
    )
}

/// `spawn_static_command` without the seam, over the snapshot and login env the goldens captured.
async fn main_spawn_static_command(
    static_shell: &StaticShellSnapshot,
    login_env: Option<&HashMap<String, String>>,
    command: &str,
    cwd: &Path,
    env: &HashMap<String, String>,
    shell_env_policy: Option<&ShellEnvironmentPolicy>,
) -> Vec<u8> {
    use command_fds::CommandFdExt;
    let prep = static_shell
        .prepare_command(command, SearchShadowConfig::default())
        .unwrap();
    let mut cmd = tokio::process::Command::new(&prep.binary);
    cmd.args(&prep.args)
        .current_dir(cwd)
        .stdin(xai_tty_utils::null_stdio())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    apply_child_env(&mut cmd, shell_env_policy, login_env, env);
    cmd.fd_mappings(prep.fd_mappings).unwrap();
    unsafe {
        cmd.pre_exec(xai_tty_utils::detach_pre_exec_hook());
    }
    xai_grok_sandbox::child_net::restrict_child_network(&mut cmd);
    #[allow(clippy::disallowed_methods)] // the golden for the pin; waited on by the test
    let mut child = cmd.spawn().unwrap();
    drop(cmd);
    let snapshot = static_shell.snapshot.clone();
    tokio::spawn(async move {
        crate::computer::local::static_shell::write_snapshot_to_pipe(
            &snapshot,
            prep.state_in_write,
        )
        .await
        .unwrap();
    });
    read_to_end(&mut child).await
}

/// `capture_login_env` without the seam.
async fn main_capture_login_env() -> HashMap<String, String> {
    let shell = shell_state::ShellKind::detect();
    let rc_file = shell.rc_file_name();
    let script = format!(
        "source \"$HOME/{rc_file}\" 2>/dev/null; printf '\\x01%s\\x01' \"$PATH\"; command env -0 2>/dev/null; printf '\\x01'"
    );
    let mut cmd = tokio::process::Command::new(shell.binary_path());
    cmd.args(["-lc", &script])
        .stdin(xai_tty_utils::null_stdio())
        .stdout(Stdio::piped())
        .stderr(xai_tty_utils::null_stdio())
        .kill_on_drop(true);
    crate::util::detach_command(&mut cmd);
    xai_grok_sandbox::child_net::restrict_child_network(&mut cmd);
    cmd.envs(crate::util::pager_env());
    #[allow(clippy::disallowed_methods)] // the golden for the pin; waited on by the test
    let mut child = cmd.spawn().unwrap();
    let out = read_to_end(&mut child).await;
    let stdout = String::from_utf8_lossy(&out);
    let (login_path, mut env_map) = super::parse_login_env_capture(&stdout);
    let login_path = login_path.unwrap();
    if !super::login_env_capture_enabled() {
        env_map.clear();
    }
    let current_path = std::env::var("PATH").unwrap_or_default();
    let mut seen = std::collections::HashSet::new();
    let merged: Vec<&str> = login_path
        .split(':')
        .chain(current_path.split(':'))
        .filter(|e| !e.is_empty() && seen.insert(*e))
        .collect();
    env_map.insert("PATH".to_string(), merged.join(":"));
    env_map
}

/// Site 5 (the login-env capture) returns what its golden does, and site 3 (the static-shell
/// command) execs what its golden does on top of it and the static snapshot.
#[tokio::test]
async fn cli_facing_static_shell_sites_unchanged_without_hook() {
    if !Path::new("/bin/bash").exists() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let cwd = dunce::canonicalize(tmp.path()).unwrap();

    let golden_login = main_capture_login_env().await;
    let live_login = capture_login_env(None).await;
    assert_eq!(golden_login, live_login, "login-env capture");

    // Site 6's own pin (`static_shell_cli_facing_unchanged_tests`) proves this snapshot is the
    // golden's; the golden below is built over it
    let golden_snapshot = StaticShellSnapshot::init(&cwd, None).await;

    let mut actor = actor(false, true, Some(filtering_policy()));
    actor.ensure_static_shell_initialized(&cwd).await;
    assert_eq!(
        Some(&golden_login),
        actor.login_env.as_ref(),
        "the actor's own login-env capture"
    );
    let env = request_env();
    let mut live = actor
        .spawn_static_command(PROBE, &cwd, &env, "pin-call")
        .await
        .unwrap();
    let live = read_to_end(&mut live.child).await;
    let golden = main_spawn_static_command(
        &golden_snapshot,
        Some(&golden_login),
        PROBE,
        &cwd,
        &env,
        Some(&filtering_policy()),
    )
    .await;
    assert_same_exec(&golden, &live);
}

/// `spawn_persistent_command` without the seam, over a state the golden init captured.
async fn main_spawn_persistent_command(
    state: &shell_state::ShellState,
    command: &str,
    env: &HashMap<String, String>,
    shell_env_policy: Option<&ShellEnvironmentPolicy>,
) -> Vec<u8> {
    use command_fds::CommandFdExt;
    let prep = state
        .prepare_command(command, None, SearchShadowConfig::default(), None)
        .unwrap();
    let mut cmd = tokio::process::Command::new(&prep.binary);
    cmd.args(&prep.args)
        .current_dir(&prep.cwd)
        .stdin(xai_tty_utils::null_stdio())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    apply_child_env(&mut cmd, shell_env_policy, None, env);
    cmd.fd_mappings(prep.fd_mappings).unwrap();
    unsafe {
        cmd.pre_exec(xai_tty_utils::detach_pre_exec_hook());
    }
    xai_grok_sandbox::child_net::restrict_child_network(&mut cmd);
    #[allow(clippy::disallowed_methods)] // the golden for the pin; waited on by the test
    let mut child = cmd.spawn().unwrap();
    drop(cmd);
    let snapshot = state.snapshot.clone();
    tokio::spawn(async move {
        shell_state::write_snapshot_to_pipe(&snapshot, prep.state_in_write)
            .await
            .unwrap();
    });
    let dump =
        tokio::spawn(async move { shell_state::read_dump_from_pipe(prep.state_out_read).await });
    let out = read_to_end(&mut child).await;
    let _ = dump.await;
    out
}

/// Site 4 (the persistent command) execs what its golden does on top of the persistent state.
#[tokio::test]
async fn cli_facing_persistent_shell_sites_unchanged_without_hook() {
    if !Path::new("/bin/bash").exists() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let cwd = dunce::canonicalize(tmp.path()).unwrap();
    let shell = shell_state::ShellKind::detect();
    let policy = filtering_policy();

    // Site 7's own pin (`shell_state_cli_facing_unchanged_tests`) proves this state is the
    // golden's; the golden below is built over it
    let golden_state = shell_state::ShellState::init(shell, &cwd, Some(&policy), None)
        .await
        .unwrap();

    let mut actor = actor(true, false, Some(policy.clone()));
    let env = request_env();
    let mut live = actor
        .spawn_persistent_command(PROBE, &cwd, &env, "pin-call")
        .await
        .unwrap();
    let live_out = read_to_end(&mut live.child).await;
    if let Some(dump) = live.state_dump_handle.take() {
        let _ = dump.await;
    }
    let golden = main_spawn_persistent_command(&golden_state, PROBE, &env, Some(&policy)).await;
    assert_same_exec(&golden, &live_out);
}
