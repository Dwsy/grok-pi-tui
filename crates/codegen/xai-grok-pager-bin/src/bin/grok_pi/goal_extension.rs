use anyhow::{Context, Result};
use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
};
use tempfile::{NamedTempFile, TempDir};

/// Process-private goal extension source + control file for GoalHost.
pub(super) struct GoalExtension {
    _source_dir: TempDir,
    source_path: PathBuf,
    control: NamedTempFile,
}

impl GoalExtension {
    pub(super) fn source_path(&self) -> &Path {
        &self.source_path
    }

    pub(super) fn control_path(&self) -> &Path {
        self.control.path()
    }
}

fn write_source_file(dir: &Path, name: &str, source: &str) -> Result<PathBuf> {
    let path = dir.join(name);
    let mut file = File::create(&path)
        .with_context(|| format!("create Pi goal extension module {name}"))?;
    file.write_all(source.as_bytes())
        .with_context(|| format!("write Pi goal extension module {name}"))?;
    file.flush()
        .with_context(|| format!("flush Pi goal extension module {name}"))?;
    file.sync_all().ok();
    Ok(path)
}

/// Materialize the goal extension and empty control file (retained until Pi exits).
pub(super) fn write_goal_extension() -> Result<GoalExtension> {
    let source_dir = tempfile::Builder::new()
        .prefix("pi-grok-goal-")
        .tempdir()
        .context("create Pi goal extension source directory")?;
    let goal_dir = source_dir.path().join("pi-grok-goal");
    fs::create_dir_all(&goal_dir).context("create pi-grok-goal dir")?;
    super::tui_kit::materialize_tui_kit(source_dir.path())?;

    let source_path = write_source_file(
        &goal_dir,
        "index.ts",
        include_str!("../../../../../../extensions/pi-grok-goal/index.ts"),
    )?;
    write_source_file(
        &goal_dir,
        "goal-detail-tui.ts",
        include_str!("../../../../../../extensions/pi-grok-goal/goal-detail-tui.ts"),
    )?;

    let mut control = tempfile::Builder::new()
        .prefix("pi-grok-goal-control-")
        .suffix(".json")
        .tempfile()
        .context("create Pi goal control tempfile")?;
    // Empty object: extension treats missing/invalid as no goal.
    control
        .write_all(b"{}")
        .context("write Pi goal control seed")?;
    control.flush().context("flush Pi goal control")?;
    File::open(control.path())
        .and_then(|file| file.sync_all())
        .ok();

    Ok(GoalExtension {
        _source_dir: source_dir,
        source_path,
        control,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn goal_extension_source_loads() {
        let ext = write_goal_extension().expect("write");
        let source = std::fs::read_to_string(ext.source_path()).expect("read");
        assert!(source.contains("name: \"goal\""));
        assert!(source.contains("Type.Literal(\"create\")"));
        assert!(source.contains("Type.Literal(\"get\")"));
        assert!(source.contains("Type.Literal(\"update\")"));
        assert!(source.contains("Type.Literal(\"resume\")"));
        assert!(!source.contains("name: \"create_goal\""));
        assert!(!source.contains("name: \"update_goal\""));
        assert!(source.contains("registerCommand(\"goal\""));
        assert!(source.contains("registerCommand(PERSIST_COMMAND)"));
        assert!(source.contains("__pi_grok_goal_persist"));
        assert!(source.contains("session_tree"));
        assert!(source.contains("getBranch"));
        assert!(source.contains("PI_GROK_GOAL_CONTROL"));
        assert!(source.contains("pi-grok-goal/v1"));
        // Mid-turn /goal must queue; bare sendUserMessage throws "already processing".
        assert!(source.contains("deliverAs: \"followUp\""));
        assert!(source.contains("goal_replaced"));
    }
}
