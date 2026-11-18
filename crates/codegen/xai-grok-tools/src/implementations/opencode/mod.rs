//! OpenCode-specific tool implementations.
//!
//! This module contains tools ported from the [sst/opencode] project
//! (`packages/opencode/src/tool/`), which is licensed under the MIT
//! License. Copyright (c) 2025 opencode. See `THIRD_PARTY_NOTICES.md` at
//! the crate root for the full license text.
//!
//! [sst/opencode]: https://github.com/sst/opencode
//!
//! Most tools follow the opencode parameter
//! naming conventions (e.g., `read`/`edit` use `filePath`, `oldString`,
//! `newString`); `write` is the exception — its input was normalized to
//! snake_case (`file_path`) for grok_build consistency.

pub mod bash;
pub mod edit;
pub mod glob;
pub mod grep;
pub mod read;
pub mod skill;
pub mod todowrite;
pub mod write;

#[cfg(feature = "stock-tool-dialects")]
pub use bash::BashTool as OpenCodeBashTool;
#[cfg(feature = "stock-tool-dialects")]
pub use edit::EditTool as OpenCodeEditTool;
#[cfg(feature = "stock-tool-dialects")]
pub use glob::GlobTool as OpenCodeGlobTool;
#[cfg(feature = "stock-tool-dialects")]
pub use grep::GrepTool as OpenCodeGrepTool;
#[cfg(feature = "stock-tool-dialects")]
pub use read::ReadTool as OpenCodeReadTool;
#[cfg(feature = "stock-tool-dialects")]
pub use skill::SkillTool as OpenCodeSkillTool;
#[cfg(feature = "stock-tool-dialects")]
pub use todowrite::TodoWriteTool as OpenCodeTodoWriteTool;
#[cfg(feature = "stock-tool-dialects")]
pub use write::WriteTool as OpenCodeWriteTool;
