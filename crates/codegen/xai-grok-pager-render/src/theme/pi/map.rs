//! Map resolved Pi theme tokens onto the Grok [`Theme`] struct.

use ratatui::style::{Color, Modifier};

use super::color::{
    ColorError, blend, color_to_rgb, or_fallback, relative_luminance, resolve_color,
    shift_luminance,
};
use super::schema::{ColorValue, PiThemeJson};
use crate::theme::Theme;

/// Errors while mapping a Pi theme to Grok colors.
#[derive(Debug)]
pub enum MapError {
    Color(String, ColorError),
}

impl std::fmt::Display for MapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Color(token, e) => write!(f, "token `{token}`: {e}"),
        }
    }
}

impl std::error::Error for MapError {}

/// Convert a validated Pi theme JSON document into a Grok [`Theme`].
pub fn map_pi_theme(doc: &PiThemeJson) -> Result<Theme, MapError> {
    let vars = &doc.vars;
    let c = &doc.colors;

    let res = |token: &str, v: &ColorValue| -> Result<Color, MapError> {
        resolve_color(v, vars).map_err(|e| MapError::Color(token.to_string(), e))
    };

    let accent = res("accent", &c.accent)?;
    let border = res("border", &c.border)?;
    let border_accent = res("borderAccent", &c.border_accent)?;
    let border_muted = res("borderMuted", &c.border_muted)?;
    let success = res("success", &c.success)?;
    let error = res("error", &c.error)?;
    let warning = res("warning", &c.warning)?;
    let muted = res("muted", &c.muted)?;
    let dim = res("dim", &c.dim)?;
    let text = res("text", &c.text)?;
    let thinking_text = res("thinkingText", &c.thinking_text)?;

    let selected_bg = res("selectedBg", &c.selected_bg)?;
    let user_message_bg = res("userMessageBg", &c.user_message_bg)?;
    let user_message_text = res("userMessageText", &c.user_message_text)?;
    let custom_message_label = res("customMessageLabel", &c.custom_message_label)?;
    let tool_pending_bg = res("toolPendingBg", &c.tool_pending_bg)?;
    let tool_success_bg = res("toolSuccessBg", &c.tool_success_bg)?;
    let tool_error_bg = res("toolErrorBg", &c.tool_error_bg)?;
    let tool_title = res("toolTitle", &c.tool_title)?;
    let _tool_output = res("toolOutput", &c.tool_output)?;

    let md_heading = res("mdHeading", &c.md_heading)?;
    let md_link = res("mdLink", &c.md_link)?;
    let _md_link_url = res("mdLinkUrl", &c.md_link_url)?;
    let md_code = res("mdCode", &c.md_code)?;
    let md_code_block = res("mdCodeBlock", &c.md_code_block)?;
    let _md_code_block_border = res("mdCodeBlockBorder", &c.md_code_block_border)?;
    let md_quote = res("mdQuote", &c.md_quote)?;
    let md_list_bullet = res("mdListBullet", &c.md_list_bullet)?;

    let diff_added = res("toolDiffAdded", &c.tool_diff_added)?;
    let diff_removed = res("toolDiffRemoved", &c.tool_diff_removed)?;
    let diff_context = res("toolDiffContext", &c.tool_diff_context)?;

    let bash_mode = res("bashMode", &c.bash_mode)?;

    // Optional export.pageBg drives canvas when present.
    let export_page_bg = doc
        .export
        .as_ref()
        .and_then(|e| e.page_bg.as_ref())
        .map(|v| res("export.pageBg", v))
        .transpose()?;

    let text_primary = or_fallback(text, Color::Rgb(0xd4, 0xd4, 0xd4));
    let text_secondary = or_fallback(muted, Color::Rgb(0x80, 0x80, 0x80));
    let gray = text_secondary;
    let gray_dim = or_fallback(dim, Color::Rgb(0x66, 0x66, 0x66));
    let gray_bright = blend(muted, text_primary, 0.45);

    let bg_base = derive_canvas(
        export_page_bg,
        user_message_bg,
        tool_pending_bg,
        selected_bg,
    );
    let dark = is_dark_rgb(bg_base);
    let elev = if dark { 10 } else { -10 };
    let elev2 = if dark { 18 } else { -18 };

    let bg_dark = shift_luminance(bg_base, elev);
    let bg_terminal = shift_luminance(bg_base, elev / 2);
    let bg_light = or_fallback(user_message_bg, shift_luminance(bg_base, elev2));
    let bg_highlight = or_fallback(selected_bg, shift_luminance(bg_base, elev2));
    let bg_hover = blend(bg_highlight, bg_base, 0.35);
    let bg_visual = blend(selected_bg, border, 0.25);

    // In transparent themes (bg_base == Reset) code blocks should not paint
    // an opaque surface — keep them transparent like the page canvas.
    let md_code_bg = if bg_base == Color::Reset {
        Color::Reset
    } else {
        or_fallback(tool_pending_bg, bg_dark)
    };
    let paste_bg = bg_dark;
    let paste_fg = or_fallback(user_message_text, text_secondary);
    let paste_dim = gray_dim;

    let heading_mod = Modifier::BOLD;

    Ok(Theme {
        bg_base,
        bg_light,
        bg_dark,
        bg_highlight,
        bg_hover,
        bg_terminal,

        accent_user: accent,
        accent_assistant: border_accent,
        accent_thinking: thinking_text,
        accent_tool: or_fallback(tool_title, gray_bright),
        accent_system: border,
        accent_error: error,
        accent_success: success,
        accent_running: border_accent,
        accent_skill: custom_message_label,

        text_primary,
        text_secondary,

        gray_dim,
        gray,
        gray_bright,

        command: bash_mode,
        path: md_link,
        running: border_accent,
        warning,

        fuzzy_accent: accent,

        accent_plan: warning,
        accent_verify: custom_message_label,
        accent_remember: blend(success, accent, 0.4),

        selection_border: border_accent,
        hover_border: border_muted,
        prompt_border: border,
        prompt_border_active: border_accent,

        accent_model: accent,

        scrollbar_bg: bg_dark,
        scrollbar_fg: bg_highlight,

        diff_delete_bg: or_fallback(
            tool_error_bg,
            shift_luminance(error, if dark { -80 } else { 80 }),
        ),
        diff_delete_fg: diff_removed,
        diff_insert_bg: or_fallback(
            tool_success_bg,
            shift_luminance(success, if dark { -80 } else { 80 }),
        ),
        diff_insert_fg: diff_added,
        diff_equal_fg: diff_context,
        diff_gutter_fg: gray_dim,

        bg_visual,

        paste_bg,
        paste_fg,
        paste_dim,

        md_heading_h1: md_heading,
        md_heading_h1_mod: heading_mod,
        md_heading_h2: blend(md_heading, accent, 0.35),
        md_heading_h2_mod: heading_mod,
        md_heading_h3: blend(md_heading, warning, 0.4),
        md_heading_h3_mod: heading_mod,
        md_heading_h4: md_list_bullet,
        md_heading_h4_mod: heading_mod,
        md_heading_h5: md_code,
        md_heading_h5_mod: heading_mod,
        md_heading_h6: custom_message_label,
        md_heading_h6_mod: heading_mod,
        md_code: or_fallback(md_code, md_code_block),
        md_task_checked: success,
        md_task_unchecked: border,
        md_muted: or_fallback(md_quote, gray),
        md_code_bg,
        md_text: text_primary,
        link_fg: md_link,
    })
}

