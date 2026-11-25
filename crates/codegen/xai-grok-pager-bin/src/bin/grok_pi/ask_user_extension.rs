use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// Process-private ask_user_question extension + response directory.
pub(super) struct AskUserExtension {
    source_path: PathBuf,
    dir: TempDir,
}

impl AskUserExtension {
    pub(super) fn source_path(&self) -> &Path {
        &self.source_path
    }

    pub(super) fn dir_path(&self) -> &Path {
        self.dir.path()
    }
}

/// Materialize the Q&A extension and empty control directory (retained until Pi exits).
pub(super) fn write_ask_user_extension() -> Result<AskUserExtension> {
    let root = super::tui_kit::materialize_cached_bundle(
        "pi-grok-ask-user-question",
        &[(
            "index.ts",
            include_str!("../../../../../../extensions/pi-grok-ask-user-question/index.ts"),
        )],
    )?;
    let source_path = root.join("index.ts");

    let dir = tempfile::Builder::new()
        .prefix("pi-grok-ask-user-dir-")
        .tempdir()
        .context("create Pi ask_user_question control directory")?;

    Ok(AskUserExtension { source_path, dir })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ask_user_extension_source_loads() {
        let ext = write_ask_user_extension().expect("write");
        let source = std::fs::read_to_string(ext.source_path()).expect("read");
        assert!(source.contains("ask_user_question"));
        assert!(source.contains("PI_GROK_ASK_USER_DIR"));
        assert!(source.contains("right questions to nail the details"));
        assert!(ext.dir_path().is_dir());
    }
}
