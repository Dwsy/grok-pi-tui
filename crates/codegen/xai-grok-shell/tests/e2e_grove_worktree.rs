//! Product E2E: ACP `create_from_worktree_sync` through a live grove daemon.
//!
//! Session `-w` / fork / resume never call `ensure_daemon`; the daemon is
//! started out of band. This process isolates `GROVE_CONTROL_SOCK` and
//! `XDG_RUNTIME_DIR`; `run_agent_test` isolates `GROK_HOME`. The daemon child
//! also gets a private HOME/XDG.

#![cfg(unix)]

#[allow(dead_code)]
mod acp_harness;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use acp_harness::{AutoApproveClient, connect_and_auth_with_remote, ext_method, run_agent_test};
use serde_json::{Value, json};
use tempfile::TempDir;
use xai_grok_shell::util::config::{ENV_WORKTREE_TYPE, RemoteSettings};

const CREATE_SYNC: &str = "x.ai/git/worktree/create_from_worktree_sync";
const DAEMON_READY: Duration = Duration::from_secs(15);

fn env_truthy(name: &str) -> bool {
    matches!(
        std::env::var(name).ok().as_deref(),
        Some("1")
            | Some("true")
            | Some("TRUE")
            | Some("yes")
            | Some("YES")
            | Some("on")
            | Some("ON")
    )
}

fn fuse_usable() -> bool {
    if std::env::var_os("GROVE_DISABLE_FUSE").is_some() {
        return false;
    }
    let helper = ["fusermount3", "fusermount"].iter().any(|bin| {
        std::env::var_os("PATH")
            .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(bin).is_file()))
    });
    helper
        && Path::new("/dev/fuse").exists()
        && std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/fuse")
            .is_ok()
}

fn require_fuse_or_skip() -> bool {
    if cfg!(not(target_os = "linux")) {
        if env_truthy("GROVE_REQUIRE_FUSE") {
            panic!("GROVE_REQUIRE_FUSE=1 requires Linux");
        }
        eprintln!("skip session grove FUSE cell: not Linux");
        return false;
    }
    if fuse_usable() {
        return true;
    }
    if env_truthy("GROVE_REQUIRE_FUSE") {
        panic!("/dev/fuse required (GROVE_REQUIRE_FUSE truthy) but not usable");
    }
    eprintln!(
        "skip session grove FUSE cell: /dev/fuse not usable (set GROVE_REQUIRE_FUSE=1 to fail hard)"
    );
    false
}

fn grove_bin() -> Option<PathBuf> {
    let p = PathBuf::from(std::env::var_os("GROVE_BIN")?);
    p.is_file().then_some(p)
}

fn require_grove_bin() -> Option<PathBuf> {
    match grove_bin() {
        Some(p) => Some(p),
        None if env_truthy("GROVE_REQUIRE_FUSE") => {
            panic!("GROVE_BIN required (GROVE_REQUIRE_FUSE truthy) but grove is not on disk")
        }
        None => {
            eprintln!("skip session grove cell: grove binary missing (set GROVE_BIN)");
            None
        }
    }
}

struct EnvRestore {
    key: &'static str,
    prev: Option<OsString>,
}

impl EnvRestore {
    fn set(key: &'static str, val: impl AsRef<std::ffi::OsStr>) -> Self {
        let prev = std::env::var_os(key);
        // SAFETY: `run_agent_test` holds the process env lock for the body.
        unsafe {
            std::env::set_var(key, val);
        }
        Self { key, prev }
    }
}

