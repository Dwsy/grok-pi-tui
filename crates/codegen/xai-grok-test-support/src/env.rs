//! Binary resolution, serial env guards, and git sandbox creation.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use crate::sandbox::TestSandbox;

// First setter wins and later sets are ignored, so parallel tests never race the process-wide choice.
static GROK_BINARY_OVERRIDE: OnceLock<PathBuf> = OnceLock::new();

pub fn set_grok_binary_override(path: PathBuf) {
    let _ = GROK_BINARY_OVERRIDE.set(path);
}

pub fn resolved_grok_binary_override() -> Option<PathBuf> {
    resolved_override()
}

/// Parse env var `key` into `T`, falling back to `default` when it is unset or present-but-unparseable (warning in the latter case).
pub fn env_parse<T: std::str::FromStr>(key: &str, default: T) -> T {
    let Ok(raw) = std::env::var(key) else {
        return default;
    };
    match raw.parse() {
        Ok(value) => value,
        Err(_) => {
            eprintln!("[test-support] ignoring unparseable {key}={raw:?}; using default");
            default
        }
    }
}

/// RAII guard for a single environment variable in `#[serial]` tests. Restoring rather than always unsetting avoids
/// clobbering vars a parent process/harness set (e.g. `RUST_LOG`). Callers MUST be `#[serial_test::serial]`. The `unsafe`
/// `set_var`/`remove_var` are sound only when no other thread accesses the environment concurrently.
pub struct EnvGuard {
    key: &'static str,
    prior: Option<OsString>,
}

impl EnvGuard {
    /// Set `key` to `value` for the guard's lifetime.
    pub fn set(key: &'static str, value: impl AsRef<OsStr>) -> Self {
        let prior = std::env::var_os(key);
        // SAFETY: callers are `#[serial]`, so no other thread touches the env.
        unsafe { std::env::set_var(key, value) };
        Self { key, prior }
    }

    /// Unset `key` for the guard's lifetime.
    pub fn unset(key: &'static str) -> Self {
        let prior = std::env::var_os(key);
        // SAFETY: see [`EnvGuard::set`].
        unsafe { std::env::remove_var(key) };
        Self { key, prior }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: see [`EnvGuard::set`].
        match self.prior.take() {
            Some(v) => unsafe { std::env::set_var(self.key, v) },
            None => unsafe { std::env::remove_var(self.key) },
        }
    }
}

/// # Safety
/// No other thread may access the environment concurrently; call before any other thread exists.
pub unsafe fn isolate_grok_env(home: &Path) {
    // SAFETY: forwarded to the caller.
    unsafe {
        std::env::set_var("GROK_HOME", home);
        std::env::set_var("GROK_TELEMETRY_ENABLED", "false");
        std::env::set_var("GROK_TELEMETRY_MIXPANEL_ENABLED", "false");
        std::env::set_var("GROK_TELEMETRY_MIXPANEL_TOKEN", "");
        std::env::set_var("GROK_TELEMETRY_EVENTS_URL", "");
        std::env::set_var("GROK_TELEMETRY_EVENTS_API_KEY", "");
        std::env::set_var("GROK_FEEDBACK_ENABLED", "false");
        std::env::set_var("GROK_TRACE_UPLOAD", "false");
        for var in [
            "GROK_AUTH",
            "GROK_AUTH_PATH",
            "GROK_DEPLOYMENT_KEY",
            "GROK_MANAGED_CONFIG",
            "GROK_CONFIG",
            "GROK_CONFIG_PATH",
            "GROK_CLI_CHAT_PROXY_BASE_URL",
            "GROK_MODELS_BASE_URL",
            "GROK_MODELS_LIST_URL",
            "XAI_API_KEY",
            "GROK_API_KEY",
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
        ] {
            std::env::remove_var(var);
        }
    }
}

fn workspace_root() -> PathBuf {
    // nth(3): crate is nested three levels below the cargo workspace root.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("workspace root")
        .to_path_buf()
}

fn target_dir() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace_root().join("target"))
}

