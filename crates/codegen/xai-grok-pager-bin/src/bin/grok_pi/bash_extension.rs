use anyhow::{Context, Result};
use std::{fs::File, io::Write, path::Path};
use tempfile::NamedTempFile;

/// Private paths shared by the grok-pi composition binary, the injected Pi
/// extension, and the headless adapter. The metadata file is process-unique;
/// it avoids a global tmp-file collision between concurrent grok-pi sessions.
pub(super) struct BashExtension {
    source: NamedTempFile,
    control_meta: NamedTempFile,
}

impl BashExtension {
    pub(super) fn source_path(&self) -> &Path {
        self.source.path()
    }

    pub(super) fn control_meta_path(&self) -> &Path {
        self.control_meta.path()
    }
}

/// Materialize the private grok-pi Bash enhancement and its control metadata.
/// Both files remain alive for the Pi process lifetime.
pub(super) fn write_bash_extension() -> Result<BashExtension> {
    let mut source = tempfile::Builder::new()
        .prefix("pi-grok-bash-")
        .suffix(".ts")
        .tempfile()
        .context("create Pi Bash extension tempfile")?;
    const SOURCE: &str = include_str!("../../../../../../extensions/pi-grok-bash/index.ts");
    source
        .write_all(SOURCE.as_bytes())
        .context("write Pi Bash extension source")?;
    source.flush().context("flush Pi Bash extension source")?;
    File::open(source.path())
        .and_then(|file| file.sync_all())
        .ok();

    let control_meta = tempfile::Builder::new()
        .prefix("pi-grok-bash-control-")
        .suffix(".json")
        .tempfile()
        .context("create Pi Bash control metadata tempfile")?;
    Ok(BashExtension {
        source,
        control_meta,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bash_extension_source_is_a_loadable_typescript_module() {
        let extension = write_bash_extension().expect("write extension");
        let source = std::fs::read_to_string(extension.source_path()).expect("read extension");
        assert!(source.contains("const nativeBash = createBashToolDefinition"));
        assert!(source.contains("pi.registerTool({"));
        assert!(source.contains("name: \"eval\""));
        assert!(source.contains("PersistentEvalKernel"));
        assert!(source.contains("JS_EVAL_WORKER"));
        assert!(source.contains("PYTHON_EVAL_WORKER"));
        assert!(source.contains("is_background"));
        assert!(source.contains("name: \"get_task_output\""));
        assert!(source.contains("name: \"wait_tasks\""));
        assert!(source.contains("name: \"kill_task\""));
        assert!(source.contains("runningTaskIds"));
        assert!(source.contains("op === \"kill\""));
        assert!(source.contains("PI_GROK_BASH_CONTROL_META"));
        assert!(source.contains("pi-grok-background-bash/v1"));
        assert!(source.contains("Background Bash task failed:"));
        assert!(source.contains(
            "shouldWake ? { triggerTurn: true, deliverAs: \"followUp\" } : { triggerTurn: false }"
        ));
        // Terminal state must reach the adapter out of band: the bridge message
        // above shares the agent's queue lifetime and is dropped on abort.
        assert!(source.contains("__pi_grok_bash_task__"));
        assert!(source.contains("publishTerminalState(task)"));
        assert_eq!(
            extension
                .source_path()
                .extension()
                .and_then(|value| value.to_str()),
            Some("ts")
        );
        assert_eq!(
            extension
                .control_meta_path()
                .extension()
                .and_then(|value| value.to_str()),
            Some("json")
        );
    }
}
