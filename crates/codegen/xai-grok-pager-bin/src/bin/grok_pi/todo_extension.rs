use anyhow::{Context, Result};
use std::{
    fs::File,
    io::Write,
    path::{Path, PathBuf},
};
use tempfile::TempDir;

/// Materialized built-in grok-pi Todo extension bundle.
///
/// The entry imports `./v1.ts` / `./v2.ts`, so the injector must materialize
/// every module of the bundle; the source directory stays alive for the Pi
/// process lifetime so relative imports keep resolving.
pub(super) struct TodoExtension {
    _source_dir: TempDir,
    source_path: PathBuf,
}

impl TodoExtension {
    pub(super) fn path(&self) -> &Path {
        &self.source_path
    }
}

fn write_source_file(dir: &Path, name: &str, source: &str) -> Result<PathBuf> {
    let path = dir.join(name);
    let mut file =
        File::create(&path).with_context(|| format!("create Pi todo extension module {name}"))?;
    file.write_all(source.as_bytes())
        .with_context(|| format!("write Pi todo extension module {name}"))?;
    file.flush()
        .with_context(|| format!("flush Pi todo extension module {name}"))?;
    file.sync_all().ok();
    Ok(path)
}

pub(super) fn write_todo_extension() -> Result<TodoExtension> {
    let source_dir = tempfile::Builder::new()
        .prefix("pi-grok-todo-")
        .tempdir()
        .context("create Pi todo extension source directory")?;
    let source_path = write_source_file(
        source_dir.path(),
        "index.ts",
        include_str!("../../../../../../extensions/pi-grok-todo/index.ts"),
    )?;
    write_source_file(
        source_dir.path(),
        "v1.ts",
        include_str!("../../../../../../extensions/pi-grok-todo/v1.ts"),
    )?;
    write_source_file(
        source_dir.path(),
        "v2.ts",
        include_str!("../../../../../../extensions/pi-grok-todo/v2.ts"),
    )?;
    Ok(TodoExtension {
        _source_dir: source_dir,
        source_path,
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
