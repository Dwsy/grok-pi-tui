//! Pin for the static-snapshot capture (spawn site 6): with no `SandboxLaunch` installed,
//! `StaticShellSnapshot::init` captures exactly what the golden, `init` without the seam,
//! captures: same script, argv, cwd and env. Linux-only, like the other spawn-site pins.

use std::path::Path;
use std::process::Stdio;

use tokio::io::AsyncReadExt;
use xai_grok_config::shell::UnixShellKind;

use super::{StaticShellSnapshot, rc_file_name, shell_binary};

/// `StaticShellSnapshot::init` without the seam.
async fn main_static_shell_init(cwd: &Path) -> StaticShellSnapshot {
    let shell = xai_grok_config::shell::detect_unix_shell_kind();
    let capture = match shell {
        UnixShellKind::Bash => "builtin alias -p 2>/dev/null; builtin declare -f 2>/dev/null",
        UnixShellKind::Zsh => {
            "{ builtin alias -L; builtin alias -gL; builtin alias -sL } 2>/dev/null; \
             builtin typeset -f 2>/dev/null"
        }
    };
    let script = format!(
        "source \"$HOME/{rc}\" 2>/dev/null; \
         printf '\\x01'; {capture}; printf '\\x01'",
        rc = rc_file_name(shell)
    );
    let mut cmd = tokio::process::Command::new(shell_binary(shell));
    cmd.args(["-lc", &script])
        .current_dir(cwd)
        .stdin(xai_tty_utils::null_stdio())
        .stdout(Stdio::piped())
        .stderr(xai_tty_utils::null_stdio())
        .kill_on_drop(true);
    crate::util::detach_command(&mut cmd);
    xai_grok_sandbox::child_net::restrict_child_network(&mut cmd);
    cmd.envs(crate::util::pager_env());
    #[allow(clippy::disallowed_methods)] // the golden for the pin; waited on by the test
    let mut child = cmd.spawn().unwrap();
    let mut stdout_buf = Vec::new();
    if let Some(stdout) = child.stdout.as_mut() {
        stdout.read_to_end(&mut stdout_buf).await.unwrap();
    }
    assert!(child.wait().await.unwrap().success());
    let stdout = String::from_utf8_lossy(&stdout_buf);
    let parts: Vec<&str> = stdout.split('\x01').collect();
    StaticShellSnapshot {
        snapshot: parts.get(1).map(|s| s.to_string()).unwrap_or_default(),
        shell,
    }
}

#[tokio::test]
async fn cli_facing_static_snapshot_capture_unchanged_without_hook() {
    if !Path::new("/bin/bash").exists() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let cwd = dunce::canonicalize(tmp.path()).unwrap();
    let golden = main_static_shell_init(&cwd).await;
    let live = StaticShellSnapshot::init(&cwd, None).await;
    assert_eq!(golden.shell, live.shell);
    assert_eq!(golden.snapshot, live.snapshot);
}
