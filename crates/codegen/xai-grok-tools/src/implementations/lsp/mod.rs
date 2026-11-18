#[cfg(feature = "stock-lsp")]
pub mod capabilities;
#[cfg(feature = "stock-lsp")]
pub mod client;
pub mod config;
#[cfg(feature = "stock-lsp")]
pub mod diagnostics;
#[cfg(feature = "stock-lsp")]
pub mod dispatch;
#[cfg(feature = "stock-lsp")]
pub mod documents;
#[cfg(feature = "stock-lsp")]
pub mod format;
#[cfg(feature = "stock-lsp")]
pub mod manager;
#[cfg(feature = "stock-lsp")]
pub mod pending;
#[cfg(feature = "stock-lsp")]
pub mod pull;
#[cfg(feature = "stock-lsp")]
pub mod refresh;
#[cfg(feature = "stock-lsp")]
pub mod restart;
mod types;
#[cfg(feature = "stock-lsp")]
mod watched_files;
#[cfg(feature = "stock-lsp")]
pub mod workspace_open;

#[cfg(all(test, feature = "stock-lsp"))]
mod tests;

#[cfg(feature = "stock-lsp")]
pub use dispatch::LspBackendAdapter;
#[cfg(feature = "stock-lsp")]
pub use manager::{LspManager, drain_lsp_diagnostics};
#[cfg(feature = "stock-lsp")]
pub use restart::restart_monitor;
pub use types::{
    DiagnosticEntry, DiagnosticSeverityLevel, DiagnosticsSummary, DiskChangeKind,
    FileDiagnosticEntry, LspBackend, LspConfig, LspOperation, LspToolInput, LspToolResult,
};

// ── Shared types used across submodules ─────────────────────────────────

#[cfg(feature = "stock-lsp")]
use std::path::Path;
#[cfg(feature = "stock-lsp")]
use std::sync::Arc;

#[cfg(feature = "stock-lsp")]
use async_lsp::lsp_types::{Position, TextDocumentIdentifier, TextDocumentPositionParams, Url};

/// How long a reader will wait for diagnostics to arrive after an edit before reporting what it has. This is the budget the whole after-edit
/// diagnostics path is sized against: anything scheduled to happen later than this — a pull retry, say — answers after the reader has already
/// given up. Kept here, next to the pieces that have to agree on it, rather than as a number at the call site.
pub const DIAGNOSTICS_DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(500);

#[cfg(feature = "stock-lsp")]
#[derive(Debug, thiserror::Error)]
pub enum LspError {
    #[error("failed to spawn LSP server: {0}")]
    SpawnFailed(String),
    #[error("LSP server '{0}' timed out after {1:?}")]
    Timeout(String, std::time::Duration),
    #[error("LSP initialization failed: {0}")]
    InitFailed(String),
    #[error("LSP request failed: {0}")]
    RequestFailed(String),
    #[error("invalid file path")]
    InvalidPath,
}

#[cfg(feature = "stock-lsp")]
pub type DiagnosticsNotify = Arc<tokio::sync::Notify>;
#[cfg(feature = "stock-lsp")]
pub type LspMainLoop = async_lsp::MainLoop<async_lsp::router::Router<()>>;

#[cfg(feature = "stock-lsp")]
pub fn file_uri(path: &Path) -> Result<Url, LspError> {
    Url::from_file_path(path).map_err(|_| LspError::InvalidPath)
}

#[cfg(feature = "stock-lsp")]
pub fn text_document_position(
    path: &Path,
    line: u32,
    column: u32,
) -> Result<TextDocumentPositionParams, LspError> {
    Ok(TextDocumentPositionParams {
        text_document: TextDocumentIdentifier {
            uri: file_uri(path)?,
        },
        position: Position {
            line,
            character: column,
        },
    })
}
