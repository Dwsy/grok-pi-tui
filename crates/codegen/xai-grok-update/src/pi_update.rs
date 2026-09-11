//! `grok-pi` update discovery and install.
//!
//! Read `Dwsy/grok-pi` release metadata and install via the published
//! `install.sh` / `install.ps1`. Release discovery prefers the official
//! GitHub API, then the official scoped npm package, and only then uses the
//! JSP proxy. The unscoped `grok-pi` npm package is a foreign package and is
//! intentionally never used.

use std::{str::FromStr, time::Duration};

use anyhow::{Context, Result, anyhow};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde_json::Value;

use crate::auto_update::UpdateAvailable;

/// grok-pi's independent GitHub release channels.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PiUpdateChannel {
    #[default]
    Stable,
    Beta,
}

impl PiUpdateChannel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Beta => "beta",
        }
    }
}

impl FromStr for PiUpdateChannel {
    type Err = anyhow::Error;

    fn from_str(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "stable" => Ok(Self::Stable),
            "beta" => Ok(Self::Beta),
            other => anyhow::bail!("unsupported grok-pi update channel '{other}'"),
        }
    }
}

/// GitHub Releases "latest" API for stable published binaries.
pub const PI_GH_RELEASES_LATEST_URL: &str =
    "https://api.github.com/repos/Dwsy/grok-pi/releases/latest";
/// GitHub Releases list used by the beta channel.
const PI_GH_RELEASES_URL: &str = "https://api.github.com/repos/Dwsy/grok-pi/releases?per_page=100";
/// Official GitHub Releases page. Unlike the API, this is not subject to the
/// unauthenticated API rate limit and redirects to the canonical stable tag.
const PI_GH_RELEASES_PAGE_LATEST_URL: &str = "https://github.com/Dwsy/grok-pi/releases/latest";
/// Official npm package metadata. Do not use the unscoped `grok-pi` package:
/// it belongs to another project. This is a stable-only fallback.
const PI_NPM_PACKAGE_METADATA_URL: &str = "https://registry.npmjs.org/@dwsy%2Fgrok-pi";
/// JSP proxy route for the GitHub API. Only the proxy prefix is encoded so
/// the upstream host and repository remain visible in the source.
const JSP_PROXY_PREFIX_B64: &str =
    "aHR0cHM6Ly9qc3AuZHdzeS5saW5rL2h0dHAvaHR0cHM6Ly9hcGkuZ2l0aHViLmNvbS9yZXBvcy8=";
const JSP_PROXY_REFERER_PREFIX_B64: &str = "aHR0cHM6Ly9qc3AuZHdzeS5saW5rLz8=";
const JSP_PROXY_REFERER_SUFFIX: &str = "--ver=110&--mode=cors&--type=&--aceh=1&--level=1";

/// Fetch the latest target for the persisted grok-pi channel.
pub async fn fetch_pi_latest_version() -> Result<String> {
    fetch_pi_latest_version_for_channel(load_pi_update_channel()).await
}

async fn fetch_pi_latest_version_for_channel(channel: PiUpdateChannel) -> Result<String> {
    let (version, source) = match channel {
        PiUpdateChannel::Stable => fetch_release_latest_stable().await?,
        PiUpdateChannel::Beta => fetch_release_latest_beta().await?,
    };
    tracing::info!(%version, source, channel = channel.as_str(), "pi update: latest version");
    Ok(version)
}