fn is_dark_rgb(c: Color) -> bool {
    match color_to_rgb(c) {
        Some((r, g, b)) => relative_luminance(r, g, b) < 0.45,
        None => true,
    }
}

/// Derive a readable canvas color when Pi does not define a global bg.
fn derive_canvas(
    export_page_bg: Option<Color>,
    user_message_bg: Color,
    tool_pending_bg: Color,
    selected_bg: Color,
) -> Color {
    if let Some(page) = export_page_bg {
        return page;
    }

    // Prefer tool pending / selected as structural surface samples.
    let samples: [Color; 3] = [tool_pending_bg, user_message_bg, selected_bg];
    let mut dark_count = 0;
    let mut light_count = 0;
    let mut darkest = Color::Rgb(0x18, 0x18, 0x1e);
    let mut darkest_luma = f32::MAX;
    let mut lightest = Color::Rgb(0xf8, 0xf8, 0xf8);
    let mut lightest_luma = f32::MIN;

    for s in samples {
        if let Some((r, g, b)) = color_to_rgb(s) {
            let l = relative_luminance(r, g, b);
            if l < 0.45 {
                dark_count += 1;
            } else {
                light_count += 1;
            }
            if l < darkest_luma {
                darkest_luma = l;
                darkest = Color::Rgb(r, g, b);
            }
            if l > lightest_luma {
                lightest_luma = l;
                lightest = Color::Rgb(r, g, b);
            }
        }
    }

    if dark_count >= light_count {
        // Dark theme: canvas slightly darker than message surfaces.
        shift_luminance(darkest, -12)
    } else {
        // Light theme: canvas slightly lighter / cleaner than surfaces.
        shift_luminance(lightest, 8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::pi::load::load_from_str;

    #[test]
    fn maps_embedded_dark() {
        let doc = load_from_str(include_str!("../../../assets/pi-themes/dark.json")).unwrap();
        let theme = map_pi_theme(&doc).unwrap();
        assert!(theme.is_dark());
        assert!(matches!(theme.accent_user, Color::Rgb(_, _, _)));
        assert!(matches!(theme.bg_base, Color::Rgb(_, _, _)));
        assert_eq!(theme.text_primary, Color::Rgb(0xd4, 0xd4, 0xd4));
    }

    #[test]
    fn maps_embedded_light() {
        let doc = load_from_str(include_str!("../../../assets/pi-themes/light.json")).unwrap();
        let theme = map_pi_theme(&doc).unwrap();
        assert!(!theme.is_dark());
        assert!(matches!(theme.bg_base, Color::Rgb(_, _, _)));
    }

    fn assert_transparent_canvas(theme: Theme) {
        assert_eq!(theme.bg_base, Color::Reset);
        assert_eq!(theme.bg_dark, Color::Reset);
        assert_eq!(theme.bg_light, Color::Reset);
        assert_ne!(theme.bg_highlight, Color::Reset);
        // Code blocks are also transparent in transparent themes.
        assert_eq!(theme.md_code_bg, Color::Reset);
        assert_ne!(theme.diff_insert_bg, Color::Reset);
    }

    #[test]
    fn maps_transparent_dark_page_background_to_terminal_default() {
        let doc =
            load_from_str(include_str!("../../../assets/pi-themes/transparent.json")).unwrap();
        assert_transparent_canvas(map_pi_theme(&doc).unwrap());
    }

    #[test]
    fn maps_transparent_light_page_background_to_terminal_default() {
        let doc = load_from_str(include_str!(
            "../../../assets/pi-themes/transparent-light.json"
        ))
        .unwrap();
        assert_transparent_canvas(map_pi_theme(&doc).unwrap());
    }
}
