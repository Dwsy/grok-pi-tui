use anyhow::{Context, Result};
use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
};
use tempfile::TempDir;

/// Materialized `pi-grok-review` extension bundle. Preserves sibling
/// `pi-grok-tui-kit/` and `pi-grok-mentions/` directories inside `_source_dir`
/// so relative imports (`../pi-grok-tui-kit/index.ts`,
/// `../pi-grok-mentions/file-data.ts`) resolve identically to the source tree.
pub(super) struct ReviewExtension {
    _source_dir: TempDir,
    source_path: PathBuf,
}

impl ReviewExtension {
    pub(super) fn path(&self) -> &Path {
        &self.source_path
    }
}

fn write_source_file(dir: &Path, name: &str, source: &str) -> Result<PathBuf> {
    let path = dir.join(name);
    let mut file = File::create(&path)
        .with_context(|| format!("create review extension module {name}"))?;
    file.write_all(source.as_bytes())
        .with_context(|| format!("write review extension module {name}"))?;
    file.flush()
        .with_context(|| format!("flush review extension module {name}"))?;
    file.sync_all().ok();
    Ok(path)
}

/// Materialize the standalone TS `/review-session` and `/review-message`
/// extension along with its shared `pi-grok-tui-kit` primitives and
/// `pi-grok-mentions/file-data.ts` preview helper.
pub(super) fn write_review_extension() -> Result<ReviewExtension> {
    let source_dir = tempfile::Builder::new()
        .prefix("pi-grok-review-")
        .tempdir()
        .context("create review extension source directory")?;
    let review_dir = source_dir.path().join("pi-grok-review");
    let mentions_dir = source_dir.path().join("pi-grok-mentions");
    let kit_dir = source_dir.path().join("pi-grok-tui-kit");
    fs::create_dir_all(&review_dir).context("create pi-grok-review dir")?;
    fs::create_dir_all(&mentions_dir).context("create shared pi-grok-mentions dir")?;
    fs::create_dir_all(&kit_dir).context("create shared pi-grok-tui-kit dir")?;

    // Shared TS TUI kit primitives
    write_source_file(
        &kit_dir,
        "frame.ts",
        include_str!("../../../../../../extensions/pi-grok-tui-kit/frame.ts"),
    )?;
    write_source_file(
        &kit_dir,
        "icons.ts",
        include_str!("../../../../../../extensions/pi-grok-tui-kit/icons.ts"),
    )?;
    write_source_file(
        &kit_dir,
        "index.ts",
        include_str!("../../../../../../extensions/pi-grok-tui-kit/index.ts"),
    )?;

    // Shared file preview helper from pi-grok-mentions
    write_source_file(
        &mentions_dir,
        "file-data.ts",
        include_str!("../../../../../../extensions/pi-grok-mentions/file-data.ts"),
    )?;

    // Standalone review plugin modules
    let source_path = write_source_file(
        &review_dir,
        "index.ts",
        include_str!("../../../../../../extensions/pi-grok-review/index.ts"),
    )?;
    write_source_file(
        &review_dir,
        "review-data.ts",
        include_str!("../../../../../../extensions/pi-grok-review/review-data.ts"),
    )?;
    write_source_file(
        &review_dir,
        "review-tui.ts",
        include_str!("../../../../../../extensions/pi-grok-review/review-tui.ts"),
    )?;

    Ok(ReviewExtension {
        _source_dir: source_dir,
        source_path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_extension_materializes_with_shared_file_lib() {
        let ext = write_review_extension().expect("write");
        let root = ext.path().parent().unwrap().parent().unwrap();
        let index = fs::read_to_string(ext.path()).expect("read index.ts");
        let shared = fs::read_to_string(root.join("pi-grok-mentions/file-data.ts"))
            .expect("read shared file-data.ts");
        let kit = fs::read_to_string(root.join("pi-grok-tui-kit/frame.ts"))
            .expect("read pi-grok-tui-kit/frame.ts");
        assert!(index.contains("__pi_review_session"));
        assert!(index.contains("__pi_review_message"));
        assert!(shared.contains("loadFileOrDirPreview"));
        assert!(kit.contains("renderSplitOverlayFrame"));
    }
}