impl Drop for EnvRestore {
    fn drop(&mut self) {
        // SAFETY: inverse of [`EnvRestore::set`].
        unsafe {
            match &self.prev {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

fn chmod_private(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path).expect("meta").permissions();
    perms.set_mode(0o700);
    std::fs::set_permissions(path, perms).expect("chmod 0700");
}

struct IsolatedGrove {
    _tmp: TempDir,
    bin: PathBuf,
    runtime_dir: PathBuf,
    sock: PathBuf,
    child: Option<Child>,
    dests: Vec<PathBuf>,
}

impl IsolatedGrove {
    fn dirs() -> Self {
        let tmp = TempDir::new().expect("tempdir");
        chmod_private(tmp.path());
        let runtime_dir = tmp.path().join("run");
        std::fs::create_dir_all(&runtime_dir).expect("runtime");
        chmod_private(&runtime_dir);
        let sock = runtime_dir.join("grove").join("control.sock");
        std::fs::create_dir_all(sock.parent().expect("sock parent")).expect("grove runtime");
        chmod_private(sock.parent().expect("sock parent"));
        Self {
            _tmp: tmp,
            bin: require_grove_bin().expect("grove bin"),
            runtime_dir,
            sock,
            child: None,
            dests: Vec::new(),
        }
    }

    fn install_env(&self) -> Vec<EnvRestore> {
        vec![
            EnvRestore::set("GROVE_CONTROL_SOCK", &self.sock),
            EnvRestore::set("XDG_RUNTIME_DIR", &self.runtime_dir),
            EnvRestore::set(ENV_WORKTREE_TYPE, "grove"),
        ]
    }

    fn start_daemon(&mut self) {
        let mut cmd = Command::new(&self.bin);
        cmd.env("GROVE_CONTROL_SOCK", &self.sock)
            .env("XDG_RUNTIME_DIR", &self.runtime_dir)
            .env("HOME", self._tmp.path().join("home"))
            .env("XDG_CONFIG_HOME", self._tmp.path().join("config"))
            .env("XDG_DATA_HOME", self._tmp.path().join("share"))
            .args(["daemon", "--foreground"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        for dir in ["home", "config", "share"] {
            let p = self._tmp.path().join(dir);
            std::fs::create_dir_all(&p).unwrap();
            chmod_private(&p);
        }
        #[allow(clippy::disallowed_methods)] // Drop SIGKILLs this child
        let child = cmd.spawn().expect("spawn grove daemon");
        self.child = Some(child);
        self.wait_alive();
    }

    fn wait_alive(&mut self) {
        let start = Instant::now();
        loop {
            if let Some(child) = self.child.as_mut()
                && let Some(status) = child.try_wait().expect("try_wait daemon")
            {
                panic!("grove daemon exited early ({status})");
            }
            let mut cmd = Command::new(&self.bin);
            cmd.env("GROVE_CONTROL_SOCK", &self.sock)
                .env("XDG_RUNTIME_DIR", &self.runtime_dir)
                .args(["status", "--json"])
                .stdin(Stdio::null());
            let out = cmd.output().expect("grove status");
            let alive = out.status.success()
                && serde_json::from_slice::<serde_json::Value>(&out.stdout)
                    .ok()
                    .and_then(|v| v.get("daemon_alive")?.as_bool())
                    == Some(true);
            if alive {
                return;
            }
            if start.elapsed() > DAEMON_READY {
                panic!(
                    "grove daemon did not become alive within {DAEMON_READY:?}: {}",
                    String::from_utf8_lossy(&out.stderr)
                );
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn note_mount(&mut self, dest: &Path) {
        self.dests.push(dest.to_path_buf());
    }

    fn unmount(&self, dest: &Path) {
        let dest_s = dest.display().to_string();
        let mut cmd = Command::new(&self.bin);
        cmd.env("GROVE_CONTROL_SOCK", &self.sock)
            .env("XDG_RUNTIME_DIR", &self.runtime_dir)
            .args(["unmount", dest_s.as_str(), "--forget"])
            .stdin(Stdio::null());
        if cmd.output().map(|o| !o.status.success()).unwrap_or(true) {
            let _ = Command::new("fusermount3")
                .args(["-uz", dest_s.as_str()])
                .output();
        }
    }
}

impl Drop for IsolatedGrove {
    fn drop(&mut self) {
        let dests = self.dests.clone();
        for dest in dests {
            self.unmount(&dest);
        }
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn init_repo(cwd: &Path) {
    xai_test_utils::git::init_git_repo(cwd);
    std::fs::write(cwd.join("tracked.txt"), "hello\n").expect("write tracked");
    xai_test_utils::git::git_commit_all(cwd, "initial");
    std::fs::write(cwd.join("dirty.txt"), "uncommitted\n").expect("write dirty");
}

fn create_params(cwd: &Path, label: &str) -> Value {
    json!({
        "sourceWorktreePath": cwd.to_string_lossy(),
        "newSessionId": label,
        "copyMode": "dirty",
        "label": label,
        "groveWorktree": true,
    })
}

fn strategy_of(resp: &Value) -> &Value {
    resp.get("result")
        .unwrap_or(resp)
        .get("strategy")
        .unwrap_or_else(|| panic!("response carries no strategy report: {resp}"))
}

fn dest_is_live_mount(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    if !path.exists() {
        return false;
    }
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    let Some(parent) = path.parent() else {
        return false;
    };
    std::fs::metadata(parent)
        .map(|p| p.dev() != meta.dev())
        .unwrap_or(false)
}

fn gate_remote() -> RemoteSettings {
    RemoteSettings {
        grove_worktree: Some(true),
        ..Default::default()
    }
}

#[test]
fn worktree_create_grove_success_reports_fuse() {
    if !require_fuse_or_skip() {
        return;
    }
    let Some(_) = require_grove_bin() else {
        return;
    };
    run_agent_test(|cwd, _mock| async move {
        let mut grove = IsolatedGrove::dirs();
        grove.start_daemon();
        let _env = grove.install_env();
        init_repo(&cwd);
        let (conn, _init) =
            connect_and_auth_with_remote(AutoApproveClient, "test", Some(gate_remote())).await;
        let resp = ext_method(&conn, CREATE_SYNC, create_params(&cwd, "grove-ok")).await;
        if let Some(dest) = resp
            .get("result")
            .unwrap_or(&resp)
            .get("worktreePath")
            .and_then(Value::as_str)
        {
            grove.note_mount(Path::new(dest));
        }
        let strategy = strategy_of(&resp);
        assert_eq!(strategy["requestedStrategy"], json!("grove"));
        assert_eq!(
            strategy["resolvedStrategy"],
            json!("grove-fuse"),
            "live daemon + open gate must resolve grove-fuse: {strategy}"
        );
        assert_eq!(
            strategy["transport"],
            json!("fuse"),
            "Linux must never label FUSE as NFS: {strategy}"
        );
        assert!(
            strategy.get("fallbackReason").is_none(),
            "grove success carries no fallback: {strategy}"
        );
        let dest = resp
            .get("result")
            .unwrap_or(&resp)
            .get("worktreePath")
            .and_then(Value::as_str)
            .expect("worktreePath");
        let dest = PathBuf::from(dest);
        assert!(
            dest_is_live_mount(&dest),
            "create dest must be a live mount"
        );
        grove.unmount(&dest);
    });
}

#[test]
fn worktree_create_daemon_down_falls_back_with_named_reason() {
    if !require_fuse_or_skip() {
        return;
    }
    if require_grove_bin().is_none() {
        return;
    }
    run_agent_test(|cwd, _mock| async move {
        let grove = IsolatedGrove::dirs();
        let _env = grove.install_env();
        init_repo(&cwd);
        let (conn, _init) =
            connect_and_auth_with_remote(AutoApproveClient, "test", Some(gate_remote())).await;
        let resp = ext_method(&conn, CREATE_SYNC, create_params(&cwd, "grove-down")).await;
        let strategy = strategy_of(&resp);
        assert_eq!(strategy["requestedStrategy"], json!("grove"));
        assert_eq!(strategy["resolvedStrategy"], json!("copy"));
        let reason = strategy["fallbackReason"]
            .as_str()
            .unwrap_or_else(|| panic!("named fallback required: {strategy}"));
        assert!(
            reason.contains("grove-fuse") && reason.contains("unreachable"),
            "daemon-down must name the grove skip, got {reason}"
        );
    });
}
