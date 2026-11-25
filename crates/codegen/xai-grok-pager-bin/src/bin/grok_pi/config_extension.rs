use anyhow::{Context, Result};
use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
};
use tempfile::TempDir;

/// Materialized `pi-grok-config` extension bundle. Preserves the sibling
/// `pi-grok-tui-kit/` directory inside `_source_dir` so relative imports
/// (`../pi-grok-tui-kit/index.ts`) resolve identically to the source tree.
pub(super) struct ConfigExtension {
    _source_dir: TempDir,
    source_path: PathBuf,
}

impl ConfigExtension {
    pub(super) fn path(&self) -> &Path {
        &self.source_path
    }
}

fn write_source_file(dir: &Path, name: &str, source: &str) -> Result<PathBuf> {
    let path = dir.join(name);
    let mut file = File::create(&path)
        .with_context(|| format!("create config extension module {name}"))?;
    file.write_all(source.as_bytes())
        .with_context(|| format!("write config extension module {name}"))?;
    file.flush()
        .with_context(|| format!("flush config extension module {name}"))?;
    file.sync_all().ok();
    Ok(path)
}

/// Materialize the standalone TS `/pi-config` resource manager renderer along
/// with the shared `pi-grok-tui-kit` primitives.
pub(super) fn write_config_extension() -> Result<ConfigExtension> {
    let source_dir = tempfile::Builder::new()
        .prefix("pi-grok-config-")
        .tempdir()
        .context("create config extension source directory")?;
    let config_dir = source_dir.path().join("pi-grok-config");
    fs::create_dir_all(&config_dir).context("create pi-grok-config dir")?;
    super::tui_kit::materialize_tui_kit(source_dir.path())?;

    let source_path = write_source_file(
        &config_dir,
        "index.ts",
        include_str!("../../../../../../extensions/pi-grok-config/index.ts"),
    )?;
    write_source_file(
        &config_dir,
        "config-data.ts",
        include_str!("../../../../../../extensions/pi-grok-config/config-data.ts"),
    )?;
    write_source_file(
        &config_dir,
        "config-tui.ts",
        include_str!("../../../../../../extensions/pi-grok-config/config-tui.ts"),
    )?;
    write_source_file(
        &config_dir,
        "extensions-modal-tui.ts",
        include_str!("../../../../../../extensions/pi-grok-config/extensions-modal-tui.ts"),
    )?;

    Ok(ConfigExtension {
        _source_dir: source_dir,
        source_path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_extension_materializes_with_tui_kit() {
        let ext = write_config_extension().expect("write");
        let root = ext.path().parent().unwrap().parent().unwrap();
        let index = fs::read_to_string(ext.path()).expect("read index.ts");
        let kit = fs::read_to_string(root.join("pi-grok-tui-kit/list.ts"))
            .expect("read pi-grok-tui-kit/list.ts");
        assert!(index.contains("__pi_config_manager"));
        assert!(kit.contains("ListController"));
    }
}
