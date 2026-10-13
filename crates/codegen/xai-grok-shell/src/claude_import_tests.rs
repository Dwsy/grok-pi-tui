use super::*;
use toml::Value as TomlValue;
use xai_grok_workspace::HookSourceConfig;

use serial_test::serial;

fn source_paths(
    dir: &std::path::Path,
    compat: &xai_grok_tools::types::compat::CompatConfig,
) -> xai_grok_hooks::discovery::HookSourcePaths {
    xai_grok_hooks::discovery::discover_hook_source_paths(
        xai_grok_hooks::discovery::DiscoveryOptions {
            git_root: Some(dir),
            grok_home: None,
            home: Some(dir),
            compat: compat.hooks(),
            claude_import: crate::claude_import::import_marker(),
            trust: xai_grok_hooks::trust::Trust::Trusted,
        },
    )
}

fn source_path_strs(sources: &[HookSourceConfig]) -> Vec<String> {
    sources
        .iter()
        .map(|source| match source {
            HookSourceConfig::SettingsFile(path) | HookSourceConfig::Directory(path) => {
                path.to_string_lossy().into_owned()
            }
        })
        .collect()
}

struct MarkerGuard;

impl Drop for MarkerGuard {
    fn drop(&mut self) {
        reset_marker_cache_for_test();
        unsafe { std::env::remove_var("_GROK_CLAUDE_MARKER_OVERRIDE") };
    }
}

#[test]
#[serial]
fn discover_hook_source_paths_skips_claude_when_marker_set() {
    let _g = MarkerGuard;
    refresh_marker_cache(true);
    let dir = tempfile::tempdir().unwrap();
    let compat = xai_grok_tools::types::compat::CompatConfig::default();
    let paths = source_paths(dir.path(), &compat);
    let project_strs = source_path_strs(&paths.project);
    assert!(
        !project_strs.iter().any(|s| s.contains(".claude")),
        "project sources should not include .claude/ when marker set; got {:?}",
        project_strs
    );
    let global_strs = source_path_strs(&paths.global);
    assert!(
        !global_strs.iter().any(|s| s.contains("/.claude/")),
        "global sources should not include ~/.claude/ when marker set; got {:?}",
        global_strs
    );
}

#[test]
#[serial]
fn gate_load_claude_env_returns_empty_when_marker_set() {
    let _g = MarkerGuard;
    refresh_marker_cache(true);
    let dir = tempfile::tempdir().unwrap();
    let env = xai_grok_workspace::permission::claude_settings::load_claude_env_with_project(
        dir.path(),
        true,
    );
    assert!(
        env.is_empty(),
        "load_claude_env_with_project should be empty when marker set"
    );
}

#[test]
#[serial]
fn discover_hook_source_paths_includes_claude_when_marker_unset() {
    let _g = MarkerGuard;
    refresh_marker_cache(false);
    let dir = tempfile::tempdir().unwrap();
    let compat = xai_grok_tools::types::compat::CompatConfig::default();
    let paths = source_paths(dir.path(), &compat);
    let project_strs = source_path_strs(&paths.project);
    assert!(
        project_strs.iter().any(|s| s.contains(".claude")),
        "project sources should include .claude/ when marker unset; got {:?}",
        project_strs
    );
}

#[test]
#[serial]
fn discover_hook_source_paths_includes_cursor_hooks_json() {
    let _g = MarkerGuard;
    refresh_marker_cache(false);
    let dir = tempfile::tempdir().unwrap();
    let compat = xai_grok_tools::types::compat::CompatConfig::default();
    let paths = source_paths(dir.path(), &compat);
    let global_strs = source_path_strs(&paths.global);
    assert!(
        global_strs
            .iter()
            .any(|s| s.contains(".cursor") && s.ends_with("hooks.json")),
        "global sources should include ~/.cursor/hooks.json; got {:?}",
        global_strs
    );
    let project_strs = source_path_strs(&paths.project);
    assert!(
        project_strs
            .iter()
            .any(|s| s.contains(".cursor") && s.ends_with("hooks.json")),
        "project sources should include .cursor/hooks.json; got {:?}",
        project_strs
    );
}

