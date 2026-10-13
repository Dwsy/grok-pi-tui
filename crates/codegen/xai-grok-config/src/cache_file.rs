//! Shared on-disk cache-file primitives.

use chrono::{DateTime, Duration as ChronoDuration, Utc};

/// Why an on-disk cache read yielded no usable entry. Shared by the models
/// and settings caches so both classify and log misses with one policy.
#[derive(Debug, thiserror::Error)]
pub enum CacheLoadError {
    #[error("cache file not found")]
    NotFound,
    #[error("cache parse failed")]
    ParseFailed,
    #[error("cache signature invalid")]
    SignatureInvalid,
    #[error("cache version mismatch")]
    VersionMismatch,
    #[error("cache {0} mismatch")]
    ScopeMismatch(&'static str),
    #[error("cache stale")]
    Stale,
}

impl CacheLoadError {
    /// A missing file is silent; corruption or tampering (parse/signature)
    /// warns; scope and staleness misses are expected and log at debug.
    pub fn log(&self, path: &std::path::Path) {
        match self {
            CacheLoadError::NotFound => {}
            CacheLoadError::ParseFailed | CacheLoadError::SignatureInvalid => {
                tracing::warn!(path = %path.display(), "{self}");
            }
            CacheLoadError::VersionMismatch
            | CacheLoadError::ScopeMismatch(_)
            | CacheLoadError::Stale => tracing::debug!(path = %path.display(), "{self}"),
        }
    }
}

pub fn is_fresh(fetched_at: DateTime<Utc>, ttl: std::time::Duration) -> bool {
    let Ok(ttl) = ChronoDuration::from_std(ttl) else {
        return false;
    };
    let age = Utc::now().signed_duration_since(fetched_at);
    age >= ChronoDuration::zero() && age < ttl
}

pub fn read_capped(path: &std::path::Path, max: u64) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut buf = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(max + 1)
        .read_to_end(&mut buf)
        .ok()?;
    if buf.len() as u64 > max {
        tracing::debug!("cache file exceeds size cap");
        return None;
    }
    Some(buf)
}

fn unique_tmp_path(path: &std::path::Path) -> std::path::PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    path.with_extension(format!("json.tmp.{}.{n}", std::process::id()))
}

fn sweep_stale_tmp(path: &std::path::Path, ttl: std::time::Duration) {
    let (Some(parent), Some(stem)) = (path.parent(), path.file_name().and_then(|s| s.to_str()))
    else {
        return;
    };
    let prefix = format!("{stem}.tmp.");
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if !name.starts_with(&prefix) {
            continue;
        }
        let is_stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age > ttl);
        if is_stale {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Who may read a cache file [`write_atomic`] creates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheFileMode {
    /// Mode 0600, for the sensitive settings cache.
    OwnerOnly,
    /// The process's default mode.
    Default,
}

/// Atomic replace via a unique temp file and rename.
pub fn write_atomic(
    path: &std::path::Path,
    ttl: std::time::Duration,
    bytes: &[u8],
    mode: CacheFileMode,
) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    sweep_stale_tmp(path, ttl);
    let tmp = unique_tmp_path(path);
    let written = {
        #[cfg(unix)]
        {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create(true).truncate(true);
            match mode {
                CacheFileMode::OwnerOnly => {
                    options.mode(0o600);
                }
                CacheFileMode::Default => {}
            }
            options.open(&tmp).and_then(|mut f| f.write_all(bytes))
        }
        #[cfg(not(unix))]
        {
            let _ = mode;
            std::fs::write(&tmp, bytes)
        }
    }
    .and_then(|()| std::fs::rename(&tmp, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}
