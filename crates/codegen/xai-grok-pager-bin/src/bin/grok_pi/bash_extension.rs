use anyhow::{Context, Result};
use std::{
    fs::File,
    io::Write,
    path::{Path, PathBuf},
};
use tempfile::{NamedTempFile, TempDir};

/// Private paths shared by the grok-pi composition binary, the injected
/// Bash/Eval extension bundle, and the headless adapter. The Bash control
/// metadata file is process-unique; it avoids a global tmp-file collision
/// between concurrent grok-pi sessions.
pub(super) struct BashExtension {
    _source_dir: TempDir,
    source_path: PathBuf,
    control_meta: NamedTempFile,
}

impl BashExtension {
    pub(super) fn source_path(&self) -> &Path {
        &self.source_path
    }

    pub(super) fn control_meta_path(&self) -> &Path {
        self.control_meta.path()
    }
}

fn write_source_file(dir: &Path, name: &str, source: &str) -> Result<PathBuf> {
    let path = dir.join(name);
    let mut file =
        File::create(&path).with_context(|| format!("create Pi Bash extension module {name}"))?;
    file.write_all(source.as_bytes())
        .with_context(|| format!("write Pi Bash extension module {name}"))?;
    file.flush()
        .with_context(|| format!("flush Pi Bash extension module {name}"))?;
    file.sync_all().ok();
    Ok(path)
}

/// Materialize the private grok-pi Bash/Eval bundle and Bash control metadata.
/// The source directory remains alive for the Pi process lifetime so relative
/// imports between the authored TypeScript modules continue to resolve. Eval
/// may use the bundle while Bash control is disabled by the child environment.
pub(super) fn write_bash_extension() -> Result<BashExtension> {
    let source_dir = tempfile::Builder::new()
        .prefix("pi-grok-bash-")
        .tempdir()
        .context("create Pi Bash extension source directory")?;
    let source_path = write_source_file(
        source_dir.path(),
        "index.ts",
        include_str!("../../../../../../extensions/pi-grok-bash/index.ts"),
    )?;
    write_source_file(
        source_dir.path(),
        "eval.ts",
        include_str!("../../../../../../extensions/pi-grok-bash/eval.ts"),
    )?;
    write_source_file(
        source_dir.path(),
        "bash-tasks.ts",
        include_str!("../../../../../../extensions/pi-grok-bash/bash-tasks.ts"),
    )?;
    write_source_file(
        source_dir.path(),
        "eval-tasks.ts",
        include_str!("../../../../../../extensions/pi-grok-bash/eval-tasks.ts"),
    )?;
    write_source_file(
        source_dir.path(),
        "prompts.ts",
        include_str!("../../../../../../extensions/pi-grok-bash/prompts.ts"),
    )?;
    write_source_file(
        source_dir.path(),
        "shared.ts",
        include_str!("../../../../../../extensions/pi-grok-bash/shared.ts"),
    )?;
    write_source_file(
        source_dir.path(),
        "tool-bridge.ts",
        include_str!("../../../../../../extensions/pi-grok-bash/tool-bridge.ts"),
    )?;

    let control_meta = tempfile::Builder::new()
        .prefix("pi-grok-bash-control-")
        .suffix(".json")
        .tempfile()
        .context("create Pi Bash control metadata tempfile")?;
    Ok(BashExtension {
        _source_dir: source_dir,
        source_path,
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
        let source_dir = extension.source_path().parent().expect("source dir");
        let eval_source =
            std::fs::read_to_string(source_dir.join("eval.ts")).expect("read eval module");
        let bash_source = std::fs::read_to_string(source_dir.join("bash-tasks.ts"))
            .expect("read bash tasks module");
        let eval_tasks_source = std::fs::read_to_string(source_dir.join("eval-tasks.ts"))
            .expect("read eval tasks module");
        let prompts_source =
            std::fs::read_to_string(source_dir.join("prompts.ts")).expect("read prompts module");
        let shared_source =
            std::fs::read_to_string(source_dir.join("shared.ts")).expect("read shared module");
        let tool_bridge_source = std::fs::read_to_string(source_dir.join("tool-bridge.ts"))
            .expect("read tool bridge module");

        assert!(source.contains("const nativeBash = createBashToolDefinition"));
        assert!(source.contains("pi.registerTool({"));
        assert!(source.contains("name: \"eval\""));
        assert!(source.contains("from \"./eval.ts\""));
        assert!(source.contains("from \"./bash-tasks.ts\""));
        assert!(source.contains("from \"./eval-tasks.ts\""));
        assert!(source.contains("from \"./prompts.ts\""));
        assert!(source.contains("from \"./tool-bridge.ts\""));
        assert!(source.contains("invokeEvalHostCall"));
        assert!(source.contains("is_background"));
        assert!(source.contains("name: \"get_task_output\""));
        assert!(source.contains("name: \"wait_tasks\""));
        assert!(source.contains("name: \"kill_task\""));
        assert!(source.contains("resolveMaxWaitMs"));
        assert!(source.contains("autoBackgroundMs: maxWaitMs"));
        assert!(source.contains("capWaitMs"));

        assert!(eval_source.contains("PersistentEvalKernel"));
        assert!(eval_source.contains("JS_EVAL_WORKER"));
        assert!(eval_source.contains("PYTHON_EVAL_WORKER"));
        assert!(eval_source.contains("PI_GROK_EVAL_VERSION"));
        assert!(eval_source.contains("JS_EVAL_WORKER_V2"));
        assert!(eval_source.contains("PYTHON_EVAL_WORKER_V2"));
        assert!(eval_source.contains("HostCallGate"));
        assert!(eval_source.contains("type: \"host_call\""));

        assert!(bash_source.contains("runningTaskIds"));
        assert!(bash_source.contains("op === \"kill\""));
        assert!(bash_source.contains("PI_GROK_BASH_CONTROL_META"));
        assert!(bash_source.contains("pi-grok-background-bash/v1"));
        assert!(bash_source.contains("Background Bash task failed:"));
        assert!(bash_source.contains("autoBackgroundHandle"));
        assert!(bash_source.contains(
            "shouldWake ? { triggerTurn: true, deliverAs: \"followUp\" } : { triggerTurn: false }"
        ));
        // Terminal state must reach the adapter out of band: the bridge message
        // above shares the agent's queue lifetime and is dropped on abort.
        assert!(bash_source.contains("__pi_grok_bash_task__"));
        assert!(bash_source.contains("publishTerminalState(task)"));
        assert!(eval_tasks_source.contains("startEvalBackgroundTask"));
        assert!(eval_tasks_source.contains("killEvalTask"));
        assert!(prompts_source.contains("buildBashPrompts"));
        assert!(prompts_source.contains("buildEvalPrompts"));
        assert!(prompts_source.contains("automatically backgrounded"));
        assert!(shared_source.contains("killChildProcess"));
        assert!(shared_source.contains("DEFAULT_MAX_WAIT_MINS"));
        assert!(tool_bridge_source.contains("getAllRegisteredTools"));
        assert!(tool_bridge_source.contains("wrapRegisteredTool"));
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
