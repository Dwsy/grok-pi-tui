//! The notice rows directly above the prompt, painted while the current model's ACP `meta` carries a `notice`.
//! It has no dismiss control and leaves only when the user switches to a model without a notice.
//! The text word-wraps under a hanging indent.
//! [`height`] must measure with the same wrap that [`render`] paints.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use xai_grok_shell::sampling::types::{ModelNotice, ModelNoticeSeverity};

use crate::glyphs::sanitize_toast_message;
use crate::render::line_utils::truncate_str;
use crate::theme::Theme;

/// Rows a notice may take. Only a very narrow terminal needs more.
pub const MAX_ROWS: u16 = 4;

const ALERT_PREFIX: &str = "! ";
const INFO_PREFIX: &str = "i ";
/// Continuation rows start under the text, past the prefix.
const HANGING_INDENT: &str = "  ";
const LABEL_SEPARATOR: &str = " · ";

/// Rows the notice needs at `width`, at most [`MAX_ROWS`].
pub fn height(notice: &ModelNotice, width: u16) -> u16 {
    let rows = wrap(notice, width).len();
    u16::try_from(rows).unwrap_or(MAX_ROWS).min(MAX_ROWS)
}

pub fn render(area: Rect, buf: &mut Buffer, notice: &ModelNotice) {
    let theme = Theme::current();
    let (fg, prefix) = match notice.severity {
        ModelNoticeSeverity::Info => (theme.text_secondary, INFO_PREFIX),
        ModelNoticeSeverity::Warning => (theme.warning, ALERT_PREFIX),
        ModelNoticeSeverity::Critical => (theme.accent_error, ALERT_PREFIX),
    };
    let body = Style::default().fg(fg).bg(theme.bg_base);
    buf.set_style(area, body);

    let rows = wrap(notice, area.width);
    let is_cut = rows.len() > usize::from(area.height);
    for (y, (index, row)) in (area.y..area.bottom()).zip(rows.iter().enumerate()) {
        let lead = if index == 0 { prefix } else { HANGING_INDENT };
        // The last row that fits carries the rest of the text, cut with an ellipsis
        let row = if is_cut && y + 1 == area.bottom() {
            let rest = rows.get(index..).unwrap_or_default().join(" ");
            truncate_str(&rest, body_width(area.width))
        } else {
            row.clone()
        };
        let line = Line::from(vec![
            Span::styled(lead, body.add_modifier(Modifier::BOLD)),
            Span::styled(row, body),
        ]);
        buf.set_line(area.x, y, &line, area.width);
    }
}

fn body_width(width: u16) -> usize {
    usize::from(width).saturating_sub(HANGING_INDENT.len())
}

fn wrap(notice: &ModelNotice, width: u16) -> Vec<String> {
    let width = body_width(width);
    if width == 0 {
        return Vec::new();
    }
    let text = match &notice.label {
        Some(label) => format!("{label}{LABEL_SEPARATOR}{}", notice.text),
        None => notice.text.clone(),
    };
    textwrap::wrap(&sanitize_toast_message(&text), width)
        .into_iter()
        .map(std::borrow::Cow::into_owned)
        .collect()
}

#[cfg(test)]
#[path = "model_notice_banner_tests.rs"]
mod tests;
