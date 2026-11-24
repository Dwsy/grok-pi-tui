//! On-demand detection, download, extraction, and `--handshake` verification
//! for the standalone `grok-pi-shell-host` binary (OMP Shell backend).
//!
//! `grok-pi-shell-host` is intentionally NOT bundled inside `grok-pi` release
//! archives so the main binary stays lean. When a user enables `OMP Shell backend`
//! in F2 settings and no local binary is found, the pager invokes
//! [`install_omp_shell_host`] to download the latest standalone archive from the
//! `omp-shell-host-latest` GitHub release, verify its `--handshake`, and install
//! it into `${GROK_HOME:-~/.grok-pi}/bin/grok-pi-shell-host`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use serde::Deserialize;

/// Rolling GitHub release API endpoint hosting standalone `grok-pi-shell-host` archives.
pub const DEFAULT_RELEASE_API_URL: &str =
    "https://api.github.com/repos/Dwsy/grok-pi-tui/releases/tags/omp-shell-host-latest";

/// Live progress snapshot shared between the background download task and the TUI modal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OmpShellInstallPhase {
    Resolving,
    Downloading {
        asset_name: String,
        downloaded: u64,
        total: Option<u64>,
    },
    Extracting,
    Verifying,
    Done(PathBuf),
    Failed(String),
    Cancelled,
}

/// Shared progress handle polled by the TUI modal while an install task runs.
pub type SharedOmpShellInstallProgress = Arc<Mutex<OmpShellInstallPhase>>;

/// Platform-specific binary filename.
pub fn omp_shell_binary_name() -> &'static str {
    if cfg!(windows) {
        "grok-pi-shell-host.exe"
    } else {
        "grok-pi-shell-host"
    }
}

/// Target install directory matching `extensions/pi-grok-bash/shell-backend.ts`:
/// `${GROK_HOME:-~/.grok-pi}/bin`.
pub fn omp_shell_install_dir() -> Result<PathBuf> {
    if let Ok(raw) = std::env::var("GROK_HOME") {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return Ok(PathBuf::from(trimmed).join("bin"));
        }
    }
    let home = xai_dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine home directory for ~/.grok-pi/bin"))?;
    Ok(home.join(".grok-pi").join("bin"))
}

/// Target install path (`${GROK_HOME:-~/.grok-pi}/bin/grok-pi-shell-host`).
pub fn omp_shell_install_path() -> Result<PathBuf> {
    Ok(omp_shell_install_dir()?.join(omp_shell_binary_name()))
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Locate an already-installed `grok-pi-shell-host` binary using the exact
/// precedence order of `extensions/pi-grok-bash/shell-backend.ts`:
/// 1. `PI_GROK_OMP_SHELL_HOST`
/// 2. `${GROK_HOME:-~/.grok-pi}/bin/grok-pi-shell-host`
/// 3. `PATH`
pub fn find_installed_omp_shell_host() -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("PI_GROK_OMP_SHELL_HOST") {
        let trimmed = explicit.trim();
        if !trimmed.is_empty() {
            let candidate = PathBuf::from(trimmed);
            if is_executable_file(&candidate) {
                return Some(candidate);
            }
        }
    }

    if let Ok(home_candidate) = omp_shell_install_path()
        && is_executable_file(&home_candidate)
    {
        return Some(home_candidate);
    }

    let binary_name = omp_shell_binary_name();
    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join(binary_name);
            if is_executable_file(&candidate) {
                return Some(candidate);
            }
        }
    }

    None
}

fn platform_asset_prefix() -> Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Ok("grok-pi-shell-host-macos-arm64"),
        ("macos", "x86_64") => Ok("grok-pi-shell-host-macos-x86_64"),
        ("linux", "aarch64") => Ok("grok-pi-shell-host-linux-arm64"),
        ("linux", "x86_64") => Ok("grok-pi-shell-host-linux-x86_64"),
        ("windows", "x86_64") => Ok("grok-pi-shell-host-windows-x86_64"),
        (os, arch) => bail!(
            "No prebuilt grok-pi-shell-host release is available for {os}-{arch}"
        ),
    }
}

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    assets: Vec<GitHubAsset>,
}

