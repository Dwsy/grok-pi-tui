use super::{Args, home};
use anyhow::{Context, Result};
use pi_grok_adapter::{PiBootstrap, PiRpc, SpawnConfig};
use std::future::Future;

// ── Extension self-heal (VSCode-style binary search) ─────────────────────────
//
// When Pi RPC bootstrap fails:
// 1. Preserve the original child error (including stderr).
// 2. Fail closed on fatal tool registration conflicts (avoid dropping sandboxes).
// 3. For other failures, confirm Pi boots without extensions.
// 4. Binary-search the ordered `--extension` list and relaunch without the
//    isolated culprit, or with no extensions if a combination cannot be isolated.
//
// The user can run `grok-pi -ne --no-bridge-extensions` to skip all extensions.

const PI_BOOTSTRAP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

pub(super) async fn bootstrap_with_deadline<T>(
    future: impl Future<Output = anyhow::Result<T>>,
    deadline: std::time::Duration,
) -> anyhow::Result<T> {
    tokio::time::timeout(deadline, future).await.map_err(|_| {
        anyhow::anyhow!(
            "Pi RPC bootstrap timed out after {} ms",
            deadline.as_millis()
        )
    })?
}

/// Spawn Pi with the full extension set. If bootstrap fails, run the
/// self-heal bisection and return a working process.
///
/// `interactive` gates the self-heal stdin prompts (upgrade offer, crash
/// report). A background attach runs behind a live TUI and must never steal
/// keystrokes from the Pager input loop, so it passes `false`.
pub(super) async fn spawn_with_extension_self_heal(
    args: &Args,
    cwd: &std::path::Path,
    pi_args: Vec<String>,
    env: &[(String, String)],
    interactive: bool,
) -> Result<(pi_grok_adapter::PiProcess, PiBootstrap, Vec<String>)> {
    let config = SpawnConfig {
        program: args.pi_bin.clone(),
        prefix_args: args.pi_prefix_args.clone(),
        cwd: cwd.to_path_buf(),
        pi_args: pi_args.clone(),
        env: env.to_vec(),
    };

    let process = PiRpc::spawn(config).await?;
    let initial_error = match bootstrap_with_deadline(
        PiBootstrap::load(&process.rpc),
        PI_BOOTSTRAP_TIMEOUT,
    )
    .await
    {
        Ok(bootstrap) => return Ok((process, bootstrap, pi_args)),
        Err(error) => {
            process.rpc.kill().await;
            tracing::warn!(%error, "Pi bootstrap failed; starting extension self-heal");
            error
        }
    };

    // PiRpc includes the exit status and buffered Pi stderr in this error.
    // Preserve it before any retry, rather than replacing it with an upgrade hint.
    let initial_failure = format!("{initial_error:#}");
    eprintln!();
    eprintln!("\x1b[1;31m✗ Pi RPC bootstrap failed\x1b[0m");
    eprintln!("  {initial_failure}");
    eprintln!(
        "  Full Pi stderr: {}/logs/pi-rpc-stderr.log",
        home::display_home(&home::effective_grok_home())
    );

    // Pi 1.4+ exits before RPC bootstrap on duplicate extension tools.
    // Do not let bisection silently drop a security-critical extension like
    // pi-permission-modes and thereby remove its OS-level Bash sandbox.
    if let Some((tool, original_line)) = tool_registration_conflict(&initial_failure) {
        eprintln!();
        eprintln!("\x1b[1;31m✗ Duplicate Pi tool registration: {tool}\x1b[0m");
        let guidance = tool_conflict_guidance(tool, &initial_failure);
        eprintln!("  {guidance}");
        // Deferred Pager attaches may hide stderr; keep the actionable advice
        // in the returned error too, so the user can fix their next launch.
        anyhow::bail!(
            "Pi RPC refused duplicate tool registration ({tool}). {original_line}\n\
             {guidance}\n\
             No extensions were automatically disabled."
        );
    }

    // Extract extension paths from pi_args (pairs: "--extension" <path>).
    let ext_paths = extract_extension_paths(&pi_args);
    if ext_paths.is_empty() {
        // No extensions to bisect — the failure is not extension-related.
        anyhow::bail!(
            "Pi RPC bootstrap failed and no explicit extensions are loaded.\n\
             Original error: {initial_failure}\n\
             Try: grok-pi -ne --no-bridge-extensions"
        );
    }

    // Step 1: Confirm Pi boots with zero extensions.
    let no_ext_args = disable_all_extensions(&pi_args);
    let probe_config = SpawnConfig {
        program: args.pi_bin.clone(),
        prefix_args: args.pi_prefix_args.clone(),
        cwd: cwd.to_path_buf(),
        pi_args: no_ext_args.clone(),
        env: env.to_vec(),
    };
    let probe = PiRpc::spawn(probe_config).await?;
    match bootstrap_with_deadline(PiBootstrap::load(&probe.rpc), PI_BOOTSTRAP_TIMEOUT).await {
        Ok(_) => {
            probe.rpc.kill().await;
        }
        Err(e) => {
            probe.rpc.kill().await;
            anyhow::bail!(
                "Pi RPC bootstrap fails even with zero extensions.\n\
                 This is not an extension problem.\n\
                 Initial error: {initial_failure}\n\
                 Probe error: {e}"
            );
        }
    }

    // Step 2: Binary search for the culprit extension.
    let culprit = bisect_extension_culprit(args, cwd, &no_ext_args, env, &ext_paths).await;

    match culprit {
        Some(bad_path) => {
            let display = std::path::Path::new(&bad_path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| bad_path.clone());

            eprintln!();
            eprintln!("\x1b[1;31m✗ Extension bootstrap failure detected\x1b[0m");
            eprintln!("  Culprit: \x1b[1m{display}\x1b[0m");
            eprintln!("  Path:    {bad_path}");
            eprintln!();
            if offer_grok_pi_upgrade(interactive).await? {
                anyhow::bail!("grok-pi was upgraded; restart grok-pi to retry extensions");
            }
            eprintln!("  \x1b[1mSelf-healing:\x1b[0m relaunching without this extension.");
            eprintln!();
            eprintln!(
                "  To disable all extensions:  \x1b[1mgrok-pi -ne --no-bridge-extensions\x1b[0m"
            );
            eprintln!(
                "  To permanently block it, add to {}/config.toml:",
                home::display_home(&home::effective_grok_home())
            );
            eprintln!("    [pi.resources]");
            eprintln!("    block = [\"{bad_path}\"]");
            eprintln!("  Or project sidecar: .grok-pi/pi-resources.toml  block = [\"...\"]");
            eprintln!();

            // After successful bisection: report automatically (token-gated).
            report_ext_crash(&bad_path, "crash");

            // Step 3: Relaunch without the culprit.
            let healed_args = remove_extension_path(&pi_args, &bad_path);
            let heal_config = SpawnConfig {
                program: args.pi_bin.clone(),
                prefix_args: args.pi_prefix_args.clone(),
                cwd: cwd.to_path_buf(),
                pi_args: healed_args.clone(),
                env: env.to_vec(),
            };
            let process = PiRpc::spawn(heal_config).await?;
            let bootstrap =
                bootstrap_with_deadline(PiBootstrap::load(&process.rpc), PI_BOOTSTRAP_TIMEOUT)
                    .await
                    .with_context(|| {
                        format!("self-heal relaunch still failed. Initial error: {initial_failure}")
                    })?;
            Ok((process, bootstrap, healed_args))
        }
        None => {
            // Bisection couldn't isolate a single culprit (e.g. combination
            // conflict). Fall back to disabling all extensions.
            eprintln!();
            eprintln!("\x1b[1;31m✗ Extension conflict detected\x1b[0m");
            eprintln!("  Could not isolate a single culprit (possible combination conflict).");
            eprintln!();
            if offer_grok_pi_upgrade(interactive).await? {
                anyhow::bail!("grok-pi was upgraded; restart grok-pi to retry extensions");
            }
            eprintln!("  \x1b[1mSelf-healing:\x1b[0m relaunching with all extensions disabled.");
            eprintln!("  To do this manually:  \x1b[1mgrok-pi -ne --no-bridge-extensions\x1b[0m");
            eprintln!();

            report_ext_crash("combo", "combo");

            let process = PiRpc::spawn(SpawnConfig {
                program: args.pi_bin.clone(),
                prefix_args: args.pi_prefix_args.clone(),
                cwd: cwd.to_path_buf(),
                pi_args: no_ext_args.clone(),
                env: env.to_vec(),
            })
            .await?;
            let bootstrap =
                bootstrap_with_deadline(PiBootstrap::load(&process.rpc), PI_BOOTSTRAP_TIMEOUT)
                    .await
                    .with_context(|| {
                        format!(
                            "fallback no-extension launch failed. Initial error: {initial_failure}"
                        )
                    })?;
            Ok((process, bootstrap, no_ext_args))
        }
    }
}

