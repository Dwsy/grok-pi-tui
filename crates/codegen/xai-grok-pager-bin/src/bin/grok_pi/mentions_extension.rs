use anyhow::{Context, Result};
use std::{
    fs::File,
    io::Write,
    path::{Path, PathBuf},
};
use tempfile::TempDir;

/// Materialized `@` object-mentions extension bundle. `_source_dir` must stay
/// alive for the Pi process lifetime so relative imports between `index.ts`
/// and its helper modules keep resolving.
pub(super) struct MentionsExtension {
    _source_dir: TempDir,
    source_path: PathBuf,
}

impl MentionsExtension {
    pub(super) fn path(&self) -> &Path {
        &self.source_path
    }
}

fn write_source_file(dir: &Path, name: &str, source: &str) -> Result<PathBuf> {
    let path = dir.join(name);
    let mut file = File::create(&path)
        .with_context(|| format!("create mentions extension module {name}"))?;
    file.write_all(source.as_bytes())
        .with_context(|| format!("write mentions extension module {name}"))?;
    file.flush()
        .with_context(|| format!("flush mentions extension module {name}"))?;
    file.sync_all().ok();
    Ok(path)
}

/// Materialize the built-in `@` object-mentions extension (`@skill`, `@session`,
/// `@mcp`, `@file`) along with its TS session-picker TUI modules.
///
/// Publishes its catalog via `setStatus("__pi_grok_mentions__", ...)` which
/// `pi-grok-adapter` forwards as `pi/ui/mentions_catalog`, and returns chosen
/// items via `setStatus("__pi_grok_mention_insert__", ...)` → `pi/ui/mention_insert`.
pub(super) fn write_mentions_extension() -> Result<MentionsExtension> {
    let source_dir = tempfile::Builder::new()
        .prefix("pi-grok-mentions-")
        .tempdir()
        .context("create mentions extension source directory")?;
    let source_path = write_source_file(
        source_dir.path(),
        "index.ts",
        include_str!("../../../../../../extensions/pi-grok-mentions/index.ts"),
    )?;
    write_source_file(
        source_dir.path(),
        "session-data.ts",
        include_str!("../../../../../../extensions/pi-grok-mentions/session-data.ts"),
    )?;
    write_source_file(
        source_dir.path(),
        "session-picker-tui.ts",
        include_str!("../../../../../../extensions/pi-grok-mentions/session-picker-tui.ts"),
    )?;
    write_source_file(
        source_dir.path(),
        "skill-data.ts",
        include_str!("../../../../../../extensions/pi-grok-mentions/skill-data.ts"),
    )?;
    write_source_file(
        source_dir.path(),
        "skill-picker-tui.ts",
        include_str!("../../../../../../extensions/pi-grok-mentions/skill-picker-tui.ts"),
    )?;
    Ok(MentionsExtension {
        _source_dir: source_dir,
        source_path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mentions_source_materializes_from_extension() {
        let ext = write_mentions_extension().expect("write");
        let dir = ext.path().parent().expect("source dir");
        let source = std::fs::read_to_string(ext.path()).expect("read index.ts");
        let data = std::fs::read_to_string(dir.join("session-data.ts")).expect("read session-data.ts");
        let tui = std::fs::read_to_string(dir.join("session-picker-tui.ts")).expect("read session-picker-tui.ts");
        let skill_data = std::fs::read_to_string(dir.join("skill-data.ts")).expect("read skill-data.ts");
        let skill_tui = std::fs::read_to_string(dir.join("skill-picker-tui.ts")).expect("read skill-picker-tui.ts");
        assert!(source.contains("__pi_grok_mentions__"));
        assert!(source.contains("__pi_grok_mention_insert__"));
        assert!(source.contains("__piGrokRegisterMentionProvider"));
        assert!(source.contains("grok-pi://skill/"));
        assert!(source.contains("grok-pi://session/"));
        assert!(source.contains("grok-pi://mcp/"));
        assert!(data.contains("loadSessionPickerRecords"));
        assert!(tui.contains("createSessionPickerTui"));
        assert!(skill_data.contains("buildSkillRecords"));
        assert!(skill_tui.contains("createSkillPickerTui"));
    }
}
