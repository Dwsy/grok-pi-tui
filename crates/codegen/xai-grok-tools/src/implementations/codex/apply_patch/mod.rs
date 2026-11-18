//! Codex `apply_patch` — core patch engine (pure library, no I/O).
//!
//! This module ports the codex patch parser, fuzzy matcher, and replacement
//! logic as pure functions with zero filesystem dependencies. All I/O
//! (reading/writing files) is handled by the tool layer in a later milestone.
//!
//! # Submodules
//!
//! - [`apply`] — `derive_new_contents()`, `compute_replacements()`,
//!   `apply_replacements()` — all accept `&str` input.
//! - [`errors`] — `ApplyPatchError`, `ParseError`.
//! - [`parser`] — `parse_patch()`, `Hunk`, `UpdateFileChunk`.
//! - [`seek_sequence`] — 4-tier fuzzy line matcher.

#[cfg(feature = "stock-tool-dialects")]
pub mod apply;
#[cfg(feature = "stock-tool-dialects")]
pub mod errors;
#[cfg(feature = "stock-tool-dialects")]
pub mod parser;
#[cfg(feature = "stock-tool-dialects")]
pub mod seek_sequence;
pub mod tool;

// Re-exports for convenience.
#[cfg(feature = "stock-tool-dialects")]
pub use apply::derive_new_contents;
#[cfg(feature = "stock-tool-dialects")]
pub use errors::{ApplyPatchError, ParseError};
#[cfg(feature = "stock-tool-dialects")]
pub use parser::{Hunk, ParsedPatch, UpdateFileChunk, parse_patch};
pub use tool::ApplyPatchInput;
#[cfg(feature = "stock-tool-dialects")]
pub use tool::ApplyPatchTool;
