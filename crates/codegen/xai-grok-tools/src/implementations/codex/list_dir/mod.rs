//! Codex `list_dir` — paginated, depth-limited directory listing.
//!
//! Ported from `codex-rs/core/src/tools/handlers/list_dir.rs`.

pub mod tool;

pub use tool::CodexListDirInput;
#[cfg(feature = "stock-tool-dialects")]
pub use tool::CodexListDirTool;
