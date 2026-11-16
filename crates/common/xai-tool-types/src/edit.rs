//! Structured edit detail shapes shared by the stock tool backend and native
//! Pager diff rendering. These are data contracts, not tool implementations.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Context emitted alongside each search/replace edit for accurate line diffs.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SearchReplaceEditContextInformation {
    pub details: Vec<SearchReplaceEditDetail>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SearchReplaceEditDetail {
    /// Exact matched text from the original file.
    pub old_string: String,
    /// One-based line in the original file.
    pub old_line: usize,
    pub new_string: String,
    /// One-based line in the updated file.
    pub new_line: usize,
    pub context_before: String,
    pub context_after: String,
    /// Leading characters on the first affected line, if the match begins
    /// after indentation or other text rather than at a line boundary.
    #[serde(default)]
    pub line_prefix: String,
}
