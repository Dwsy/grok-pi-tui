//! Defines the `MarkdownStyle` used by agent message and thinking blocks.
//! Colors come from the `md_*` fields on the current [`Theme`], which are already quantized to the terminal's color capability level.

use anstyle::{Ansi256Color, AnsiColor, Color, Style};
use xai_grok_markdown::MarkdownStyle;

/// Bridge after [`Theme::current()`] quantization. `None` for `Reset`: anstyle has no terminal-default, and unset renders as that default.
fn to_anstyle(c: ratatui::style::Color) -> Option<Color> {
    Some(match c {
        ratatui::style::Color::Reset => return None,
        ratatui::style::Color::Rgb(r, g, b) => Color::Rgb(anstyle::RgbColor(r, g, b)),
        ratatui::style::Color::Indexed(n) => Color::Ansi256(Ansi256Color(n)),
        // Named ANSI colors (from 16-color quantization).
        ratatui::style::Color::Black => Color::Ansi(AnsiColor::Black),
        ratatui::style::Color::Red => Color::Ansi(AnsiColor::Red),
        ratatui::style::Color::Green => Color::Ansi(AnsiColor::Green),
        ratatui::style::Color::Yellow => Color::Ansi(AnsiColor::Yellow),
        ratatui::style::Color::Blue => Color::Ansi(AnsiColor::Blue),
        ratatui::style::Color::Magenta => Color::Ansi(AnsiColor::Magenta),
        ratatui::style::Color::Cyan => Color::Ansi(AnsiColor::Cyan),
        ratatui::style::Color::Gray => Color::Ansi(AnsiColor::White),
        ratatui::style::Color::DarkGray => Color::Ansi(AnsiColor::BrightBlack),
        ratatui::style::Color::LightRed => Color::Ansi(AnsiColor::BrightRed),
        ratatui::style::Color::LightGreen => Color::Ansi(AnsiColor::BrightGreen),
        ratatui::style::Color::LightYellow => Color::Ansi(AnsiColor::BrightYellow),
        ratatui::style::Color::LightBlue => Color::Ansi(AnsiColor::BrightBlue),
        ratatui::style::Color::LightMagenta => Color::Ansi(AnsiColor::BrightMagenta),
        ratatui::style::Color::LightCyan => Color::Ansi(AnsiColor::BrightCyan),
        ratatui::style::Color::White => Color::Ansi(AnsiColor::BrightWhite),
    })
}

/// `anstyle::Style` with the given foreground color (converted from ratatui).
fn fg(c: ratatui::style::Color) -> Style {
    Style::new().fg_color(to_anstyle(c))
}

/// `anstyle::Style` with the given background color (converted from ratatui).
fn bg(c: ratatui::style::Color) -> Style {
    Style::new().bg_color(to_anstyle(c))
}

/// A status chip: `fg` text on a `fill` background.
fn chip(fg_color: ratatui::style::Color, fill: ratatui::style::Color) -> Style {
    Style::new()
        .fg_color(to_anstyle(fg_color))
        .bg_color(to_anstyle(fill))
}

fn modifier_to_anstyle(m: ratatui::style::Modifier) -> Style {
    let mut s = Style::new();
    if m.contains(ratatui::style::Modifier::BOLD) {
        s = s.bold();
    }
    if m.contains(ratatui::style::Modifier::ITALIC) {
        s = s.italic();
    }
    if m.contains(ratatui::style::Modifier::UNDERLINED) {
        s = s.underline();
    }
    if m.contains(ratatui::style::Modifier::DIM) {
        s = s.dimmed();
    }
    if m.contains(ratatui::style::Modifier::HIDDEN) {
        s = s.hidden();
    }
    if m.contains(ratatui::style::Modifier::CROSSED_OUT) {
        s = s.strikethrough();
    }
    s
}

fn heading_inner_styles(
    colors: [ratatui::style::Color; 6],
    mods: [ratatui::style::Modifier; 6],
) -> [Style; 6] {
    std::array::from_fn(|i| {
        let color_style = fg(colors[i]);
        let mod_style = modifier_to_anstyle(mods[i]);
        // Combine fg color with modifier effects.
        let mut s = color_style;
        let effects = mod_style.get_effects();
        if !effects.is_plain() {
            s = s.effects(s.get_effects() | effects);
        }
        s
    })
}

/// Build heading outer styles (dimmed and hidden, for syntax markers).
fn heading_outer_styles(colors: [ratatui::style::Color; 6]) -> [Style; 6] {
    colors.map(|c| fg(c).dimmed().hidden())
}

/// The style is built fresh from [`Theme::current()`] on each call.
/// Both the theme construction and style mapping are trivial struct copies.
pub fn style() -> MarkdownStyle {
    build_style()
}

