//! What the `/rewind` picker shows for each rewind point.

use crate::session::user_message::extract_user_query;
use crate::util::truncate;

const PREVIEW_MAX_CHARS: usize = 60;

/// First non-empty line of the prompt, shortened to fit the picker. `None` if the prompt is blank.
pub fn rewind_prompt_preview(prompt: &str) -> Option<String> {
    let query = extract_user_query(prompt);
    let first_line = query.lines().map(str::trim).find(|line| !line.is_empty())?;
    if first_line.chars().count() > PREVIEW_MAX_CHARS {
        Some(format!(
            "{}...",
            truncate(first_line, PREVIEW_MAX_CHARS - 3)
        ))
    } else {
        Some(first_line.to_owned())
    }
}

/// Empty when `ms` is out of chrono's range
pub fn ms_to_rfc3339(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "rewind_preview_tests.rs"]
mod tests;
