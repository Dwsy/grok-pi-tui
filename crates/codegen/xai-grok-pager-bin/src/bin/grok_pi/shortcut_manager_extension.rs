use anyhow::Result;
use std::path::{Path, PathBuf};

/// Materialized shortcut manager extension bundle in a content-addressed cache
/// directory so `jiti`'s `fsCache` hits across launches.
pub(super) struct ShortcutManagerExtension {
    source_path: PathBuf,
}

impl ShortcutManagerExtension {
    pub(super) fn source_path(&self) -> &Path {
        &self.source_path
    }
}

/// Materialize the shortcut manager before user extensions load.
///
/// Captures Pi extension-registered shortcuts (`pi.registerShortcut`) into a
/// global registry and dispatches them in the Remote TUI key path. Every
/// authored module must be materialized here because this injector owns the
/// transitive closure of `index.ts`'s relative imports.
pub(super) fn write_shortcut_manager_extension() -> Result<ShortcutManagerExtension> {
    let root = super::tui_kit::materialize_cached_bundle(
        "pi-grok-shortcut-manager",
        &[
            (
                "index.ts",
                include_str!("../../../../../../extensions/pi-grok-shortcut-manager/index.ts"),
            ),
            (
                "shared.ts",
                include_str!("../../../../../../extensions/pi-grok-shortcut-manager/shared.ts"),
            ),
            (
                "config.ts",
                include_str!("../../../../../../extensions/pi-grok-shortcut-manager/config.ts"),
            ),
            (
                "dispatch.ts",
                include_str!("../../../../../../extensions/pi-grok-shortcut-manager/dispatch.ts"),
            ),
            (
                "host.ts",
                include_str!("../../../../../../extensions/pi-grok-shortcut-manager/host.ts"),
            ),
            (
                "commands.ts",
                include_str!("../../../../../../extensions/pi-grok-shortcut-manager/commands.ts"),
            ),
        ],
    )?;
    Ok(ShortcutManagerExtension {
        source_path: root.join("index.ts"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Bundle {
        index: String,
        shared: String,
        config: String,
        dispatch: String,
        host: String,
        commands: String,
    }

    fn write_bundle() -> (ShortcutManagerExtension, Bundle) {
        let extension = write_shortcut_manager_extension().expect("write extension");
        let dir = extension.source_path().parent().expect("source dir");
        let read = |name: &str| {
            std::fs::read_to_string(dir.join(name)).unwrap_or_else(|e| panic!("read {name}: {e}"))
        };
        let bundle = Bundle {
            index: read("index.ts"),
            shared: read("shared.ts"),
            config: read("config.ts"),
            dispatch: read("dispatch.ts"),
            host: read("host.ts"),
            commands: read("commands.ts"),
        };
        (extension, bundle)
    }

    #[test]
    fn shortcut_manager_extension_materializes_every_module_of_the_entry_closure() {
        let (extension, bundle) = write_bundle();
        assert!(bundle.index.contains("__pi_shortcut_dispatch"));
        assert!(bundle.index.contains("registerShortcutCommand"));
        assert!(bundle.index.contains("from \"./commands.ts\""));
        assert!(bundle.index.contains("from \"./dispatch.ts\""));
        assert!(bundle.index.contains("from \"./host.ts\""));
        assert!(bundle.index.contains("from \"./shared.ts\""));
        assert!(bundle.config.contains("from \"./shared.ts\""));
        assert!(bundle.dispatch.contains("from \"./config.ts\""));
        assert!(bundle.dispatch.contains("from \"./shared.ts\""));
        assert!(bundle.host.contains("from \"./config.ts\""));
        assert!(bundle.host.contains("from \"./dispatch.ts\""));
        assert!(bundle.host.contains("from \"./shared.ts\""));
        assert!(bundle.commands.contains("from \"./config.ts\""));
        assert!(bundle.commands.contains("from \"./host.ts\""));
        assert!(bundle.commands.contains("from \"./shared.ts\""));
        assert!(bundle.shared.contains("shortcutRegistry"));
        assert!(bundle.config.contains("loadConfig"));
        assert!(bundle.dispatch.contains("ensureDispatchChannel"));
        assert!(bundle.host.contains("installRunnerCapture"));
        assert!(bundle.commands.contains("Manage extension shortcuts"));
        assert_eq!(
            extension
                .source_path()
                .extension()
                .and_then(|value| value.to_str()),
            Some("ts")
        );
    }

    #[test]
    fn shortcut_manager_extension_captures_extension_shortcuts_only() {
        let extension = write_shortcut_manager_extension().expect("write extension");
        let source = std::fs::read_to_string(extension.source_path()).expect("read extension");
        assert!(source.contains("__pi_shortcut_dispatch"));
        assert!(!source.contains("RESERVED_KEYS"));
        assert!(!source.contains("app.interrupt"));
    }
}
