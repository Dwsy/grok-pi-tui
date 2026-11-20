//! A pill's tone must be independent of the text around it.
//!
//! `[[Done]]` and the words beside it share a paragraph, so a renderer that merges one style across
//! the segment would paint the chip in the body colour. These tests assert the chip's own span keeps
//! its tone while its neighbours keep theirs.

use crate::pill::PillTone;
use crate::style::test_style;
use crate::render_markdown_ratatui_full;

fn rt(style: anstyle::Style) -> ratatui::style::Style {
    use ratatui::style::{Modifier, Style as RStyle};
    let mut out = RStyle::default();
    if let Some(fg) = style.get_fg_color() {
        out = out.fg(color(fg));
    }
    if let Some(bg) = style.get_bg_color() {
        out = out.bg(color(bg));
    }
    let effects = style.get_effects();
    let mut modifier = Modifier::empty();
    for (effect, flag) in [
        (anstyle::Effects::BOLD, Modifier::BOLD),
        (anstyle::Effects::DIMMED, Modifier::DIM),
        (anstyle::Effects::ITALIC, Modifier::ITALIC),
        (anstyle::Effects::UNDERLINE, Modifier::UNDERLINED),
        (anstyle::Effects::INVERT, Modifier::REVERSED),
        (anstyle::Effects::HIDDEN, Modifier::HIDDEN),
        (anstyle::Effects::STRIKETHROUGH, Modifier::CROSSED_OUT),
    ] {
        if effects.contains(effect) {
            modifier |= flag;
        }
    }
    out.add_modifier(modifier)
}

fn color(c: anstyle::Color) -> ratatui::style::Color {
    use ratatui::style::Color as RColor;
    match c {
        anstyle::Color::Ansi(a) => match a {
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

/// The drawn spans, keyed by visible text: the zero-width spaces that erase a pill's brackets are
/// dropped so a span can be found by what the user actually reads.
fn spans(md: &str) -> Vec<(String, ratatui::style::Style)> {
    let (output, _) = render_markdown_ratatui_full(md, test_style::STYLE, true, None);
    output
        .lines
        .iter()
        .flat_map(|l| l.spans.iter())
        .map(|s| (s.content.replace('\u{200b}', ""), s.style))
        .collect()
}

fn style_of(md: &str, text: &str) -> ratatui::style::Style {
    let all = spans(md);
    all.iter()
        .find(|(content, _)| content == text)
        .unwrap_or_else(|| panic!("{text:?} should be its own span; got {all:#?}"))
        .1
}

/// The chip keeps its tone while the words on either side keep theirs.
#[test]
fn a_chip_keeps_its_tone_next_to_plain_text() {
    let md = "before [[Done]] after.\n\n";
    assert_eq!(
        style_of(md, "Done"),
        rt(test_style::STYLE.pill_success),
        "the chip must not inherit the body style"
    );
    // The neighbours stay unstyled: `ms.text` on this test style is plain.
    assert_ne!(
        style_of(md, "before "),
        rt(test_style::STYLE.pill_success),
        "plain prose must not take the chip's tone"
    );
}

/// Two chips of different tones on one line each keep their own.
#[test]
fn two_chips_keep_different_tones_on_one_line() {
    let md = "[[Failed 1]] then [[Done 3]].\n\n";
    assert_eq!(style_of(md, "Failed 1"), rt(test_style::STYLE.pill_error));
    assert_eq!(style_of(md, "Done 3"), rt(test_style::STYLE.pill_success));
}

/// A solid chip keeps its inverse video, which is how it reads louder than the panel tones.
#[test]
fn a_solid_chip_keeps_its_inverse_video() {
    let md = "[[Destructive]] the rows.\n\n";
    let style = style_of(md, "Destructive");
    assert!(
        style.add_modifier.contains(ratatui::style::Modifier::REVERSED),
        "a solid chip must draw inverted; got {style:?}"
    );
}

/// Every tone reaches its chip when the chip sits in a sentence, not alone in the document.
#[test]
fn every_tone_survives_a_sentence() {
    let ms = test_style::STYLE;
    for (label, tone) in [
        ("Done", PillTone::Success),
        ("Failed", PillTone::Error),
        ("Needs you", PillTone::Warning),
        ("Destructive", PillTone::Danger),
        ("Implement", PillTone::Mode),
        ("Shared", PillTone::Owner),
    ] {
        let md = format!("we [[{label}]] it.\n\n");
        assert_eq!(
            style_of(&md, label),
            rt(tone.style(&ms)),
            "{label} lost its tone inside a sentence"
        );
    }
}

/// The chip's tone must not leak into the following text.
#[test]
fn a_chip_does_not_colour_what_follows() {
    let md = "[[Failed]] then plain words here.\n\n";
    let after = style_of(md, " then plain words here.");
    assert_ne!(
        after,
        rt(test_style::STYLE.pill_error),
        "the tone must end with the chip"
    );
}