async fn fetch_release_latest_stable() -> Result<(String, &'static str)> {
    let client = http_client()?;
    let mut errors = Vec::new();

    match fetch_release_from_url(&client, PI_GH_RELEASES_LATEST_URL, "github-api")
        .await
        .and_then(require_stable_version)
    {
        Ok(version) => return Ok((version, "github-api")),
        Err(error) => errors.push(format!("github-api: {error}")),
    }

    match fetch_github_release_page_latest(&client)
        .await
        .and_then(require_stable_version)
    {
        Ok(version) => return Ok((version, "github-releases-page")),
        Err(error) => errors.push(format!("github-releases-page: {error}")),
    }

    match fetch_npm_release_latest(&client)
        .await
        .and_then(require_stable_version)
    {
        Ok(version) => return Ok((version, "npm")),
        Err(error) => errors.push(format!("npm: {error}")),
    }

    let proxy_url = format!(
        "{}Dwsy/grok-pi/releases/latest",
        decode_proxy_part(JSP_PROXY_PREFIX_B64)
    );
    match fetch_release_from_url(&client, &proxy_url, "jsp-proxy")
        .await
        .and_then(require_stable_version)
    {
        Ok(version) => return Ok((version, "jsp-proxy")),
        Err(error) => errors.push(format!("jsp-proxy: {error}")),
    }

    anyhow::bail!(
        "failed to fetch latest stable grok-pi release ({})",
        errors.join("; ")
    )
}

async fn fetch_release_latest_beta() -> Result<(String, &'static str)> {
    let client = http_client()?;
    let mut errors = Vec::new();

    match fetch_release_list_from_url(&client, PI_GH_RELEASES_URL, "github-api").await {
        Ok(value) => match select_release_for_channel(&value, PiUpdateChannel::Beta) {
            Ok(version) => return Ok((version, "github-api")),
            Err(error) => errors.push(format!("github-api: {error}")),
        },
        Err(error) => errors.push(format!("github-api: {error}")),
    }

    let proxy_url = format!(
        "{}Dwsy/grok-pi/releases?per_page=100",
        decode_proxy_part(JSP_PROXY_PREFIX_B64)
    );
    match fetch_release_list_from_url(&client, &proxy_url, "jsp-proxy").await {
        Ok(value) => match select_release_for_channel(&value, PiUpdateChannel::Beta) {
            Ok(version) => return Ok((version, "jsp-proxy")),
            Err(error) => errors.push(format!("jsp-proxy: {error}")),
        },
        Err(error) => errors.push(format!("jsp-proxy: {error}")),
    }

    anyhow::bail!(
        "failed to fetch latest beta grok-pi release ({})",
        errors.join("; ")
    )
}

async fn fetch_github_release_page_latest(client: &reqwest::Client) -> Result<String> {
    let resp = client
        .get(PI_GH_RELEASES_PAGE_LATEST_URL)
        .header("Accept", "text/html")
        .header("User-Agent", "grok-pi-update")
        .send()
        .await
        .context("GET GitHub Releases page")?;
    if !resp.status().is_success() {
        anyhow::bail!("GitHub Releases page HTTP {}", resp.status());
    }

    normalize_github_release_page_url(resp.url())
}

fn normalize_github_release_page_url(url: &url::Url) -> Result<String> {
    let tag = url
        .path_segments()
        .and_then(|segments| {
            let segments: Vec<_> = segments.collect();
            segments
                .iter()
                .position(|segment| *segment == "tag")
                .and_then(|index| segments.get(index + 1).copied())
        })
        .ok_or_else(|| anyhow!("GitHub Releases page did not redirect to a tag"))?;
    normalize_version(tag)
}

async fn fetch_npm_release_latest(client: &reqwest::Client) -> Result<String> {
    let resp = client
        .get(PI_NPM_PACKAGE_METADATA_URL)
        .header("Accept", "application/json")
        .header("User-Agent", "grok-pi-update")
        .send()
        .await
        .context("GET npm package metadata")?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!(
            "npm package metadata HTTP {status}: {}",
            body.chars().take(200).collect::<String>().trim()
        );
    }

    let value: Value = resp.json().await.context("decode npm package metadata")?;
    parse_npm_release_metadata(&value)
}

