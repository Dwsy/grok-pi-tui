use super::{Args, home};
use anyhow::{Context, Result};
use pi_grok_adapter::{PiBootstrap, PiRpc, SpawnConfig};
use std::future::Future;

// ── Extension self-heal (VSCode-style binary search) ─────────────────────────
//
// When an extension crashes the Pi RPC child during bootstrap, grok-pi used to
// exit with an opaque error. Now we:
//
// 1. Confirm Pi boots with zero extensions (sanity check).
// 2. Binary-search the ordered `--extension` list to isolate the culprit.
// 3. Print a diagnostic naming the bad extension.
// 4. Relaunch without it (self-heal) so the user is never stuck.
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
pub(super) async fn spawn_with_extension_self_heal(
    args: &Args,
    cwd: &std::path::Path,
    pi_args: Vec<String>,
    env: &[(String, String)],
) -> Result<(pi_grok_adapter::PiProcess, PiBootstrap, Vec<String>)> {
    let config = SpawnConfig {
        program: args.pi_bin.clone(),
        prefix_args: args.pi_prefix_args.clone(),
        cwd: cwd.to_path_buf(),
        pi_args: pi_args.clone(),
        env: env.to_vec(),
    };

    let process = PiRpc::spawn(config).await?;
    match bootstrap_with_deadline(PiBootstrap::load(&process.rpc), PI_BOOTSTRAP_TIMEOUT).await {
        Ok(bootstrap) => return Ok((process, bootstrap, pi_args)),
        Err(error) => {
            process.rpc.kill().await;
            tracing::warn!(%error, "Pi bootstrap failed; starting extension self-heal");
        }
    }

    // Extract extension paths from pi_args (pairs: "--extension" <path>).
    let ext_paths = extract_extension_paths(&pi_args);
    if ext_paths.is_empty() {
        // No extensions to bisect — the failure is not extension-related.
        anyhow::bail!(
            "Pi RPC bootstrap failed and no extensions are loaded.\n\
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
                 Error: {e}"
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
            eprintln!("\x1b[1;31m✗ Extension crash detected\x1b[0m");
            eprintln!("  Culprit: \x1b[1m{display}\x1b[0m");
            eprintln!("  Path:    {bad_path}");
            eprintln!();
            if offer_grok_pi_upgrade().await? {
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

            // After successful bisection: Y / any key = report, only N = skip.
            prompt_and_maybe_report_ext_crash(&bad_path, "crash");

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
                    .context("self-heal relaunch still failed")?;
            Ok((process, bootstrap, healed_args))
        }
        None => {
            // Bisection couldn't isolate a single culprit (e.g. combination
            // conflict). Fall back to disabling all extensions.
            eprintln!();
            eprintln!("\x1b[1;31m✗ Extension conflict detected\x1b[0m");
            eprintln!("  Could not isolate a single culprit (possible combination conflict).");
            eprintln!();
            if offer_grok_pi_upgrade().await? {
                anyhow::bail!("grok-pi was upgraded; restart grok-pi to retry extensions");
            }
            eprintln!("  \x1b[1mSelf-healing:\x1b[0m relaunching with all extensions disabled.");
            eprintln!("  To do this manually:  \x1b[1mgrok-pi -ne --no-bridge-extensions\x1b[0m");
            eprintln!();

            prompt_and_maybe_report_ext_crash("combo", "combo");

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
                    .context("fallback no-extension launch failed")?;
            Ok((process, bootstrap, no_ext_args))
        }
    }
}

async fn offer_grok_pi_upgrade() -> Result<bool> {
    use std::io::{IsTerminal, Write};

    if !std::io::stdin().is_terminal() {
        return Ok(false);
    }

    eprintln!("  This may be caused by an outdated grok-pi host.");
    eprint!("  Upgrade grok-pi before self-healing? [Y/n] ");
    let _ = std::io::stderr().flush();
    let key = read_one_key_char();
    if matches!(key, Some('n') | Some('N')) {
        eprintln!("n — continuing with extension self-healing.");
        return Ok(false);
    }

    match key {
        Some(c) => eprintln!("{c} — upgrading grok-pi…"),
        None => eprintln!("Enter — upgrading grok-pi…"),
    }
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

/// After bisection succeeds: interactive confirm then fire-and-forget POST.
///
/// Key semantics:
/// - `N` / `n` → do **not** report
/// - `Y` / any other key → report
/// Non-TTY → skip (never block CI / piped stdin).
fn prompt_and_maybe_report_ext_crash(path_or_label: &str, kind: &str) {
    use std::io::{IsTerminal, Write};

    if !std::io::stdin().is_terminal() {
        return;
    }

    let (ext_name, package_dir) = if kind == "combo" {
        ("combo".to_owned(), "combo".to_owned())
    } else {
        ext_identity_from_path(path_or_label)
    };

    eprint!(
        "  Report this {kind} to telemetry (name only: {package_dir})? [Y/n]  \
(N = no, any other key = yes) "
    );
    let _ = std::io::stderr().flush();

    let key = read_one_key_char();
    match key {
        Some('n') | Some('N') => {
            eprintln!("n — skipped report.");
            return;
        }
        Some(c) => eprintln!("{c} — reporting…"),
        None => eprintln!("— reporting…"),
    }

    let url = std::env::var("GROK_PI_EXT_TELEMETRY_URL")
        .or_else(|_| std::env::var("REPORT_URL"))
        .unwrap_or_else(|_| DEFAULT_EXT_TELEMETRY_URL.to_owned());
    let endpoint = format!("{}/v1/report", url.trim_end_matches('/'));
    let body = serde_json::json!({
        "ext_name": ext_name,
        "package_dir": package_dir,
        "kind": kind,
        "client": "grok-pi",
    })
    .to_string();

    // Token required server-side (fail closed). Prefer env, then ~/.grok-pi file.
    let token = std::env::var("REPORT_TOKEN")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(load_ext_telemetry_token_file);
    let Some(token) = token else {
        eprintln!(
            "  REPORT_TOKEN missing (set env or ~/.grok-pi/ext-telemetry.token); skip report."
        );
        return;
    };

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
fn extract_extension_paths(pi_args: &[String]) -> Vec<String> {
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