fn build_style() -> MarkdownStyle {
    let theme = super::Theme::current();

    let heading_colors = [
        theme.md_heading_h1,
        theme.md_heading_h2,
        theme.md_heading_h3,
        theme.md_heading_h4,
        theme.md_heading_h5,
        theme.md_heading_h6,
    ];
    let heading_mods = [
        theme.md_heading_h1_mod,
        theme.md_heading_h2_mod,
        theme.md_heading_h3_mod,
        theme.md_heading_h4_mod,
        theme.md_heading_h5_mod,
        theme.md_heading_h6_mod,
    ];

    MarkdownStyle {
        heading_inner: heading_inner_styles(heading_colors, heading_mods),
        heading_outer: heading_outer_styles(heading_colors),
        strong_inner: fg(theme.md_text).bold(),
        strong_outer: Style::new().dimmed().hidden(),
        emphasis_inner: fg(theme.md_text).italic(),
        emphasis_outer: Style::new().dimmed().hidden(),
        strikethrough_inner: fg(theme.md_text).strikethrough(),
        strikethrough_outer: Style::new().dimmed().hidden(),
        inline_code_inner: fg(theme.md_code).bold(),
        inline_code_outer: fg(theme.md_code).dimmed().hidden(),
        // The selection-side bar detection in xai-grok-pager (scrollback/blocks/quote_bar.rs, `quote_bar_style`) mirrors this exact style
        // Its end-to-end tests fail if this line changes
        blockquote_outer: fg(theme.md_muted).dimmed(),
        task_checked: fg(theme.md_task_checked),
        task_unchecked: fg(theme.md_task_unchecked).dimmed(),
        list_item: fg(theme.md_muted),
        rule: fg(theme.md_muted),
        link_outer: fg(theme.md_muted),
        link_text: fg(theme.link_fg).underline(),
        link_url: fg(theme.md_muted),
        link_title: fg(theme.md_heading_h5),
        code_outer: fg(theme.md_code).dimmed().hidden(),
        code_language: fg(theme.md_heading_h3).hidden(),
        code_untagged: fg(theme.md_text),
        code_background: bg(theme.md_code_bg),
        table_outer: fg(theme.md_heading_h2).hidden(),
        text: fg(theme.md_text),
        math: fg(theme.md_text).italic(),
        // Status pills. The three panel tones are tinted chips keyed to the theme's own semantic
        // accents, so a settled state reads green, a failure red and an ask amber in every theme
        // rather than inheriting a heading colour that happens to be grey. Danger, mode and owner
        // use inverse video so a consequence or an ownership claim is louder than a plain status.
        pill_success: chip(theme.md_code_bg, theme.accent_success),
        pill_error: chip(theme.md_code_bg, theme.accent_error),
        pill_warning: chip(theme.md_code_bg, theme.warning),
        pill_neutral: fg(theme.md_muted),
        pill_danger: fg(theme.accent_error).invert(),
        pill_mode: fg(theme.accent_success).invert(),
        pill_owner: fg(theme.accent_running).invert(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xai_grok_markdown::pill::PillTone;

    /// Regression: `Reset` used to fall back to a concrete `AnsiColor::White` (ANSI-7 silver).
    /// That rendered Reset-themed markdown washed-out gray on light terminals and broke the `NO_COLOR` opt-out.
    #[test]
    fn reset_maps_to_no_color() {
        assert_eq!(to_anstyle(ratatui::style::Color::Reset), None);
        assert_eq!(fg(ratatui::style::Color::Reset).get_fg_color(), None);
        assert_eq!(bg(ratatui::style::Color::Reset).get_bg_color(), None);
    }

    /// Spot checks around the Gray/DarkGray naming mismatch between ratatui and anstyle.
    #[test]
    fn named_colors_map_concretely() {
        assert_eq!(
            to_anstyle(ratatui::style::Color::DarkGray),
            Some(Color::Ansi(AnsiColor::BrightBlack))
        );
        assert_eq!(
            to_anstyle(ratatui::style::Color::Gray),
            Some(Color::Ansi(AnsiColor::White))
        );
        assert_eq!(
            to_anstyle(ratatui::style::Color::Red),
            Some(Color::Ansi(AnsiColor::Red))
        );
    }

    #[test]
    fn terminal_default_body_text_has_no_fg() {
        let theme = super::super::Theme::terminal_default();
        assert_eq!(fg(theme.md_text).get_fg_color(), None);
        assert_eq!(bg(theme.md_code_bg).get_bg_color(), None);
    }

    /// The three panel tones must not collapse into one chip colour.
    ///
    /// They are keyed to different theme slots (task-checked, h1, h5), so a theme that fails to
    /// distinguish them would make a failure look like a success. This guards the mapping.
    #[test]
    fn panel_tones_stay_distinct() {
        let theme = super::super::Theme::terminal_default();
        let style = build_style();
        let fills: Vec<_> = [PillTone::Success, PillTone::Error, PillTone::Warning]
            .iter()
            .map(|tone| tone.style(&style).get_bg_color())
            .collect();
        for (i, a) in fills.iter().enumerate() {
            for b in fills.iter().skip(i + 1) {
                assert_ne!(a, b, "two panel tones share a background: {fills:?}");
            }
        }
        // Every chip showing the body background would mean the theme had nothing to say, and the
        // three statuses would be indistinguishable.
        let body_bg = style.text.get_bg_color();
        assert!(
            fills.iter().any(|f| *f != body_bg),
            "at least one tone must differ from the body background: {fills:?}, body {body_bg:?}"
        );
        let _ = theme;
    }

    /// The solid tones draw by inverting, so they stay distinguishable without a background fill.
    #[test]
    fn solid_tones_are_inverted_and_tinted() {
        let style = build_style();
        for tone in [PillTone::Danger, PillTone::Mode, PillTone::Owner] {
            let s = tone.style(&style);
            assert!(
                s.get_effects().contains(anstyle::Effects::INVERT),
                "{tone:?} must draw inverted"
            );
            assert!(
                s.get_fg_color().is_some(),
                "{tone:?} must carry its tint, or it draws as plain inverse video"
            );
        }
    }
}