#[derive(Debug, Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
    size: u64,
    #[serde(default)]
    updated_at: String,
}

fn select_matching_asset<'a>(
    assets: &'a [GitHubAsset],
    prefix: &str,
) -> Option<&'a GitHubAsset> {
    let exact_alias = format!("{prefix}.tar.gz");
    if let Some(alias) = assets.iter().find(|a| a.name == exact_alias) {
        return Some(alias);
    }
    let hashed_prefix = format!("{prefix}-");
    assets
        .iter()
        .filter(|a| a.name.starts_with(&hashed_prefix) && a.name.ends_with(".tar.gz"))
        .max_by(|a, b| a.updated_at.cmp(&b.updated_at))
}

fn set_phase(progress: &SharedOmpShellInstallProgress, phase: OmpShellInstallPhase) {
    if let Ok(mut guard) = progress.lock() {
        *guard = phase;
    }
}

fn check_cancelled(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        bail!("cancelled");
    }
    Ok(())
}

/// Download, extract, verify (`--handshake`), and atomically install
/// `grok-pi-shell-host` into `${GROK_HOME:-~/.grok-pi}/bin/grok-pi-shell-host`.
pub async fn install_omp_shell_host(
    cancel: Arc<AtomicBool>,
    progress: SharedOmpShellInstallProgress,
) -> Result<PathBuf> {
    let result = install_omp_shell_host_inner(&cancel, &progress).await;
    match &result {
        Ok(path) => set_phase(&progress, OmpShellInstallPhase::Done(path.clone())),
        Err(_) if cancel.load(Ordering::Relaxed) => {
            set_phase(&progress, OmpShellInstallPhase::Cancelled);
        }
        Err(err) => {
            set_phase(&progress, OmpShellInstallPhase::Failed(format!("{err:#}")));
        }
    }
    result
}

