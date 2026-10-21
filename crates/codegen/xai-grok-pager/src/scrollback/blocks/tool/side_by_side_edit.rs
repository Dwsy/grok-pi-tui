//! Optional side-by-side renderer for edit diffs.
//!
//! This module deliberately sits beside `edit.rs`: the upstream edit block
//! remains the source of truth for headers, selection, highlighting state, and
//! unified rendering. When the grok-pi-only F2 flag is enabled and enough width
//! is available, `EditToolCallBlock::render_diff_lines` delegates here.

use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use similar::{ChangeTag, TextDiff};
use syntect::easy::HighlightLines;

use super::edit::{
    DiffLineOutput, DiffLinePair, DiffRenderConfig, EditHighlightPhase, EditLineStyles,
    EditToolCallBlock, expand_tabs, map_spans_for_line, render_content_spans, wrap_text,
};
use crate::syntax::{Syntect, get_syntect};
use crate::theme::Theme;
use xai_grok_pager_diff::{DiffHunk, DiffLine};

const INDENT: &str = "  ";
const CONTENT_GAP: &str = "  ";
const SIDE_DIVIDER: &str = " │ ";
const CHANGE_MARKER_WIDTH: usize = 1;
const MIN_SIDE_CONTENT_WIDTH: usize = 8;

#[derive(Debug, Clone, Copy)]
struct SideBySideLayout {
    old_num_width: usize,
    new_num_width: usize,
    old_content_width: usize,
    new_content_width: usize,
}

#[derive(Debug, Clone)]
struct SideCellRow {
    line: Line<'static>,
    text: String,
    continuation: bool,
}

/// Byte ranges of changed words in tab-expanded old/new text.
/// Missing counterparts produce a full-content (but not padding) range.
fn changed_word_ranges(
    old: Option<&DiffLine>,
    new: Option<&DiffLine>,
) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    let visible =
        |line: &DiffLine| expand_tabs(line.text.trim_end_matches(['\r', '\n'])).into_owned();
    let old_text = old.map(visible).unwrap_or_default();
    let new_text = new.map(visible).unwrap_or_default();
    if old.is_none() {
        return (Vec::new(), vec![0..new_text.len()]);
    }
    if new.is_none() {
        return (vec![0..old_text.len()], Vec::new());
    }
    if old_text == new_text {
        return (Vec::new(), Vec::new());
    }
    // Bound the word diff cost for minified/generated one-line files.
    // Oversized changed lines still shade their content, never the padding.
    const MAX_WORD_DIFF_BYTES: usize = 8 * 1024;
    if old_text.len() > MAX_WORD_DIFF_BYTES || new_text.len() > MAX_WORD_DIFF_BYTES {
        return (vec![0..old_text.len()], vec![0..new_text.len()]);
    }
    let mut old_ranges = Vec::new();
    let mut new_ranges = Vec::new();
    let (mut old_pos, mut new_pos) = (0, 0);
    for change in TextDiff::from_words(old_text.as_str(), new_text.as_str()).iter_all_changes() {
        let len = change.value().len();
        match change.tag() {
            ChangeTag::Equal => {
                old_pos += len;
                new_pos += len;
            }
            ChangeTag::Delete => {
                old_ranges.push(old_pos..old_pos + len);
                old_pos += len;
            }
            ChangeTag::Insert => {
                new_ranges.push(new_pos..new_pos + len);
                new_pos += len;
            }
        }
    }
    (old_ranges, new_ranges)
}

/// Overlay diff backgrounds on syntax-colored spans without losing foregrounds.
/// Diff token boundaries and syntax spans are always UTF-8 boundaries.
fn mark_changed_spans(
    spans: Vec<Span<'static>>,
    changed: &[Range<usize>],
    background: Color,
) -> Vec<Span<'static>> {
    let mut result = Vec::new();
    let mut offset = 0;
    for span in spans {
        let content = span.content.as_ref();
        let end = offset + content.len();
        let mut cursor = 0;
        for range in changed {
            if range.end <= offset || range.start >= end {
                continue;
            }
            let start = range.start.saturating_sub(offset).max(cursor);
            let stop = (range.end - offset).min(content.len());
            if cursor < start {
                result.push(Span::styled(content[cursor..start].to_owned(), span.style));
            }
            if start < stop {
                result.push(Span::styled(
                    content[start..stop].to_owned(),
                    span.style.bg(background),
                ));
            }
            cursor = cursor.max(stop);
        }
        if cursor < content.len() {
            result.push(Span::styled(content[cursor..].to_owned(), span.style));
        }
        offset = end;
    }
    result
}