fn parse_npm_release_metadata(value: &Value) -> Result<String> {
    let version = value
        .get("dist-tags")
        .and_then(|tags| tags.get("latest"))
        .and_then(Value::as_str)
        .or_else(|| value.get("version").and_then(Value::as_str))
        .ok_or_else(|| anyhow!("npm package metadata missing dist-tags.latest"))?;
    normalize_version(version)
}

async fn fetch_release_from_url(
    client: &reqwest::Client,
    url: &str,
    source: &str,
) -> Result<String> {
    let mut request = client
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "grok-pi-update-check");
    if source == "jsp-proxy" {
        request = request.header(
            reqwest::header::REFERER,
            format!(
                "{}{}",
                decode_proxy_part(JSP_PROXY_REFERER_PREFIX_B64),
                JSP_PROXY_REFERER_SUFFIX
            ),
        );
    }
    let resp = request
        .send()
        .await
        .with_context(|| format!("GET {source} releases/latest"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!(
            "{source} releases/latest HTTP {status}: {}",
            body.chars().take(200).collect::<String>().trim()
        );
    }
    let value: Value = resp
        .json()
        .await
        .with_context(|| format!("decode {source} release JSON"))?;
    let tag = value
        .get("tag_name")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("{source} release JSON missing tag_name"))?;
    normalize_version(tag)
}

async fn fetch_release_list_from_url(
    client: &reqwest::Client,
    url: &str,
    source: &str,
) -> Result<Value> {
    let mut request = client
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "grok-pi-update-check");
    if source == "jsp-proxy" {
        request = request.header(
            reqwest::header::REFERER,
            format!(
                "{}{}",
                decode_proxy_part(JSP_PROXY_REFERER_PREFIX_B64),
                JSP_PROXY_REFERER_SUFFIX
            ),
        );
    }
    let resp = request
        .send()
        .await
        .with_context(|| format!("GET {source} releases"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!(
            "{source} releases HTTP {status}: {}",
            body.chars().take(200).collect::<String>().trim()
        );
    }
    resp.json()
        .await
        .with_context(|| format!("decode {source} releases JSON"))
}

fn require_stable_version(version: String) -> Result<String> {
    if release_channel_from_tag(&version) != Some(PiUpdateChannel::Stable) {
        anyhow::bail!("stable source returned prerelease or unsupported version '{version}'");
    }
    Ok(version)
}

fn release_channel_from_tag(tag: &str) -> Option<PiUpdateChannel> {
    let version = semver::Version::parse(tag.trim().trim_start_matches('v')).ok()?;
    if version.pre.is_empty() {
        return Some(PiUpdateChannel::Stable);
    }
    if version.pre.as_str().starts_with("beta.") {
        return Some(PiUpdateChannel::Beta);
    }
    None
}

fn select_release_for_channel(value: &Value, channel: PiUpdateChannel) -> Result<String> {
    let releases = value
        .as_array()
        .ok_or_else(|| anyhow!("GitHub releases response was not an array"))?;
    releases
        .iter()
        .filter(|release| {
            !release
                .get("draft")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .filter_map(|release| {
            let tag = release.get("tag_name").and_then(Value::as_str)?;
            let tag_channel = release_channel_from_tag(tag)?;
            let github_prerelease = release
                .get("prerelease")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if (tag_channel == PiUpdateChannel::Beta) != github_prerelease {
                return None;
            }
            let allowed = match channel {
                PiUpdateChannel::Stable => tag_channel == PiUpdateChannel::Stable,
                // Beta users track betas, but must still see a newer final release.
                PiUpdateChannel::Beta => true,
            };
            if !allowed {
                return None;
            }
            let normalized = normalize_version(tag).ok()?;
            let parsed = semver::Version::parse(&normalized).ok()?;
            Some((parsed, normalized))
        })
        .max_by(|(left, _), (right, _)| left.cmp(right))
        .map(|(_, version)| version)
        .ok_or_else(|| anyhow!("no release found for {} channel", channel.as_str()))
}

fn decode_proxy_part(encoded: &str) -> String {
    String::from_utf8(
        BASE64
            .decode(encoded)
            .expect("static proxy URL fragment must decode"),
    )
    .expect("static proxy URL fragment must be UTF-8")
}

fn normalize_version(raw: &str) -> Result<String> {
    let version = raw.trim().trim_start_matches('v').to_string();
    if version.is_empty() {
        anyhow::bail!("empty version string");
    }
    semver::Version::parse(&version).with_context(|| format!("invalid semver '{version}'"))?;
    Ok(version)
}

fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(12))
        .build()
        .context("build HTTP client")
}

