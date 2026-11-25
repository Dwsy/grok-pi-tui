use anyhow::Result;
use std::path::{Path, PathBuf};

/// Materialized `pi-grok-tree` extension bundle in a content-addressed cache
/// directory so `jiti`'s `fsCache` hits across launches.
pub(super) struct TreeExtension {
    source_path: PathBuf,
}

impl TreeExtension {
    pub(super) fn path(&self) -> &Path {
        &self.source_path
    }
}

/// Materialize the standalone TS `/tree` session explorer extension along with
/// the shared `pi-grok-tui-kit` primitives.
pub(super) fn write_tree_extension() -> Result<TreeExtension> {
    let root = super::tui_kit::materialize_cached_tui_bundle(
        "pi-grok-tree",
        &[
            (
                "pi-grok-tree/index.ts",
                include_str!("../../../../../../extensions/pi-grok-tree/index.ts"),
            ),
            (
                "pi-grok-tree/tree-data.ts",
                include_str!("../../../../../../extensions/pi-grok-tree/tree-data.ts"),
            ),
            (
                "pi-grok-tree/tree-tui.ts",
                include_str!("../../../../../../extensions/pi-grok-tree/tree-tui.ts"),
            ),
        ],
    )?;

    Ok(TreeExtension {
        source_path: root.join("pi-grok-tree/index.ts"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

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
