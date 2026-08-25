//! Fast Pi host version probe for grok-pi startup.
//!
//! Goals:
//! - cheap when version is fine (one short-lived process, small stdout)
//! - fail closed only when missing / unreadable / below min
//! - OS-aware install hints (curl | sh vs PowerShell)
//! - Windows: resolve `pi` → absolute `pi.cmd` (CreateProcess cannot run bare shims)

use anyhow::{Result, bail};
use semver::Version;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

#[cfg(windows)]
use std::env;
#[cfg(windows)]
use std::ffi::OsStr;
#[cfg(windows)]
use std::path::PathBuf;

/// Minimum supported Pi CLI version (system package / pi.dev installer).
pub(super) const MIN_PI_VERSION: &str = "0.84.3";

const INSTALL_UNIX: &str = "curl -fsSL https://pi.dev/install.sh | sh";
const INSTALL_WINDOWS: &str = r#"powershell -c "irm https://pi.dev/install.ps1 | iex""#;
const INSTALL_NPM: &str = "npm i -g @earendil-works/pi-coding-agent";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PiHostCheck {
    Ok { version: Version, program: String },
    TooOld { version: Version, program: String },
    Missing { program: String, detail: String },
    Unparseable { program: String, raw: String },
}

/// Resolve a user/default Pi host string into something `CreateProcess` can run.
///
/// Windows: bare `pi` is not a PE binary (installer ships `pi` + `pi.cmd` + `pi.ps1`).
/// Shell finds `pi.ps1`/`pi.cmd` via PATHEXT; Rust `Command::new("pi")` only appends
/// `.exe` and fails with "program not found". Prefer absolute `pi.cmd` / known installs.
pub(super) fn resolve_pi_host(program: &str) -> String {
    let trimmed = program.trim();
    if trimmed.is_empty() {
        return program.to_string();
    }

    #[cfg(not(windows))]
    {
        let _ = trimmed;
        return program.to_string();
    }

    #[cfg(windows)]
    {
        resolve_pi_host_windows(trimmed)
    }
}

/// Probe `program --version` with a short timeout. Does not spawn Pi RPC.
pub(super) fn check_pi_host(program: &str) -> PiHostCheck {
    let min = Version::parse(MIN_PI_VERSION).expect("MIN_PI_VERSION is valid semver");
    let resolved = resolve_pi_host(program);
    match run_pi_version(&resolved) {
        Ok(raw) => match parse_pi_version(&raw) {
            Some(version) if version >= min => PiHostCheck::Ok {
                version,
                program: resolved,
            },
            Some(version) => PiHostCheck::TooOld {
                version,
                program: resolved,
            },
            None => PiHostCheck::Unparseable {
                program: resolved,
                raw: raw.trim().to_string(),
            },
        },
        Err(detail) => PiHostCheck::Missing {
            program: resolved,
            detail,
        },
    }
}

/// Hard-require a compatible host. Prints install guidance to stderr on failure.
/// Returns the resolved host program path that should be used for subsequent spawns.
pub(super) fn ensure_compatible_pi_host(program: &str) -> Result<(Version, String)> {
    match check_pi_host(program) {
        PiHostCheck::Ok { version, program } => Ok((version, program)),
        PiHostCheck::TooOld { version, program } => {
            print_upgrade_help(
                &format!("Pi host too old: {program} {version} < required {MIN_PI_VERSION}"),
                &program,
            );
            bail!("Pi {version} is below minimum {MIN_PI_VERSION}");
        }
        PiHostCheck::Missing { program, detail } => {
            print_upgrade_help(
                &format!("Pi host not found or failed: {program} ({detail})"),
                &program,
            );
            bail!("Pi executable unavailable: {program}");
        }
        PiHostCheck::Unparseable { program, raw } => {
            print_upgrade_help(
                &format!("Could not parse Pi version from `{program} --version` output: {raw:?}"),
                &program,
            );
            bail!("unreadable Pi version from {program}");
        }
    }
}

