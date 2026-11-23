use anyhow::{Context, Result};
use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
};
use tempfile::TempDir;

/// Materialized `pi-grok-tree` extension bundle. Preserves the sibling
/// `pi-grok-tui-kit/` directory inside `_source_dir` so relative imports
/// (`../pi-grok-tui-kit/index.ts`) resolve identically to the source tree.
pub(super) struct TreeExtension {
    _source_dir: TempDir,
    source_path: PathBuf,
}

impl TreeExtension {
    pub(super) fn path(&self) -> &Path {
        &self.source_path
    }
}

fn write_source_file(dir: &Path, name: &str, source: &str) -> Result<PathBuf> {
    let path = dir.join(name);
    let mut file = File::create(&path)
        .with_context(|| format!("create tree extension module {name}"))?;
    file.write_all(source.as_bytes())
        .with_context(|| format!("write tree extension module {name}"))?;
    file.flush()
        .with_context(|| format!("flush tree extension module {name}"))?;
    file.sync_all().ok();
    Ok(path)
}

/// Materialize the standalone TS `/tree` session explorer extension along with
/// the shared `pi-grok-tui-kit` primitives.
pub(super) fn write_tree_extension() -> Result<TreeExtension> {
    let source_dir = tempfile::Builder::new()
        .prefix("pi-grok-tree-")
        .tempdir()
        .context("create tree extension source directory")?;
    let tree_dir = source_dir.path().join("pi-grok-tree");
    fs::create_dir_all(&tree_dir).context("create pi-grok-tree dir")?;
    super::tui_kit::materialize_tui_kit(source_dir.path())?;

    let source_path = write_source_file(
        &tree_dir,
        "index.ts",
        include_str!("../../../../../../extensions/pi-grok-tree/index.ts"),
    )?;
    write_source_file(
        &tree_dir,
        "tree-data.ts",
        include_str!("../../../../../../extensions/pi-grok-tree/tree-data.ts"),
    )?;
    write_source_file(
        &tree_dir,
        "tree-tui.ts",
        include_str!("../../../../../../extensions/pi-grok-tree/tree-tui.ts"),
    )?;

    Ok(TreeExtension {
        _source_dir: source_dir,
        source_path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_extension_materializes_with_tui_kit() {
        let ext = write_tree_extension().expect("write");
        let root = ext.path().parent().unwrap().parent().unwrap();
        let index = fs::read_to_string(ext.path()).expect("read index.ts");
        let kit = fs::read_to_string(root.join("pi-grok-tui-kit/list.ts"))
            .expect("read pi-grok-tui-kit/list.ts");
        assert!(index.contains("__pi_session_tree"));
        assert!(kit.contains("ListController"));
    }
}
