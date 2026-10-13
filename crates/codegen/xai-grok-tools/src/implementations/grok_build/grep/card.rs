//! The grep tool's result card.
//!
//! [`finalize_grep`] turns `rg --heading` output into the model-facing card: the
//! `<workspace_result …>` wrapper, the "Found …" summary, the body (lines cut to
//! `max_chars_per_line`, then to the cumulative `max_output_bytes`) and the
//! truncation footer. The streamed body (`BodyStreamer`) projects lines with the
//! same helpers, so it stays a prefix of the card body.

use std::io;

use crate::implementations::grok_build::grep::OutputMode;
use crate::types::output::{GrepFileMatch, GrepLineMatch, GrepSearchOutput};
use crate::util::truncate::truncate_line;

/// Formatting/projection knobs resolved once in `prepare_grep` and consumed by
/// both the streamed body (`BodyStreamer`) and the terminal card
/// ([`finalize_grep`]) so the two never drift.
pub(crate) struct GrepFormatConfig {
    pub(crate) output_mode: OutputMode,
    /// Line/entry budget: model `head_limit` clamped to the per-mode cap, or
    /// the per-mode default when omitted. Always finite so we can kill `rg`
    /// once enough output is collected.
    pub(crate) effective_head_limit: usize,
    /// Per-line truncation width (`trim_line`).
    pub(crate) max_chars_per_line: usize,
    /// Cumulative body byte cap.
    pub(crate) max_output_bytes: usize,
    /// Stable display path used in the `<workspace_result …>` wrapper / errors.
    pub(crate) cwd_display: String,
}

/// Terminal card for a grep that exceeded its wall-clock timeout. Shared by the
/// blocking and streaming paths.
pub(crate) fn grep_timeout_output(secs: u64) -> GrepSearchOutput {
    GrepSearchOutput {
        stdout: format!(
            "Ripgrep search timed out after {secs} seconds. \
             The search may have matched files but did not complete in time. \
             Try searching a more specific path or pattern."
        )
        .into_bytes(),
        stderr: Vec::new(),
        exit_code: -1,
        match_count: 0,
        file_matches: Vec::new(),
    }
}

/// Terminal card for a search the deadline cut short: the partial matches with a
/// notice (the streamed body stays a prefix), or the timeout-only card when
/// nothing was found.
pub(crate) fn grep_partial_timeout_output(
    stdout: Vec<u8>,
    secs: u64,
    config: &GrepFormatConfig,
) -> GrepSearchOutput {
    if stdout.is_empty() {
        return grep_timeout_output(secs);
    }
    let mut output = finalize_grep(stdout, true, Vec::new(), 0, config);
    output.stdout.extend_from_slice(
        format!(
            "\nRipgrep search timed out after {secs} seconds; \
             the matches above are partial. Try searching a more specific \
             path or pattern."
        )
        .as_bytes(),
    );
    output.exit_code = -1;
    output
}

/// Terminal card when `rg` could not be spawned.
pub(crate) fn spawn_failure_output(error: &io::Error) -> GrepSearchOutput {
    GrepSearchOutput {
        stdout: Vec::new(),
        stderr: format!("Error calling tool: {}", error).into_bytes(),
        exit_code: -1,
        match_count: 0,
        file_matches: Vec::new(),
    }
}

