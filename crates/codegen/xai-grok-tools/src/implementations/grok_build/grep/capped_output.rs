//! grep's search output under the tool's byte and line budgets.
//!
//! Every source of `rg --heading` bytes goes through [`CappedOutput::accept`], so
//! the budgets cannot drift between the blocking and the streaming paths. The
//! line budget is `effective_head_limit + 1`: one line past the head limit tells
//! an overflow from an exact fit apart, matching `finalize_grep`'s `> limit` check.
//! When streaming, each accepted line is also projected by [`BodyStreamer`], so
//! the streamed deltas stay a prefix of the card body built from the same bytes.

use crate::implementations::grok_build::grep::card::{
    GrepFormatConfig, exceeds_cum_byte_cap, trim_line,
};

/// Hard cap on bytes read from ripgrep's stdout (5 MB).
pub(crate) const MAX_STDOUT_BYTES: usize = 5_000_000;

/// Budget state after [`CappedOutput::accept`].
pub(crate) enum Fill {
    /// The whole chunk fit and no budget is full.
    Open,
    /// A budget filled with bytes of the chunk left over, or the streamed body is
    /// complete: the output is cut.
    Overflowed,
    /// A budget filled on the chunk's last byte. Only the source knows whether
    /// more output follows.
    FullAtChunkEnd,
}

/// What [`CappedOutput::accept`] did with a chunk.
pub(crate) struct Accepted {
    /// Streamed body deltas for the lines the chunk completed.
    pub(crate) deltas: Vec<xai_tool_runtime::ToolProgress>,
    pub(crate) fill: Fill,
}

/// Accepted search output bytes, their complete-line count, and the optional
/// streamed body projection.
pub(crate) struct CappedOutput<'a> {
    bytes: Vec<u8>,
    complete_lines: usize,
    max_lines: usize,
    streamer: Option<BodyStreamer<'a>>,
}

impl<'a> CappedOutput<'a> {
    pub(crate) fn new(config: &GrepFormatConfig) -> CappedOutput<'a> {
        CappedOutput {
            bytes: Vec::with_capacity(MAX_STDOUT_BYTES.min(65_536)),
            complete_lines: 0,
            max_lines: config.effective_head_limit.saturating_add(1),
            streamer: None,
        }
    }

    /// Like [`CappedOutput::new`], and projects accepted lines into streamed body deltas.
    pub(crate) fn streaming(
        spec: &'a xai_tool_protocol::StreamingSpec,
        config: &'a GrepFormatConfig,
    ) -> CappedOutput<'a> {
        CappedOutput {
            streamer: Some(BodyStreamer::new(spec, config)),
            ..CappedOutput::new(config)
        }
    }

    /// Accept the leading part of `chunk` that fits the budgets and project it
    /// into the streamed body.
    pub(crate) fn accept(&mut self, chunk: &[u8]) -> Accepted {
        let (accepted, hit_cap) =
            accept_rg_stdout_chunk(chunk, self.bytes.len(), self.complete_lines, self.max_lines);
        let Some(taken) = chunk.get(..accepted) else {
            return Accepted {
                deltas: Vec::new(),
                fill: Fill::Overflowed,
            };
        };
        self.complete_lines += taken.iter().filter(|&&b| b == b'\n').count();
        self.bytes.extend_from_slice(taken);
        let deltas = self
            .streamer
            .as_mut()
            .map(|streamer| streamer.feed(taken))
            .unwrap_or_default();
        // The formatted body can hit its own head/byte budget before the raw line
        // count does (a small `max_output_bytes`).
        let is_body_done = self.streamer.as_ref().is_some_and(|streamer| streamer.done);
        let fill = if is_body_done || (hit_cap && accepted < chunk.len()) {
            Fill::Overflowed
        } else if hit_cap {
            Fill::FullAtChunkEnd
        } else {
            Fill::Open
        };
        Accepted { deltas, fill }
    }

    /// Flush the streamed body's final segment (see [`BodyStreamer::finish`]).
    pub(crate) fn finish(&mut self) -> Option<xai_tool_runtime::ToolProgress> {
        self.streamer.as_mut().and_then(BodyStreamer::finish)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    pub(crate) fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

/// Longest prefix of `bytes` that ends on a UTF-8 character boundary. Used when a hard *byte* budget would otherwise
/// cut mid-code-unit; line-budget stops already land on `\n` (ASCII), so they are always boundaries. Counting lines by
/// `b'\n'` is UTF-8-safe (newlines are never multi-byte).
fn utf8_char_boundary_prefix_len(bytes: &[u8]) -> usize {
    match std::str::from_utf8(bytes) {
        Ok(_) => bytes.len(),
        Err(e) => e.valid_up_to(),
    }
}

/// How many leading bytes of a newly-read `rg` chunk to accept, given the running byte/line budgets. Returns `(accepted_len, hit_cap)`. Stops
/// at the first of: remaining room under [`MAX_STDOUT_BYTES`], or the newline that brings complete line count to `max_lines`.
fn accept_rg_stdout_chunk(
    chunk: &[u8],
    buf_len: usize,
    complete_lines: usize,
    max_lines: usize,
) -> (usize, bool) {
    if chunk.is_empty() {
        return (0, false);
    }
    if complete_lines >= max_lines || buf_len >= MAX_STDOUT_BYTES {
        return (0, true);
    }

    let byte_room = MAX_STDOUT_BYTES - buf_len;
    let Some(limited) = chunk.get(..chunk.len().min(byte_room)) else {
        return (0, false);
    };
    let mut lines = complete_lines;
    for (i, &b) in limited.iter().enumerate() {
        if b == b'\n' {
            lines += 1;
            if lines >= max_lines {
                // Include the newline that filled the budget, then stop.
                // `\n` is a single-byte ASCII boundary — no UTF-8 snap needed.
                return (i + 1, true);
            }
        }
    }
    let hit_byte_cap = limited.len() < chunk.len();
    if hit_byte_cap {
        // Prefer a complete UTF-8 prefix over a mid-code-unit cut. If the entire
        // limited slice is an incomplete sequence (shouldn't happen when the
        // prior buffer always ends on a boundary), accept 0 and hit the cap.
        let safe = utf8_char_boundary_prefix_len(limited);
        return (safe, true);
    }
    (limited.len(), false)
}

/// Incremental builder for grep's streamed card body: raw stdout in via [`BodyStreamer::feed`], flushed at EOF via [`BodyStreamer::finish`].
/// Each line is projected exactly as `finalize_grep` projects the terminal body, so the concatenated deltas equal the card body (prefix
/// mode). Line splitting matches `str::lines()` exactly (incl. trailing-`\r` handling).
struct BodyStreamer<'a> {
    spec: &'a xai_tool_protocol::StreamingSpec,
    config: &'a GrepFormatConfig,
    /// Accumulated card body. Equals the body `finalize_grep` produces.
    body: String,
    /// Monotonic body bytes already surfaced as deltas.
    last_total: u64,
    /// Body lines emitted so far (drives the head-limit).
    emitted_lines: usize,
    /// Cumulative trimmed-line length (drives the byte-cap).
    cum_len: usize,
    /// Set once the head-limit or byte-cap is hit (body complete).
    done: bool,
    /// Bytes after the last newline — the in-progress line, carried across feeds.
    pending: Vec<u8>,
}