/// Extract the real Pi tool-registration conflict from a multi-line child
/// error, such as: Failed to load extension "...": Tool "bash" conflicts with ...
fn tool_registration_conflict(error: &str) -> Option<(&str, &str)> {
    error.lines().find_map(|line| {
        let (_, rest) = line.split_once("Tool \"")?;
        let (tool, other_owner) = rest.split_once("\" conflicts with ")?;
        if tool.is_empty() || other_owner.trim().is_empty() {
            return None;
        }
        Some((tool, line.trim()))
    })
}

fn tool_conflict_guidance(tool: &str, failure: &str) -> &'static str {
    if tool == "bash" && failure.contains("pi-permission-modes") {
        "Grok-Pi enhanced Bash and pi-permission-modes both register 'bash'.\n\
         Keep the permission extension and disable Grok-Pi enhanced Bash:\n\
         PI_GROK_BASH=0 grok-pi\n\
         Or set [ui] pi_bash = false in ~/.grok-pi/config.toml.\n\
         Verify sandbox coverage of other execution tools separately."
    } else {
        "Configure only one extension to own the conflicting tool before restarting."
    }
}

fn upgrade_requested(key: Option<char>) -> bool {
    matches!(key, Some('y') | Some('Y'))
}

async fn offer_grok_pi_upgrade(interactive: bool) -> Result<bool> {
    use std::io::{IsTerminal, Write};

    // Background attach runs behind a live TUI: never steal stdin or write
    // prompts over the Pager surface; skip the upgrade offer entirely.
    if !interactive || !std::io::stdin().is_terminal() {
        return Ok(false);
    }

    eprintln!("  An outdated Grok-Pi host is one possible cause.");
    eprint!("  Try upgrading Grok-Pi before self-healing? [y/N] ");
    let _ = std::io::stderr().flush();
    let key = read_one_key_char();
    if !upgrade_requested(key) {
        eprintln!("No — continuing with extension self-healing.");
        return Ok(false);
    }

    eprintln!("Yes — upgrading grok-pi…");
    match xai_grok_update::install_pi_update(env!("GROK_PI_VERSION"), None).await {
        Ok(version) => {
            eprintln!("grok-pi v{version} installed. Restart grok-pi to load extensions.");
            Ok(true)
        }
        Err(error) => {
            eprintln!("grok-pi upgrade failed: {error}");
            eprintln!("Continuing with extension self-healing.");
            Ok(false)
        }
    }
}

