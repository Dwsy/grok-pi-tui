use super::*;
use crate::DEFAULT_TOOL_OUTPUT_BYTES;
use crate::implementations::grok_build::grep::card::finalize_grep;
use crate::implementations::grok_build::grep::tests::read_grep_delta;
use crate::implementations::grok_build::grep::{
    CONTENT_LINE_DEFAULT, DEFAULT_MAX_CHARS_PER_LINE, FILE_COUNT_DEFAULT, GREP_CAPABILITIES,
    OutputMode,
};

/// Drive a `BodyStreamer` over `raw` (one synthetic read + flush) and return the concatenation of every emitted delta —
/// i.e. the streamed card body. Feeding the whole buffer at once is equivalent to chunked feeds (the pending buffer
/// stitches partial lines), so this exercises the same projection `execute` runs incrementally.
fn stream_body(raw: &[u8], config: &GrepFormatConfig) -> String {
    let spec = GREP_CAPABILITIES.streaming.as_ref().unwrap();
    let mut streamer = BodyStreamer::new(spec, config);
    let mut body = String::new();
    for p in streamer.feed(raw) {
        body.push_str(&read_grep_delta(&p));
    }
    if let Some(p) = streamer.finish() {
        body.push_str(&read_grep_delta(&p));
    }
    body
}

/// Extract the card *body* from a `finalize_grep` card by string slicing (NOT `str::lines()`, which would strip a trailing `\r` off a body line
/// and thereby hide the very divergence these tests guard). Drops the `<workspace_result>` wrapper, the "Found …" summary (first line), and an
/// optional `... [N lines truncated] ...` footer (last line).
fn card_body(card: &str) -> String {
    let nl = card.find('\n').expect("wrapper newline");
    let Some(after_open) = card.get(nl + 1..) else {
        panic!("wrapper newline not a char boundary");
    };
    let formatted = after_open
        .strip_suffix("\n</workspace_result>")
        .expect("wrapper close");
    // Drop the summary (first line).
    let body_and_footer = formatted.split_once('\n').map_or("", |(_, rest)| rest);
    // Drop an optional trailing footer line.
    if let Some((body, last)) = body_and_footer.rsplit_once('\n')
        && last.starts_with("... [")
        && last.ends_with("] ...")
    {
        return body.to_string();
    }
    body_and_footer.to_string()
}

fn grep_config(
    output_mode: OutputMode,
    max_output_bytes: usize,
    effective_head_limit: Option<usize>,
) -> GrepFormatConfig {
    let default_budget = match output_mode {
        OutputMode::Content => CONTENT_LINE_DEFAULT,
        OutputMode::FilesWithMatches | OutputMode::Count => FILE_COUNT_DEFAULT,
    };
    GrepFormatConfig {
        output_mode,
        // Tests pass `None` for "use the production default budget".
        effective_head_limit: effective_head_limit.unwrap_or(default_budget),
        max_chars_per_line: DEFAULT_MAX_CHARS_PER_LINE,
        max_output_bytes,
        cwd_display: "/ws".to_string(),
    }
}

#[test]
fn accept_rg_stdout_chunk_stops_at_line_budget() {
    let chunk = b"a\nb\nc\nd\n";
    let (n, hit) = accept_rg_stdout_chunk(chunk, 0, 0, 2);
    assert!(hit);
    assert_eq!(chunk.get(..n), Some(b"a\nb\n".as_slice()));
}

#[test]
fn accept_rg_stdout_chunk_stops_at_byte_budget() {
    // MAX_STDOUT_BYTES is huge; simulate an already-full buffer.
    let chunk = b"more\n";
    let (n, hit) = accept_rg_stdout_chunk(chunk, MAX_STDOUT_BYTES, 0, 100);
    assert_eq!(n, 0);
    assert!(hit);
}

/// Byte-cap must not cut mid multi-byte UTF-8 sequence (e.g. "é" = C3 A9).
#[test]
fn accept_rg_stdout_chunk_byte_cap_snaps_to_utf8_boundary() {
    // One byte of room left, but next char is 2-byte UTF-8.
    let chunk = "é\n".as_bytes(); // [0xC3, 0xA9, 0x0A]
    assert_eq!(chunk.len(), 3);
    let (n, hit) = accept_rg_stdout_chunk(chunk, MAX_STDOUT_BYTES - 1, 0, 100);
    assert!(hit, "must hit byte cap");
    assert_eq!(
        n, 0,
        "must not accept a leading incomplete UTF-8 byte; got {n}"
    );

    // Two bytes of room: full "é", drop the trailing newline for this test's
    // room — actually 2 bytes fits "é" exactly.
    let (n2, hit2) = accept_rg_stdout_chunk(chunk, MAX_STDOUT_BYTES - 2, 0, 100);
    assert!(hit2);
    assert_eq!(chunk.get(..n2), Some("é".as_bytes()));
}