/// Build the authoritative terminal card from the fully-read rg output.
/// Single source of truth; the streamed body is a faithful prefix of it.
pub(crate) fn finalize_grep(
    stdout_buf: Vec<u8>,
    stdout_truncated: bool,
    stderr_buf: Vec<u8>,
    exit_code: i32,
    config: &GrepFormatConfig,
) -> GrepSearchOutput {
    let stdout = String::from_utf8_lossy(&stdout_buf);
    let stderr = String::from_utf8_lossy(&stderr_buf);

    // Handle exit codes.
    if (exit_code == 1 && stdout.is_empty())
        || (exit_code == 2 && stderr.contains("No files were searched"))
    {
        let result = format!(
            "<workspace_result workspace_path=\"{}\">\nNo matches found\n</workspace_result>",
            config.cwd_display
        );
        return GrepSearchOutput {
            stdout: result.into_bytes(),
            stderr: Vec::new(),
            exit_code,
            match_count: 0,
            file_matches: Vec::new(),
        };
    }

    if exit_code == 2 {
        let error_msg = format!(
            "Error calling tool: {} (exit 2, root: {})",
            stderr, config.cwd_display
        );
        return GrepSearchOutput {
            stdout: error_msg.into_bytes(),
            stderr: stderr_buf,
            exit_code,
            match_count: 0,
            file_matches: Vec::new(),
        };
    }

    if exit_code != 0 {
        let error_msg = format!(
            "Error calling tool: unknown error (exit {}, root: {})",
            exit_code, config.cwd_display
        );
        return GrepSearchOutput {
            stdout: error_msg.into_bytes(),
            stderr: stderr_buf,
            exit_code,
            match_count: 0,
            file_matches: Vec::new(),
        };
    }

    let (formatted_output, match_count, file_matches) = {
        let mut output_lines: Vec<String> = stdout.lines().map(|s| s.to_string()).collect();
        let mut is_truncated = stdout_truncated;
        if output_lines.len() > config.effective_head_limit {
            is_truncated = true;
            output_lines.truncate(config.effective_head_limit);
        }

        let file_matches = if matches!(config.output_mode, OutputMode::Content) {
            parse_file_matches(&output_lines, config.max_chars_per_line)
        } else {
            Vec::new()
        };

        let match_count_value = match config.output_mode {
            OutputMode::Content => count_matches(&output_lines),
            OutputMode::FilesWithMatches => output_lines.len(),
            OutputMode::Count => {
                let mut sum_matches = 0usize;
                for line in &output_lines {
                    if let Some(count_str) = line.split(':').next_back()
                        && let Ok(count) = count_str.parse::<usize>()
                    {
                        sum_matches += count;
                    }
                }
                sum_matches
            }
        };

        let formatted = match config.output_mode {
            OutputMode::Content => format_content_output(
                output_lines,
                is_truncated,
                config.max_chars_per_line,
                config.max_output_bytes,
            ),
            OutputMode::FilesWithMatches => format_files_with_matches_output(
                output_lines,
                is_truncated,
                config.max_chars_per_line,
                config.max_output_bytes,
            ),
            OutputMode::Count => format_count_output(
                output_lines,
                is_truncated,
                config.max_chars_per_line,
                config.max_output_bytes,
            ),
        };
        (formatted, match_count_value, file_matches)
    };

    GrepSearchOutput {
        stdout: format!(
            "<workspace_result workspace_path=\"{}\">\n{}\n</workspace_result>",
            config.cwd_display, formatted_output
        )
        .into_bytes(),
        stderr: stderr_buf,
        exit_code,
        match_count,
        file_matches,
    }
}

pub(crate) fn trim_line(line: &str, max_chars_per_line: usize) -> String {
    truncate_line(line, max_chars_per_line).into_owned()
}

/// Parse a ripgrep "numbered line" prefix: `123:content` or `45-context`. `pub` so siblings can
/// reuse the parser instead of duplicating it -- avoids drift between the two namespaces' rg-output
/// reformatters.
pub fn parse_numbered_line_prefix(line: &str) -> Option<(usize, char, &str)> {
    let bytes = line.as_bytes();
    let mut idx = 0usize;
    while bytes.get(idx).is_some_and(|b| b.is_ascii_digit()) {
        idx += 1;
    }
    if idx == 0 || idx >= bytes.len() {
        return None;
    }

    let sep = *bytes.get(idx)? as char;
    if sep != ':' && sep != '-' {
        return None;
    }

    let line_number = line.get(..idx)?.parse::<usize>().ok()?;
    Some((line_number, sep, line.get(idx + 1..)?))
}

/// Parse ripgrep `--heading` output into structured per-file matches.
pub fn parse_file_matches(
    output_lines: &[String],
    max_chars_per_line: usize,
) -> Vec<GrepFileMatch> {
    let mut file_matches: Vec<GrepFileMatch> = Vec::new();
    let mut current_file: Option<GrepFileMatch> = None;

    let mut flush_current = |current: &mut Option<GrepFileMatch>| {
        if let Some(file) = current.take()
            && !file.matches.is_empty()
        {
            file_matches.push(file);
        }
    };

    for line in output_lines {
        let stripped = line.trim();
        if stripped.is_empty() {
            flush_current(&mut current_file);
            continue;
        }
        if stripped == "--" {
            continue;
        }

        if current_file.is_none() {
            current_file = Some(GrepFileMatch {
                path: stripped.to_owned(),
                matches: Vec::new(),
            });
            continue;
        }

        if let Some((line_number, sep, rest)) = parse_numbered_line_prefix(line) {
            if sep == ':'
                && let Some(ref mut file) = current_file
            {
                file.matches.push(GrepLineMatch {
                    line_number,
                    content: trim_line(rest, max_chars_per_line),
                });
            }
            continue;
        }

        flush_current(&mut current_file);
        current_file = Some(GrepFileMatch {
            path: stripped.to_owned(),
            matches: Vec::new(),
        });
    }

    flush_current(&mut current_file);
    file_matches
}

