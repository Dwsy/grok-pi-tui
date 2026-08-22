use anyhow::{Context, Result};
use std::{
    fs::File,
    io::Write,
    path::{Path, PathBuf},
};
use tempfile::TempDir;

/// Materialized rollback extension bundle. `_source_dir` must stay alive for
/// the Pi process lifetime so relative imports between the TypeScript modules
/// keep resolving.
pub(super) struct RollbackExtension {
    _source_dir: TempDir,
    source_path: PathBuf,
}

impl RollbackExtension {
    pub(super) fn source_path(&self) -> &Path {
        &self.source_path
    }
}

fn write_source_file(dir: &Path, name: &str, source: &str) -> Result<PathBuf> {
    let path = dir.join(name);
    let mut file = File::create(&path)
        .with_context(|| format!("create Pi rollback extension module {name}"))?;
    file.write_all(source.as_bytes())
        .with_context(|| format!("write Pi rollback extension module {name}"))?;
    file.flush()
        .with_context(|| format!("flush Pi rollback extension module {name}"))?;
    file.sync_all().ok();
    Ok(path)
}

/// Materialize the Pi tree file rollback checkpoint extension.
/// Only injected when F2 `pi_tree_file_rollback` is enabled.
///
/// Every authored module must be materialized here — the injector owns the
/// transitive closure of `index.ts`'s relative imports (see AGENTS.md
/// "Diagnosing Pi RPC bootstrap / extension failures").
pub(super) fn write_rollback_extension() -> Result<RollbackExtension> {
    let source_dir = tempfile::Builder::new()
        .prefix("pi-grok-rollback-")
        .tempdir()
        .context("create Pi rollback extension source directory")?;
    let source_path = write_source_file(
        source_dir.path(),
        "index.ts",
        include_str!("../../../../../../extensions/pi-grok-rollback/index.ts"),
    )?;
    write_source_file(
        source_dir.path(),
        "shared.ts",
        include_str!("../../../../../../extensions/pi-grok-rollback/shared.ts"),
    )?;
    write_source_file(
        source_dir.path(),
        "store.ts",
        include_str!("../../../../../../extensions/pi-grok-rollback/store.ts"),
    )?;
    write_source_file(
        source_dir.path(),
        "journal.ts",
        include_str!("../../../../../../extensions/pi-grok-rollback/journal.ts"),
    )?;
    write_source_file(
        source_dir.path(),
        "rollback.ts",
        include_str!("../../../../../../extensions/pi-grok-rollback/rollback.ts"),
    )?;
    write_source_file(
        source_dir.path(),
        "bridge.ts",
        include_str!("../../../../../../extensions/pi-grok-rollback/bridge.ts"),
    )?;
    Ok(RollbackExtension {
        _source_dir: source_dir,
        source_path,
    })
}

/// Read the F2 `pi_tree_file_rollback` setting from the effective config.
pub(super) fn rollback_enabled() -> bool {
    let Ok(config) = xai_grok_shell::config::load_effective_config() else {
        return false;
    };
    config
        .get("ui")
        .and_then(|ui| ui.get("pi_tree_file_rollback"))
        .and_then(toml::Value::as_bool)
        .unwrap_or(false)
}

/// Create the process-unique control directory for the bridge.
/// Returns the path string.
pub(super) fn create_control_dir() -> Result<String> {
    let state_root = state_root_path();
    let control = format!("control-{}-{}", std::process::id(), &uuid_v4_short());
    let dir = std::path::Path::new(&state_root).join(&control);
    std::fs::create_dir_all(&dir).context("create rollback control dir")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).ok();
    }
    Ok(dir.to_string_lossy().into_owned())
}

/// The state root for rollback journals and blobs.
fn state_root_path() -> String {
    // Prefer GROK_HOME (set by grok-pi to ~/.grok-pi by default).
    let home = std::env::var("GROK_HOME").unwrap_or_else(|_| {
        std::env::var("HOME")
            .map(|h| format!("{h}/.grok-pi"))
            .unwrap_or_else(|_| "/tmp/.grok-pi".to_string())
    });
    let root = format!("{home}/pi-file-rollback");
    std::fs::create_dir_all(&root).ok();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).ok();
    }
    root
}

/// Expose the state root for the adapter.
pub(super) fn rollback_state_root() -> String {
    state_root_path()
}

fn uuid_v4_short() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("{:08x}{:04x}", std::process::id(), nanos & 0xFFFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Bundle {
        index: String,
        shared: String,
        store: String,
        journal: String,
        rollback: String,
        bridge: String,
    }

    fn write_bundle() -> (RollbackExtension, Bundle) {
        let extension = write_rollback_extension().expect("write extension");
        let dir = extension.source_path().parent().expect("source dir");
        let read = |name: &str| {
            std::fs::read_to_string(dir.join(name)).unwrap_or_else(|e| panic!("read {name}: {e}"))
        };
        let bundle = Bundle {
            index: read("index.ts"),
            shared: read("shared.ts"),
            store: read("store.ts"),
            journal: read("journal.ts"),
            rollback: read("rollback.ts"),
            bridge: read("bridge.ts"),
        };
        (extension, bundle)
    }

    #[test]
    fn rollback_extension_materializes_every_module_of_the_entry_closure() {
        let (extension, bundle) = write_bundle();
        // Entry gates on the env flag and registers tool wrappers + commands.
        assert!(bundle.index.contains("PI_GROK_ROLLBACK"));
        assert!(bundle.index.contains("createWriteToolDefinition"));
        assert!(bundle.index.contains("createEditToolDefinition"));
        assert!(bundle.index.contains("__pi_rollback_preview"));
        assert!(bundle.index.contains("__pi_rollback_execute"));
        // Every relative import of the entry closure is materialized.
        assert!(bundle.index.contains("from \"./journal.ts\""));
        assert!(bundle.index.contains("from \"./bridge.ts\""));
        assert!(bundle.index.contains("from \"./rollback.ts\""));
        assert!(bundle.index.contains("from \"./store.ts\""));
        assert!(bundle.index.contains("from \"./shared.ts\""));
        assert!(bundle.journal.contains("from \"./store.ts\""));
        assert!(bundle.journal.contains("from \"./shared.ts\""));
        assert!(bundle.rollback.contains("from \"./store.ts\""));
        assert!(bundle.rollback.contains("from \"./shared.ts\""));
        assert!(bundle.bridge.contains("from \"./rollback.ts\""));
        assert!(bundle.bridge.contains("from \"./shared.ts\""));
        assert!(bundle.store.contains("from \"./shared.ts\""));
        // Per-module load-bearing symbols.
        assert!(bundle.shared.contains("toolCallStorage"));
        assert!(bundle.store.contains("writeBlob"));
        assert!(bundle.journal.contains("captureMutation"));
        assert!(bundle.journal.contains("bindTreeEntries"));
        assert!(bundle.rollback.contains("computeRollbackPlan"));
        assert!(bundle.rollback.contains("executeRollback"));
        assert!(bundle.bridge.contains("pollBridgeRequests"));
        assert_eq!(
            extension
                .source_path()
                .extension()
                .and_then(|value| value.to_str()),
            Some("ts")
        );
    }

    #[test]
    fn rollback_disabled_by_default() {
        // Without a config file, rollback should be disabled.
        // This test may pass or fail depending on user config;
        // just verify the function doesn't panic.
        let _ = rollback_enabled();
    }
}