#[test]
fn accept_rg_stdout_chunk_line_budget_includes_ascii_newline_boundary() {
    // Multi-byte content + newline: line stop lands on `\n` (safe boundary).
    let chunk = "café\nmore\n".as_bytes();
    let (n, hit) = accept_rg_stdout_chunk(chunk, 0, 0, 1);
    assert!(hit);
    assert_eq!(chunk.get(..n), Some("café\n".as_bytes()));
    assert!(
        chunk
            .get(..n)
            .is_some_and(|b| std::str::from_utf8(b).is_ok())
    );
}

/// Regression: a stdout truncation landing mid-CRLF leaves a final segment with no trailing `\n` that ends in `\r`. `str::lines()` (used by
/// `finalize_grep`) keeps that `\r`, so the streamed body must too — the final flush must NOT strip it. (Without the fix the streamed body
/// would drop the `\r` and diverge from the terminal card body by one byte.)
#[test]
fn body_streamer_keeps_final_crlf_segment_like_str_lines() {
    // Last segment "3:gamma\r" has no trailing '\n' (truncated mid-CRLF).
    let raw = b"src/a.rs\n1:alpha\r\n2:beta\r\n3:gamma\r";
    let config = grep_config(OutputMode::Content, DEFAULT_TOOL_OUTPUT_BYTES, None);

    let streamed = stream_body(raw, &config);
    let card =
        String::from_utf8_lossy(&finalize_grep(raw.to_vec(), false, Vec::new(), 0, &config).stdout)
            .into_owned();

    assert_eq!(
        streamed,
        card_body(&card),
        "streamed body must byte-match the terminal card body on a CRLF final segment"
    );
    assert!(
        streamed.ends_with("3:gamma\r"),
        "the final `\\r` must be preserved (matches str::lines()): {streamed:?}"
    );
}

/// Byte-cap truncation appends a `... [N lines truncated] ...`
/// footer. The streamed body equals `trimmed_lines[..cut_idx]` — excluding
/// BOTH the summary and the footer (both terminal-only).
#[test]
fn body_streamer_matches_card_body_with_bytecap_footer() {
    let raw = b"f.txt\n1:aaaaaaaaaa\n2:bbbbbbbbbb\n3:cccccccccc\n4:dddddddddd\n";
    // Small cap so the cut lands mid-output and a footer is appended.
    let config = grep_config(OutputMode::Content, 30, None);

    let streamed = stream_body(raw, &config);
    let card =
        String::from_utf8_lossy(&finalize_grep(raw.to_vec(), false, Vec::new(), 0, &config).stdout)
            .into_owned();

    assert!(
        card.contains("lines truncated]"),
        "expected a truncation footer in the card: {card}"
    );
    assert_eq!(
        streamed,
        card_body(&card),
        "streamed body excludes the terminal-only footer"
    );
    assert!(
        !streamed.contains("truncated"),
        "footer must be terminal-only: {streamed:?}"
    );
}

/// The body projection is mode-independent — `files_with_matches`
/// and `count` bodies (which carry a different summary) also byte-match.
#[test]
fn body_streamer_matches_card_body_count_and_files_modes() {
    let files_raw = b"src/a.rs\nsrc/b.rs\nsrc/c.rs\n";
    let files_cfg = grep_config(
        OutputMode::FilesWithMatches,
        DEFAULT_TOOL_OUTPUT_BYTES,
        None,
    );
    let files_card = String::from_utf8_lossy(
        &finalize_grep(files_raw.to_vec(), false, Vec::new(), 0, &files_cfg).stdout,
    )
    .into_owned();
    assert!(files_card.contains("Found 3 files"), "card: {files_card}");
    assert_eq!(stream_body(files_raw, &files_cfg), card_body(&files_card));

    let count_raw = b"src/a.rs:3\nsrc/b.rs:2\n";
    let count_cfg = grep_config(OutputMode::Count, DEFAULT_TOOL_OUTPUT_BYTES, None);
    let count_card = String::from_utf8_lossy(
        &finalize_grep(count_raw.to_vec(), false, Vec::new(), 0, &count_cfg).stdout,
    )
    .into_owned();
    assert!(
        count_card.contains("Found 5 across 2 files"),
        "card: {count_card}"
    );
    assert_eq!(stream_body(count_raw, &count_cfg), card_body(&count_card));
}
