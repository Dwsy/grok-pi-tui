use anyhow::Result;
use std::path::{Path, PathBuf};

/// Materialized `pi-grok-config` extension bundle in a content-addressed cache
/// directory so `jiti`'s `fsCache` hits across launches.
pub(super) struct ConfigExtension {
    source_path: PathBuf,
}

impl ConfigExtension {
    pub(super) fn path(&self) -> &Path {
        &self.source_path
    }
}

/// Materialize the standalone TS `/pi-config` resource manager renderer along
/// with the shared `pi-grok-tui-kit` primitives.
pub(super) fn write_config_extension() -> Result<ConfigExtension> {
    let root = super::tui_kit::materialize_cached_tui_bundle(
        "pi-grok-config",
        &[
            (
                "pi-grok-config/index.ts",
                include_str!("../../../../../../extensions/pi-grok-config/index.ts"),
            ),
            (
                "pi-grok-config/config-data.ts",
                include_str!("../../../../../../extensions/pi-grok-config/config-data.ts"),
            ),
            (
                "pi-grok-config/config-tui.ts",
                include_str!("../../../../../../extensions/pi-grok-config/config-tui.ts"),
            ),
            (
                "pi-grok-config/extensions-modal-tui.ts",
                include_str!("../../../../../../extensions/pi-grok-config/extensions-modal-tui.ts"),
            ),
        ],
    )?;

    Ok(ConfigExtension {
        source_path: root.join("pi-grok-config/index.ts"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

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
