use anyhow::{Context, Result};
use std::{
    fs::{File, create_dir_all},
    io::Write,
    path::{Path, PathBuf},
};
use tempfile::TempDir;

/// Materialized pi-grok-subagents extension bundle. The source directory
/// remains alive for the Pi process lifetime so relative imports between the
/// authored TypeScript modules continue to resolve.
pub(super) struct SubagentExtension {
    _source_dir: TempDir,
    source_path: PathBuf,
}

impl SubagentExtension {
    pub(super) fn source_path(&self) -> &Path {
        &self.source_path
    }
}

fn write_source_file(dir: &Path, name: &str, source: &str) -> Result<PathBuf> {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        create_dir_all(parent)
            .with_context(|| format!("create Pi subagent extension module directory for {name}"))?;
    }
    let mut file = File::create(&path)
        .with_context(|| format!("create Pi subagent extension module {name}"))?;
    file.write_all(source.as_bytes())
        .with_context(|| format!("write Pi subagent extension module {name}"))?;
    file.flush()
        .with_context(|| format!("flush Pi subagent extension module {name}"))?;
    File::open(&path).and_then(|source| source.sync_all()).ok();
    Ok(path)
}

/// Materialize the bundled Pi child-session lifecycle owner as a standalone
/// extension. The source remains a TypeScript Pi extension; this wrapper only
/// gives the launched Pi process a durable `.ts` entry path for its lifetime.
/// Every authored module must be materialized: Pi loads the entry `index.ts`
/// from the temp directory and resolves its relative imports there.
pub(super) fn write_subagent_extension() -> Result<SubagentExtension> {
    let source_dir = tempfile::Builder::new()
        .prefix("pi-grok-subagents-")
        .tempdir()
        .context("create Pi subagent extension source directory")?;
    let source_path = write_source_file(
        source_dir.path(),
        "index.ts",
        include_str!("../../../../../../extensions/pi-grok-subagents/index.ts"),
    )?;
    write_source_file(
        source_dir.path(),
        "shared.ts",
        include_str!("../../../../../../extensions/pi-grok-subagents/shared.ts"),
    )?;
    write_source_file(
        source_dir.path(),
        "definitions.ts",
        include_str!("../../../../../../extensions/pi-grok-subagents/definitions.ts"),
    )?;
    write_source_file(
        source_dir.path(),
        "config-ui.ts",
        include_str!("../../../../../../extensions/pi-grok-subagents/config-ui.ts"),
    )?;
    write_source_file(
        source_dir.path(),
        "bridge.ts",
        include_str!("../../../../../../extensions/pi-grok-subagents/bridge.ts"),
    )?;
    write_source_file(
        source_dir.path(),
        "runtime.ts",
        include_str!("../../../../../../extensions/pi-grok-subagents/runtime.ts"),
    )?;
    write_source_file(
        source_dir.path(),
        "tools-v1.ts",
        include_str!("../../../../../../extensions/pi-grok-subagents/tools-v1.ts"),
    )?;
    write_source_file(
        source_dir.path(),
        "teams.ts",
        include_str!("../../../../../../extensions/pi-grok-subagents/teams.ts"),
    )?;
    write_source_file(
        source_dir.path(),
        "v2.ts",
        include_str!("../../../../../../extensions/pi-grok-subagents/v2.ts"),
    )?;
    write_source_file(
        source_dir.path(),
        "teams/research.json",
        include_str!("../../../../../../extensions/pi-grok-subagents/teams/research.json"),
    )?;
    write_source_file(
        source_dir.path(),
        "teams/implementation.json",
        include_str!("../../../../../../extensions/pi-grok-subagents/teams/implementation.json"),
    )?;
    write_source_file(
        source_dir.path(),
        "teams/review.json",
        include_str!("../../../../../../extensions/pi-grok-subagents/teams/review.json"),
    )?;
    Ok(SubagentExtension {
        _source_dir: source_dir,
        source_path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subagent_extension_materializes_every_relative_import() {
        // Regression guard (mirrors the pi-grok-bash/recap bundles): a module
        // added to the authored extension but not to the injector breaks Pi
        // bootstrap with `Cannot find module './<name>.ts'` at RPC startup.
        let extension = write_subagent_extension().expect("write extension");
        let dir = extension.source_path().parent().expect("source dir");
        for entry in std::fs::read_dir(dir).expect("bundle dir") {
            let path = entry.expect("bundle entry").path();
            if path.extension().and_then(|value| value.to_str()) != Some("ts") {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("read module");
            for found in source.match_indices("./") {
                let rest = &source[found.0 + 2..];
                let end = rest.find('"').expect("relative import closing quote");
                let target = &rest[..end];
                assert!(
                    target.ends_with(".ts"),
                    "{} imports {:?} without an explicit .ts extension",
                    path.display(),
                    target
                );
                assert!(
                    dir.join(target).is_file(),
                    "{} imports ./{target}; injector must materialize it",
                    path.file_name().and_then(|n| n.to_str()).unwrap_or("?")
                );
            }
        }
        assert_eq!(
            extension
                .source_path()
                .extension()
                .and_then(|value| value.to_str()),
            Some("ts")
        );
    }

    #[test]
    fn subagent_extension_materializes_every_bundled_team_json() {
        let extension = write_subagent_extension().expect("write extension");
        let dir = extension.source_path().parent().expect("source dir");
        let authored_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../extensions/pi-grok-subagents/teams");
        let json_names = |path: &std::path::Path| {
            let mut names = std::fs::read_dir(path)
                .unwrap_or_else(|_| panic!("read team directory {}", path.display()))
                .filter_map(|entry| entry.ok())
                .filter_map(|entry| {
                    let path = entry.path();
                    (path.extension().and_then(|value| value.to_str()) == Some("json"))
                        .then(|| entry.file_name().to_string_lossy().into_owned())
                })
                .collect::<Vec<_>>();
            names.sort();
            names
        };
        assert_eq!(
            json_names(&dir.join("teams")),
            json_names(&authored_dir),
            "Rust injector must materialize exactly the authored bundled team JSON files"
        );
    }

    #[test]
    fn subagent_extension_source_is_a_loadable_typescript_module() {
        let extension = write_subagent_extension().expect("write extension");
        let dir = extension.source_path().parent().expect("source dir");
        let read = |name: &str| {
            std::fs::read_to_string(dir.join(name)).unwrap_or_else(|_| panic!("read {name} module"))
        };
        let index = read("index.ts");
        let shared = read("shared.ts");
        let config_ui = read("config-ui.ts");
        let bridge = read("bridge.ts");
        let runtime = read("runtime.ts");
        let tools_v1 = read("tools-v1.ts");
        let teams = read("teams.ts");
        let v2 = read("v2.ts");
        assert!(bridge.contains("export type BridgeKind = \"spawned\" | \"finished\";"));
        assert!(bridge.contains("pi.appendEntry(BRIDGE_TYPE, envelope);"));
        assert!(!index.contains("PROGRESS_INTERVAL_MS"));
        assert!(!index.contains("emitProgress("));
        assert!(!index.contains("emit(record, \"child_update\""));
        assert!(index.contains("process.env.PI_GROK_SUBAGENTS !== \"1\""));
        assert!(index.contains("process.env.PI_GROK_SUBAGENTS_V2 === \"1\""));
        assert!(tools_v1.contains("name: \"spawn_subagent\""));
        assert!(tools_v1.contains("__pi_grok_subagent_cancel"));
        assert!(tools_v1.contains("pi.registerCommand(\"subagents\""));
        assert!(config_ui.contains("PI_GROK_SUBAGENT_EXTENSION_CATALOG"));
        assert!(runtime.contains("additionalExtensionPaths"));
        assert!(runtime.contains("additionalSkillPaths"));
        assert!(runtime.contains("customTools"));
        assert!(v2.contains("pi-grok-team-message/v2"));
        assert!(v2.contains("name: \"spawn_team\""));
        assert!(v2.contains("name: \"team_send_message\""));
        assert!(!teams.contains("projectProjectDir"));
        assert!(teams.contains("productProjectDir"));
        assert!(dir.join("teams/research.json").is_file());
        assert!(dir.join("teams/implementation.json").is_file());
        assert!(dir.join("teams/review.json").is_file());
        assert!(shared.contains("MAX_AGENT_MODELS = 3"));
    }
}
