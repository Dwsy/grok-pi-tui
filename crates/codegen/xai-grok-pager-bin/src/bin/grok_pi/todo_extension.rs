use anyhow::Result;
use std::path::{Path, PathBuf};

/// Materialized built-in grok-pi Todo extension bundle in a content-addressed
/// cache directory so `jiti`'s `fsCache` hits across launches.
pub(super) struct TodoExtension {
    source_path: PathBuf,
}

impl TodoExtension {
    pub(super) fn path(&self) -> &Path {
        &self.source_path
    }
}

pub(super) fn write_todo_extension() -> Result<TodoExtension> {
    let root = super::tui_kit::materialize_cached_bundle(
        "pi-grok-todo",
        &[
            (
                "index.ts",
                include_str!("../../../../../../extensions/pi-grok-todo/index.ts"),
            ),
            (
                "v1.ts",
                include_str!("../../../../../../extensions/pi-grok-todo/v1.ts"),
            ),
            (
                "v2.ts",
                include_str!("../../../../../../extensions/pi-grok-todo/v2.ts"),
            ),
        ],
    )?;
    Ok(TodoExtension {
        source_path: root.join("index.ts"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn todo_extension_materializes_full_bundle() {
        let ext = write_todo_extension().expect("write todo extension");
        let source = std::fs::read_to_string(ext.path()).expect("read index module");
        let source_dir = ext.path().parent().expect("source dir");
        let v1_source = std::fs::read_to_string(source_dir.join("v1.ts")).expect("read v1 module");
        let v2_source = std::fs::read_to_string(source_dir.join("v2.ts")).expect("read v2 module");

        assert!(source.contains("from \"./v1.ts\""));
        assert!(source.contains("from \"./v2.ts\""));
        assert!(source.contains("PI_GROK_TODO_VERSION"));
        assert!(source.contains("resolveTodoVersion"));

        // v1: Grok-native merge/replace model.
        assert!(v1_source.contains("name: \"todo\""));
        assert!(v1_source.contains("details:"));
        assert!(v1_source.contains("getBranch()"));
        assert!(v1_source.contains("registerCommand(\"todos\""));
        assert!(v1_source.contains("applyV1Merge"));
        assert!(v1_source.contains("applyV1Replace"));
        assert!(v1_source.contains("version: 1 as const"));

        // v2: rich action-based model with steering.
        assert!(v2_source.contains("name: \"todo\""));
        assert!(v2_source.contains("details:"));
        assert!(v2_source.contains("getBranch()"));
        assert!(v2_source.contains("registerCommand(\"todos\""));
        assert!(v2_source.contains("pi-grok:todo-backing"));
        assert!(v2_source.contains("pi-grok-todo-mid-run-nudge/v1"));
        assert!(v2_source.contains("pi-grok-todo-completion-reminder/v1"));
        assert!(v2_source.contains("version: 2 as const"));
    }
}
