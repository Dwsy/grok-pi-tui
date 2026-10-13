//! `bearer_token_file` travels from config to the MCP client in the ACP server's `_meta`, since
//! `acp::McpServerHttp` has no field for it. The config side writes the configured string and
//! [`BearerTokenPath::from_meta`] parses it where the server starts, so a bad path fails that server
//! with a reason instead of silently changing how it authenticates.

use std::fmt;
use std::path::{Path, PathBuf};

use agent_client_protocol as acp;

const META_KEY: &str = "x.ai/mcp/bearerTokenFile";

pub(crate) fn bearer_token_file_meta(configured: &str) -> acp::Meta {
    acp::Meta::from_iter([(
        META_KEY.to_owned(),
        serde_json::Value::String(configured.to_owned()),
    )])
}

/// The configured string, unparsed, for listing a client-provided server as it was given.
pub(crate) fn configured_bearer_token_file(meta: Option<&acp::Meta>) -> Option<String> {
    meta?.get(META_KEY)?.as_str().map(str::to_owned)
}

/// An absolute path to a bearer token file, with a leading `~/` already expanded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BearerTokenPath(PathBuf);

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum BearerTokenPathError {
    #[error("`_meta[\"{META_KEY}\"]` must be a string")]
    NotAString,
    #[error("bearer_token_file `{0}` starts with ~/ but the home directory is unknown")]
    NoHomeDir(String),
    #[error("bearer_token_file `{0}` must be an absolute or ~/ path")]
    NotAbsolute(String),
}

impl BearerTokenPath {
    /// # Errors
    ///
    /// [`BearerTokenPathError`] when the meta value is not a string or does not name an absolute
    /// or `~/` path.
    pub fn from_meta(meta: Option<&acp::Meta>) -> Result<Option<Self>, BearerTokenPathError> {
        let Some(value) = meta.and_then(|meta| meta.get(META_KEY)) else {
            return Ok(None);
        };
        let configured = value.as_str().ok_or(BearerTokenPathError::NotAString)?;
        Self::parse(configured, xai_dirs::home_dir().as_deref()).map(Some)
    }

    fn parse(configured: &str, home: Option<&Path>) -> Result<Self, BearerTokenPathError> {
        match configured.strip_prefix("~/") {
            Some(rest) => {
                let home =
                    home.ok_or_else(|| BearerTokenPathError::NoHomeDir(configured.to_owned()))?;
                BearerTokenPath::try_from(home.join(rest))
            }
            None => BearerTokenPath::try_from(PathBuf::from(configured)),
        }
    }
}

impl TryFrom<PathBuf> for BearerTokenPath {
    type Error = BearerTokenPathError;

    fn try_from(path: PathBuf) -> Result<Self, Self::Error> {
        if path.is_absolute() {
            Ok(BearerTokenPath(path))
        } else {
            Err(BearerTokenPathError::NotAbsolute(
                path.display().to_string(),
            ))
        }
    }
}

impl AsRef<Path> for BearerTokenPath {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl fmt::Display for BearerTokenPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.display().fmt(f)
    }
}

#[cfg(test)]
#[path = "mcp_bearer_token_file_tests.rs"]
mod tests;
