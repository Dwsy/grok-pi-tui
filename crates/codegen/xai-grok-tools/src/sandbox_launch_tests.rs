use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use super::{
    CallId, CallKind, LaunchReceipt, OriginalArgv, SandboxLaunch, SandboxLaunchError,
    SandboxLaunchHook, child_env, original_argv, prepare, prepare_restoring, wire_prepared,
};
use crate::computer::types::ComputerError;

/// Replaces the command with a wrapper the way a backend does: a new `Command` carrying only
/// program/args/cwd/envs of the original.
struct WrappingHook {
    seen: Mutex<Vec<(OriginalArgv, CallId)>>,
}

impl SandboxLaunch for WrappingHook {
    fn prepare(
        &self,
        cmd: &mut tokio::process::Command,
        original: &OriginalArgv,
        call: &CallId,
    ) -> Result<Option<LaunchReceipt>, SandboxLaunchError> {
        self.seen
            .lock()
            .unwrap()
            .push((original.clone(), call.clone()));
        let envs: Vec<(OsString, Option<OsString>)> = cmd
            .as_std()
            .get_envs()
            .map(|(k, v)| (k.to_os_string(), v.map(|v| v.to_os_string())))
            .collect();
        let mut wrapped = tokio::process::Command::new("/usr/bin/wrapper");
        wrapped
            .arg("--tag")
            .arg(format!("grok-{call}"))
            .arg("--")
            .arg(&original.program)
            .args(&original.args)
            .current_dir(&original.cwd);
        for (key, value) in envs {
            match value {
                Some(value) => wrapped.env(key, value),
                None => wrapped.env_remove(key),
            };
        }
        *cmd = wrapped;
        Ok(Some(LaunchReceipt {
            sandboxed: true,
            backend: Some(super::BackendName::Seatbelt),
        }))
    }
}

struct RefusingHook;

#[derive(Debug, thiserror::Error)]
#[error("no per-command sandbox backend on this host")]
struct NoBackend;

impl SandboxLaunch for RefusingHook {
    fn prepare(
        &self,
        _cmd: &mut tokio::process::Command,
        _original: &OriginalArgv,
        _call: &CallId,
    ) -> Result<Option<LaunchReceipt>, SandboxLaunchError> {
        Err(SandboxLaunchError::new(NoBackend))
    }
}

