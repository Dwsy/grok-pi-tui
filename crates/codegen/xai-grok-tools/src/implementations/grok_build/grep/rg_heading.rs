//! Renders served content search hits as the bytes `rg` prints.
//!
//! [`render_rg_heading`] reproduces the stdout of `rg` 15 with the grep tool's
//! flags (`--heading --with-filename --line-number --color=never --max-columns N
//! --max-columns-preview`, plus `-l`, `-c` or context lines), so served hits go
//! through the same budgets, streaming and card code as `rg` output. The golden
//! files under `tests/rg-15.0.0-heading/` pin the bytes.
//!
//! Known difference, outside what a served search reaches in practice: a last
//! line of exactly `max_columns` bytes without a trailing `\n` is cut where `rg`
//! prints it whole (the hits do not say whether a line ends with `\n`). The count
//! mode counts match lines, which is what `rg -c` prints without `-U`; the tool
//! never offers a multiline count.

use bstr::ByteSlice;

use crate::computer::content_search::{
    ContentSearch, ContentSearchMode, ContentSearchRequest, HitKind,
};
use crate::implementations::grok_build::grep::capped_output::MAX_STDOUT_BYTES;

const OMITTED_LONG_LINE: &[u8] = b" [... omitted end of long line]";

/// The bytes `rg` prints for `search`, up to the first line that ends past
/// [`MAX_STDOUT_BYTES`]: `CappedOutput` drops everything after that byte, so the
/// render stays bounded however many hits a backend returns. `None` when a cut
/// line's text before that point does not reach past the part `rg` would print.
pub(crate) fn render_rg_heading(
    search: &ContentSearch,
    request: &ContentSearchRequest,
) -> Option<Vec<u8>> {
    let has_context = request.context_before > 0 || request.context_after > 0;
    let max_columns = usize::try_from(request.max_columns).unwrap_or(usize::MAX);
    let mut out = Vec::new();
    for file in &search.files {
        if out.len() > MAX_STDOUT_BYTES {
            break;
        }
        let path = file.path.as_os_str().as_encoded_bytes();
        match request.mode {
            ContentSearchMode::FilesWithMatches => {
                out.extend_from_slice(path);
                out.push(b'\n');
            }
            ContentSearchMode::Count => {
                let count = file
                    .lines
                    .iter()
                    .filter(|line| line.kind == HitKind::Match)
                    .count();
                if count > 0 {
                    out.extend_from_slice(path);
                    out.extend_from_slice(format!(":{count}\n").as_bytes());
                }
            }
            ContentSearchMode::Content => {
                if file.lines.is_empty() {
                    continue;
                }
                // An empty line separates files; `--` only separates line groups
                // inside a file.
                if !out.is_empty() {
                    out.push(b'\n');
                }
                out.extend_from_slice(path);
                out.push(b'\n');
                let mut previous_line_no = None;
                for line in &file.lines {
                    if out.len() > MAX_STDOUT_BYTES {
                        break;
                    }
                    if has_context
                        && previous_line_no
                            .is_some_and(|previous| line.line_no.saturating_sub(previous) > 1)
                    {
                        out.extend_from_slice(b"--\n");
                    }
                    previous_line_no = Some(line.line_no);
                    let separator = match line.kind {
                        HitKind::Match => ':',
                        HitKind::Context => '-',
                    };
                    out.extend_from_slice(format!("{}{separator}", line.line_no).as_bytes());
                    // `rg` cuts a line longer than `max_columns` counting its `\n`.
                    if !line.is_cut && line.text.len() < max_columns {
                        out.extend_from_slice(&line.text);
                    } else {
                        // It keeps the first `max_columns` grapheme clusters (bstr's, each invalid
                        // UTF-8 subpart is one). A cut text may end inside a cluster, so the kept
                        // clusters must be followed by one more.
                        let mut ends = line.text.grapheme_indices().map(|(_, end, _)| end);
                        let kept = ends.by_ref().take(max_columns).last().unwrap_or(0);
                        if line.is_cut && ends.next().is_none() {
                            return None;
                        }
                        out.extend_from_slice(line.text.get(..kept)?);
                        out.extend_from_slice(OMITTED_LONG_LINE);
                    }
                    out.push(b'\n');
                }
            }
        }
    }
    Some(out)
}

#[cfg(test)]
#[path = "rg_heading_tests.rs"]
mod tests;
