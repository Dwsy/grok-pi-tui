use pretty_assertions::assert_eq;
use ratatui::layout::Rect;
use ratatui::style::Color;

use super::*;

#[test]
fn selected_row_fills_highlight_across_full_width() {
    let _pinned = crate::theme::cache::pin_theme();
    let theme = Theme::current();
    let mut buf = Buffer::empty(Rect::new(0, 0, 12, 1));
    let row = ListRow {
        is_selected: true,
        ..plain_row("a")
    };

    assert_eq!(1, render_row(&mut buf, 1, 10, 0, &row, &theme));

    let reset = Color::Reset;
    let expected = [vec![reset], vec![theme.bg_highlight; 10], vec![reset]].concat();
    let bgs: Vec<Color> = buf.content.iter().map(|cell| cell.bg).collect();
    assert_eq!(expected, bgs);
}

#[test]
fn a_selected_row_reverses_on_a_theme_without_a_background_band() {
    let theme = Theme::terminal();
    let mut buf = Buffer::empty(Rect::new(0, 0, 8, 1));
    let row = ListRow {
        is_selected: true,
        ..plain_row("a")
    };

    render_row(&mut buf, 0, 8, 0, &row, &theme);

    assert!(
        buf.content
            .iter()
            .all(|cell| cell.modifier.contains(Modifier::REVERSED))
    );
}

#[test]
fn detail_line_is_indented_gray_and_highlighted_when_selected() {
    let _pinned = crate::theme::cache::pin_theme();
    let theme = Theme::current();
    let mut buf = Buffer::empty(Rect::new(0, 0, 16, 2));
    let row = ListRow {
        detail: Some("details"),
        is_selected: true,
        ..plain_row("name")
    };

    assert_eq!(2, render_row(&mut buf, 0, 16, 0, &row, &theme));

    assert_eq!("      details   ", row_text(&buf, 1));
    assert_eq!(
        (theme.gray, theme.bg_highlight, Modifier::empty()),
        cell_look(&buf, DETAIL_INDENT, 1)
    );
}

#[test]
fn a_row_without_an_arrow_starts_at_its_dot_and_tags_follow_the_name() {
    let theme = Theme::current();
    let mut buf = Buffer::empty(Rect::new(0, 0, 24, 1));
    let tags = [
        tag(" active", theme.accent_success, 0),
        tag(" user ", theme.accent_user, 1),
    ];
    let row = ListRow {
        expand: None,
        status: RowStatus::Pending,
        tags: &tags,
        ..plain_row("name")
    };

    render_row(&mut buf, 0, 24, 0, &row, &theme);

    let dot = crate::glyphs::filled_dot();
    assert_eq!(format!("{dot} name active  user     "), row_text(&buf, 0));
    let plain = |fg| (fg, Color::Reset, Modifier::empty());
    assert_eq!(plain(theme.warning), cell_look(&buf, 0, 0));
    assert_eq!(plain(theme.accent_success), cell_look(&buf, 7, 0));
    assert_eq!(plain(Color::Reset), cell_look(&buf, 13, 0));
    assert_eq!(plain(theme.accent_user), cell_look(&buf, 15, 0));
}

/// A flat row's detail sits under its name; an arrow row's sits where wrapped detail lines go
#[test]
fn detail_starts_under_the_name_or_at_the_wrap_column() {
    let theme = Theme::default();
    for (expand, column) in [(Some(false), DETAIL_INDENT), (None, NAME_INDENT)] {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 3));
        let row = ListRow {
            expand,
            detail: Some("first"),
            ..plain_row("name")
        };

        render_row(&mut buf, 0, 20, 0, &row, &theme);
        render_detail_line(&mut buf, 0, 20, 2, "wrap", false, &theme);

        let start = |y| row_text(&buf, y).len() - row_text(&buf, y).trim_start().len();
        assert_eq!(usize::from(column), start(1), "expand: {expand:?}");
        assert_eq!(usize::from(DETAIL_INDENT), start(2));
    }
}

#[test]
fn narrow_width_clips_name_and_skips_tags_that_do_not_fit() {
    let theme = Theme::current();
    let mut buf = Buffer::empty(Rect::new(0, 0, 12, 1));
    let tags = [tag(" [off]", theme.gray_dim, 0)];
    let row = ListRow {
        tags: &tags,
        ..plain_row("longname")
    };

    render_row(&mut buf, 0, 8, 0, &row, &theme);

    let dot = crate::glyphs::filled_dot();
    assert_eq!(format!("\u{25b6} {dot} long    "), row_text(&buf, 0));
}

#[test]
fn section_header_draws_bold_gray_label_then_rule() {
    let theme = Theme::current();
    let mut buf = Buffer::empty(Rect::new(0, 0, 10, 1));

    render_section_header(&mut buf, 0, 10, 0, "Tools", &theme);

    assert_eq!(" Tools \u{2500}\u{2500}\u{2500}", row_text(&buf, 0));
    let look = |x| cell_look(&buf, x, 0);
    assert_eq!((theme.gray, theme.bg_base, Modifier::BOLD), look(1));
    assert_eq!((theme.gray_dim, theme.bg_base, Modifier::empty()), look(7));
}

fn plain_row(name: &str) -> ListRow<'_> {
    ListRow {
        expand: Some(false),
        status: RowStatus::Enabled,
        name,
        tags: &[],
        detail: None,
        is_selected: false,
    }
}

fn tag(text: &str, fg: Color, gap: u16) -> RowTag<'_> {
    RowTag {
        text,
        style: Style::default().fg(fg),
        gap,
    }
}

fn row_text(buf: &Buffer, y: u16) -> String {
    (0..buf.area.width)
        .filter_map(|x| buf.cell((x, y)))
        .map(|cell| cell.symbol())
        .collect()
}

fn cell_look(buf: &Buffer, x: u16, y: u16) -> (Color, Color, Modifier) {
    buf.cell((x, y))
        .map(|cell| (cell.fg, cell.bg, cell.modifier))
        .unwrap_or_default()
}