fn argv(cmd: &tokio::process::Command) -> Vec<String> {
    let std_cmd = cmd.as_std();
    std::iter::once(std_cmd.get_program())
        .chain(std_cmd.get_args())
        .map(|s| s.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn no_hook_leaves_the_command_untouched() {
    let mut cmd = tokio::process::Command::new("/bin/bash");
    cmd.args(["-c", "echo hi"]).current_dir("/tmp");
    let receipt = prepare(None, &mut cmd, &CallId::tool("tc-1")).unwrap();
    assert_eq!(None, receipt);
    assert_eq!(vec!["/bin/bash", "-c", "echo hi"], argv(&cmd));
}

#[test]
fn hook_sees_the_pre_wrap_argv_and_the_call_id() {
    let hook = WrappingHook {
        seen: Mutex::new(Vec::new()),
    };
    let mut cmd = tokio::process::Command::new("/bin/bash");
    cmd.args(["-c", "echo hi"]).current_dir("/tmp");
    let receipt = prepare(Some(&hook), &mut cmd, &CallId::tool("tc-7"))
        .unwrap()
        .unwrap();
    assert!(receipt.sandboxed);
    let seen = hook.seen.lock().unwrap();
    let [(original, call)] = seen.as_slice() else {
        panic!("one prepare call, got {seen:?}");
    };
    assert_eq!(&CallId::tool("tc-7"), call);
    assert_eq!(Path::new("/bin/bash"), original.program);
    assert_eq!(
        vec![OsString::from("-c"), OsString::from("echo hi")],
        original.args
    );
    assert_eq!(Path::new("/tmp"), original.cwd);
}

/// Pins the spawn-site order: what `wire_prepared` sets *after* `prepare` lands on the wrapper and
/// the wrapper's argv is intact.
#[test]
fn stdio_set_after_prepare_keeps_the_wrapper_argv_intact() {
    let hook = WrappingHook {
        seen: Mutex::new(Vec::new()),
    };
    let mut cmd = tokio::process::Command::new("/bin/bash");
    cmd.args(["-c", "echo hi"])
        .current_dir("/tmp")
        .env("HTTP_PROXY", "http://127.0.0.1:3128")
        .env_remove("AWS_SECRET_ACCESS_KEY");
    prepare(Some(&hook), &mut cmd, &CallId::tool("tc-9")).unwrap();
    wire_prepared(&mut cmd, Stdio::piped());
    assert_eq!(
        vec![
            "/usr/bin/wrapper",
            "--tag",
            "grok-tc-9",
            "--",
            "/bin/bash",
            "-c",
            "echo hi"
        ],
        argv(&cmd)
    );
    assert_eq!(Some(Path::new("/tmp")), cmd.as_std().get_current_dir());
    let envs: Vec<(String, Option<String>)> = cmd
        .as_std()
        .get_envs()
        .map(|(k, v)| {
            (
                k.to_string_lossy().into_owned(),
                v.map(|v| v.to_string_lossy().into_owned()),
            )
        })
        .collect();
    assert!(envs.contains(&(
        "HTTP_PROXY".to_owned(),
        Some("http://127.0.0.1:3128".to_owned())
    )));
    assert!(envs.contains(&("AWS_SECRET_ACCESS_KEY".to_owned(), None)));
}

#[test]
fn a_refusing_hook_stops_the_spawn_with_its_reason() {
    let mut cmd = tokio::process::Command::new("/bin/bash");
    cmd.args(["-c", "echo hi"]);
    let err = prepare(Some(&RefusingHook), &mut cmd, &CallId::tool("tc-2")).unwrap_err();
    assert!(err.to_string().contains("no per-command sandbox backend"));
    assert!(
        err.downcast_ref::<NoBackend>().is_some(),
        "the daemon's error is reachable"
    );
    assert_eq!(vec!["/bin/bash", "-c", "echo hi"], argv(&cmd));
}

/// A refusal is one error at every spawn site: permission denied, naming the sandbox and the
/// daemon's reason, whether the site returns an `io::Error` or a `ComputerError`.
#[test]
fn a_refusal_is_one_permission_denied_error_at_every_site() {
    let text = "sandbox refused to run the command: no per-command sandbox backend on this host";
    let io = std::io::Error::from(SandboxLaunchError::new(NoBackend));
    assert_eq!(std::io::ErrorKind::PermissionDenied, io.kind());
    assert_eq!(text, io.to_string());
    let computer = ComputerError::from(SandboxLaunchError::new(NoBackend));
    assert_eq!(
        Some(std::io::ErrorKind::PermissionDenied),
        computer.io_error_kind()
    );
    assert!(computer.to_string().contains(text), "{computer}");
}

/// `wire_prepared` leaves the child a closed stdin and a session of its own, so it cannot reach
/// the terminal grok runs in, and the child dies with its handle.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn wire_prepared_detaches_the_child_and_closes_its_stdin() {
    const PROBE: &str = "read -r line; r=$?; read -r _ _ _ _ _ sid _ < /proc/$$/stat; \
                         echo \"$r $$ $sid\"";
    let mut cmd = tokio::process::Command::new("/bin/sh");
    cmd.args(["-c", PROBE]);
    wire_prepared(&mut cmd, Stdio::null());
    assert!(cmd.get_kill_on_drop());
    let output = cmd.output().await.unwrap();
    assert!(output.status.success(), "{output:?}");
    let out = String::from_utf8_lossy(&output.stdout);
    let fields: Vec<&str> = out.split_whitespace().collect();
    let [read_status, pid, sid] = fields.as_slice() else {
        panic!("probe output {out:?}");
    };
    assert_ne!(
        "0", *read_status,
        "stdin is closed: the read hits end of file"
    );
    assert_eq!(pid, sid, "the child leads a session of its own");
}

#[test]
fn the_hook_wrapper_hands_out_the_same_implementation_twice() {
    let hook = SandboxLaunchHook::new(Arc::new(RefusingHook));
    let mut cmd = tokio::process::Command::new("/bin/true");
    assert!(prepare(Some(hook.as_launch()), &mut cmd, &CallId::tool("tc-3")).is_err());
    let original = original_argv(&cmd);
    assert!(
        hook.shared()
            .prepare(&mut cmd, &original, &CallId::tool("tc-4"))
            .is_err()
    );
    assert_eq!("SandboxLaunchHook(..)", format!("{hook:?}"));
}

/// What the command runs under is the daemon's environment with `cmd`'s sets and removals
/// applied, then what a replaying shell exports: an inherited variable `cmd` never names is part
/// of it, a removed one is not, and a restored value wins over `cmd`'s.
#[test]
fn the_child_environment_is_the_inherited_one_with_the_commands_entries_and_the_restored() {
    let inherited: Vec<(OsString, OsString)> = std::env::vars_os().take(2).collect();
    let [(kept, kept_value), (removed, _)] = inherited.as_slice() else {
        panic!("the test process inherits at least two variables: {inherited:?}");
    };
    let mut cmd = tokio::process::Command::new("/bin/true");
    cmd.env("GROK_CHILD_ENV_SET", "set")
        .env("GROK_CHILD_ENV_RESTORED", "set")
        .env_remove(removed);
    let restored = [(
        OsString::from("GROK_CHILD_ENV_RESTORED"),
        OsString::from("restored"),
    )];
    let env: BTreeMap<OsString, OsString> = child_env(&cmd, restored).into_iter().collect();
    assert_eq!(Some(kept_value), env.get(kept), "{kept:?} is inherited");
    assert_eq!(None, env.get(removed), "{removed:?} was removed");
    assert_eq!(
        Some(&OsString::from("set")),
        env.get(&OsString::from("GROK_CHILD_ENV_SET"))
    );
    assert_eq!(
        Some(&OsString::from("restored")),
        env.get(&OsString::from("GROK_CHILD_ENV_RESTORED"))
    );
}

/// Records what `prepare_restoring` hands the hook.
struct RestoringHook {
    restored: Mutex<Vec<Vec<(OsString, OsString)>>>,
}

impl SandboxLaunch for RestoringHook {
    fn prepare(
        &self,
        _cmd: &mut tokio::process::Command,
        _original: &OriginalArgv,
        _call: &CallId,
    ) -> Result<Option<LaunchReceipt>, SandboxLaunchError> {
        Err(SandboxLaunchError::new(NoBackend))
    }

    fn prepare_restoring(
        &self,
        _cmd: &mut tokio::process::Command,
        _original: &OriginalArgv,
        restored: &[(OsString, OsString)],
        _call: &CallId,
    ) -> Result<Option<LaunchReceipt>, SandboxLaunchError> {
        self.restored.lock().unwrap().push(restored.to_vec());
        Ok(None)
    }
}

/// A replaying shell's variables reach the hook through `prepare_restoring`, read only when a
/// hook is injected; a hook that reads no environment keeps `prepare`'s behavior.
#[test]
fn a_replaying_shell_hands_the_hook_what_it_exports() {
    let pyuser = (
        OsString::from("PYTHONUSERBASE"),
        OsString::from("/opt/pyuser"),
    );
    let hook = RestoringHook {
        restored: Mutex::new(Vec::new()),
    };
    let mut cmd = tokio::process::Command::new("/bin/true");
    prepare_restoring(
        Some(&hook),
        &mut cmd,
        || vec![pyuser.clone()],
        &CallId::tool("tc-10"),
    )
    .unwrap();
    assert_eq!(vec![vec![pyuser]], *hook.restored.lock().unwrap());
    let unread = || -> Vec<(OsString, OsString)> { panic!("no hook, nothing to read") };
    assert_eq!(
        None,
        prepare_restoring(None, &mut cmd, unread, &CallId::tool("tc-11")).unwrap()
    );
    let wrapping = WrappingHook {
        seen: Mutex::new(Vec::new()),
    };
    let receipt = prepare_restoring(Some(&wrapping), &mut cmd, Vec::new, &CallId::tool("tc-12"))
        .unwrap()
        .unwrap();
    assert!(receipt.sandboxed);
    assert_eq!(
        Some("/usr/bin/wrapper"),
        argv(&cmd).first().map(String::as_str)
    );
}

#[test]
fn original_argv_falls_back_to_the_process_cwd() {
    let cmd = tokio::process::Command::new("/bin/true");
    let original = original_argv(&cmd);
    assert_eq!(PathBuf::from("/bin/true"), original.program);
    assert!(original.args.is_empty());
    assert_eq!(std::env::current_dir().unwrap(), original.cwd);
}

/// The kind is a field, the spelling on the wire is the one `CommandTag` embeds, and it
/// parses back.
#[test]
fn call_ids_carry_their_kind_and_round_trip_through_the_wire_spelling() {
    let tool = CallId::tool("tc-5");
    let init = CallId::shell_init("login-env");
    assert_eq!((CallKind::Tool, "tc-5"), (tool.kind(), tool.as_str()));
    assert_eq!(
        (CallKind::ShellInit, "login-env"),
        (init.kind(), init.as_str())
    );
    assert_eq!("tc-5", tool.to_string());
    assert_eq!("shell-init:login-env", init.to_string());
    assert_eq!(tool, CallId::parse(&tool.to_string()));
    assert_eq!(init, CallId::parse(&init.to_string()));
}
