use std::path::{Path, PathBuf};

use crate::McpPreferencesFile;

/// Overwriting a `Corrupt` file would erase the setup values of other servers.
#[derive(Debug, Clone)]
pub enum McpPreferencesLoad {
    Ok(McpPreferencesFile),
    Missing,
    Corrupt,
}

impl McpPreferencesLoad {
    pub fn file(&self) -> McpPreferencesFile {
        match self {
            McpPreferencesLoad::Ok(f) => f.clone(),
            McpPreferencesLoad::Missing | McpPreferencesLoad::Corrupt => {
                McpPreferencesFile::default()
            }
        }
    }

    pub fn is_writable(&self) -> bool {
        !matches!(self, McpPreferencesLoad::Corrupt)
    }
}

pub fn load_mcp_preferences() -> McpPreferencesLoad {
    load_mcp_preferences_from(&mcp_preferences_path())
}

pub fn mcp_preferences_path() -> PathBuf {
    crate::grok_home().join("mcp_preferences.json")
}

pub fn load_mcp_preferences_from(path: &Path) -> McpPreferencesLoad {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return McpPreferencesLoad::Missing,
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "failed to read MCP preferences");
            return McpPreferencesLoad::Corrupt;
        }
    };
    match serde_json::from_str(&content) {
        Ok(file) => McpPreferencesLoad::Ok(file),
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "failed to parse MCP preferences");
            McpPreferencesLoad::Corrupt
        }
    }
}

#[cfg(test)]
#[path = "preferences_tests.rs"]
mod tests;
