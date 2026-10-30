use anyhow::{Context, Result};
use std::{fs::File, io::Write, path::Path};
use tempfile::NamedTempFile;

/// Process-private goal extension source + control file for GoalHost.
pub(super) struct GoalExtension {
    source: NamedTempFile,
    control: NamedTempFile,
}

impl GoalExtension {
    pub(super) fn source_path(&self) -> &Path {
        self.source.path()
    }

    pub(super) fn control_path(&self) -> &Path {
        self.control.path()
    }
}

/// Materialize the goal extension and empty control file (retained until Pi exits).
pub(super) fn write_goal_extension() -> Result<GoalExtension> {
    let mut source = tempfile::Builder::new()
        .prefix("pi-grok-goal-")
        .suffix(".ts")
        .tempfile()
        .context("create Pi goal extension tempfile")?;
    const SOURCE: &str = include_str!("../../../../../../extensions/pi-grok-goal/index.ts");
    source
        .write_all(SOURCE.as_bytes())
        .context("write Pi goal extension source")?;
    source.flush().context("flush Pi goal extension source")?;
    File::open(source.path())
        .and_then(|file| file.sync_all())
        .ok();

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

    Ok(GoalExtension { source, control })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn goal_extension_source_loads() {
        let ext = write_goal_extension().expect("write");
        let source = std::fs::read_to_string(ext.source_path()).expect("read");
        assert!(source.contains("update_goal"));
        assert!(source.contains("create_goal"));
        assert!(source.contains("registerCommand(\"goal\""));
        assert!(source.contains("PI_GROK_GOAL_CONTROL"));
        assert!(source.contains("pi-grok-goal/v1"));
        // Mid-turn /goal must queue; bare sendUserMessage throws "already processing".
        assert!(source.contains("deliverAs: \"followUp\""));
        assert!(source.contains("goal_replaced"));
    }
}
