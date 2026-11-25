use anyhow::Result;
use std::path::{Path, PathBuf};

/// Materialized pi-grok-subagents extension bundle in a content-addressed cache
/// directory so `jiti`'s `fsCache` hits across launches.
pub(super) struct SubagentExtension {
    source_path: PathBuf,
}

impl SubagentExtension {
    pub(super) fn source_path(&self) -> &Path {
        &self.source_path
    }
}

/// Materialize the bundled Pi child-session lifecycle owner into a deterministic
/// content-addressed cache directory so `jiti` reuses compiled output across runs.
pub(super) fn write_subagent_extension() -> Result<SubagentExtension> {
    let root = super::tui_kit::materialize_cached_bundle(
        "pi-grok-subagents",
        &[
            (
                "index.ts",
                include_str!("../../../../../../extensions/pi-grok-subagents/index.ts"),
            ),
            (
                "shared.ts",
                include_str!("../../../../../../extensions/pi-grok-subagents/shared.ts"),
            ),
            (
                "definitions.ts",
                include_str!("../../../../../../extensions/pi-grok-subagents/definitions.ts"),
            ),
            (
                "config-ui.ts",
                include_str!("../../../../../../extensions/pi-grok-subagents/config-ui.ts"),
            ),
            (
                "ui-primitives.ts",
                include_str!("../../../../../../extensions/pi-grok-subagents/ui-primitives.ts"),
            ),
            (
                "runtime-settings.ts",
                include_str!("../../../../../../extensions/pi-grok-subagents/runtime-settings.ts"),
            ),
            (
                "control-center.ts",
                include_str!("../../../../../../extensions/pi-grok-subagents/control-center.ts"),
            ),
            (
                "bridge.ts",
                include_str!("../../../../../../extensions/pi-grok-subagents/bridge.ts"),
            ),
            (
                "runtime.ts",
                include_str!("../../../../../../extensions/pi-grok-subagents/runtime.ts"),
            ),
            (
                "fork.ts",
                include_str!("../../../../../../extensions/pi-grok-subagents/fork.ts"),
            ),
            (
                "tools-v1.ts",
                include_str!("../../../../../../extensions/pi-grok-subagents/tools-v1.ts"),
            ),
            (
                "teams.ts",
                include_str!("../../../../../../extensions/pi-grok-subagents/teams.ts"),
            ),
            (
                "thinking.ts",
                include_str!("../../../../../../extensions/pi-grok-subagents/thinking.ts"),
            ),
            (
                "team-ui.ts",
                include_str!("../../../../../../extensions/pi-grok-subagents/team-ui.ts"),
            ),
            (
                "v2.ts",
                include_str!("../../../../../../extensions/pi-grok-subagents/v2.ts"),
            ),
            (
                "skills/multi-agent-proactive/SKILL.md",
                include_str!(
                    "../../../../../../extensions/pi-grok-subagents/skills/multi-agent-proactive/SKILL.md"
                ),
            ),
            (
                "skills/multitask/SKILL.md",
                include_str!(
                    "../../../../../../extensions/pi-grok-subagents/skills/multitask/SKILL.md"
                ),
            ),
            (
                "teams/research.json",
                include_str!("../../../../../../extensions/pi-grok-subagents/teams/research.json"),
            ),
            (
                "teams/implementation.json",
                include_str!(
                    "../../../../../../extensions/pi-grok-subagents/teams/implementation.json"
                ),
            ),
            (
                "teams/review.json",
                include_str!("../../../../../../extensions/pi-grok-subagents/teams/review.json"),
            ),
        ],
    )?;
    Ok(SubagentExtension {
        source_path: root.join("index.ts"),
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
    fn subagent_extension_materializes_every_bundled_skill() {
        let extension = write_subagent_extension().expect("write extension");
        let dir = extension.source_path().parent().expect("source dir");
        let authored = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../extensions/pi-grok-subagents/skills");
        for entry in std::fs::read_dir(authored).expect("authored skills") {
            let entry = entry.expect("skill entry");
            let source = entry.path().join("SKILL.md");
            if !source.is_file() {
                continue;
            }
            let bundled = dir.join("skills").join(entry.file_name()).join("SKILL.md");
            assert_eq!(
                std::fs::read_to_string(&source).expect("authored skill"),
                std::fs::read_to_string(&bundled)
                    .unwrap_or_else(|_| panic!("missing bundled skill {}", bundled.display())),
                "bundled skill must match its authored content"
            );
        }
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
        let control_center = read("control-center.ts");
        let bridge = read("bridge.ts");
        let fork = read("fork.ts");
        let runtime = read("runtime.ts");
        let tools_v1 = read("tools-v1.ts");
        let teams = read("teams.ts");
        let v2 = read("v2.ts");
        assert!(
            bridge.contains(
                "export type BridgeKind = \"spawned\" | \"finished\" | \"child_update\" | \"replay_complete\";"
            )
        );
        assert!(!bridge.contains("pi.appendEntry("));
        assert!(bridge.contains("ready: Promise<void>"));
        assert!(runtime.contains("await this.emit.ready"));
        assert!(bridge.contains("SUBAGENT_STATE_SUFFIX = \".subagents.jsonl\""));
        assert!(bridge.contains("appendFileSync(stateFile"));
        assert!(bridge.contains("appendPersistedRecord(record.stateFile, snapshot)"));
        assert!(!shared.contains("pi-grok-subagent-state/v1"));
        assert!(!index.contains("PROGRESS_INTERVAL_MS"));
        assert!(!index.contains("emitProgress("));
        assert!(runtime.contains("this.emit(record, \"child_update\""));
        assert!(index.contains("process.env.PI_GROK_SUBAGENTS !== \"1\""));
        assert!(index.contains("process.env.PI_GROK_SUBAGENTS_V2 === \"1\""));
        assert!(index.contains("resources_discover"));
        assert!(index.contains("multi-agent-proactive"));
        let proactive_skill =
            std::fs::read_to_string(dir.join("skills/multi-agent-proactive/SKILL.md"))
                .expect("read bundled proactive skill");
        assert!(proactive_skill.contains("spawn_subagent"));
        assert!(proactive_skill.contains("spawn_team_agent"));
        assert!(proactive_skill.contains("child 没有因为父级 Proactive/Ultra 自动继续 fan-out"));
        assert!(tools_v1.contains("name: \"spawn_subagent\""));
        assert!(tools_v1.contains("__pi_grok_subagent_cancel"));
        assert!(shared.contains("__pi_grok_subagent_replay"));
        assert!(tools_v1.contains("pi.registerCommand(\"subagents\""));
        assert!(tools_v1.contains("pi.registerCommand(\"subagent-control\""));
        assert!(control_center.contains("openSubagentControlCenter"));
        assert!(config_ui.contains("PI_GROK_SUBAGENT_EXTENSION_CATALOG"));
        assert!(fork.contains("createForkedChildSessionManager"));
        assert!(fork.contains("isHostSingletonBridgeExtension"));
        assert!(runtime.contains("createForkedChildSessionManager"));
        assert!(runtime.contains("noExtensions: true"));
        assert!(runtime.contains("noSkills: definition?.inheritSkills === false"));
        assert!(runtime.contains("noContextFiles: definition?.inheritContext === false"));
        assert!(runtime.contains("additionalExtensionPaths: [...defaultSubagentExtensions(), ...(definition?.extensions ?? [])]"));
        // Children load no auto-discovered extensions, so the search extension the
        // operator installed for the root session must be added explicitly.
        assert!(runtime.contains("\"@ff-labs\", \"pi-fff\""));
        assert!(runtime.contains("function defaultSubagentExtensions"));
        assert!(runtime.contains("additionalSkillPaths: definition?.skills ?? []"));
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
