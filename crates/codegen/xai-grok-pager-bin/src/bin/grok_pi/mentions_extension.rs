use anyhow::{Context, Result};
use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
};
use tempfile::TempDir;

/// Materialized `@` object-mentions extension bundle. `_source_dir` must stay
/// alive for the Pi process lifetime so relative imports between
/// `pi-grok-mentions/` and the sibling `pi-grok-tui-kit/` keep resolving.
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
/// `@mcp`, `@file`) along with its TS picker TUI modules and shared
/// `pi-grok-tui-kit` primitives.
///
/// Publishes its catalog via `setStatus("__pi_grok_mentions__", ...)` which
/// `pi-grok-adapter` forwards as `pi/ui/mentions_catalog`, and returns chosen
/// items via `setStatus("__pi_grok_mention_insert__", ...)` → `pi/ui/mention_insert`.
pub(super) fn write_mentions_extension() -> Result<MentionsExtension> {
    let source_dir = tempfile::Builder::new()
        .prefix("pi-grok-mentions-")
        .tempdir()
        .context("create mentions extension source directory")?;
    let mentions_dir = source_dir.path().join("pi-grok-mentions");
    fs::create_dir_all(&mentions_dir).context("create pi-grok-mentions dir")?;
    super::tui_kit::materialize_tui_kit(source_dir.path())?;

    let source_path = write_source_file(
        &mentions_dir,
        "index.ts",
        include_str!("../../../../../../extensions/pi-grok-mentions/index.ts"),
    )?;
    write_source_file(
        &mentions_dir,
        "session-data.ts",
        include_str!("../../../../../../extensions/pi-grok-mentions/session-data.ts"),
    )?;
    write_source_file(
        &mentions_dir,
        "session-picker-tui.ts",
        include_str!("../../../../../../extensions/pi-grok-mentions/session-picker-tui.ts"),
    )?;
    write_source_file(
        &mentions_dir,
        "skill-data.ts",
        include_str!("../../../../../../extensions/pi-grok-mentions/skill-data.ts"),
    )?;
    write_source_file(
        &mentions_dir,
        "skill-picker-tui.ts",
        include_str!("../../../../../../extensions/pi-grok-mentions/skill-picker-tui.ts"),
    )?;
    write_source_file(
        &mentions_dir,
        "file-data.ts",
        include_str!("../../../../../../extensions/pi-grok-mentions/file-data.ts"),
    )?;
    write_source_file(
        &mentions_dir,
        "file-picker-tui.ts",
        include_str!("../../../../../../extensions/pi-grok-mentions/file-picker-tui.ts"),
    )?;
    write_source_file(
        &mentions_dir,
        "mcp-picker-tui.ts",
        include_str!("../../../../../../extensions/pi-grok-mentions/mcp-picker-tui.ts"),
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
        let dir = ext.path().parent().expect("mentions dir");
        let root = dir.parent().expect("bundle root");
        let source = std::fs::read_to_string(ext.path()).expect("read index.ts");
        let data = std::fs::read_to_string(dir.join("session-data.ts")).expect("read session-data.ts");
        let tui = std::fs::read_to_string(dir.join("session-picker-tui.ts")).expect("read session-picker-tui.ts");
        let skill_data = std::fs::read_to_string(dir.join("skill-data.ts")).expect("read skill-data.ts");
        let skill_tui = std::fs::read_to_string(dir.join("skill-picker-tui.ts")).expect("read skill-picker-tui.ts");
        let file_data = std::fs::read_to_string(dir.join("file-data.ts")).expect("read file-data.ts");
        let file_tui = std::fs::read_to_string(dir.join("file-picker-tui.ts")).expect("read file-picker-tui.ts");
        let mcp_tui = std::fs::read_to_string(dir.join("mcp-picker-tui.ts")).expect("read mcp-picker-tui.ts");
        let kit_frame = std::fs::read_to_string(root.join("pi-grok-tui-kit/frame.ts"))
            .expect("read pi-grok-tui-kit/frame.ts");
        let kit_icons = std::fs::read_to_string(root.join("pi-grok-tui-kit/icons.ts"))
            .expect("read pi-grok-tui-kit/icons.ts");
        assert!(source.contains("__pi_grok_mentions__"));
        assert!(source.contains("__pi_grok_mention_insert__"));
        assert!(source.contains("__piGrokRegisterMentionProvider"));
        assert!(source.contains("__pi_mention_mcp_picker"));
        assert!(source.contains("__pi_resume_session"));
        assert!(source.contains("grok-pi://skill/"));
        assert!(source.contains("grok-pi://session/"));
        assert!(source.contains("grok-pi://mcp/"));
        assert!(data.contains("loadSessionPickerRecords"));
        assert!(tui.contains("createSessionPickerTui"));
        assert!(skill_data.contains("buildSkillRecords"));
        assert!(skill_tui.contains("createSkillPickerTui"));
        assert!(file_data.contains("listWorkspaceFilesWithFd"));
        assert!(file_tui.contains("createFilePickerTui"));
        assert!(mcp_tui.contains("createMcpPickerTui"));
        assert!(kit_frame.contains("renderSplitOverlayFrame"));
        assert!(kit_icons.contains("getFileIcon"));
    }
}