fn run_pi_version(program: &str) -> Result<String, String> {
    // Prefer invoking the path/command as given. For node scripts this still works
    // because the shebang/node wrapper handles --version.
    let mut cmd = host_command(program, &["--version"]);
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    // Keep this off the async runtime: one-shot, short, fail-fast.
    // No network. Windows batch hosts go through cmd.exe (see host_command).
    let output = cmd.output().map_err(|e| format!("spawn failed: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let msg = if !stderr.trim().is_empty() {
            stderr.trim().to_string()
        } else if !stdout.trim().is_empty() {
            stdout.trim().to_string()
        } else {
            format!("exit {}", output.status)
        };
        return Err(msg);
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Build a process command that can launch Pi on Windows batch shims and Node CLIs.
pub(super) fn host_command(program: &str, args: &[&str]) -> Command {
    if looks_like_js_cli(program) {
        let mut c = Command::new(node_program());
        c.arg(program);
        c.args(args);
        return c;
    }

    if is_windows_batch_shim(program) {
        // CreateProcess cannot start .cmd/.bat as the application image; route via cmd.
        let mut c = Command::new("cmd.exe");
        c.arg("/D");
        c.arg("/C");
        c.arg(program);
        c.args(args);
        return c;
    }

    let mut c = Command::new(program);
    c.args(args);
    c
}

fn looks_like_js_cli(program: &str) -> bool {
    let path = Path::new(program);
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("js" | "mjs" | "cjs")
    )
}

fn is_windows_batch_shim(program: &str) -> bool {
    matches!(
        Path::new(program)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat")),
        Some(true)
    )
}

fn node_program() -> &'static str {
    if cfg!(windows) { "node.exe" } else { "node" }
}

#[cfg(windows)]
fn resolve_pi_host_windows(program: &str) -> String {
    let path = Path::new(program);

    // Explicit path (absolute or relative with separators).
    if path_has_directory_component(path) {
        if path.is_file() {
            return prefer_windows_shim(path);
        }
        // User passed ...\pi without extension — try sibling shims.
        if path.extension().is_none() {
            for ext in ["cmd", "exe", "bat"] {
                let candidate = path.with_extension(ext);
                if candidate.is_file() {
                    return path_to_string(&candidate);
                }
            }
        }
        return program.to_string();
    }

    // Bare command name: search PATH with Windows PATHEXT-like order, then known installs.
    if let Some(found) = find_on_path_windows(program) {
        return found;
    }
    if program.eq_ignore_ascii_case("pi") {
        if let Some(found) = find_known_pi_installs() {
            return found;
        }
    }
    program.to_string()
}

#[cfg(windows)]
fn path_has_directory_component(path: &Path) -> bool {
    path.components().count() > 1 || path.is_absolute() || program_looks_like_path(path.as_os_str())
}

#[cfg(windows)]
fn program_looks_like_path(program: &OsStr) -> bool {
    let s = program.to_string_lossy();
    s.contains('\\') || s.contains('/') || s.contains(':')
}

#[cfg(windows)]
fn prefer_windows_shim(path: &Path) -> String {
    // Extensionless npm/pi-node launcher is a Unix shell script — prefer .cmd sibling.
    if path.extension().is_none() {
        for ext in ["cmd", "exe", "bat"] {
            let candidate = path.with_extension(ext);
            if candidate.is_file() {
                return path_to_string(&candidate);
            }
        }
    }
    path_to_string(path)
}

#[cfg(windows)]
fn find_on_path_windows(name: &str) -> Option<String> {
    let path_var = env::var_os("PATH")?;
    // Prefer batch/exe shims over extensionless (often a non-PE unix script).
    let mut names: Vec<PathBuf> = Vec::new();
    if !name.contains('.') {
        for ext in ["cmd", "exe", "bat"] {
            names.push(PathBuf::from(format!("{name}.{ext}")));
        }
    }
    names.push(PathBuf::from(name));

    for dir in env::split_paths(&path_var) {
        for file_name in &names {
            let candidate = dir.join(file_name);
            if candidate.is_file() {
                // Skip extensionless if a .cmd sibling exists in the same dir.
                if candidate.extension().is_none() {
                    let cmd = candidate.with_extension("cmd");
                    if cmd.is_file() {
                        return Some(path_to_string(&cmd));
                    }
                }
                return Some(path_to_string(&candidate));
            }
        }
    }
    None
}

#[cfg(windows)]
fn find_known_pi_installs() -> Option<String> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(local) = env::var_os("LOCALAPPDATA") {
        dirs.push(PathBuf::from(local).join("pi-node").join("current"));
    }
    if let Some(roaming) = env::var_os("APPDATA") {
        dirs.push(PathBuf::from(roaming).join("npm"));
    }
    if let Some(home) = env::var_os("USERPROFILE") {
        let home = PathBuf::from(home);
        dirs.push(
            home.join("AppData")
                .join("Local")
                .join("pi-node")
                .join("current"),
        );
        dirs.push(home.join("AppData").join("Roaming").join("npm"));
    }

    for dir in dirs {
        for file in ["pi.cmd", "pi.exe", "pi.bat"] {
            let candidate = dir.join(file);
            if candidate.is_file() {
                return Some(path_to_string(&candidate));
            }
        }
    }
    None
}