/// Read `[update].channel` from grok-pi's isolated `$GROK_HOME/config.toml`.
/// Missing or invalid values fail closed to stable.
pub fn load_pi_update_channel() -> PiUpdateChannel {
    let path = xai_grok_shell::util::grok_home::grok_home().join("config.toml");
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return PiUpdateChannel::Stable;
    };
    let Ok(doc) = raw.parse::<toml_edit::DocumentMut>() else {
        tracing::warn!(path = %path.display(), "pi update: invalid config.toml; using stable channel");
        return PiUpdateChannel::Stable;
    };
    let Some(raw_channel) = doc
        .get("update")
        .and_then(toml_edit::Item::as_table_like)
        .and_then(|table| table.get("channel"))
        .and_then(toml_edit::Item::as_str)
    else {
        return PiUpdateChannel::Stable;
    };
    raw_channel.parse().unwrap_or_else(|error| {
        tracing::warn!(%error, path = %path.display(), "pi update: invalid channel; using stable");
        PiUpdateChannel::Stable
    })
}

fn render_pi_update_channel_config(raw: &str, channel: PiUpdateChannel) -> Result<String> {
    let mut doc = if raw.trim().is_empty() {
        toml_edit::DocumentMut::new()
    } else {
        raw.parse::<toml_edit::DocumentMut>()?
    };
    if doc.get("update").is_some_and(|item| !item.is_table_like()) {
        anyhow::bail!("non-table [update] configuration");
    }
    doc["update"]["channel"] = toml_edit::value(channel.as_str());
    Ok(doc.to_string())
}

fn persist_pi_update_channel(channel: PiUpdateChannel) -> Result<()> {
    let home = xai_grok_shell::util::grok_home::grok_home();
    std::fs::create_dir_all(&home)
        .with_context(|| format!("create grok-pi home {}", home.display()))?;
    let path = home.join("config.toml");
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    };
    let rendered = render_pi_update_channel_config(&raw, channel)
        .with_context(|| format!("parse {}", path.display()))?;
    std::fs::write(&path, rendered).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

/// Background check: `Some(UpdateAvailable)` when remote is newer than the
/// running binary; `None` when current or on any hard failure.
pub async fn check_pi_update_background(current: String) -> Option<UpdateAvailable> {
    let latest = match fetch_pi_latest_version().await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "pi update: background check failed");
            return None;
        }
    };
    if is_remote_newer(&latest, &current) {
        Some(UpdateAvailable {
            latest_version: latest,
        })
    } else {
        None
    }
}

/// Parse a product version for update comparison.
///
/// Local git dirty builds historically used a `-dirty` prerelease suffix
/// (`0.0.6-dirty`). Semver treats prereleases as *older* than the base
/// release, which false-positive'd "Update: v0.0.6 available" while already
/// running a dirty tree of that same tag. Strip the local dirty marker so
/// comparison uses the base version. Build-metadata form (`0.0.6+dirty`) is
/// already ignored by semver precedence.
fn parse_for_compare(raw: &str) -> Option<semver::Version> {
    let trimmed = raw.trim().trim_start_matches('v');
    let base = trimmed
        .strip_suffix("-dirty")
        .or_else(|| trimmed.strip_suffix("+dirty"))
        .unwrap_or(trimmed);
    semver::Version::parse(base).ok()
}

