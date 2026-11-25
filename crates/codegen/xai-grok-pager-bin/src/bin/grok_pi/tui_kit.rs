use anyhow::{Context, Result};
use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
};

/// Write `source` to `dir/rel` only when absent or size-mismatched, using a
/// sibling temp file + atomic rename so concurrent `grok-pi` launches never
/// observe half-written TypeScript files and avoid per-file `fsync` stalls.
pub(super) fn write_cached_file(dir: &Path, rel: &str, source: &str) -> Result<PathBuf> {
    let path = dir.join(rel);
    if fs::metadata(&path)
        .map(|meta| meta.is_file() && meta.len() == source.len() as u64)
        .unwrap_or(false)
    {
        return Ok(path);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create cached extension dir for {rel}"))?;
    }
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("module.ts");
    let tmp_path = path.with_file_name(format!(".{file_name}.tmp.{}", std::process::id()));
    let mut file = File::create(&tmp_path)
        .with_context(|| format!("create cached extension temp module {rel}"))?;
    file.write_all(source.as_bytes())
        .with_context(|| format!("write cached extension module {rel}"))?;
    file.flush()
        .with_context(|| format!("flush cached extension module {rel}"))?;
    drop(file);
    if let Err(err) = fs::rename(&tmp_path, &path) {
        let _ = fs::remove_file(&tmp_path);
        if !path.is_file() {
            return Err(err).with_context(|| format!("rename cached extension module {rel}"));
        }
    }
    Ok(path)
}

/// Materialize an embedded TS extension bundle into a deterministic
/// content-addressed cache directory under `<temp_dir>/grok-pi-ext-cache/`
/// so `jiti`'s on-disk `fsCache` hits across launches instead of recompiling
/// every built-in extension from scratch in a fresh random `TempDir`.
pub(super) fn materialize_cached_bundle(name: &str, files: &[(&str, &str)]) -> Result<PathBuf> {
    let mut hash: u64 = 0xcbf29ce484222325;
    let feed = |h: &mut u64, bytes: &[u8]| {
        for &b in bytes {
            *h ^= u64::from(b);
            *h = h.wrapping_mul(0x100000001b3);
        }
    };
    feed(&mut hash, name.as_bytes());
    for (rel, src) in files {
        feed(&mut hash, b"\0");
        feed(&mut hash, rel.as_bytes());
        feed(&mut hash, b"\0");
        feed(&mut hash, src.as_bytes());
    }
    let root = std::env::temp_dir()
        .join("grok-pi-ext-cache")
        .join(format!("{name}-{hash:016x}"));
    fs::create_dir_all(&root)
        .with_context(|| format!("create cached extension root for {name}"))?;
    for (rel, src) in files {
        write_cached_file(&root, rel, src)?;
    }
    Ok(root)
}

/// Handle to a single-file cached TS extension module (drop-in replacement for
/// `NamedTempFile` with `.path()`).
pub(super) struct CachedExtensionFile(PathBuf);

impl CachedExtensionFile {
    pub(super) fn path(&self) -> &Path {
        &self.0
    }
}

/// Materialize a single-file embedded TS extension into a deterministic
/// content-addressed cache path (`<temp_dir>/grok-pi-ext-cache/<name>-<hash>/index.ts`).
pub(super) fn materialize_cached_file(name: &str, source: &str) -> Result<CachedExtensionFile> {
    let root = materialize_cached_bundle(name, &[("index.ts", source)])?;
    Ok(CachedExtensionFile(root.join("index.ts")))
}

pub(super) const TUI_KIT_FILES: &[(&str, &str)] = &[
    (
        "pi-grok-tui-kit/frame.ts",
        include_str!("../../../../../../extensions/pi-grok-tui-kit/frame.ts"),
    ),
    (
        "pi-grok-tui-kit/icons.ts",
        include_str!("../../../../../../extensions/pi-grok-tui-kit/icons.ts"),
    ),
    (
        "pi-grok-tui-kit/image.ts",
        include_str!("../../../../../../extensions/pi-grok-tui-kit/image.ts"),
    ),
    (
        "pi-grok-tui-kit/list.ts",
        include_str!("../../../../../../extensions/pi-grok-tui-kit/list.ts"),
    ),
    (
        "pi-grok-tui-kit/index.ts",
        include_str!("../../../../../../extensions/pi-grok-tui-kit/index.ts"),
    ),
];

/// Materialize an embedded TS TUI extension bundle together with the shared
/// `pi-grok-tui-kit/` directory in a deterministic content-addressed cache dir.
pub(super) fn materialize_cached_tui_bundle(
    name: &str,
    files: &[(&str, &str)],
) -> Result<PathBuf> {
    let mut all = Vec::with_capacity(TUI_KIT_FILES.len() + files.len());
    all.extend_from_slice(TUI_KIT_FILES);
    all.extend_from_slice(files);
    materialize_cached_bundle(name, &all)
}

/// Materialize the shared `pi-grok-tui-kit/` sibling directory inside `root`
/// so any embedded TS TUI extension can resolve `../pi-grok-tui-kit/index.ts`.
#[allow(dead_code)]
pub(super) fn materialize_tui_kit(root: &Path) -> Result<PathBuf> {
    for (rel, src) in TUI_KIT_FILES {
        write_cached_file(root, rel, src)?;
    }
    Ok(root.join("pi-grok-tui-kit"))
}