async fn install_omp_shell_host_inner(
    cancel: &AtomicBool,
    progress: &SharedOmpShellInstallProgress,
) -> Result<PathBuf> {
    set_phase(progress, OmpShellInstallPhase::Resolving);
    check_cancelled(cancel)?;

    let prefix = platform_asset_prefix()?;
    let api_url = std::env::var("PI_GROK_OMP_SHELL_RELEASE_URL")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_RELEASE_API_URL.to_string());

    let client = reqwest::Client::builder()
        .user_agent("grok-pi-omp-shell-installer")
        .build()
        .context("failed to initialize HTTP client")?;

    let mut req = client
        .get(&api_url)
        .header("Accept", "application/vnd.github+json");
    if let Some(token) = std::env::var("GITHUB_TOKEN")
        .ok()
        .or_else(|| std::env::var("GH_TOKEN").ok())
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
    {
        req = req.bearer_auth(token);
    }

    let release_resp = req
        .send()
        .await
        .context("failed to query omp-shell-host-latest release")?;
    if !release_resp.status().is_success() {
        bail!(
            "GitHub release lookup failed with HTTP {}",
            release_resp.status()
        );
    }
    check_cancelled(cancel)?;

    let release: GitHubRelease = release_resp
        .json()
        .await
        .context("failed to parse GitHub release metadata")?;
    let asset = select_matching_asset(&release.assets, prefix).ok_or_else(|| {
        anyhow::anyhow!(
            "No matching asset `{prefix}*.tar.gz` found in omp-shell-host-latest release"
        )
    })?;

    let asset_name = asset.name.clone();
    let expected_total = if asset.size > 0 {
        Some(asset.size)
    } else {
        None
    };

    set_phase(
        progress,
        OmpShellInstallPhase::Downloading {
            asset_name: asset_name.clone(),
            downloaded: 0,
            total: expected_total,
        },
    );

    let install_dir = omp_shell_install_dir()?;
    std::fs::create_dir_all(&install_dir)
        .with_context(|| format!("failed to create {}", install_dir.display()))?;

    let work_dir = install_dir.join(format!(".omp-shell-install-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work_dir);
    std::fs::create_dir_all(&work_dir)
        .with_context(|| format!("failed to create staging dir {}", work_dir.display()))?;

    let cleanup = TempDirGuard(work_dir.clone());
    let archive_path = work_dir.join(&asset_name);

    let download_resp = client
        .get(&asset.browser_download_url)
        .send()
        .await
        .context("failed to start grok-pi-shell-host download")?;
    if !download_resp.status().is_success() {
        bail!(
            "asset download failed with HTTP {}",
            download_resp.status()
        );
    }
    let total = download_resp.content_length().or(expected_total);

    let mut file = std::fs::File::create(&archive_path)
        .with_context(|| format!("failed to create {}", archive_path.display()))?;
    let mut downloaded: u64 = 0;
    let mut stream = download_resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        check_cancelled(cancel)?;
        let chunk = chunk.context("download stream interrupted")?;
        file.write_all(&chunk)
            .context("failed to write downloaded archive chunk")?;
        downloaded = downloaded.saturating_add(chunk.len() as u64);
        set_phase(
            progress,
            OmpShellInstallPhase::Downloading {
                asset_name: asset_name.clone(),
                downloaded,
                total,
            },
        );
    }
    file.flush().context("failed to flush downloaded archive")?;
    drop(file);

    check_cancelled(cancel)?;
    set_phase(progress, OmpShellInstallPhase::Extracting);

    let extract_dir = work_dir.join("extract");
    std::fs::create_dir_all(&extract_dir)
        .with_context(|| format!("failed to create {}", extract_dir.display()))?;

    let tar_output = tokio::process::Command::new("tar")
        .arg("-xzf")
        .arg(&archive_path)
        .arg("-C")
        .arg(&extract_dir)
        .output()
        .await
        .context("failed to run `tar` to extract grok-pi-shell-host archive")?;
    if !tar_output.status.success() {
        let stderr = String::from_utf8_lossy(&tar_output.stderr);
        bail!("tar extraction failed: {}", stderr.trim());
    }

    let extracted_bin = extract_dir.join(omp_shell_binary_name());
    if !extracted_bin.is_file() {
        bail!(
            "extracted archive did not contain `{}`",
            omp_shell_binary_name()
        );
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&extracted_bin, std::fs::Permissions::from_mode(0o755))
            .context("failed to mark grok-pi-shell-host executable")?;
    }

    check_cancelled(cancel)?;
    set_phase(progress, OmpShellInstallPhase::Verifying);

    let handshake_output = tokio::process::Command::new(&extracted_bin)
        .arg("--handshake")
        .output()
        .await
        .context("failed to execute `grok-pi-shell-host --handshake`")?;
    if !handshake_output.status.success() {
        let stderr = String::from_utf8_lossy(&handshake_output.stderr);
        bail!("handshake verification failed: {}", stderr.trim());
    }
    let handshake: serde_json::Value = serde_json::from_slice(&handshake_output.stdout)
        .context("handshake output is not valid JSON")?;
    let proto = handshake.get("protocol_version").and_then(|v| v.as_u64());
    let trace = handshake.get("trace_version").and_then(|v| v.as_u64());
    let engine = handshake.get("engine").and_then(|v| v.as_str());
    if proto != Some(1) || trace != Some(1) || engine != Some("omp-shell") {
        bail!("unexpected handshake payload: {handshake}");
    }

    check_cancelled(cancel)?;

    let final_path = install_dir.join(omp_shell_binary_name());
    let staged_final = install_dir.join(format!(
        ".{}.tmp-{}",
        omp_shell_binary_name(),
        std::process::id()
    ));
    std::fs::copy(&extracted_bin, &staged_final)
        .with_context(|| format!("failed to stage {}", staged_final.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staged_final, std::fs::Permissions::from_mode(0o755))
            .context("failed to set executable permissions on staged binary")?;
    }
    std::fs::rename(&staged_final, &final_path).with_context(|| {
        let _ = std::fs::remove_file(&staged_final);
        format!("failed to install binary to {}", final_path.display())
    })?;

    drop(cleanup);
    Ok(final_path)
}

struct TempDirGuard(PathBuf);

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
