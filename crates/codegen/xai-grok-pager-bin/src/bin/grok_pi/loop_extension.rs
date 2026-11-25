use anyhow::{Context, Result};
use std::{
    fs::File,
    io::Write,
    path::{Path, PathBuf},
};
use tempfile::NamedTempFile;

/// Process-private loop extension bundle + control file for scheduled tasks.
pub(super) struct LoopExtension {
    source_path: PathBuf,
    control: NamedTempFile,
}

impl LoopExtension {
    pub(super) fn source_path(&self) -> &Path {
        &self.source_path
    }

    pub(super) fn control_path(&self) -> &Path {
        self.control.path()
    }
}

/// Materialize the loop extension and empty control file (retained until Pi exits).
///
/// Every authored module must be materialized here because this injector owns
/// the transitive closure of `index.ts`'s relative imports.
pub(super) fn write_loop_extension() -> Result<LoopExtension> {
    let root = super::tui_kit::materialize_cached_bundle(
        "pi-grok-loop",
        &[
            (
                "index.ts",
                include_str!("../../../../../../extensions/pi-grok-loop/index.ts"),
            ),
            (
                "shared.ts",
                include_str!("../../../../../../extensions/pi-grok-loop/shared.ts"),
            ),
            (
                "control.ts",
                include_str!("../../../../../../extensions/pi-grok-loop/control.ts"),
            ),
            (
                "scheduler.ts",
                include_str!("../../../../../../extensions/pi-grok-loop/scheduler.ts"),
            ),
            (
                "command.ts",
                include_str!("../../../../../../extensions/pi-grok-loop/command.ts"),
            ),
            (
                "tools.ts",
                include_str!("../../../../../../extensions/pi-grok-loop/tools.ts"),
            ),
        ],
    )?;
    let source_path = root.join("index.ts");

    let mut control = tempfile::Builder::new()
        .prefix("pi-grok-loop-control-")
        .suffix(".json")
        .tempfile()
        .context("create Pi loop control tempfile")?;
    control
        .write_all(b"{\"tasks\":[]}")
        .context("write Pi loop control seed")?;
    control.flush().context("flush Pi loop control")?;
    File::open(control.path())
        .and_then(|file| file.sync_all())
        .ok();

    Ok(LoopExtension {
        source_path,
        control,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Bundle {
        index: String,
        shared: String,
        control: String,
        scheduler: String,
        command: String,
        tools: String,
    }

    fn write_bundle() -> (LoopExtension, Bundle) {
        let extension = write_loop_extension().expect("write extension");
        let dir = extension.source_path().parent().expect("source dir");
        let read = |name: &str| {
            std::fs::read_to_string(dir.join(name)).unwrap_or_else(|e| panic!("read {name}: {e}"))
        };
        let bundle = Bundle {
            index: read("index.ts"),
            shared: read("shared.ts"),
            control: read("control.ts"),
            scheduler: read("scheduler.ts"),
            command: read("command.ts"),
            tools: read("tools.ts"),
        };
        (extension, bundle)
    }

    #[test]
    fn loop_extension_materializes_every_module_of_the_entry_closure() {
        let (extension, bundle) = write_bundle();
        assert!(bundle.index.contains("from \"./command.ts\""));
        assert!(bundle.index.contains("from \"./control.ts\""));
        assert!(bundle.index.contains("from \"./scheduler.ts\""));
        assert!(bundle.index.contains("from \"./tools.ts\""));
        assert!(bundle.control.contains("from \"./shared.ts\""));
        assert!(bundle.scheduler.contains("from \"./control.ts\""));
        assert!(bundle.scheduler.contains("from \"./shared.ts\""));
        assert!(bundle.command.contains("from \"./scheduler.ts\""));
        assert!(bundle.command.contains("from \"./shared.ts\""));
        assert!(bundle.tools.contains("from \"./scheduler.ts\""));
        assert!(bundle.tools.contains("from \"./shared.ts\""));
        assert!(bundle.shared.contains("pi-grok-loop/v1"));
        assert!(bundle.command.contains("registerCommand(\"loop\""));
        assert!(bundle.tools.contains("scheduler_create"));
        assert_eq!(
            extension
                .source_path()
                .extension()
                .and_then(|value| value.to_str()),
            Some("ts")
        );
        assert!(extension.control_path().exists());
    }
}