// ── Extension crash telemetry (privacy: name + package_dir only) ─────────────

const DEFAULT_EXT_TELEMETRY_URL: &str = "https://ext-crash-telemetry.dwsycode.workers.dev";

/// After bisection succeeds: fire-and-forget POST, no prompt.
///
/// Reports on every run shape. The old `[Y/n]` prompt was gated on a TTY, so
/// background-attach and CI crashes were never counted, and its default was
/// already "yes". `GROK_PI_NO_TELEMETRY=1` is the opt-out; one stderr line keeps
/// the send visible.
fn report_ext_crash(path_or_label: &str, kind: &str) {
    if telemetry_disabled() {
        return;
    }

    let (ext_name, package_dir) = if kind == "combo" {
        ("combo".to_owned(), "combo".to_owned())
    } else {
        ext_identity_from_path(path_or_label)
    };

    // Token required server-side (fail closed). Prefer env, then `$GROK_HOME` file.
    let token = std::env::var("REPORT_TOKEN")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(load_ext_telemetry_token_file);
    let Some(token) = token else {
        eprintln!("  Crash telemetry token missing; report skipped.");
        return;
    };

    eprintln!(
        "  Reported {package_dir} ({kind}) to crash telemetry — \
GROK_PI_NO_TELEMETRY=1 to disable."
    );

    let url = std::env::var("GROK_PI_EXT_TELEMETRY_URL")
        .or_else(|_| std::env::var("REPORT_URL"))
        .unwrap_or_else(|_| DEFAULT_EXT_TELEMETRY_URL.to_owned());
    let endpoint = format!("{}/v1/report", url.trim_end_matches('/'));
    let body = serde_json::json!({
        "ext_name": ext_name,
        "package_dir": package_dir,
        "kind": kind,
        "client": "grok-pi",
        "grok_pi_ver": env!("GROK_PI_VERSION"),
    })
    .to_string();

    // Fire-and-forget so self-heal is not blocked on network.
    std::thread::spawn(move || {
        let mut cmd = std::process::Command::new("curl");
        cmd.args([
            "-sS",
            "-m",
            "5",
            "-X",
            "POST",
            &endpoint,
            "-H",
            "content-type: application/json",
            "-H",
            &format!("authorization: Bearer {token}"),
            "-d",
            &body,
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
        let _ = cmd.status();
    });
}

fn telemetry_disabled() -> bool {
    std::env::var("GROK_PI_NO_TELEMETRY")
        .map(|v| {
            let v = v.trim();
            v == "1" || v.eq_ignore_ascii_case("true")
        })
        .unwrap_or(false)
}

fn load_ext_telemetry_token_file() -> Option<String> {
    let home = home::effective_grok_home();
    let path = home.join("ext-telemetry.token");
    let text = std::fs::read_to_string(path).ok()?;
    let t = text.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_owned())
    }
}

