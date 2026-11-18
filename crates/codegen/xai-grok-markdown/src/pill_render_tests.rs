//! End-to-end pill rendering: what a `[[Word]]` looks like once drawn.
//!
//! `crate::pill` covers the vocabulary and the scan; these tests cover the draw, so a change that
//! breaks what the user sees fails here even when the tables stay correct.

use crate::pill::PillTone;
use crate::style::{MarkdownStyle, test_style};
use crate::{render_markdown, render_markdown_ratatui_full};

/// The drawn text as the user reads it: a chip's bracket padding collapses to nothing.
///
/// A pill's `[[` and `]]` become two spaces each and carry the chip's style, so they draw as the
/// chip's shoulders. They are part of the chip, not of the sentence, so the comparison strips the
/// spaces that sit between a chip and the punctuation or word beside it.
fn lines_to_text(lines: &[ratatui::text::Line<'static>]) -> Vec<String> {
    lines
        .iter()
        .map(|l| {
            let text: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
            text.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                // A chip's trailing shoulder leaves a space before punctuation.
                .replace(" .", ".")
                .replace(" ,", ",")
        })
        .collect()
}

fn pretty_lines(text: &str) -> Vec<String> {
    let (output, _) = render_markdown_ratatui_full(text, test_style::STYLE, true, None);
    lines_to_text(&output.lines)
}

/// The style covering the drawn chip, so a test can assert the tone reached the span.
fn pill_style_of(
    lines: &[ratatui::text::Line<'static>],
    word: &str,
) -> Option<ratatui::style::Style> {
    lines.iter().find_map(|line| {
        line.spans
            .iter()
            .find(|span| span.content.as_ref() == word)
            .map(|span| span.style)
    })
}

/// `anstyle::Style` → `ratatui::style::Style`, the conversion the renderer uses.
fn rt(style: anstyle::Style) -> ratatui::style::Style {
    // Re-implemented here because the renderer's `StyleInto` is private to `parse`.
    use ratatui::style::{Modifier, Style as RStyle};

    let mut out = RStyle::default();
    if let Some(fg) = style.get_fg_color() {
        out = out.fg(from_anstyle_color(fg));
    }
    if let Some(bg) = style.get_bg_color() {
        out = out.bg(from_anstyle_color(bg));
    }
    if let Some(ul) = style.get_underline_color() {
        out = out.underline_color(from_anstyle_color(ul));
    }
    let effects = style.get_effects();
    let mut modifier = Modifier::empty();
    for (effect, modifier_flag) in [
        (anstyle::Effects::BOLD, Modifier::BOLD),
        (anstyle::Effects::DIMMED, Modifier::DIM),
        (anstyle::Effects::ITALIC, Modifier::ITALIC),
        (anstyle::Effects::UNDERLINE, Modifier::UNDERLINED),
        (anstyle::Effects::BLINK, Modifier::SLOW_BLINK),
        (anstyle::Effects::INVERT, Modifier::REVERSED),
        (anstyle::Effects::HIDDEN, Modifier::HIDDEN),
        (anstyle::Effects::STRIKETHROUGH, Modifier::CROSSED_OUT),
    ] {
        if effects.contains(effect) {
            modifier |= modifier_flag;
        }
    }
    out.add_modifier(modifier)
}

fn from_anstyle_color(color: anstyle::Color) -> ratatui::style::Color {
    use ratatui::style::Color as RColor;
    match color {
        anstyle::Color::Ansi(c) => match c {
            anstyle::AnsiColor::Black => RColor::Black,
            anstyle::AnsiColor::Red => RColor::Red,
            anstyle::AnsiColor::Green => RColor::Green,
            anstyle::AnsiColor::Yellow => RColor::Yellow,
            anstyle::AnsiColor::Blue => RColor::Blue,
            anstyle::AnsiColor::Magenta => RColor::Magenta,
            anstyle::AnsiColor::Cyan => RColor::Cyan,
            anstyle::AnsiColor::White => RColor::Gray,
            anstyle::AnsiColor::BrightBlack => RColor::DarkGray,
            anstyle::AnsiColor::BrightRed => RColor::LightRed,
            anstyle::AnsiColor::BrightGreen => RColor::LightGreen,
            anstyle::AnsiColor::BrightYellow => RColor::LightYellow,
            anstyle::AnsiColor::BrightBlue => RColor::LightBlue,
            anstyle::AnsiColor::BrightMagenta => RColor::LightMagenta,
            anstyle::AnsiColor::BrightCyan => RColor::LightCyan,
            anstyle::AnsiColor::BrightWhite => RColor::White,
        },
        anstyle::Color::Ansi256(c) => RColor::Indexed(c.0),
        anstyle::Color::Rgb(c) => RColor::Rgb(c.0, c.1, c.2),
    }
}

/// Brackets are hidden, so pretty mode draws the chip without the `[[` / `]]` syntax.
#[test]
fn a_pill_draws_without_its_brackets() {
    let lines = pretty_lines("[[Done]] the migration.\n\n");
    assert_eq!(lines[0], "Done the migration.", "got: {lines:#?}");
}

#[test]
fn a_known_word_keeps_its_detail() {
    let lines = pretty_lines("[[Failed: 3 tests]] in the parser.\n\n");
    assert_eq!(lines[0], "Failed: 3 tests in the parser.", "got: {lines:#?}");
}

/// A synonym keeps its own spelling when the canonical word does not fit the same byte length.
///
/// `Completed` (9 bytes) cannot become `Done` (4) without shifting every following offset, so the
/// drawn word stays as written. The tone and icon still come from `split_label`, so the chip is
/// green with a check either way — that is what carries the meaning.
#[test]
fn a_synonym_keeps_its_spelling_when_the_length_differs() {
    let lines = pretty_lines("[[Completed]] and [[Fail]] and [[OK]].\n\n");
    assert_eq!(lines[0], "Completed and Fail and OK.", "got: {lines:#?}");
}

/// A synonym whose canonical word has the same byte length is rewritten on screen.
#[test]
fn an_equal_length_synonym_reads_as_its_canonical_word() {
    // `Pass` (4) → `Passed` (6) differs; `Fail` (4) → `Failed` (6) differs. Every synonym in the
    // table happens to be shorter than its canonical word, so none is rewritten today. This test
    // pins the rule itself rather than one example of it.
    for (label, canonical) in [("Completed", "Done"), ("Fail", "Failed"), ("OK", "Done")] {
        let md = format!("[[{label}]] x.\n\n");
        let drawn = pretty_lines(&md);
        let expected = if label.len() == canonical.len() {
            format!("{canonical} x.")
        } else {
            format!("{label} x.")
        };
        assert_eq!(drawn[0], expected, "{label} should draw as {expected:?}");
    }
}

#[test]
fn an_unknown_word_still_draws_without_brackets() {
    let lines = pretty_lines("[[Anything else]] here.\n\n");
    assert_eq!(lines[0], "Anything else here.", "got: {lines:#?}");
}

/// Every tone reaches a drawn span. A single-word pill shares its span with no neighbouring text,
/// so the chip's own style is assertable for six of the seven tones; the neutral tone is the
/// body-text style and is checked by the drawn text instead.
#[test]
fn each_tone_reaches_the_drawn_span() {
    let ms: MarkdownStyle = test_style::STYLE;
    for (label, tone) in [
        ("Done", PillTone::Success),
        ("Failed", PillTone::Error),
        ("Needs you", PillTone::Warning),
        ("Destructive", PillTone::Danger),
        ("Implement", PillTone::Mode),
        ("Shared", PillTone::Owner),
    ] {
        let md = format!("[[{label}]] x.\n\n");
        let (output, _) = render_markdown_ratatui_full(&md, ms, true, None);
        let drawn = lines_to_text(&output.lines);
        let style = pill_style_of(&output.lines, label)
            .unwrap_or_else(|| panic!("{label} should draw as its own span; got {drawn:#?}"));
        assert_eq!(
            style,
            rt(tone.style(&ms)),
            "{label} should draw in its tone's style; got {style:?}"
        );
    }
}

/// A pill inside inline code is a literal, not a chip.
#[test]
fn a_pill_in_inline_code_stays_literal() {
    let lines = pretty_lines("Run `[[Done]]` yourself.\n\n");
    assert_eq!(lines[0], "Run [[Done]] yourself.", "got: {lines:#?}");
}

/// A fenced block is code: no pill is drawn inside it.
#[test]
fn a_pill_in_a_fence_stays_literal() {
    let lines = pretty_lines("```\n[[Done]]\n```\n\n");
    assert!(
        lines.iter().any(|l| l.contains("[[Done]]")),
        "pills in a fence must stay as typed; got {lines:#?}"
    );
}

/// An unclosed pill is left as typed, so a streaming half-arrival never draws a truncated chip.
#[test]
fn a_split_pill_is_not_half_drawn() {
    let lines = pretty_lines("[[Do\n\n");
    assert!(
        lines.iter().any(|l| l.contains("[[Do")),
        "an unclosed pill must stay as typed; got {lines:#?}"
    );
}

/// Raw mode keeps the label and its tone; only the brackets are erased, and they are erased in
/// both modes so the chip never shows its syntax.
#[test]
fn raw_mode_keeps_the_word_and_its_tone() {
    let (ansi, _) = render_markdown("[[Done]] x.\n\n", test_style::STYLE, false, None);
    assert!(
        ansi.contains("Done"),
        "raw output should keep the word: {ansi:?}"
    );
    // The brackets are gone: two zero-width spaces erased them, and no literal `[[` survives.
    assert!(
        !ansi.contains("[["),
        "the brackets must be erased in raw mode too: {ansi:?}"
    );
    // The tone still reaches the label, so raw output is coloured like the drawn chip.
    assert!(
        ansi.contains("\u{1b}[32m") || ansi.contains("38;5;"),
        "the label should keep its tone: {ansi:?}"
    );
}

/// Two pills on one line each become their own span with its own tone.
#[test]
fn two_pills_on_a_line_keep_their_own_tones() {
    let md = "[[Done 3]] [[Failed 1]].\n\n";
    let (output, _) = render_markdown_ratatui_full(md, test_style::STYLE, true, None);
    let lines = lines_to_text(&output.lines);
    assert_eq!(lines[0], "Done 3 Failed 1.", "got: {lines:#?}");
    assert_eq!(
        pill_style_of(&output.lines, "Done 3"),
        Some(rt(test_style::STYLE.pill_success))
    );
    assert_eq!(
        pill_style_of(&output.lines, "Failed 1"),
        Some(rt(test_style::STYLE.pill_error))
    );
}

/// A pill must not break the source map the streaming renderer relies on.
#[test]
fn a_pill_keeps_the_source_map_valid() {
    let md = "Intro line.\n\n[[Done]] second.\n\n";
    let (output, _) = render_markdown_ratatui_full(md, test_style::STYLE, true, None);
    assert_eq!(output.line_source_map.len(), output.lines.len());
    for offset in &output.line_source_map {
        assert!(
            *offset <= md.len(),
            "a source offset must stay inside the source: {offset} of {}",
            md.len()
        );
    }
}

/// A pill inside a table cell draws as a chip like it does in prose.
///
/// The table path buffers each cell into its own string and never runs `scan_pills`, which is why
/// the pill has to be split while the cell is collected.
#[test]
fn a_pill_in_a_table_cell_draws_as_a_chip() {
    let md = "| State | Note |\n|---|---|\n| [[Failed]] | build broke |\n\n";
    let lines = pretty_lines(md);
    let row = lines
        .iter()
        .find(|l| l.contains("Failed"))
        .unwrap_or_else(|| panic!("the body row should draw; got {lines:#?}"));
    assert!(
        !row.contains("[["),
        "the brackets must not show in a cell: {row:?}"
    );
    assert!(
        row.contains('\u{f00d}'),
        "the cell pill should carry its tone's icon: {row:?}"
    );
}

/// The cell pill is its own span, so its tone is assertable.
#[test]
fn a_table_pill_carries_its_tone_style() {
    let ms: MarkdownStyle = test_style::STYLE;
    let md = "| State |\n|---|\n| [[Failed]] |\n\n";
    let (output, _) = render_markdown_ratatui_full(md, ms, true, None);
    let drawn = lines_to_text(&output.lines);
    let style = pill_style_of(&output.lines, "\u{f00d} Failed")
        .unwrap_or_else(|| panic!("the cell pill should be its own span; got {drawn:#?}"));
    let expected = rt(PillTone::Error.style(&ms));
    assert_eq!(
        (style.fg, style.bg),
        (expected.fg, expected.bg),
        "the cell pill should draw in its tone's colours; got {style:?}"
    );
}

/// A cell pill is buffered text, so a synonym is free to be rewritten to its canonical word.
#[test]
fn a_table_pill_reads_as_its_canonical_word() {
    let lines = pretty_lines("| State |\n|---|\n| [[Completed]] |\n\n");
    let row = lines
        .iter()
        .find(|l| l.contains("Done"))
        .unwrap_or_else(|| panic!("the canonical word should draw; got {lines:#?}"));
    assert!(
        !row.contains("Completed"),
        "`[[Completed]]` should read as `Done` in a cell: {row:?}"
    );
}

/// A pill inside a cell's code span stays literal, matching prose.
#[test]
fn a_pill_in_a_table_code_span_stays_literal() {
    let md = "| Run |\n|---|\n| `[[Done]]` |\n\n";
    let (output, _) = render_markdown_ratatui_full(md, test_style::STYLE, true, None);
    let drawn = lines_to_text(&output.lines);
    assert!(
        drawn.iter().any(|l| l.contains("[[Done]]")),
        "a cell's code span must keep the brackets: {drawn:#?}"
    );
}

/// The icon and the canonical word must not push a column out of line.
#[test]
fn a_table_pill_keeps_the_columns_aligned() {
    let md = "| State | Note |\n|---|---|\n| [[Failed]] | build broke |\n| plain | ok |\n\n";
    let (output, _) = render_markdown_ratatui_full(md, test_style::STYLE, true, None);
    let widths: Vec<usize> = output
        .lines
        .iter()
        .map(|l| {
            let text: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
            crate::buffers::unicode_display_width(&text)
        })
        .collect();
    assert!(
        widths.windows(2).all(|w| w[0] == w[1]),
        "every table row must draw the same width, got {widths:?}"
    );
}
