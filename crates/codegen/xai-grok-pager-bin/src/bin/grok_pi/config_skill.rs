use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub(super) const CONFIG_SKILL_NAME: &str = "grok-pi-config";
const CONFIG_SKILL_BODY: &str = include_str!("skills/grok-pi-config/SKILL.md");

/// F2 `[ui].pi_config_skill` — load the embedded grok-pi configuration skill.
/// Missing/invalid config preserves the default-on behavior.
pub(super) fn config_skill_enabled() -> bool {
    let config = xai_grok_shell::config::load_effective_config().ok();
    config_skill_enabled_from_config(config.as_ref())
}

fn config_skill_enabled_from_config(config: Option<&toml::Value>) -> bool {
    config
        .and_then(|root| root.get("ui"))
        .and_then(|ui| ui.get("pi_config_skill"))
        .and_then(toml::Value::as_bool)
        .unwrap_or(true)
}

pub(super) fn sync_config_skill_cache(enabled: bool) -> Result<Option<PathBuf>> {
    sync_config_skill_cache_in_home(&xai_grok_shell::util::grok_home::grok_home(), enabled)
}

fn sync_config_skill_cache_in_home(home: &Path, enabled: bool) -> Result<Option<PathBuf>> {
    let dir = config_skill_dir(home);
    if enabled {
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create {}", dir.display()))?;
        let path = dir.join("SKILL.md");
        xai_grok_config::fs_atomic::write_atomically(&path, CONFIG_SKILL_BODY, None)
            .with_context(|| format!("failed to write {}", path.display()))?;
        Ok(Some(path))
    } else {
        if dir.exists() {
            std::fs::remove_dir_all(&dir)
                .with_context(|| format!("failed to remove {}", dir.display()))?;
        }
        Ok(None)
    }
}

fn config_skill_dir(home: &Path) -> PathBuf {
    home.join("bundled").join("skills").join(CONFIG_SKILL_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_skill_enabled_from_config_defaults_on_and_honors_false() {
        assert!(config_skill_enabled_from_config(None));
        let enabled: toml::Value = toml::from_str(
            "[ui]
pi_config_skill = true
",
        )
        .expect("parse enabled config");
        assert!(config_skill_enabled_from_config(Some(&enabled)));
        let disabled: toml::Value = toml::from_str(
            "[ui]
pi_config_skill = false
",
        )
        .expect("parse disabled config");
        assert!(!config_skill_enabled_from_config(Some(&disabled)));
        assert!(CONFIG_SKILL_BODY.contains("name: grok-pi-config"));
    }

    #[test]
    fn sync_cache_writes_and_removes_embedded_skill_under_home() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = sync_config_skill_cache_in_home(tmp.path(), true)
            .expect("write skill")
            .expect("enabled skill path");
        let expected = tmp
            .path()
            .join("bundled")
            .join("skills")
            .join(CONFIG_SKILL_NAME)
            .join("SKILL.md");
        assert_eq!(path, expected);
        let body = std::fs::read_to_string(&path).expect("read skill");
        assert!(body.contains("~/.grok-pi/config.toml"));

        assert_eq!(
            sync_config_skill_cache_in_home(tmp.path(), false).expect("remove skill"),
            None
        );
        assert!(!path.exists());
    }
}
