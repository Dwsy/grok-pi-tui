use std::fs::File;
use std::io::{BufRead as _, BufReader};
use std::path::Path;

use crate::batch_dream::Result;
use crate::batch_dream_control::BatchDreamControl;

pub(crate) const MAX_LINE_PREFIX_BYTES: usize = 4 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Section {
    pub heading: String,
    pub level: u8,
    pub start: u64,
    pub end: u64,
}

pub(crate) struct Line<'a> {
    pub(crate) number: u64,
    pub(crate) start: u64,
    pub(crate) len: u64,
    pub(crate) has_newline: bool,
    pub(crate) prefix: &'a [u8],
}

pub(crate) fn scan_lines(
    path: &Path,
    control: &BatchDreamControl,
    mut visit: impl FnMut(&Line<'_>) -> Result<bool>,
) -> Result<()> {
    let mut reader = BufReader::with_capacity(64 * 1024, File::open(path)?);
    let mut prefix = Vec::with_capacity(MAX_LINE_PREFIX_BYTES);
    let mut start = 0u64;
    let mut number = 1u64;
    loop {
        control.check()?;
        prefix.clear();
        let mut len = 0u64;
        let mut has_newline = false;
        loop {
            let buffer = reader.fill_buf()?;
            if buffer.is_empty() {
                break;
            }
            let (chunk, is_line_end) = match buffer.iter().position(|byte| *byte == b'\n') {
                Some(index) => (buffer.get(..=index).unwrap_or(buffer), true),
                None => (buffer, false),
            };
            let room = MAX_LINE_PREFIX_BYTES.saturating_sub(prefix.len());
            prefix.extend_from_slice(chunk.get(..room.min(chunk.len())).unwrap_or_default());
            let consumed = chunk.len();
            len += consumed as u64;
            reader.consume(consumed);
            if is_line_end {
                has_newline = true;
                break;
            }
        }
        if len == 0 {
            return Ok(());
        }
        let line = Line {
            number,
            start,
            len,
            has_newline,
            prefix: &prefix,
        };
        if !visit(&line)? {
            return Ok(());
        }
        start += len;
        number += 1;
    }
}

pub(crate) fn heading_level(line: &[u8]) -> Option<u8> {
    let trimmed = line
        .strip_prefix(b"   ")
        .or_else(|| line.strip_prefix(b"  "));
    let trimmed = trimmed.or_else(|| line.strip_prefix(b" ")).unwrap_or(line);
    let level = trimmed.iter().take_while(|byte| **byte == b'#').count();
    let rest = trimmed.get(level..)?;
    let is_heading = (1..=6).contains(&level)
        && rest
            .first()
            .is_none_or(|byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\r'));
    is_heading.then(|| u8::try_from(level).unwrap_or(6))
}

pub(crate) fn is_fence(line: &[u8]) -> bool {
    let trimmed = line.trim_ascii_start();
    trimmed.starts_with(b"```") || trimmed.starts_with(b"~~~")
}

pub(crate) fn outline(
    path: &Path,
    control: &BatchDreamControl,
    max_sections: usize,
) -> Result<(Vec<Section>, bool)> {
    let mut sections: Vec<Section> = Vec::new();
    let mut is_fenced = false;
    let mut is_truncated = false;
    let mut size = 0u64;
    let mut open: Vec<usize> = Vec::new();
    scan_lines(path, control, |line| {
        size = line.start + line.len;
        if is_fence(line.prefix) {
            is_fenced = !is_fenced;
            return Ok(true);
        }
        if is_fenced {
            return Ok(true);
        }
        if let Some(level) = heading_level(line.prefix) {
            while let Some(section) = open.last().and_then(|index| sections.get_mut(*index)) {
                if section.level < level {
                    break;
                }
                section.end = line.start;
                open.pop();
            }
            if sections.len() == max_sections {
                is_truncated = true;
                return Ok(true);
            }
            let heading = String::from_utf8_lossy(line.prefix)
                .trim_end_matches(['\n', '\r'])
                .to_owned();
            open.push(sections.len());
            sections.push(Section {
                heading,
                level,
                start: line.start,
                end: 0,
            });
        }
        Ok(true)
    })?;
    for index in open {
        if let Some(section) = sections.get_mut(index) {
            section.end = size;
        }
    }
    Ok((sections, is_truncated))
}

/// Must match the manifest's description rule: the first non-empty line that is not a heading, comment, or quote.
pub(crate) fn description_range(
    path: &Path,
    control: &BatchDreamControl,
) -> Result<std::result::Result<(u64, u64), u64>> {
    let mut found = None;
    let mut after_title = 0u64;
    let mut has_title = false;
    scan_lines(path, control, |line| {
        let text = line.prefix.trim_ascii();
        if text.is_empty() {
            return Ok(true);
        }
        if text.starts_with(b"#") {
            if !has_title {
                has_title = true;
                after_title = line.start + line.len;
            }
            return Ok(true);
        }
        if text.starts_with(b"<!--") || text.starts_with(b">") {
            return Ok(true);
        }
        let mut end = line.start + line.len - u64::from(line.has_newline);
        let is_whole = line.prefix.len() as u64 == line.len;
        if is_whole && line.prefix.ends_with(b"\r\n") {
            end -= 1;
        }
        found = Some((line.start, end));
        Ok(false)
    })?;
    Ok(found.ok_or(after_title))
}

#[cfg(test)]
#[path = "batch_dream_outline_tests.rs"]
mod tests;
