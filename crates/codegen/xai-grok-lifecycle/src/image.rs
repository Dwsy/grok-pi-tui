//! Image handlers: JSON manifests baked into the image, one `<name>.json` per handler, same body as a PUT.
//!
//! The directory is read at trigger (and list) time, never cached. An invalid manifest is skipped with one warning.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use tokio::time::Instant;

use crate::exec_spec::ExecSpec;
use crate::token::HandlerName;
use crate::wire::HandlerSpecWire;

/// Cap on one manifest, the same as the PUT body cap.
const MAX_MANIFEST_BYTES: u64 = 4096;

/// Cap on directory entries examined per read, so one scan is bounded work. Far above any baked image; past it the
/// scan stops in directory order, with a warning.
const MAX_IMAGE_DIR_ENTRIES: usize = 4096;

/// Cap on manifest names kept per read. Past it the names last in order are dropped, so which manifests load never
/// depends on directory order.
const MAX_IMAGE_CANDIDATES: usize = 256;

/// Cap on image handlers per trigger; manifests past it (in name order) are skipped.
pub(crate) const MAX_IMAGE_HANDLERS: usize = 32;

const MANIFEST_SUFFIX: &str = ".json";

#[derive(Debug, Clone)]
pub(crate) struct ImageHandler {
    pub(crate) name: HandlerName,
    pub(crate) spec: ExecSpec,
}

/// Reads the manifests on the blocking pool, giving up (with no image handlers) at `until`.
pub(crate) async fn load_image_handlers(dir: PathBuf, until: Instant) -> Vec<ImageHandler> {
    match tokio::time::timeout_at(
        until,
        tokio::task::spawn_blocking(move || load_image_handlers_sync(&dir)),
    )
    .await
    {
        Ok(Ok(handlers)) => handlers,
        Ok(Err(join)) => {
            tracing::warn!(error = %join, "image handler scan failed");
            Vec::new()
        }
        Err(_elapsed) => {
            tracing::warn!("image handler scan timed out; running without image handlers");
            Vec::new()
        }
    }
}

/// Whether `name` has a manifest file in `dir`, valid or not; such a name is reserved for the image.
pub(crate) async fn is_image_name(dir: &Path, name: &HandlerName, until: Instant) -> bool {
    let path = manifest_path(dir, name);
    match tokio::time::timeout_at(until, tokio::fs::try_exists(&path)).await {
        Ok(Ok(exists)) => exists,
        Ok(Err(e)) => {
            tracing::warn!(path = %path.display(), error = %e, "image manifest unreadable; treating the name as reserved");
            true
        }
        Err(_elapsed) => {
            tracing::warn!(path = %path.display(), "image manifest check timed out; treating the name as reserved");
            true
        }
    }
}

fn manifest_path(dir: &Path, name: &HandlerName) -> PathBuf {
    dir.join(format!("{name}{MANIFEST_SUFFIX}"))
}

fn load_image_handlers_sync(dir: &Path) -> Vec<ImageHandler> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            tracing::warn!(dir = %dir.display(), error = %e, "image handler dir unreadable");
            return Vec::new();
        }
    };
    let mut candidates: BTreeMap<HandlerName, PathBuf> = BTreeMap::new();
    let mut dropped = 0_usize;
    for (index, entry) in entries.enumerate() {
        if index >= MAX_IMAGE_DIR_ENTRIES {
            tracing::warn!(
                dir = %dir.display(),
                cap = MAX_IMAGE_DIR_ENTRIES,
                "image handler dir has too many entries; ignoring the rest"
            );
            break;
        }
        let path = match entry {
            Ok(entry) => entry.path(),
            Err(e) => {
                tracing::warn!(dir = %dir.display(), error = %e, "image handler dir entry unreadable");
                continue;
            }
        };
        let Some(stem) = path
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .and_then(|file_name| file_name.strip_suffix(MANIFEST_SUFFIX))
        else {
            continue;
        };
        match HandlerName::try_from(stem) {
            Ok(name) => {
                candidates.insert(name, path);
                if candidates.len() > MAX_IMAGE_CANDIDATES {
                    candidates.pop_last();
                    dropped += 1;
                }
            }
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "skipping image handler manifest");
            }
        }
    }
    if dropped > 0 {
        tracing::warn!(
            dir = %dir.display(),
            dropped,
            cap = MAX_IMAGE_CANDIDATES,
            "too many image handler manifests; ignoring the last by name"
        );
    }
    candidates
        .into_iter()
        .filter_map(|(name, path)| match read_manifest(&path) {
            Ok(spec) => Some(ImageHandler { name, spec }),
            Err(reason) => {
                tracing::warn!(path = %path.display(), %reason, "skipping invalid image handler manifest");
                None
            }
        })
        .take(MAX_IMAGE_HANDLERS)
        .collect()
}

fn read_manifest(path: &Path) -> Result<ExecSpec, String> {
    // Only regular files: opening a FIFO here would block the scan until the deadline.
    let is_file = fs::metadata(path).map_err(|e| e.to_string())?.is_file();
    if !is_file {
        return Err("not a regular file".to_owned());
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .and_then(|file| file.take(MAX_MANIFEST_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|e| e.to_string())?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_MANIFEST_BYTES {
        return Err(format!("larger than {MAX_MANIFEST_BYTES} bytes"));
    }
    let wire: HandlerSpecWire = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    ExecSpec::from_wire(wire).map_err(|class| <&'static str>::from(class).to_owned())
}

#[cfg(test)]
#[path = "image_tests.rs"]
mod tests;
