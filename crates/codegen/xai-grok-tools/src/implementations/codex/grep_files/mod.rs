//! Codex `grep_files` — file-path-only regex search.
//!
//! Ported from `codex-rs/core/src/tools/handlers/grep_files.rs`.

pub mod tool;

pub use tool::CodexGrepFilesInput;
#[cfg(feature = "stock-tool-dialects")]
pub use tool::CodexGrepFilesTool;
