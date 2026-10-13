#![deny(clippy::indexing_slicing)]

mod config_values;
mod error;
mod hooks_cla;
mod mcp;
mod reporting;
mod rewrite;
mod scope;
mod source_cla;

pub use config_values::{
    ClaudeSettingsSnapshot, ImportPatternMode, ImportPermission, ImportPlan, ImportResult,
    ImportRuleAction, ImportScope, ImportTool, ImportableItem, PathKind,
};
pub use error::MigrationError;
pub use rewrite::{apply_import, write_import_marker};
pub use scope::find_project_root;
pub use source_cla::scan_importable_settings;