/// Privacy-safe identity from an extension path (no absolute path returned).
fn ext_identity_from_path(input: &str) -> (String, String) {
    let raw = input.replace('\\', "/");
    let parts: Vec<&str> = raw.split('/').filter(|p| !p.is_empty()).collect();

    // grok-pi injects its own bundles as `$TMPDIR/pi-grok-<name>-<6 random chars>`
    // (`*_extension.rs` tempdir prefixes). Without this the random suffix splits
    // reports into one bucket per run, or the macOS `T` tmpdir parent is reported
    // as the package name.
    if let Some(label) = parts.iter().find_map(|p| injected_bundle_label(p)) {
        return (label.to_owned(), label.to_owned());
    }

    if let Some(nm) = parts.iter().rposition(|p| *p == "node_modules") {
        if nm + 1 < parts.len() {
            let a = parts[nm + 1];
            if a.starts_with('@') && nm + 2 < parts.len() {
                let name = parts[nm + 2].to_owned();
                let pkg = format!("{a}/{name}");
                return (name, pkg);
            }
            return (a.to_owned(), a.to_owned());
        }
    }

    if let Some(ei) = parts.iter().rposition(|p| *p == "extensions") {
        if ei + 1 < parts.len() && !parts[ei + 1].contains('.') {
            let d = parts[ei + 1].to_owned();
            return (d.clone(), d);
        }
    }

    let leaf = parts.last().copied().unwrap_or("unknown");
    let name = leaf
        .trim_end_matches(".ts")
        .trim_end_matches(".js")
        .trim_end_matches(".mjs")
        .to_owned();
    if parts.len() >= 2 {
        let parent = parts[parts.len() - 2];
        if parent != "node_modules" && !parent.starts_with('.') {
            return (name, parent.to_owned());
        }
    }
    (name.clone(), name)
}

/// Collapse a `pi-grok-<name>-<6 random chars>` injected bundle dir to `pi-grok-<name>`.
fn injected_bundle_label(dir: &str) -> Option<&str> {
    let rest = dir.strip_prefix("pi-grok-")?;
    let (stem, suffix) = rest.rsplit_once('-')?;
    let is_random = suffix.len() == 6 && suffix.chars().all(|c| c.is_ascii_alphanumeric());
    if !is_random || stem.is_empty() {
        return None;
    }
    Some(&dir[.."pi-grok-".len() + stem.len()])
}

/// Read a single key (raw mode on Unix). Falls back to first char of a line.
fn read_one_key_char() -> Option<char> {
    #[cfg(unix)]
    {
        if let Some(c) = read_one_key_raw_unix() {
            return Some(c);
        }
    }
    let mut line = String::new();
    match std::io::stdin().read_line(&mut line) {
        Ok(0) => None,
        Ok(_) => line.chars().next().filter(|c| *c != '\n' && *c != '\r'),
        Err(_) => None,
    }
}