/// Resolve a workspace binary: the prebuilt `target/debug/<bin>`, else `cargo build -p <package>
/// --bin <bin>` with stdin closed, `pager_env()` applied, and the child detached from the TTY.
pub fn ensure_cargo_bin(package: &str, bin: &str) -> PathBuf {
    let binary = target_dir()
        .join("debug")
        .join(format!("{bin}{}", std::env::consts::EXE_SUFFIX));
    if binary.exists() {
        return binary;
    }

    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let mut cmd = Command::new(&cargo);
    cmd.current_dir(workspace_root())
        .args(["build", "-p", package, "--bin", bin])
        .stdin(std::process::Stdio::null())
        .envs(xai_tty_utils::pager_env());
    xai_tty_utils::detach_std_command(&mut cmd);
    let output = cmd
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn {cargo} to build {bin}: {e}"));

    assert!(
        output.status.success(),
        "failed to build {bin} (exit {:?})\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        binary.exists(),
        "{bin} build completed but binary missing at {}",
        binary.display()
    );
    binary
}

// Reserve four cores for the pager children other test threads spawn, so a cold build does not starve them.
fn build_jobs() -> usize {
    let cores = std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1);
    cores.saturating_sub(4).max(1)
}

pub fn ensure_cargo_bin_with_features(
    package: &str,
    bin: &str,
    features: &[&str],
    target_subdir: &str,
) -> PathBuf {
    let out_target = target_dir().join(target_subdir);
    let binary = out_target
        .join("debug")
        .join(format!("{bin}{}", std::env::consts::EXE_SUFFIX));
    if binary.exists() {
        return binary;
    }

    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let mut cmd = Command::new(&cargo);
    cmd.current_dir(workspace_root())
        .args(["build", "-p", package, "--bin", bin])
        .args(["--features", &features.join(",")])
        .args(["--jobs", &build_jobs().to_string()])
        .env("CARGO_TARGET_DIR", &out_target)
        .stdin(std::process::Stdio::null())
        .envs(xai_tty_utils::pager_env());
    xai_tty_utils::detach_std_command(&mut cmd);
    let output = cmd
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn {cargo} to build {bin}: {e}"));

    assert!(
        output.status.success(),
        "failed to build {bin} with features {features:?} (exit {:?})\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        binary.exists(),
        "{bin} build completed but binary missing at {}",
        binary.display()
    );
    binary
}

pub fn grok_binary() -> PathBuf {
    if let Some(path) = resolved_override() {
        return path;
    }
    if let Some(path) = env_binary("GROK_BINARY") {
        return path;
    }

    if let Ok(path) = std::env::var("CARGO_BIN_EXE_xai-grok-pager") {
        let p = PathBuf::from(path);
        if p.exists() {
            return p;
        }
    }

    ensure_cargo_bin("xai-grok-pager-bin", "xai-grok-pager")
}

fn resolved_override() -> Option<PathBuf> {
    GROK_BINARY_OVERRIDE.get().cloned()
}

pub fn env_binary(key: &str) -> Option<PathBuf> {
    let path = std::env::var(key).ok().filter(|path| !path.is_empty())?;
    let p = PathBuf::from(path);
    assert!(p.exists(), "{key} does not exist: {}", p.display());
    // Bazel leaves this path runfiles-relative, and the child cwd is not the runfiles root.
    Some(std::path::absolute(&p).unwrap_or(p))
}

/// Off `target/debug`, so this build cannot overwrite [`grok_binary`].
fn shipped_pager_target_dir() -> PathBuf {
    target_dir().join("shipped-pager")
}

fn feature_stamp(features: &[&str]) -> String {
    format!("no-default:{}", features.join(","))
}

fn stamp_matches(stamp: &Path, wanted: &str) -> bool {
    std::fs::read_to_string(stamp)
        .ok()
        .is_some_and(|have| have == wanted)
}

fn reuse_shipped_binary(binary: &Path, stamp: &Path, wanted: &str) -> bool {
    binary.exists() && stamp_matches(stamp, wanted)
}

/// The feature stamp rejects a mismatched binary; cargo still runs.
pub fn ensure_default_target_with_features(package: &str, bin: &str, features: &[&str]) -> PathBuf {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    build_shipped_pager(
        package,
        bin,
        features,
        &shipped_pager_target_dir(),
        &workspace_root(),
        &cargo,
    )
}

