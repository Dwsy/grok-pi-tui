use anyhow::{Context, Result};
use std::{fs::File, io::Write, path::Path};
use tempfile::NamedTempFile;

/// Process-private extension source and control file for Pi plan-mode gating.
pub(super) struct PlanModeExtension {
    source: NamedTempFile,
    control: NamedTempFile,
}

impl PlanModeExtension {
    pub(super) fn source_path(&self) -> &Path {
        self.source.path()
    }

    pub(super) fn control_path(&self) -> &Path {
        self.control.path()
    }
}

/// Materialize the extension and its control file. Both are retained by the
/// composition binary until the Pi child exits.
pub(super) fn write_plan_mode_extension() -> Result<PlanModeExtension> {
    let mut source = tempfile::Builder::new()
        .prefix("pi-grok-plan-mode-")
        .suffix(".ts")
        .tempfile()
        .context("create Pi plan-mode extension tempfile")?;
    const SOURCE: &str = include_str!("../../../../../../extensions/pi-grok-plan-mode/index.ts");
    source
        .write_all(SOURCE.as_bytes())
        .context("write Pi plan-mode extension source")?;
    source
        .flush()
        .context("flush Pi plan-mode extension source")?;
    File::open(source.path())
        .and_then(|file| file.sync_all())
        .ok();

    let mut control = tempfile::Builder::new()
        .prefix("pi-grok-plan-mode-control-")
        .suffix(".json")
        .tempfile()
        .context("create Pi plan-mode control tempfile")?;
    control
        .write_all(br#"{"active":false,"planFilePath":""}"#)
        .context("initialize Pi plan-mode control metadata")?;
    control
        .flush()
        .context("flush Pi plan-mode control metadata")?;

    Ok(PlanModeExtension { source, control })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_mode_extension_source_and_control_are_materialized() {
        let extension = write_plan_mode_extension().expect("write extension");
        let source = std::fs::read_to_string(extension.source_path()).expect("read source");
        assert!(source.contains("exit_plan_mode"));
        assert!(source.contains("tool_call"));
        assert!(source.contains("PI_GROK_PLAN_CONTROL"));
        // Inactive-state calls must stay idempotent (no throw) so a model
        // retry racing the approved exit never produces a failed tool trace.
        assert!(source.contains("Plan mode is not active"));
        assert_eq!(
            std::fs::read_to_string(extension.control_path()).expect("read control"),
            r#"{"active":false,"planFilePath":""}"#
        );
    }
}