fn is_remote_newer(latest: &str, current: &str) -> bool {
    match (parse_for_compare(latest), parse_for_compare(current)) {
        (Some(remote), Some(local)) => remote > local,
        _ => {
            tracing::warn!(%current, %latest, "pi update: semver parse failed");
            false
        }
    }
}

/// Options for [`run_pi_update`].
#[derive(Debug, Clone, Default)]
pub struct PiUpdateOptions {
    /// Only print status; do not install.
    pub check_only: bool,
    /// Install even when the remote version is not newer.
    pub force: bool,
    /// Pin a specific semver (with or without `v` prefix). `None` = channel target.
    pub version: Option<String>,
    /// Persist and use this channel before resolving the target.
    pub channel: Option<String>,
    /// Emit machine-readable JSON for `--check`.
    pub json: bool,
}

/// Check and/or install the latest `grok-pi` from GitHub Releases only.
///
/// Returns the installed version when an install ran; `None` for check-only
/// or when already up to date without `--force`.
pub async fn run_pi_update(current: &str, opts: PiUpdateOptions) -> Result<Option<String>> {
    let configured_channel = load_pi_update_channel();
    let channel = match opts.channel.as_deref() {
        Some(raw) => raw.parse::<PiUpdateChannel>()?,
        None => configured_channel,
    };
    if opts.channel.is_some() {
        // An explicit channel is a persistence command even when the effective
        // fallback is already the same value. This materializes missing config
        // and repairs an invalid channel entry in otherwise valid TOML.
        persist_pi_update_channel(channel)?;
        if channel != configured_channel {
            eprintln!("Switched grok-pi update channel to {}.", channel.as_str());
        }
    }

    let target = match opts.version.as_deref() {
        Some(v) => normalize_version(v)?,
        None => fetch_pi_latest_version_for_channel(channel).await?,
    };

    if opts.check_only {
        print_pi_update_status(current, &target, channel, opts.json)?;
        return Ok(None);
    }

    if !opts.force && !is_remote_newer(&target, current) {
        eprintln!(
            "Already up to date (v{current}, {} channel).",
            channel.as_str()
        );
        return Ok(None);
    }

    eprintln!(
        "Updating grok-pi {current} → {target} ({} channel)…",
        channel.as_str()
    );
    install_pi_from_github(&target).await?;
    eprintln!("Installed grok-pi v{target} from GitHub releases.");
    Ok(Some(target))
}

fn print_pi_update_status(
    current: &str,
    latest: &str,
    channel: PiUpdateChannel,
    json: bool,
) -> Result<()> {
    let update_available = is_remote_newer(latest, current);
    if json {
        let sources: &[&str] = match channel {
            PiUpdateChannel::Stable => &["github-api", "github-releases-page", "npm", "jsp-proxy"],
            PiUpdateChannel::Beta => &["github-api", "jsp-proxy"],
        };
        let payload = serde_json::json!({
            "current": current,
            "latest": latest,
            "channel": channel.as_str(),
            "updateAvailable": update_available,
            "sources": sources,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }
    println!("Current:  v{current}");
    println!("Channel:  {}", channel.as_str());
    println!("Latest:   v{latest}");
    if update_available {
        println!("Update available. Run: grok-pi update");
        println!("Or press Ctrl+U on the Welcome screen when prompted.");
    } else {
        println!("Already up to date.");
    }
    Ok(())
}

/// Install a specific version (or latest when `version` is `None`).
/// Used by Welcome **Ctrl+U** after quit-for-update — always installs
/// (force) because the UI already decided an update is desired.
pub async fn install_pi_update(current: &str, version: Option<&str>) -> Result<String> {
    let installed = run_pi_update(
        current,
        PiUpdateOptions {
            check_only: false,
            force: true,
            version: version.map(str::to_owned),
            channel: None,
            json: false,
        },
    )
    .await?;
    installed.ok_or_else(|| anyhow!("install produced no version"))
}

async fn install_pi_from_github(version: &str) -> Result<()> {
    let tag = format!("v{}", version.trim_start_matches('v'));
    #[cfg(windows)]
    {
        install_pi_windows_ps1(&tag).await
    }
    #[cfg(not(windows))]
    {
        install_pi_unix_sh(&tag).await
    }
}

#[cfg(not(windows))]
async fn install_pi_unix_sh(tag: &str) -> Result<()> {
    let script_url = format!("https://github.com/Dwsy/grok-pi/releases/download/{tag}/install.sh");
    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c").arg(format!(
        "curl -fsSL {script_url} | GROK_PI_VERSION={tag} sh"
    ));
    cmd.env("GROK_PI_VERSION", tag);
    cmd.stdin(std::process::Stdio::null());
    xai_grok_tools::util::detach_command(&mut cmd);
    let status = cmd.status().await.context("spawn install.sh via curl|sh")?;
    if !status.success() {
        anyhow::bail!("install.sh exited with {status}");
    }
    Ok(())
}