#[test]
#[serial]
fn discover_hook_source_paths_skips_cursor_when_disabled() {
    let _g = MarkerGuard;
    refresh_marker_cache(false);
    let dir = tempfile::tempdir().unwrap();
    let mut compat = xai_grok_tools::types::compat::CompatConfig::default();
    compat.cursor.hooks = false;
    let paths = source_paths(dir.path(), &compat);
    let global_strs = source_path_strs(&paths.global);
    assert!(
        !global_strs.iter().any(|s| s.contains(".cursor")),
        "global sources should not include .cursor/ when disabled; got {:?}",
        global_strs
    );
    let project_strs = source_path_strs(&paths.project);
    assert!(
        !project_strs.iter().any(|s| s.contains(".cursor")),
        "project sources should not include .cursor/ when disabled; got {:?}",
        project_strs
    );
}

#[test]
#[serial]
fn discover_hook_source_paths_skips_claude_when_compat_disabled() {
    let _g = MarkerGuard;
    refresh_marker_cache(false);
    let dir = tempfile::tempdir().unwrap();
    let mut compat = xai_grok_tools::types::compat::CompatConfig::default();
    compat.claude.hooks = false;
    let paths = source_paths(dir.path(), &compat);
    let global_strs = source_path_strs(&paths.global);
    assert!(
        !global_strs.iter().any(|s| s.contains("/.claude/")),
        "global sources should not include ~/.claude/ when compat disabled; got {:?}",
        global_strs
    );
    let project_strs = source_path_strs(&paths.project);
    assert!(
        !project_strs.iter().any(|s| s.contains(".claude")),
        "project sources should not include .claude/ when compat disabled; got {:?}",
        project_strs
    );
}

#[test]
#[serial]
fn as_sources_gates_project_sources_on_trust() {
    let _g = MarkerGuard;
    refresh_marker_cache(false);
    let dir = tempfile::tempdir().unwrap();
    let compat = xai_grok_tools::types::compat::CompatConfig::default();
    let paths = source_paths(dir.path(), &compat);
    assert!(
        !paths.project.is_empty(),
        "project source paths should be non-empty for a git_root"
    );

    let (global_untrusted, project) = paths.as_sources(xai_grok_hooks::trust::Trust::Untrusted);
    assert_eq!(
        global_untrusted.len(),
        paths.global.len(),
        "global sources must survive untrusted"
    );
    assert!(
        project.is_empty(),
        "untrusted: as_sources must drop all project sources"
    );

    let (_global, project) = paths.as_sources(xai_grok_hooks::trust::Trust::Trusted);
    assert!(
        !project.is_empty(),
        "trusted: as_sources must keep project sources"
    );
}

#[test]
#[serial]
fn discover_hooks_honors_claude_compat_gate() {
    let _g = MarkerGuard;
    refresh_marker_cache(false);

    let git_root = tempfile::tempdir().unwrap();
    let claude_dir = git_root.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(
        claude_dir.join("settings.json"),
        r#"{"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":"claude_compat_gate_probe.sh"}]}]}}"#,
    )
    .unwrap();

    let has_probe = |reg: &xai_grok_hooks::discovery::HookRegistry| {
        reg.all_hooks().iter().any(|h| {
            h.command_raw
                .as_deref()
                .unwrap_or_default()
                .contains("claude_compat_gate_probe")
        })
    };

    let mut compat = xai_grok_tools::types::compat::CompatConfig::default();

    compat.claude.hooks = false;
    let (reg, _errs) = crate::util::hooks::discover_hooks(
        &crate::util::hooks::process_hook_inputs(),
        Some(git_root.path()),
        &compat,
        xai_grok_hooks::trust::Trust::Trusted,
    );
    assert!(
        !has_probe(&reg),
        "compat.claude.hooks=false: project .claude hook must NOT be loaded"
    );

    compat.claude.hooks = true;
    let (reg, _errs) = crate::util::hooks::discover_hooks(
        &crate::util::hooks::process_hook_inputs(),
        Some(git_root.path()),
        &compat,
        xai_grok_hooks::trust::Trust::Trusted,
    );
    assert!(
        has_probe(&reg),
        "compat.claude.hooks=true: project .claude hook must be loaded"
    );
}