pub fn count_matches(output_lines: &[String]) -> usize {
    output_lines
        .iter()
        .filter(|line| parse_numbered_line_prefix(line).is_some_and(|(_, sep, _)| sep == ':'))
        .count()
}

/// Cumulative byte-cap check shared by the batch path and `BodyStreamer`.
pub(crate) fn exceeds_cum_byte_cap(
    cum_len: usize,
    line_len: usize,
    max_output_bytes: usize,
) -> bool {
    cum_len + line_len > max_output_bytes
}

fn first_idx_exceed_cum_limit(lines: &[String], limit: usize) -> usize {
    let mut cum_len = 0;
    for (i, line) in lines.iter().enumerate() {
        if exceeds_cum_byte_cap(cum_len, line.len(), limit) {
            return i;
        }
        cum_len += line.len();
    }
    lines.len()
}

pub fn format_content_output(
    output_lines: Vec<String>,
    is_truncated: bool,
    max_chars_per_line: usize,
    max_output_bytes: usize,
) -> String {
    let is_truncated_str = if is_truncated { "at least " } else { "" };
    let num_matching_lines = count_matches(&output_lines);
    let mut final_output_lines = vec![format!(
        "Found {}{} matching lines",
        is_truncated_str, num_matching_lines
    )];

    let trimmed_lines: Vec<String> = output_lines
        .iter()
        .map(|line| trim_line(line, max_chars_per_line))
        .collect();

    let cut_idx = first_idx_exceed_cum_limit(&trimmed_lines, max_output_bytes);
    if let Some(kept) = trimmed_lines.get(..cut_idx) {
        final_output_lines.extend_from_slice(kept);
    }

    let remaining_matches = count_matches(trimmed_lines.get(cut_idx..).unwrap_or(&[]));
    if remaining_matches > 0 {
        final_output_lines.push(format!(
            "... [{}{} lines truncated] ...",
            is_truncated_str, remaining_matches
        ));
    }

    final_output_lines.join("\n")
}

pub fn format_files_with_matches_output(
    output_lines: Vec<String>,
    is_truncated: bool,
    max_chars_per_line: usize,
    max_output_bytes: usize,
) -> String {
    let is_truncated_str = if is_truncated { "at least " } else { "" };
    let mut final_output_lines = vec![format!(
        "Found {}{} files",
        is_truncated_str,
        output_lines.len()
    )];

    let trimmed_lines: Vec<String> = output_lines
        .iter()
        .map(|line| trim_line(line, max_chars_per_line))
        .collect();

    let cut_idx = first_idx_exceed_cum_limit(&trimmed_lines, max_output_bytes);
    if let Some(kept) = trimmed_lines.get(..cut_idx) {
        final_output_lines.extend_from_slice(kept);
    }

    if output_lines.len() > cut_idx {
        final_output_lines.push(format!(
            "... [{}{} lines truncated] ...",
            is_truncated_str,
            output_lines.len() - cut_idx
        ));
    }

    final_output_lines.join("\n")
}

pub fn format_count_output(
    output_lines: Vec<String>,
    is_truncated: bool,
    max_chars_per_line: usize,
    max_output_bytes: usize,
) -> String {
    let is_truncated_str = if is_truncated { "at least " } else { "" };

    let mut sum_matches = 0;
    for line in &output_lines {
        if let Some(count_str) = line.split(':').next_back()
            && let Ok(count) = count_str.parse::<usize>()
        {
            sum_matches += count;
        }
    }

    let mut final_output_lines = vec![format!(
        "Found {} across {}{} files",
        sum_matches,
        is_truncated_str,
        output_lines.len()
    )];

    let trimmed_lines: Vec<String> = output_lines
        .iter()
        .map(|line| trim_line(line, max_chars_per_line))
        .collect();

    let cut_idx = first_idx_exceed_cum_limit(&trimmed_lines, max_output_bytes);
    if let Some(kept) = trimmed_lines.get(..cut_idx) {
        final_output_lines.extend_from_slice(kept);
    }

    if output_lines.len() > cut_idx {
        final_output_lines.push(format!(
            "... [{}{} lines truncated] ...",
            is_truncated_str,
            output_lines.len() - cut_idx
        ));
    }

    final_output_lines.join("\n")
}

#[cfg(test)]
#[path = "card_tests.rs"]
mod tests;