fn build_shipped_pager(
    package: &str,
    bin: &str,
    features: &[&str],
    out_target: &Path,
    workspace: &Path,
    cargo: &str,
) -> PathBuf {
    let binary = out_target
        .join("debug")
        .join(format!("{bin}{}", std::env::consts::EXE_SUFFIX));
    let stamp = binary.with_extension("features");
    let wanted = feature_stamp(features);
    if binary.exists() && !reuse_shipped_binary(&binary, &stamp, &wanted) {
        std::fs::remove_file(&binary).unwrap_or_else(|e| {
            panic!(
                "failed to reject {bin} with a mismatched feature stamp at {}: {e}",
                binary.display()
            )
        });
    }

    let mut cmd = Command::new(cargo);
    cmd.current_dir(workspace)
        .args(["build", "-p", package, "--bin", bin])
        .args(["--no-default-features", "--features"])
        .arg(features.join(","))
        .args(["--jobs", &build_jobs().to_string()])
        .env("CARGO_TARGET_DIR", out_target)
        .stdin(std::process::Stdio::null())
        .envs(xai_tty_utils::pager_env());
    xai_tty_utils::detach_std_command(&mut cmd);
    let output = cmd
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn {cargo} to build {bin}: {e}"));
    assert!(
        output.status.success(),
        "failed to build {bin} with features {features:?} (exit {:?})\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        binary.exists(),
        "{bin} build completed but binary missing at {}",
        binary.display()
    );
    std::fs::write(&stamp, &wanted).unwrap_or_else(|e| {
        panic!(
            "failed to record features for {bin} at {}: {e}",
            stamp.display()
        )
    });
    binary
}

pub fn git_workdir() -> TestSandbox {
    TestSandbox::builder().git().build()
}

#[cfg(test)]
mod tests {
    use super::{
        build_shipped_pager, feature_stamp, reuse_shipped_binary, shipped_pager_target_dir,
        target_dir,
    };

    #[test]
    fn shipped_pager_binary_is_not_the_grok_binary_path() {
        let name = format!("xai-grok-pager{}", std::env::consts::EXE_SUFFIX);
        let grok = target_dir().join("debug").join(&name);
        let shipped = shipped_pager_target_dir().join("debug").join(&name);
        assert_ne!(grok, shipped);
    }

    #[test]
    fn feature_stamp_mismatch_rejects_the_shipped_binary() {
        let dir = std::env::temp_dir().join("shipped-pager-stamp-mismatch");
        let binary = dir.join(format!("xai-grok-pager{}", std::env::consts::EXE_SUFFIX));
        let stamp = binary.with_extension("features");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&binary, b"shipped").unwrap();
        let wanted = feature_stamp(&["jemalloc", "chat"]);
        std::fs::write(&stamp, feature_stamp(&["jemalloc"])).unwrap();
        assert!(
            !reuse_shipped_binary(&binary, &stamp, &wanted),
            "a mismatched feature stamp must not reuse the binary"
        );
        std::fs::write(&stamp, &wanted).unwrap();
        assert!(
            reuse_shipped_binary(&binary, &stamp, &wanted),
            "a matching feature stamp may reuse the binary"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn matching_feature_stamp_still_asks_cargo() {
        let dir = std::env::temp_dir().join(format!("shipped-pager-cargo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let out_target = dir.join("out");
        let debug = out_target.join("debug");
        std::fs::create_dir_all(&debug).unwrap();
        let binary = debug.join(format!("xai-grok-pager{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&binary, b"stale").unwrap();
        let features = ["jemalloc", "chat"];
        std::fs::write(binary.with_extension("features"), feature_stamp(&features)).unwrap();
        let marker = dir.join("invoked");
        let cargo = dir.join("cargo");
        std::fs::write(
            &cargo,
            format!("#!/bin/sh\nprintf yes > '{}'\n", marker.display()),
        )
        .unwrap();
        let mut perms = std::fs::metadata(&cargo).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        std::fs::set_permissions(&cargo, perms).unwrap();
        let got = build_shipped_pager(
            "xai-grok-pager-bin",
            "xai-grok-pager",
            &features,
            &out_target,
            &dir,
            cargo.to_str().unwrap(),
        );
        assert_eq!(got, binary);
        assert_eq!(
            std::fs::read_to_string(&marker).unwrap_or_default(),
            "yes",
            "matching feature stamp must not return the cached binary without cargo"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