/// Retain foreground/background styles when the side cell soft-wraps.
fn spans_in_range(spans: &[Span<'static>], range: Range<usize>) -> Vec<Span<'static>> {
    let mut result = Vec::new();
    let mut offset = 0;
    for span in spans {
        let end = offset + span.content.len();
        if offset < range.end && end > range.start {
            let start = range.start.saturating_sub(offset);
            let stop = (range.end - offset).min(span.content.len());
            result.push(Span::styled(
                span.content[start..stop].to_owned(),
                span.style,
            ));
        }
        offset = end;
    }
    result
}

/// Render side-by-side only when the grok-pi F2 flag is enabled and the
/// available width can sustain two useful content columns. Returning `None`
/// is the caller's instruction to use the untouched unified renderer.
pub(super) fn render_if_enabled(
    edit: &EditToolCallBlock,
    theme: &Theme,
    width: u16,
    config: &DiffRenderConfig,
) -> Option<Vec<DiffLineOutput>> {
    if !crate::appearance::cache::load_side_by_side_edit() {
        return None;
    }
    // File creation has no old side; side-by-side would render an empty
    // left column. Keep the unified renderer for "Creating" blocks.
    if edit.prefix.starts_with("Creating") {
        return None;
    }
    render(edit, theme, width, config)
}

fn render(
    edit: &EditToolCallBlock,
    theme: &Theme,
    width: u16,
    config: &DiffRenderConfig,
) -> Option<Vec<DiffLineOutput>> {
    let layout = side_by_side_layout(&edit.hunks, width, config)?;
    let by_new_line = match &edit.highlight {
        EditHighlightPhase::FileScoped {
            by_new_line,
            theme: baked,
        } if *baked == crate::theme::cache::current_kind() => Some(by_new_line.as_ref()),
        _ => None,
    };
    Some(render_diff_hunks_side_by_side(
        &edit.hunks,
        Path::new(&edit.path),
        by_new_line,
        theme,
        config,
        layout,
        crate::appearance::cache::load_side_by_side_word_highlight(),
    ))
}

fn side_by_side_layout(
    hunks: &[DiffHunk],
    width: u16,
    config: &DiffRenderConfig,
) -> Option<SideBySideLayout> {
    let mut max_old = 1usize;
    let mut max_new = 1usize;
    let mut has_lines = false;
    for hunk in hunks {
        for line in hunk {
            has_lines = true;
            max_old = max_old.max(line.lo.max(1));
            max_new = max_new.max(line.ln.max(1));
        }
    }
    if !has_lines {
        return None;
    }

    let old_num_width = max_old.ilog10() as usize + 1;
    let new_num_width = max_new.ilog10() as usize + 1;
    let indent_width = if config.indent { INDENT.len() } else { 0 };
    let fixed = indent_width
        + CHANGE_MARKER_WIDTH
        + old_num_width
        + CONTENT_GAP.len()
        + CHANGE_MARKER_WIDTH
        + new_num_width
        + CONTENT_GAP.len()
        + unicode_width::UnicodeWidthStr::width(SIDE_DIVIDER);
    let available = (width as usize).saturating_sub(fixed);
    let old_content_width = available / 2;
    let new_content_width = available.saturating_sub(old_content_width);
    if old_content_width < MIN_SIDE_CONTENT_WIDTH || new_content_width < MIN_SIDE_CONTENT_WIDTH {
        return None;
    }

    Some(SideBySideLayout {
        old_num_width,
        new_num_width,
        old_content_width,
        new_content_width,
    })
}

/// Similar emits a replacement run as deletes followed by inserts. Pair the
/// whole run so unequal old/new counts still produce stable rows.
fn pair_hunk_lines(hunk: &DiffHunk) -> Vec<DiffLinePair> {
    let mut rows = Vec::new();
    let mut index = 0;
    while index < hunk.len() {
        if hunk[index].tag == ChangeTag::Equal {
            rows.push((Some(hunk[index].clone()), Some(hunk[index].clone())));
            index += 1;
            continue;
        }

        let start = index;
        while index < hunk.len() && hunk[index].tag != ChangeTag::Equal {
            index += 1;
        }
        let mut old = Vec::new();
        let mut new = Vec::new();
        for line in &hunk[start..index] {
            match line.tag {
                ChangeTag::Delete => old.push(line.clone()),
                ChangeTag::Insert => new.push(line.clone()),
                ChangeTag::Equal => unreachable!("equal line outside side-by-side run"),
            }
        }
        for row in 0..old.len().max(new.len()) {
            rows.push((old.get(row).cloned(), new.get(row).cloned()));
        }
    }
    rows
}

fn side_diff_background(tag: ChangeTag, theme: &Theme) -> Option<Color> {
    match tag {
        ChangeTag::Equal => None,
        ChangeTag::Delete => Some(theme.diff_delete_bg),
        ChangeTag::Insert => Some(theme.diff_insert_bg),
    }
}

/// Whether changed spans can be marked word-by-word under this theme.
///
/// Terminal-default themes carry `Reset` diff backgrounds and tint the whole
/// line foreground instead. Shading with `Reset` paints nothing, so word mode
/// has no signal there and must yield to the line renderer.
fn supports_word_highlight(theme: &Theme) -> bool {
    !theme.diff_uses_line_fg()
}

fn fit_side_content_line(
    line: Line<'static>,
    width: usize,
    background: Option<Color>,
) -> (Line<'static>, String) {
    let raw_text = line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    let line = crate::render::line_utils::fit_line_to_width(line, width);
    let mut spans = line.spans;
    if let Some(background) = background {
        for span in &mut spans {
            span.style = span.style.bg(background);
        }
    }
    let used = spans
        .iter()
        .map(|span| unicode_width::UnicodeWidthStr::width(span.content.as_ref()))
        .sum::<usize>();
    let text = if unicode_width::UnicodeWidthStr::width(raw_text.as_str()) <= width {
        raw_text
    } else {
        spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
    };
    if used < width {
        let padding = " ".repeat(width - used);
        let style = background
            .map(|color| Style::default().bg(color))
            .unwrap_or_default();
        spans.push(Span::styled(padding, style));
    }
    (Line::from(spans), text)
}

fn render_side_rows(
    line: Option<&DiffLine>,
    is_new_side: bool,
    highlighter: &mut Option<HighlightLines<'_>>,
    by_new_line: Option<&HashMap<usize, EditLineStyles>>,
    theme: &Theme,
    syntect: &Syntect,
    width: usize,
    word_highlight: bool,
    changed_words: &[Range<usize>],
) -> Vec<SideCellRow> {
    let Some(line) = line else {
        return vec![SideCellRow {
            line: Line::from(Span::raw(" ".repeat(width))),
            text: String::new(),
            continuation: false,
        }];
    };

    let trimmed = line.text.trim_end_matches(['\r', '\n']);
    let text = expand_tabs(trimmed);
    // Word mode keeps the unmodified text's syntax foregrounds even on themes
    // that would otherwise tint the entire inserted/deleted line.
    let paint_tag = if word_highlight {
        ChangeTag::Equal
    } else {
        line.tag
    };
    let mut content_spans = render_content_spans(&text, paint_tag, theme, highlighter, syntect);
    if is_new_side
        && let Some(map) = by_new_line
        && let Some(spans) = map_spans_for_line(line, &text, map, theme)
    {
        content_spans = spans;
    }
    if word_highlight && line.tag != ChangeTag::Equal {
        if let Some(color) = side_diff_background(line.tag, theme) {
            content_spans = mark_changed_spans(content_spans, changed_words, color);
        }
    }
    let raw_content = content_spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    let chunks = if unicode_width::UnicodeWidthStr::width(raw_content.as_str()) <= width {
        vec![Line::from(content_spans)]
    } else {
        let mut offset = 0;
        wrap_text(&raw_content, width)
            .into_iter()
            .map(|chunk| {
                let end = offset + chunk.len();
                let line = Line::from(spans_in_range(&content_spans, offset..end));
                offset = end;
                line
            })
            .collect()
    };
    let background = if word_highlight {
        None
    } else {
        side_diff_background(line.tag, theme)
    };
    chunks
        .into_iter()
        .enumerate()
        .map(|(index, chunk)| {
            let (line, text) = fit_side_content_line(chunk, width, background);
            SideCellRow {
                line,
                text,
                continuation: index > 0,
            }
        })
        .collect()
}

fn render_side_gutter(
    line: Option<&DiffLine>,
    continuation: bool,
    is_new_side: bool,
    num_width: usize,
    theme: &Theme,
    config: &DiffRenderConfig,
) -> Vec<Span<'static>> {
    let mut marker = " ";
    let mut marker_fg = theme.diff_gutter_fg;
    let mut number = " ".repeat(num_width);
    let mut fg = theme.diff_gutter_fg;
    let mut background = None;
    if !continuation && let Some(line) = line {
        let number_value = if is_new_side { line.ln } else { line.lo };
        let present = if is_new_side {
            line.tag != ChangeTag::Delete
        } else {
            line.tag != ChangeTag::Insert
        } && number_value > 0;
        if present {
            number = format!("{number_value:>num_width$}");
        }
        if line.tag != ChangeTag::Equal {
            let change_fg = if line.tag == ChangeTag::Delete {
                theme.diff_delete_fg
            } else {
                theme.diff_insert_fg
            };
            fg = change_fg;
            let marker_for_side = (!is_new_side && line.tag == ChangeTag::Delete)
                || (is_new_side && line.tag == ChangeTag::Insert);
            if marker_for_side {
                marker = if is_new_side { "+" } else { "-" };
                marker_fg = change_fg;
            }
            if config.gutter_bg {
                background = side_diff_background(line.tag, theme);
            }
        }
    }
    let mut marker_style = Style::default().fg(marker_fg);
    let mut number_style = Style::default().fg(fg);
    if let Some(color) = background {
        marker_style = marker_style.bg(color);
        number_style = number_style.bg(color);
    }
    let gap_style = background
        .map(|color| Style::default().bg(color))
        .unwrap_or_default();
    vec![
        Span::styled(marker, marker_style),
        Span::styled(number, number_style),
        Span::styled(CONTENT_GAP, gap_style),
    ]
}

fn render_diff_hunks_side_by_side(
    hunks: &[DiffHunk],
    path: &Path,
    by_new_line: Option<&HashMap<usize, EditLineStyles>>,
    theme: &Theme,
    config: &DiffRenderConfig,
    layout: SideBySideLayout,
    word_highlight: bool,
) -> Vec<DiffLineOutput> {
    let syntect = get_syntect();
    // Themes without diff backgrounds show changes through line foreground
    // tinting, which word ranges cannot express. Fall back to the line renderer
    // so the diff never loses its only change signal.
    let word_highlight = word_highlight && supports_word_highlight(theme);
    let mut output = Vec::new();
    for (hunk_index, hunk) in hunks.iter().enumerate() {
        if hunk_index > 0 && !output.is_empty() && !config.hunk_separator.is_empty() {
            let indent = if config.indent { INDENT } else { "" };
            output.push(DiffLineOutput {
                line: Line::from(vec![
                    Span::raw(indent),
                    Span::styled(config.hunk_separator.clone(), theme.muted()),
                ]),
                background: None,
                source: None,
                content_start_col: 0,
                gutter_span_count: 0,
                content_text: String::new(),
                joiner: None,
                is_separator: true,
            });
        }
        if hunk.is_empty() {
            continue;
        }

        let mut old_highlighter = syntect.highlight_lines_by_file_path(path);
        let mut new_highlighter = syntect.highlight_lines_by_file_path(path);
        for pair in pair_hunk_lines(hunk) {
            let (old_changed, new_changed) = if word_highlight {
                changed_word_ranges(pair.0.as_ref(), pair.1.as_ref())
            } else {
                (Vec::new(), Vec::new())
            };
            let old_rows = render_side_rows(
                pair.0.as_ref(),
                false,
                &mut old_highlighter,
                None,
                theme,
                syntect,
                layout.old_content_width,
                word_highlight,
                &old_changed,
            );
            let new_rows = render_side_rows(
                pair.1.as_ref(),
                true,
                &mut new_highlighter,
                by_new_line,
                theme,
                syntect,
                layout.new_content_width,
                word_highlight,
                &new_changed,
            );
            let row_count = old_rows.len().max(new_rows.len());
            for row_index in 0..row_count {
                let old_row = old_rows.get(row_index);
                let new_row = new_rows.get(row_index);
                let old_line = old_row
                    .map(|row| row.line.clone())
                    .unwrap_or_else(|| Line::from(Span::raw(" ".repeat(layout.old_content_width))));
                let new_line = new_row
                    .map(|row| row.line.clone())
                    .unwrap_or_else(|| Line::from(Span::raw(" ".repeat(layout.new_content_width))));
                let old_gutter = render_side_gutter(
                    old_row.and(pair.0.as_ref()),
                    old_row.is_some_and(|row| row.continuation),
                    false,
                    layout.old_num_width,
                    theme,
                    config,
                );
                let new_gutter = render_side_gutter(
                    new_row.and(pair.1.as_ref()),
                    new_row.is_some_and(|row| row.continuation),
                    true,
                    layout.new_num_width,
                    theme,
                    config,
                );
                let mut spans = Vec::new();
                if config.indent {
                    spans.push(Span::raw(INDENT));
                }
                spans.extend(old_gutter);
                spans.extend(old_line.spans);
                spans.push(Span::styled(SIDE_DIVIDER, theme.muted()));
                spans.extend(new_gutter);
                spans.extend(new_line.spans);
                let old_text = old_row.map_or("", |row| row.text.as_str());
                let new_text = new_row.map_or("", |row| row.text.as_str());
                output.push(DiffLineOutput {
                    line: Line::from(spans),
                    background: None,
                    source: (row_index == 0).then_some(pair.clone()),
                    content_start_col: 0,
                    gutter_span_count: 0,
                    content_text: format!("{old_text}{SIDE_DIVIDER}{new_text}"),
                    joiner: (row_index > 0).then_some(String::new()),
                    is_separator: false,
                });
            }
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source_line(tag: ChangeTag, text: &str) -> DiffLine {
        DiffLine {
            tag,
            text: text.to_owned(),
            lo: 1,
            ln: 1,
        }
    }

    fn render_pair(
        old: &str,
        new: &str,
        word_highlight: bool,
        width: usize,
    ) -> (Vec<DiffLineOutput>, Theme) {
        let theme = Theme::current();
        let hunk = vec![
            source_line(ChangeTag::Delete, old),
            source_line(ChangeTag::Insert, new),
        ];
        let rows = render_diff_hunks_side_by_side(
            &[hunk],
            Path::new("test.rs"),
            None,
            &theme,
            &DiffRenderConfig::default(),
            SideBySideLayout {
                old_num_width: 1,
                new_num_width: 1,
                old_content_width: width,
                new_content_width: width,
            },
            word_highlight,
        );
        (rows, theme)
    }

    fn backgrounds_for(rows: &[DiffLineOutput], text: &str) -> Vec<Option<Color>> {
        rows.iter()
            .flat_map(|row| row.line.spans.iter())
            .filter(|span| span.content.contains(text))
            .map(|span| span.style.bg)
            .collect()
    }

    #[test]
    fn creating_blocks_never_render_side_by_side() {
        let hunk = vec![
            source_line(ChangeTag::Insert, "fn main() {}\n"),
            source_line(ChangeTag::Insert, "let x = 1;\n"),
        ];
        let creating =
            EditToolCallBlock::new("src/main.rs", vec![hunk.clone()]).with_prefix("Creating ");
        let editing = EditToolCallBlock::new("src/main.rs", vec![hunk]);
        let theme = Theme::current();
        let config = DiffRenderConfig::default();

        let previous = crate::appearance::cache::load_side_by_side_edit();
        crate::appearance::cache::set_side_by_side_edit(true);
        assert!(
            render_if_enabled(&creating, &theme, 120, &config).is_none(),
            "file creation has no old side and must stay unified"
        );
        assert!(
            render_if_enabled(&editing, &theme, 120, &config).is_some(),
            "an ordinary edit with the flag on should take the side-by-side path"
        );
        crate::appearance::cache::set_side_by_side_edit(previous);
    }

    #[test]
    fn word_mode_marks_changed_tokens_not_unchanged_prefix_or_padding() {
        let (rows, theme) = render_pair("let color = red;\n", "let color = blue;\n", true, 30);
        assert!(backgrounds_for(&rows, "let").iter().all(Option::is_none));
        assert!(backgrounds_for(&rows, "red").contains(&Some(theme.diff_delete_bg)));
        assert!(backgrounds_for(&rows, "blue").contains(&Some(theme.diff_insert_bg)));
        assert!(
            rows[0]
                .line
                .spans
                .iter()
                .any(|s| s.content.contains(" ") && s.style.bg.is_none())
        );
    }

    #[test]
    fn line_mode_restores_full_line_backgrounds() {
        let (rows, theme) = render_pair("let color = red;\n", "let color = blue;\n", false, 30);
        assert!(backgrounds_for(&rows, "let").contains(&Some(theme.diff_delete_bg)));
        assert!(backgrounds_for(&rows, "let").contains(&Some(theme.diff_insert_bg)));
    }

    #[test]
    fn word_mode_keeps_highlights_on_wrapped_rows() {
        let (rows, theme) = render_pair(
            "let background_color = crimson;\n",
            "let background_color = turquoise;\n",
            true,
            12,
        );
        assert!(rows.len() > 1);
        assert!(backgrounds_for(&rows, "crimson").contains(&Some(theme.diff_delete_bg)));
        assert!(backgrounds_for(&rows, "turquoise").contains(&Some(theme.diff_insert_bg)));
    }

    #[test]
    fn unicode_and_unpaired_changes_use_valid_byte_ranges() {
        let old = source_line(ChangeTag::Delete, "值 = 旧版本\n");
        let new = source_line(ChangeTag::Insert, "值 = 新版本\n");
        let (old_ranges, new_ranges) = changed_word_ranges(Some(&old), Some(&new));
        let old_text = "值 = 旧版本";
        let new_text = "值 = 新版本";
        assert!(
            old_ranges
                .iter()
                .any(|r| old_text[r.clone()].contains('旧'))
        );
        assert!(
            new_ranges
                .iter()
                .any(|r| new_text[r.clone()].contains('新'))
        );
        let (_, added) = changed_word_ranges(None, Some(&new));
        assert_eq!(added, vec![0..new_text.len()]);
    }

    #[test]
    fn equal_lines_never_get_word_backgrounds() {
        let theme = Theme::current();
        let hunk = vec![source_line(ChangeTag::Equal, "unchanged content\n")];
        let rows = render_diff_hunks_side_by_side(
            &[hunk],
            Path::new("test.rs"),
            None,
            &theme,
            &DiffRenderConfig::default(),
            SideBySideLayout {
                old_num_width: 1,
                new_num_width: 1,
                old_content_width: 25,
                new_content_width: 25,
            },
            true,
        );
        assert!(
            rows.iter()
                .all(|row| row.line.spans.iter().all(|span| span.style.bg.is_none()))
        );
    }

    /// Terminal-default themes paint diffs through foreground tinting only
    /// (`diff_*_bg == Reset`). Word mode cannot express a change there, so it
    /// must yield to the line renderer instead of erasing the only signal.
    #[test]
    fn themes_without_diff_backgrounds_refuse_word_mode() {
        let mut theme = Theme::current();
        theme.diff_delete_bg = Color::Reset;
        theme.diff_insert_bg = Color::Reset;
        assert!(theme.diff_uses_line_fg());
        assert!(!supports_word_highlight(&theme));

        let colored = Theme::current();
        if !colored.diff_uses_line_fg() {
            assert!(supports_word_highlight(&colored));
        }
    }
}
