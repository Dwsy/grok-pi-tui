//! Optional side-by-side renderer for edit diffs.
//!
//! This module deliberately sits beside `edit.rs`: the upstream edit block
//! remains the source of truth for headers, selection, highlighting state, and
//! unified rendering. When the grok-pi-only F2 flag is enabled and enough width
//! is available, `EditToolCallBlock::render_diff_lines` delegates here.

use std::collections::HashMap;
use std::path::Path;

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use similar::ChangeTag;
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

fn side_diff_style(tag: ChangeTag, theme: &Theme) -> Style {
    match tag {
        ChangeTag::Equal => Style::default().fg(theme.diff_equal_fg),
        ChangeTag::Delete => Style::default().fg(if theme.diff_uses_line_fg() {
            theme.diff_delete_fg
        } else {
            theme.text_primary
        }),
        ChangeTag::Insert => Style::default().fg(if theme.diff_uses_line_fg() {
            theme.diff_insert_fg
        } else {
            theme.text_primary
        }),
    }
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
    let mut content_spans = render_content_spans(&text, line.tag, theme, highlighter, syntect);
    if is_new_side
        && let Some(map) = by_new_line
        && let Some(spans) = map_spans_for_line(line, &text, map, theme)
    {
        content_spans = spans;
    }
    let raw_content = content_spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    let chunks = if unicode_width::UnicodeWidthStr::width(raw_content.as_str()) <= width {
        vec![Line::from(content_spans)]
    } else {
        let style = side_diff_style(line.tag, theme);
        wrap_text(&raw_content, width)
            .into_iter()
            .map(|chunk| Line::from(Span::styled(chunk, style)))
            .collect()
    };
    let background = side_diff_background(line.tag, theme);
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
) -> Vec<DiffLineOutput> {
    let syntect = get_syntect();
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
            let old_rows = render_side_rows(
                pair.0.as_ref(),
                false,
                &mut old_highlighter,
                None,
                theme,
                syntect,
                layout.old_content_width,
            );
            let new_rows = render_side_rows(
                pair.1.as_ref(),
                true,
                &mut new_highlighter,
                by_new_line,
                theme,
                syntect,
                layout.new_content_width,
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