#[cfg(unix)]
fn read_one_key_raw_unix() -> Option<char> {
    use std::io::Read;
    use std::os::fd::AsRawFd;

    let stdin = std::io::stdin();
    let fd = stdin.as_raw_fd();
    // SAFETY: termios get/set on the process stdin fd; restored before return.
    unsafe {
        let mut old: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(fd, &mut old) != 0 {
            return None;
        }
        let mut raw = old;
        raw.c_lflag &= !(libc::ICANON | libc::ECHO);
        raw.c_cc[libc::VMIN] = 1;
        raw.c_cc[libc::VTIME] = 0;
        if libc::tcsetattr(fd, libc::TCSANOW, &raw) != 0 {
            return None;
        }
        let mut buf = [0u8; 1];
        let n = {
            let mut lock = stdin.lock();
            lock.read(&mut buf).unwrap_or(0)
        };
        let _ = libc::tcsetattr(fd, libc::TCSANOW, &old);
        if n == 0 {
            return None;
        }
        Some(buf[0] as char)
    }
}

/// Extract all `--extension <path>` values from pi_args.
pub(super) fn extract_extension_paths(pi_args: &[String]) -> Vec<String> {
    let mut paths = Vec::new();
    let mut i = 0;
    while i < pi_args.len() {
        if pi_args[i] == "--extension" && i + 1 < pi_args.len() {
            paths.push(pi_args[i + 1].clone());
            i += 2;
        } else {
            i += 1;
        }
    }
    paths
}

/// Remove explicit extension paths and disable Pi extension auto-discovery.
pub(super) fn disable_all_extensions(pi_args: &[String]) -> Vec<String> {
    let mut result = Vec::with_capacity(pi_args.len() + 1);
    let mut i = 0;
    while i < pi_args.len() {
        if pi_args[i] == "--extension" && i + 1 < pi_args.len() {
            i += 2;
        } else {
            result.push(pi_args[i].clone());
            i += 1;
        }
    }
    if !result.iter().any(|arg| arg == "--no-extensions") {
        result.push("--no-extensions".to_owned());
    }
    result
}

/// Remove a specific `--extension <path>` pair from pi_args.
fn remove_extension_path(pi_args: &[String], path: &str) -> Vec<String> {
    let mut result = Vec::with_capacity(pi_args.len());
    let mut i = 0;
    while i < pi_args.len() {
        if pi_args[i] == "--extension" && i + 1 < pi_args.len() && pi_args[i + 1] == path {
            i += 2;
        } else {
            result.push(pi_args[i].clone());
            i += 1;
        }
    }
    result
}

/// Binary search the extension list to find the one that crashes Pi.
/// Returns the path of the culprit, or None if isolation fails.
async fn bisect_extension_culprit(
    args: &Args,
    cwd: &std::path::Path,
    base_args: &[String],
    env: &[(String, String)],
    ext_paths: &[String],
) -> Option<String> {
    // If the full set passes, there's no culprit (shouldn't happen).
    if probe_extensions_ok(args, cwd, base_args, env, ext_paths).await {
        return None;
    }

    // Binary search: find the minimal prefix that fails.
    let mut lo = 0usize;
    let mut hi = ext_paths.len();
    while lo + 1 < hi {
        let mid = (lo + hi) / 2;
        if probe_extensions_ok(args, cwd, base_args, env, &ext_paths[..mid]).await {
            lo = mid;
        } else {
            hi = mid;
        }
    }

    // Verify the single extension at index `lo` is the culprit.
    let suspect = &ext_paths[lo];
    if !probe_extensions_ok(args, cwd, base_args, env, std::slice::from_ref(suspect)).await {
        return Some(suspect.clone());
    }

    // The suspect passes alone — it's a combination conflict.
    // Try each extension individually to find one that fails alone.
    for path in ext_paths {
        if !probe_extensions_ok(args, cwd, base_args, env, std::slice::from_ref(path)).await {
            return Some(path.clone());
        }
    }

    None
}

