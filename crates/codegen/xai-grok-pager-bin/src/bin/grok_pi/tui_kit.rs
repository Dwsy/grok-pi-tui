use anyhow::{Context, Result};
use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
};

fn write_kit_file(dir: &Path, name: &str, source: &str) -> Result<PathBuf> {
    let path = dir.join(name);
    let mut file = File::create(&path)
        .with_context(|| format!("create pi-grok-tui-kit module {name}"))?;
    file.write_all(source.as_bytes())
        .with_context(|| format!("write pi-grok-tui-kit module {name}"))?;
    file.flush()
        .with_context(|| format!("flush pi-grok-tui-kit module {name}"))?;
    file.sync_all().ok();
    Ok(path)
}

/// Materialize the shared `pi-grok-tui-kit/` sibling directory inside `root`
/// so any embedded TS TUI extension (`pi-grok-mentions`, `pi-grok-review`,
/// `pi-grok-extensions`, `pi-grok-tree`, etc.) can resolve
/// `../pi-grok-tui-kit/index.ts` from a single source of truth.
pub(super) fn materialize_tui_kit(root: &Path) -> Result<PathBuf> {
    let kit_dir = root.join("pi-grok-tui-kit");
    fs::create_dir_all(&kit_dir).context("create shared pi-grok-tui-kit dir")?;
    write_kit_file(
        &kit_dir,
        "frame.ts",
        include_str!("../../../../../../extensions/pi-grok-tui-kit/frame.ts"),
    )?;
    write_kit_file(
        &kit_dir,
        "icons.ts",
        include_str!("../../../../../../extensions/pi-grok-tui-kit/icons.ts"),
    )?;
    write_kit_file(
        &kit_dir,
        "image.ts",
        include_str!("../../../../../../extensions/pi-grok-tui-kit/image.ts"),
    )?;
    write_kit_file(
        &kit_dir,
        "list.ts",
        include_str!("../../../../../../extensions/pi-grok-tui-kit/list.ts"),
    )?;
    write_kit_file(
        &kit_dir,
        "index.ts",
        include_str!("../../../../../../extensions/pi-grok-tui-kit/index.ts"),
    )?;
    Ok(kit_dir)
}