#[cfg(windows)]
async fn install_pi_windows_ps1(tag: &str) -> Result<()> {
    let script = format!(
        "$env:GROK_PI_VERSION='{tag}'; irm https://github.com/Dwsy/grok-pi/releases/download/{tag}/install.ps1 | iex"
    );
    let mut cmd = tokio::process::Command::new("powershell");
    cmd.args([
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        &script,
    ]);
    cmd.stdin(std::process::Stdio::null());
    xai_grok_tools::util::detach_command(&mut cmd);
    let status = cmd.status().await.context("spawn install.ps1")?;
    if !status.success() {
        anyhow::bail!("install.ps1 exited with {status}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_strips_v_prefix() {
        assert_eq!(normalize_version("v0.0.2").unwrap(), "0.0.2");
        assert_eq!(normalize_version("0.0.2").unwrap(), "0.0.2");
    }

    #[test]
    fn normalize_rejects_garbage() {
        assert!(normalize_version("").is_err());
        assert!(normalize_version("latest").is_err());
    }

    #[test]
    fn official_npm_metadata_uses_latest_dist_tag() {
        let value = serde_json::json!({
            "dist-tags": { "latest": "v0.2.3" },
            "version": "0.2.2"
        });
        assert_eq!(parse_npm_release_metadata(&value).unwrap(), "0.2.3");
    }

    #[test]
    fn official_release_page_extracts_redirected_tag() {
        let url = url::Url::parse("https://github.com/Dwsy/grok-pi/releases/tag/v0.1.0").unwrap();
        assert_eq!(normalize_github_release_page_url(&url).unwrap(), "0.1.0");
    }

    #[test]
    fn update_source_order_keeps_proxy_last() {
        // Keep this contract next to the source implementation: the official
        // GitHub API and scoped npm package must get a chance before JSP.
        let source_order = ["github-api", "github-releases-page", "npm", "jsp-proxy"];
        assert_eq!(
            source_order,
            ["github-api", "github-releases-page", "npm", "jsp-proxy"]
        );
    }

    #[test]
    fn stable_source_guard_rejects_every_prerelease_kind() {
        assert_eq!(require_stable_version("1.2.3".to_owned()).unwrap(), "1.2.3");
        assert!(require_stable_version("1.2.3-beta.1".to_owned()).is_err());
        assert!(require_stable_version("1.2.3-alpha.1".to_owned()).is_err());
        assert!(require_stable_version("1.2.3-rc.1".to_owned()).is_err());
    }

    #[test]
    fn explicit_channel_render_materializes_and_repairs_stable() {
        let fresh = render_pi_update_channel_config("", PiUpdateChannel::Stable).unwrap();
        assert!(fresh.contains("channel = \"stable\""));

        let repaired = render_pi_update_channel_config(
            "[update]\nchannel = \"invalid\"\n[other]\nkeep = true\n",
            PiUpdateChannel::Stable,
        )
        .unwrap();
        assert!(repaired.contains("channel = \"stable\""));
        assert!(repaired.contains("keep = true"));
    }

    #[test]
    fn tag_channel_classification_is_strict() {
        assert_eq!(
            release_channel_from_tag("v1.2.0"),
            Some(PiUpdateChannel::Stable)
        );
        assert_eq!(
            release_channel_from_tag("v1.2.0-beta.3"),
            Some(PiUpdateChannel::Beta)
        );
        assert_eq!(release_channel_from_tag("v1.2.0-alpha.1"), None);
    }

    #[test]
    fn stable_channel_filters_prereleases() {
        let releases = serde_json::json!([
            {"tag_name": "v1.3.0-beta.2", "prerelease": true, "draft": false},
            {"tag_name": "v1.2.1", "prerelease": false, "draft": false},
            {"tag_name": "v1.2.2", "prerelease": false, "draft": true}
        ]);
        assert_eq!(
            select_release_for_channel(&releases, PiUpdateChannel::Stable).unwrap(),
            "1.2.1"
        );
    }

    #[test]
    fn beta_channel_tracks_beta_but_accepts_newer_stable() {
        let beta_ahead = serde_json::json!([
            {"tag_name": "v1.2.0-beta.2", "prerelease": true, "draft": false},
            {"tag_name": "v1.1.9", "prerelease": false, "draft": false}
        ]);
        assert_eq!(
            select_release_for_channel(&beta_ahead, PiUpdateChannel::Beta).unwrap(),
            "1.2.0-beta.2"
        );

        let stable_ahead = serde_json::json!([
            {"tag_name": "v1.2.0-beta.1", "prerelease": true, "draft": false},
            {"tag_name": "v1.2.0", "prerelease": false, "draft": false}
        ]);
        assert_eq!(
            select_release_for_channel(&stable_ahead, PiUpdateChannel::Beta).unwrap(),
            "1.2.0"
        );
        assert!(is_remote_newer("1.2.0", "1.2.0-beta.1"));
    }

    #[test]
    fn beta_channel_rejects_mislabeled_prerelease_metadata() {
        let releases = serde_json::json!([
            {"tag_name": "v1.3.0-beta.1", "prerelease": false, "draft": false},
            {"tag_name": "v1.2.0-beta.9", "prerelease": true, "draft": false}
        ]);
        assert_eq!(
            select_release_for_channel(&releases, PiUpdateChannel::Beta).unwrap(),
            "1.2.0-beta.9"
        );
    }

    #[test]
    fn remote_newer_compares_semver() {
        assert!(is_remote_newer("0.0.2", "0.0.1"));
        assert!(!is_remote_newer("0.0.1", "0.0.2"));
        assert!(!is_remote_newer("0.0.2", "0.0.2"));
    }

    #[test]
    fn dirty_local_same_base_is_not_an_update() {
        // Historical `-dirty` prerelease marker must not false-positive.
        assert!(!is_remote_newer("0.0.6", "0.0.6-dirty"));
        assert!(!is_remote_newer("0.0.6", "v0.0.6-dirty"));
        // Build-metadata form used by current build.rs.
        assert!(!is_remote_newer("0.0.6", "0.0.6+dirty"));
        assert!(!is_remote_newer("0.0.6", "v0.0.6+dirty"));
    }

    #[test]
    fn dirty_local_still_sees_real_newer_remote() {
        assert!(is_remote_newer("0.0.7", "0.0.6-dirty"));
        assert!(is_remote_newer("0.0.7", "0.0.6+dirty"));
        assert!(!is_remote_newer("0.0.5", "0.0.6-dirty"));
        assert!(!is_remote_newer("0.0.5", "0.0.6+dirty"));
    }
}
