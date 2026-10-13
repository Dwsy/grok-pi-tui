use std::path::PathBuf;

/// Errors that can occur during hook loading, parsing, or execution.
#[derive(Debug, thiserror::Error)]
pub enum HookError {
    #[error("failed to read hook file {path}: {source}")]
    ReadFile {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("failed to parse hook file {path}: {detail}")]
    ParseFile { path: PathBuf, detail: String },

    #[error("hook {name} in {path}: invalid regex pattern: {source}")]
    InvalidMatcher {
        name: String,
        path: PathBuf,
        source: regex::Error,
    },

    #[error("hook {name} timed out after {elapsed_ms}ms")]
    Timeout { name: String, elapsed_ms: u64 },

    #[error("hook {name} command failed: {source}")]
    CommandFailed {
        name: String,
        source: std::io::Error,
    },

    #[error("hook {name} produced invalid output: {detail}")]
    InvalidOutput { name: String, detail: String },

    #[error("hook {name}: command not found or not executable: {path}")]
    CommandNotFound { name: String, path: PathBuf },

    #[error("hook {name} in {path}: {detail}")]
    InvalidConfig {
        name: String,
        path: PathBuf,
        detail: String,
    },

    #[error(
        "hook {name} in {path}: unsupported handler type '{handler_type}', expected 'command' or 'http'"
    )]
    UnsupportedHandlerType {
        name: String,
        path: PathBuf,
        handler_type: String,
    },

    #[error("config unreadable, so vendor hooks use their default settings: {source}")]
    ConfigUnreadable { source: std::io::Error },

    #[error(
        "remote settings were not available from the settings cache, so vendor hooks use their local settings"
    )]
    RemoteSettingsUnavailable,

    #[error(
        "git root {root} is neither the workspace {workspace} nor a parent of it, so its project hooks are not loaded"
    )]
    GitRootDoesNotContainWorkspace { root: PathBuf, workspace: PathBuf },

    #[error(
        "the git root of {workspace} could not be found, so its project hooks are not loaded: {detail}"
    )]
    GitDiscoveryFailed { workspace: PathBuf, detail: String },

    #[error(
        "{workspace} is outside {root}, the only directory the hook service's sandbox lets hooks work in, so no hooks are loaded"
    )]
    WorkspaceOutsideSandbox { workspace: PathBuf, root: PathBuf },
}