/// Probe whether Pi boots successfully with the given subset of extensions.
async fn probe_extensions_ok(
    args: &Args,
    cwd: &std::path::Path,
    base_args: &[String],
    env: &[(String, String)],
    subset: &[String],
) -> bool {
    let mut probe_args = base_args.to_vec();
    for path in subset {
        probe_args.extend(["--extension".to_string(), path.clone()]);
    }
    let config = SpawnConfig {
        program: args.pi_bin.clone(),
        prefix_args: args.pi_prefix_args.clone(),
        cwd: cwd.to_path_buf(),
        pi_args: probe_args,
        env: env.to_vec(),
    };
    let Ok(process) = PiRpc::spawn(config).await else {
        return false;
    };
    let ok = bootstrap_with_deadline(PiBootstrap::load(&process.rpc), PI_BOOTSTRAP_TIMEOUT)
        .await
        .is_ok();
    process.rpc.kill().await;
    ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_real_pi_140_duplicate_bash_error() {
        let original = "Pi RPC child exited with code 1\n\nPi stderr (last 2 lines):\n\
Error: Failed to load extension \"/tmp/pi-grok-bash-123abc/index.ts\": \
Tool \"bash\" conflicts with /home/user/.pi/agent/npm/pi-permission-modes/src/index.ts\n\
Hint: Start without extensions using \"pi -ne\".";
        let (tool, line) = tool_registration_conflict(original).expect("tool collision");
        assert_eq!(tool, "bash");
        assert!(line.contains("pi-grok-bash-123abc/index.ts"));
        assert!(line.contains("pi-permission-modes/src/index.ts"));
    }

    #[test]
    fn detects_duplicate_registration_regardless_of_load_order_or_tool() {
        assert_eq!(
            tool_registration_conflict(
                "Error: Failed to load extension \"permission/index.ts\": \
Tool \"bash\" conflicts with /tmp/pi-grok-bash-123abc/index.ts"
            )
            .map(|(tool, _)| tool),
            Some("bash")
        );
        assert_eq!(
            tool_registration_conflict(
                "Error: Failed to load extension \"b/index.ts\": \
Tool \"search\" conflicts with a/index.ts"
            )
            .map(|(tool, _)| tool),
            Some("search")
        );
    }

    #[test]
    fn permission_mode_conflict_explains_safe_recovery_in_returned_error() {
        let advice = tool_conflict_guidance(
            "bash",
            "Tool \"bash\" conflicts with /npm/pi-permission-modes/src/index.ts",
        );
        assert!(advice.contains("PI_GROK_BASH=0 grok-pi"));
        assert!(advice.contains("pi_bash = false"));
        assert!(!tool_conflict_guidance("search", "other").contains("PI_GROK_BASH=0"));
    }

    #[test]
    fn upgrade_requires_explicit_yes() {
        assert!(!upgrade_requested(None));
        assert!(!upgrade_requested(Some('\n')));
        assert!(!upgrade_requested(Some('n')));
        assert!(!upgrade_requested(Some(' ')));
        assert!(upgrade_requested(Some('y')));
        assert!(upgrade_requested(Some('Y')));
    }

    #[test]
    fn ignores_non_registration_extension_failures() {
        assert!(tool_registration_conflict("Cannot find module './prompts.ts'").is_none());
        assert!(tool_registration_conflict("Tool \"bash\" failed at runtime").is_none());
        assert!(tool_registration_conflict("Tool \"bash\" conflicts with ").is_none());
    }

    #[test]
    fn injected_bundle_dir_collapses_to_stable_label() {
        assert_eq!(
            ext_identity_from_path("/var/folders/ab/cd/T/pi-grok-rpc-compat-Qedt78"),
            (
                "pi-grok-rpc-compat".to_owned(),
                "pi-grok-rpc-compat".to_owned()
            )
        );
    }

    #[test]
    fn injected_bundle_entry_file_collapses_to_stable_label() {
        assert_eq!(
            ext_identity_from_path("/tmp/pi-grok-bash-8eneBX/index.ts"),
            ("pi-grok-bash".to_owned(), "pi-grok-bash".to_owned())
        );
    }

    #[test]
    fn third_party_extension_keeps_node_modules_identity() {
        assert_eq!(
            ext_identity_from_path("/x/node_modules/@heyhuynhgiabuu/pi-pretty/index.ts"),
            (
                "pi-pretty".to_owned(),
                "@heyhuynhgiabuu/pi-pretty".to_owned()
            )
        );
    }
}
