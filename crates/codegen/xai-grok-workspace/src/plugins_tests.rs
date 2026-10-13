use std::path::Path;
use std::path::PathBuf;

use xai_grok_hooks::discovery::ClaudeImport;
use xai_grok_hooks::trust::Trust;

use super::PluginConfigInputs;
use super::resolve_effective_plugins_config;

fn inputs<'a>(cwd: &'a Path, effective_config: Option<&'a toml::Value>) -> PluginConfigInputs<'a> {
    PluginConfigInputs {
        effective_config,
        home: None,
        grok_home: None,
        cwd,
        trust: Trust::Untrusted,
        claude_import: ClaudeImport::Imported,
    }
}

#[test]
fn skips_claude_settings_plugins_only_after_claude_import() {
    let home = tempfile::tempdir().expect("create temp home");
    let cwd = tempfile::tempdir().expect("create temp cwd");

    let claude = home.path().join(".claude");
    std::fs::create_dir_all(&claude).expect("create .claude");
    std::fs::write(
        claude.join("settings.json"),
        r#"{"enabledPlugins":{"cutoff-probe":true}}"#,
    )
    .expect("write .claude/settings.json");

    for (claude_import, expected) in [
        (ClaudeImport::Imported, false),
        (ClaudeImport::NotImported, true),
    ] {
        let config = resolve_effective_plugins_config(PluginConfigInputs {
            home: Some(home.path()),
            claude_import,
            ..inputs(cwd.path(), None)
        });
        assert_eq!(
            expected,
            config.enabled.iter().any(|name| name == "cutoff-probe"),
            "{claude_import:?}"
        );
    }
}

#[test]
fn takes_plugins_from_the_passed_effective_config() {
    let cwd = tempfile::tempdir().expect("create temp cwd");
    let effective_config: toml::Value =
        toml::from_str("[plugins]\nenabled = [\"campaign-plugin\"]\n").expect("parse config");

    let config = resolve_effective_plugins_config(inputs(cwd.path(), Some(&effective_config)));

    assert_eq!(vec!["campaign-plugin".to_owned()], config.enabled);
}

#[test]
fn a_malformed_plugins_list_keeps_the_disabled_list() {
    let cwd = tempfile::tempdir().expect("create temp cwd");
    let effective_config: toml::Value =
        toml::from_str("[plugins]\npaths = \"not-a-list\"\ndisabled = [\"kept\"]\n")
            .expect("parse config");

    let config = resolve_effective_plugins_config(inputs(cwd.path(), Some(&effective_config)));

    assert_eq!(vec!["kept".to_owned()], config.disabled);
}

#[test]
fn project_disabled_merges_untrusted_and_project_paths_only_when_trusted() {
    let cwd = tempfile::tempdir().expect("create temp cwd");
    let grok = cwd.path().join(".grok");
    std::fs::create_dir_all(&grok).expect("create .grok");
    std::fs::write(
        grok.join("config.toml"),
        "[plugins]\npaths = [\"./project-plugin\"]\ndisabled = [\"project-off\"]\n",
    )
    .expect("write .grok/config.toml");

    for (trust, expected_paths) in [
        (Trust::Untrusted, Vec::new()),
        (Trust::Trusted, vec![PathBuf::from("./project-plugin")]),
    ] {
        let config = resolve_effective_plugins_config(PluginConfigInputs {
            trust,
            ..inputs(cwd.path(), None)
        });
        assert_eq!(
            (expected_paths, vec!["project-off".to_owned()]),
            (config.config_paths, config.disabled),
            "{trust:?}"
        );
    }
}
