use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaudeImport {
    Imported,
    NotImported,
}

impl ClaudeImport {
    #[must_use]
    pub fn from_marker(is_marked: bool) -> ClaudeImport {
        if is_marked {
            ClaudeImport::Imported
        } else {
            ClaudeImport::NotImported
        }
    }
}

#[must_use]
pub fn user_config_file(grok_home: &Path) -> PathBuf {
    grok_home.join(crate::USER_CONFIG_FILENAME)
}

// A missing file, a missing key, or invalid TOML counts as not imported.
pub fn is_claude_import_marked(grok_home: &Path) -> bool {
    let Ok(contents) = std::fs::read_to_string(user_config_file(grok_home)) else {
        return false;
    };
    let Ok(config) = toml::from_str::<toml::Value>(&contents) else {
        return false;
    };
    config
        .get("claude_compat")
        .and_then(|section| section.get("imported"))
        .and_then(toml::Value::as_bool)
        .unwrap_or(false)
}

#[cfg(test)]
#[path = "claude_import_tests.rs"]
mod tests;