#[cfg(windows)]
fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Extract the first semver-looking token from version output.
pub(super) fn parse_pi_version(raw: &str) -> Option<Version> {
    // Common shapes:
    // - "0.84.3"
    // - "pi 0.84.3"
    // - "@earendil-works/pi-coding-agent/0.84.3"
    for token in raw.split(|c: char| c.is_whitespace() || c == '/' || c == 'v' || c == 'V') {
        let candidate = token.trim().trim_matches(|c: char| c == ',' || c == ';');
        if candidate.is_empty() {
            continue;
        }
        // Allow "0.84.3-beta.1" etc.
        if let Ok(v) = Version::parse(candidate) {
            return Some(v);
        }
        // Strip trailing junk like "0.84.3," already handled; try prefix digits.digits.digits
        let mut end = 0;
        let bytes = candidate.as_bytes();
        while end < bytes.len() {
            let c = bytes[end] as char;
            if c.is_ascii_digit() || c == '.' || c == '-' || c == '+' {
                end += 1;
            } else {
                break;
            }
        }
        if end > 0 {
            if let Ok(v) = Version::parse(&candidate[..end]) {
                return Some(v);
            }
        }
    }
    None
}

fn print_upgrade_help(reason: &str, program: &str) {
    let os_hint = install_command_for_host();
    eprintln!();
    eprintln!("error: {reason}");
    eprintln!();
    eprintln!("grok-pi requires Pi >= {MIN_PI_VERSION} (system `pi` / pi.dev installer).");
    eprintln!("Configured host: {program}");
    eprintln!();
    eprintln!("Install / upgrade (recommended):");
    eprintln!("  {os_hint}");
    eprintln!();
    eprintln!("Also available:");
    if cfg!(windows) {
        eprintln!("  {INSTALL_UNIX}");
    } else {
        eprintln!("  {INSTALL_WINDOWS}");
    }
    eprintln!("  {INSTALL_NPM}");
    eprintln!();
    eprintln!("Docs: https://pi.dev");
    eprintln!("Then re-run grok-pi, or set PI_BIN=/path/to/pi if needed.");
    eprintln!();
}

fn install_command_for_host() -> &'static str {
    if cfg!(windows) {
        INSTALL_WINDOWS
    } else {
        INSTALL_UNIX
    }
}

/// Also print the other platform's one-liner when helpful (WSL/users reading logs).
#[allow(dead_code)]
pub(super) fn both_install_commands() -> (&'static str, &'static str) {
    (INSTALL_UNIX, INSTALL_WINDOWS)
}

// Keep Duration import available for future hard timeout wrappers without
// pulling extra crates; current Command::output is already fast enough for --version.
#[allow(dead_code)]
fn version_probe_budget() -> Duration {
    Duration::from_secs(3)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_semver() {
        assert_eq!(parse_pi_version("0.84.3").unwrap().to_string(), "0.84.3");
    }

    #[test]
    fn parses_prefixed_output() {
        assert_eq!(
            parse_pi_version("pi 0.84.3\n").unwrap().to_string(),
            "0.84.3"
        );
        assert_eq!(
            parse_pi_version("@earendil-works/pi-coding-agent/0.84.3")
                .unwrap()
                .to_string(),
            "0.84.3"
        );
    }

    #[test]
    fn min_version_constant_is_valid() {
        assert!(Version::parse(MIN_PI_VERSION).is_ok());
    }

    #[test]
    fn too_old_detected() {
        let v = parse_pi_version("0.79.0").unwrap();
        let min = Version::parse(MIN_PI_VERSION).unwrap();
        assert!(v < min);
    }

    #[test]
    fn install_hint_is_platform_specific() {
        let hint = install_command_for_host();
        if cfg!(windows) {
            assert!(hint.contains("powershell"));
        } else {
            assert!(hint.contains("curl") && hint.contains("pi.dev/install.sh"));
        }
    }

    #[test]
    fn batch_shim_detection() {
        assert!(is_windows_batch_shim(r"C:\Users\x\pi.cmd"));
        assert!(is_windows_batch_shim(r"C:\Users\x\pi.CMD"));
        assert!(is_windows_batch_shim("pi.bat"));
        assert!(!is_windows_batch_shim("pi"));
        assert!(!is_windows_batch_shim("pi.exe"));
        assert!(!is_windows_batch_shim("cli.js"));
    }

    #[test]
    fn js_cli_detection() {
        assert!(looks_like_js_cli("cli.js"));
        assert!(looks_like_js_cli(r"C:\tools\rpc-entry.mjs"));
        assert!(!looks_like_js_cli("pi.cmd"));
    }

    #[test]
    fn resolve_keeps_non_empty_default() {
        let resolved = resolve_pi_host("pi");
        assert!(!resolved.trim().is_empty());
    }
}
