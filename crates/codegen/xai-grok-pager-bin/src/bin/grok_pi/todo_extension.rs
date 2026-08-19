use anyhow::{Context, Result};
use std::io::Write;
use std::path::Path;
use tempfile::NamedTempFile;

/// Materialized built-in grok-pi Todo extension.
pub(super) struct TodoExtension {
    source: NamedTempFile,
}

impl TodoExtension {
    pub(super) fn path(&self) -> &Path {
        self.source.path()
    }
}

pub(super) fn write_todo_extension() -> Result<TodoExtension> {
    let mut source = tempfile::Builder::new()
        .prefix("pi-grok-todo-")
        .suffix(".ts")
        .tempfile()
        .context("create Pi todo extension tempfile")?;
    const SOURCE: &str = include_str!("../../../../../../extensions/pi-grok-todo/index.ts");
    source
        .write_all(SOURCE.as_bytes())
        .context("write Pi todo extension source")?;
    source.flush().context("flush Pi todo extension source")?;
    Ok(TodoExtension { source })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn todo_extension_source_loads() {
        let ext = write_todo_extension().expect("write todo extension");
        let source = std::fs::read_to_string(ext.path()).expect("read todo extension");
        assert!(source.contains("name: \"todo\""));
        assert!(source.contains("details:"));
        assert!(source.contains("getBranch()"));
        assert!(source.contains("registerCommand(\"todos\""));
    }
}