#[test]
fn paths_config_deserializes() {
    let toml_str = r#"
[paths]
extra_skill_dirs = ["/a/skills", "/b/skills"]
extra_rule_dirs = ["/c/rules"]
"#;
    let value: TomlValue = toml::from_str(toml_str).unwrap();
    let paths = value.get("paths").unwrap();
    let cfg: crate::agent::config::PathsConfig = paths.clone().try_into().unwrap();
    assert_eq!(cfg.extra_skill_dirs, vec!["/a/skills", "/b/skills"]);
    assert_eq!(cfg.extra_rule_dirs, vec!["/c/rules"]);
}

#[test]
fn paths_config_default_empty() {
    let cfg = crate::agent::config::PathsConfig::default();
    assert!(cfg.extra_skill_dirs.is_empty());
    assert!(cfg.extra_rule_dirs.is_empty());
}

#[test]
#[serial]
fn gate_load_mcp_json_servers_returns_empty_when_marker_set() {
    let _g = MarkerGuard;
    refresh_marker_cache(true);
    let dir = tempfile::tempdir().unwrap();
    let servers = crate::util::config::load_mcp_json_servers(dir.path());
    assert!(
        servers.is_empty(),
        "load_mcp_json_servers should be empty when marker set"
    );
}

#[test]
#[serial]
fn gate_mcp_server_sources_report_imported_when_marker_set() {
    let _g = MarkerGuard;
    refresh_marker_cache(true);
    let dir = tempfile::tempdir().unwrap();
    let compat = xai_grok_tools::types::compat::CompatConfig::default();

    let sources = crate::util::config::mcp_server_sources(dir.path(), &compat, "test");

    assert_eq!(
        xai_grok_config::ClaudeImport::Imported,
        sources.claude_import
    );
}

#[tokio::test]
#[serial]
async fn gate_resolve_permissions_with_provenance_skips_claude_when_marker_set() {
    let _g = MarkerGuard;
    refresh_marker_cache(true);
    unsafe { std::env::set_var("_GROK_CLAUDE_MARKER_OVERRIDE", "1") };
    let dir = tempfile::tempdir().unwrap();
    let claude_dir = dir.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(
        claude_dir.join("settings.json"),
        r#"{ "permissions": { "allow": ["Bash(echo hi)"] } }"#,
    )
    .unwrap();

    let resolved = xai_grok_workspace::permission::resolution::resolve_permissions_with_provenance(
        dir.path(),
        true,
    )
    .await
    .resolved;
    if let Some(r) = resolved {
        let tempdir_claude = claude_dir.join("settings.json");
        use xai_grok_workspace::permission::types::RequirementSource;
        let leaked: Vec<&RequirementSource> = r
            .sources
            .iter()
            .filter(
                |s| matches!(s, RequirementSource::Settings { path } if path == &tempdir_claude),
            )
            .collect();
        assert!(
            leaked.is_empty(),
            "tempdir .claude/settings.json should not produce rules when marker set, \
             but got {} rule(s) from {}: {:?}",
            leaked.len(),
            tempdir_claude.display(),
            leaked
        );
    }
}

#[test]
#[serial]
fn gate_marker_cache_unset_means_uses_disk() {
    let _g = MarkerGuard;
    reset_marker_cache_for_test();
    let _ = is_claude_import_marked();
    assert!(
        MARKER_CACHE
            .read()
            .expect("MARKER_CACHE poisoned")
            .is_some(),
        "cache should be populated after a call"
    );
}

#[test]
fn bash_colon_star_prefix_imports_as_its_bare_prefix() {
    use xai_grok_workspace::permission::rules::parse_permission_rule;

    let colon_star = parse_permission_rule("Bash(sed:*)", RuleAction::Deny).expect("valid rule");
    let bare = parse_permission_rule("Bash(sed)", RuleAction::Deny).expect("valid rule");

    let imported = import_permission(colon_star).expect("Bash is imported");

    assert_eq!(Some("sed"), imported.pattern.as_deref());
    assert_eq!(Some(imported), import_permission(bare));
}
