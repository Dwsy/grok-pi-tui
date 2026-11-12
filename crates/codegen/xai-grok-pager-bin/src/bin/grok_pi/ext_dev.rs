use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Development override for grok-pi's bundled Pi extensions.
///
/// The authored sources under `extensions/<name>/` are baked into this binary
/// with `include_str!` and materialized into fresh temp directories at startup.
/// Pointing `--extensions-dev` (or `PI_GROK_EXTENSIONS_DEV`) at the parent folder
/// of those `extensions/` children makes the host hand Pi the on-disk entry files
/// instead, so editing an extension needs a grok-pi restart but no rebuild.
///
/// Feature gating is unchanged: only extensions the host would inject anyway are
/// resolved here, so dev mode swaps the source origin, not the enabled set.
#[derive(Debug)]
pub(super) struct ExtensionDev {
    root: PathBuf,
}

impl ExtensionDev {
    /// Resolve the flag, then the environment variable. `None` keeps embedded bundles.
    pub(super) fn from_arg(arg: Option<&Path>) -> Result<Option<Self>> {
        let Some(root) = arg.map(Path::to_path_buf).or_else(env_root) else {
            return Ok(None);
        };
        let dev = Self::at(&root)?;
        tracing::warn!(
            root = %dev.root.display(),
            "extension dev mode: loading bundled Pi extensions from disk instead of the binary"
        );
        Ok(Some(dev))
    }

    /// Validate an explicit dev root. A typo must fail the launch loudly instead of
    /// silently falling back to the embedded copies.
    pub(super) fn at(root: &Path) -> Result<Self> {
        let canonical = root.canonicalize().with_context(|| {
            format!("extension dev folder {} is not accessible", root.display())
        })?;
        if !canonical.is_dir() {
            anyhow::bail!(
                "extension dev folder {} is not a directory",
                canonical.display()
            );
        }
        Ok(Self { root: canonical })
    }

    /// `<root>/<dir>/index.ts` — the dev copy of one bundled extension.
    pub(super) fn entry(&self, dir: &str) -> Result<PathBuf> {
        let path = self.root.join(dir).join("index.ts");
        if !path.is_file() {
            anyhow::bail!(
                "extension dev mode: {} is missing; the dev folder must mirror the \
                 built-in layout `<parent>/<extension>/index.ts`",
                path.display()
            );
        }
        Ok(path)
    }
}

/// Entry path to hand Pi: the dev copy when dev mode is on, else the bundle this
/// binary materialized. `folder = None` marks Pi-owned resource ids
/// (`builtin:codemode`) that have no grok-pi extension folder.
pub(super) fn source_path(
    dev: Option<&ExtensionDev>,
    folder: Option<&str>,
    embedded: &Path,
) -> Result<PathBuf> {
    match (dev, folder) {
        (Some(dev), Some(folder)) => dev.entry(folder),
        _ => Ok(embedded.to_path_buf()),
    }
}

fn env_root() -> Option<PathBuf> {
    let value = std::env::var_os("PI_GROK_EXTENSIONS_DEV")?;
    if value.is_empty() {
        return None;
    }
    Some(PathBuf::from(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn at_rejects_a_missing_folder() {
        let dir = tempfile::tempdir().expect("tempdir");
        let err = ExtensionDev::at(&dir.path().join("missing")).expect_err("must fail");
        assert!(err.to_string().contains("is not accessible"));
    }

    #[test]
    fn at_rejects_a_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("index.ts");
        std::fs::write(&file, "// not a folder").expect("write");
        let err = ExtensionDev::at(&file).expect_err("must fail");
        assert!(err.to_string().contains("is not a directory"));
    }

    #[test]
    fn entry_requires_the_dev_copy() {
        let dir = tempfile::tempdir().expect("tempdir");
        let dev = ExtensionDev::at(dir.path()).expect("dev root");
        let err = dev
            .entry("pi-grok-bash")
            .expect_err("missing entry must fail");
        assert!(err.to_string().contains("pi-grok-bash/index.ts"));

        std::fs::create_dir_all(dir.path().join("pi-grok-bash")).expect("mkdir");
        std::fs::write(dir.path().join("pi-grok-bash/index.ts"), "// dev").expect("write");
        assert_eq!(
            dev.entry("pi-grok-bash").expect("entry"),
            dir.path()
                .canonicalize()
                .expect("canonical")
                .join("pi-grok-bash")
                .join("index.ts")
        );
    }

    #[test]
    fn source_path_keeps_the_bundle_when_dev_mode_is_off() {
        let embedded = Path::new("/tmp/pi-grok-bash-abc123/index.ts");
        assert_eq!(
            source_path(None, Some("pi-grok-bash"), embedded)
                .expect("path")
                .as_path(),
            embedded
        );
    }

    #[test]
    fn source_path_keeps_pi_owned_resource_ids() {
        let dir = tempfile::tempdir().expect("tempdir");
        let dev = ExtensionDev::at(dir.path()).expect("dev root");
        let builtin = Path::new("builtin:codemode");
        assert_eq!(
            source_path(Some(&dev), None, builtin)
                .expect("path")
                .as_path(),
            builtin
        );
    }
}
