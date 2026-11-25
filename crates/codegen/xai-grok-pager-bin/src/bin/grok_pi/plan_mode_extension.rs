use anyhow::{Context, Result};
use std::{
    fs::File,
    io::Write,
    path::{Path, PathBuf},
};
use tempfile::TempDir;

/// Process-private extension source, control file, and response directory for
/// Pi plan-mode gating and synchronous enter/exit tool handshakes.
pub(super) struct PlanModeExtension {
    _dir: TempDir,
    source_path: PathBuf,
    control_path: PathBuf,
}

impl PlanModeExtension {
    pub(super) fn source_path(&self) -> &Path {
        &self.source_path
    }

    pub(super) fn control_path(&self) -> &Path {
        &self.control_path
    }
}

/// Materialize the extension and its control file inside a private temp
/// directory. Both are retained by the composition binary until the Pi child
/// exits; the adapter writes per-tool response files into the same directory.
pub(super) fn write_plan_mode_extension() -> Result<PlanModeExtension> {
    let root = super::tui_kit::materialize_cached_bundle(
        "pi-grok-plan-mode",
        &[(
            "index.ts",
            include_str!("../../../../../../extensions/pi-grok-plan-mode/index.ts"),
        )],
    )?;
    let source_path = root.join("index.ts");
    let dir = tempfile::Builder::new()
        .prefix("pi-grok-plan-mode-")
        .tempdir()
        .context("create Pi plan-mode extension directory")?;

    let control_path = dir.path().join("control.json");
    let mut control = File::create(&control_path).context("create Pi plan-mode control file")?;
    control
        .write_all(br#"{"active":false,"planFilePath":""}"#)
        .context("initialize Pi plan-mode control metadata")?;
    control
        .flush()
        .context("flush Pi plan-mode control metadata")?;

    Ok(PlanModeExtension {
        _dir: dir,
        source_path,
        control_path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_mode_extension_source_and_control_are_materialized() {
        let extension = write_plan_mode_extension().expect("write extension");
        let source = std::fs::read_to_string(extension.source_path()).expect("read source");
        assert!(source.contains("enter_plan_mode"));
        assert!(source.contains("exit_plan_mode"));
        assert!(source.contains("tool_call"));
        assert!(source.contains("PI_GROK_PLAN_CONTROL"));
        // Inactive-state calls must stay idempotent (no throw) so a model
        // retry racing the approved exit never produces a failed tool trace.
        assert!(source.contains("Plan mode is not active"));
        assert_eq!(
            extension.source_path().parent(),
            extension.control_path().parent()
        );
        assert_eq!(
            std::fs::read_to_string(extension.control_path()).expect("read control"),
            r#"{"active":false,"planFilePath":""}"#
        );
    }
}