impl<'a> BodyStreamer<'a> {
    fn new(spec: &'a xai_tool_protocol::StreamingSpec, config: &'a GrepFormatConfig) -> Self {
        Self {
            spec,
            config,
            body: String::new(),
            last_total: 0,
            emitted_lines: 0,
            cum_len: 0,
            done: false,
            pending: Vec::new(),
        }
    }

    /// Feed raw stdout; returns a delta per newly completed line. Partial
    /// trailing line is buffered. No-op once [`Self::done`].
    fn feed(&mut self, bytes: &[u8]) -> Vec<xai_tool_runtime::ToolProgress> {
        let mut deltas = Vec::new();
        if self.done {
            return deltas;
        }
        self.pending.extend_from_slice(bytes);
        // Own the buffer to project lines from borrowed slices (no per-line
        // alloc); the unconsumed tail is carried forward at the end.
        let buf = std::mem::take(&mut self.pending);
        let mut start = 0;
        while let Some(rel) = buf
            .get(start..)
            .and_then(|tail| tail.iter().position(|&b| b == b'\n'))
        {
            let nl = start + rel;
            let mut end = nl; // exclusive; drops the '\n'
            if end > start && end.checked_sub(1).and_then(|i| buf.get(i).copied()) == Some(b'\r') {
                end -= 1; // drop the '\r' of a '\r\n' (matches `str::lines()`)
            }
            if let Some(line) = buf.get(start..end)
                && let Some(p) = self.push_line(line)
            {
                deltas.push(p);
            }
            start = nl + 1;
            if self.done {
                break;
            }
        }
        // Carry the in-progress (post-last-newline) bytes to the next feed.
        if let Some(tail) = buf.get(start..) {
            self.pending.extend_from_slice(tail);
        }
        deltas
    }

    /// Flush the final non-`\n`-terminated segment verbatim at EOF (matches
    /// `str::lines()`, which keeps a trailing `\r`).
    fn finish(&mut self) -> Option<xai_tool_runtime::ToolProgress> {
        if self.done || self.pending.is_empty() {
            return None;
        }
        let line = std::mem::take(&mut self.pending);
        self.push_line(&line)
    }

    /// Project one line into the body; returns its delta. Sets [`Self::done`]
    /// at the head-limit or byte-cap.
    fn push_line(&mut self, line: &[u8]) -> Option<xai_tool_runtime::ToolProgress> {
        // Head-limit (matches `finalize_grep`).
        if self.emitted_lines >= self.config.effective_head_limit {
            self.done = true;
            return None;
        }
        let line_str = String::from_utf8_lossy(line);
        let trimmed = trim_line(&line_str, self.config.max_chars_per_line);
        // Byte-cap; shares `exceeds_cum_byte_cap` with the batch path.
        if exceeds_cum_byte_cap(self.cum_len, trimmed.len(), self.config.max_output_bytes) {
            self.done = true;
            return None;
        }
        // Separator keyed off `emitted_lines` so a leading empty line still
        // gets one.
        if self.emitted_lines > 0 {
            self.body.push('\n');
        }
        self.body.push_str(&trimmed);
        self.cum_len += trimmed.len();
        self.emitted_lines += 1;
        xai_tool_runtime::stream_chunk(
            self.spec,
            self.body.as_bytes(),
            self.body.len() as u64,
            &mut self.last_total,
            // No upstream cumulative truncation; only the per-tick `gap`.
            false,
        )
    }
}

#[cfg(test)]
#[path = "capped_output_tests.rs"]
mod tests;
