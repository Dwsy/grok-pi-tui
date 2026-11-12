//! Ondine's native Mermaid-to-SVG compiler and standalone SVG theme.

use std::panic::AssertUnwindSafe;

use crate::{
    MermaidEngine, MermaidError, MermaidTheme, RenderLimits, RenderParams, RenderedDiagram, Rgba,
};

/// Offline Ondine renderer used by the image actions.
#[derive(Debug, Default, Clone, Copy)]
pub struct OndineEngine;

impl MermaidEngine for OndineEngine {
    fn render(&self, source: &str, params: &RenderParams) -> Result<RenderedDiagram, MermaidError> {
        let svg = build_svg(source, params.theme)?;
        crate::raster::rasterize_ondine(&svg, params)
    }
}

/// Compile a standalone SVG with the same source limit and panic boundary as PNG rendering.
/// The pager also runs this in its isolated, time-limited render subprocess.
pub fn render_svg_checked(
    source: &str,
    theme: MermaidTheme,
    limits: &RenderLimits,
) -> Result<String, MermaidError> {
    if source.len() > limits.max_source_bytes {
        return Err(MermaidError::Unsupported(format!(
            "source is {} bytes, over the {}-byte limit",
            source.len(),
            limits.max_source_bytes
        )));
    }
    std::panic::catch_unwind(AssertUnwindSafe(|| build_svg(source, theme)))
        .unwrap_or_else(|_| Err(MermaidError::Panic("Ondine panicked".into())))
}

fn build_svg(source: &str, theme: MermaidTheme) -> Result<String, MermaidError> {
    // No external resources or links in an image generated from untrusted model output.
    // Fixed dimensions are required by standalone SVG viewers and resvg.
    let rendered = ondine_compiler::render(
        source,
        r#"{"responsive":false,"images":false,"links":false}"#,
        0.0,
    )
    .map_err(|error| match error.code {
        ondine_compiler::Code::UnsupportedDiagram | ondine_compiler::Code::Limit => {
            MermaidError::Unsupported(error.message)
        }
        ondine_compiler::Code::Syntax | ondine_compiler::Code::Semantic => {
            MermaidError::Parse(error.message)
        }
    })?;
    Ok(standalone_svg(&rendered.svg, theme))
}

/// Ondine emits CSS custom properties and color-mix(), which SVG rasterizers and
/// native viewers do not consistently support. Resolve its known theme variables
/// in presentation attributes only, leaving diagram text and data attributes intact.
fn standalone_svg(svg: &str, theme: MermaidTheme) -> String {
    let bg = theme.surface_background();
    let fg = match theme {
        MermaidTheme::Light => Rgba::new(0x18, 0x18, 0x1b, 0xff),
        MermaidTheme::Dark => Rgba::new(0xfa, 0xfa, 0xfa, 0xff),
    };
    let mut result = String::with_capacity(svg.len() + 100);
    let mut pos = 0;
    let mut in_tag = false;
    let bytes = svg.as_bytes();
    while pos < bytes.len() {
        match bytes[pos] {
            b'<' => {
                in_tag = true;
                result.push('<');
                pos += 1;
            }
            b'>' => {
                in_tag = false;
                result.push('>');
                pos += 1;
            }
            b'"' if in_tag => {
                let start = pos + 1;
                let Some(end_rel) = svg[start..].find('"') else {
                    result.push_str(&svg[pos..]);
                    break;
                };
                let end = start + end_rel;
                let prefix =
                    svg[..pos].trim_end_matches(|c: char| c.is_ascii_whitespace() || c == '=');
                let attr = prefix
                    .rsplit(|c: char| c.is_ascii_whitespace() || c == '<')
                    .next()
                    .unwrap_or("");
                let value = &svg[start..end];
                result.push('"');
                if is_presentation_attribute(attr) {
                    let mut resolved = resolve_variables(value, fg, bg);
                    // usvg does not depend on CSS font shorthand support.
                    if attr == "style" {
                        if let Some(size) = resolved
                            .split(';')
                            .find_map(|part| part.trim().strip_prefix("font:"))
                            .and_then(|font| font.split_whitespace().next())
                        {
                            let size = size.to_owned();
                            resolved.push_str(";font-size:");
                            resolved.push_str(&size);
                        }
                    }
                    result.push_str(&resolved);
                } else {
                    result.push_str(value);
                }
                result.push('"');
                pos = end + 1;
            }
            _ => {
                let ch = svg[pos..].chars().next().expect("valid UTF-8");
                result.push(ch);
                pos += ch.len_utf8();
            }
        }
    }

    // Native image viewers often use a white canvas. Bake the theme's surface
    // into SVG too, otherwise dark diagrams would have invisible light strokes.
    if let Some(end) = result.find('>') {
        result.insert_str(
            end + 1,
            &format!(
                r#"<rect width="100%" height="100%" fill="{}" stroke="none"/>"#,
                bg.to_hex()
            ),
        );
    }
    result
}

fn is_presentation_attribute(attr: &str) -> bool {
    matches!(
        attr,
        "style"
            | "fill"
            | "stroke"
            | "color"
            | "font-family"
            | "font-weight"
            | "font-size"
            | "stroke-width"
            | "rx"
            | "ry"
            | "opacity"
    )
}

fn mix(fg: Rgba, bg: Rgba, percent: u32) -> String {
    let channel =
        |f: u8, b: u8| ((u32::from(f) * percent + u32::from(b) * (100 - percent) + 50) / 100) as u8;
    Rgba::new(
        channel(fg.r, bg.r),
        channel(fg.g, bg.g),
        channel(fg.b, bg.b),
        255,
    )
    .to_hex()
}

fn theme_variable(name: &str, fg: Rgba, bg: Rgba) -> Option<String> {
    let color = match name {
        "--_bg" | "--_s" | "--ondine-bg" | "--ondine-surface" => Some(bg.to_hex()),
        "--_fg" | "--_m" | "--_l" | "--_b" | "--_c" | "--ondine-fg" | "--ondine-muted"
        | "--ondine-line" | "--ondine-border" | "--ondine-accent" | "--ondine-critical" => {
            Some(fg.to_hex())
        }
        "--_a" | "--ondine-surface-alt" => Some(mix(fg, bg, 20)),
        "--_sw" | "--ondine-stroke" => Some("2".into()),
        "--_r" | "--ondine-radius" => Some("0".into()),
        "--_w"
        | "--_tw"
        | "--_hw"
        | "--ondine-bold-weight"
        | "--ondine-title-weight"
        | "--ondine-heading-weight" => Some("bold".into()),
        "--_fm" | "--ondine-font" => Some("monospace".into()),
        _ => None,
    };
    color.or_else(|| {
        let index = name
            .strip_prefix("--ondine-series-")?
            .parse::<usize>()
            .ok()?;
        let shade = [100, 57, 86, 50, 79, 64, 93, 71].get(index.checked_sub(1)?)?;
        Some(mix(fg, bg, *shade))
    })
}

fn resolve_variables(value: &str, fg: Rgba, bg: Rgba) -> String {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find("var(") {
        out.push_str(&rest[..start]);
        let body = &rest[start + 4..];
        let mut depth = 1usize;
        let mut end = None;
        for (i, ch) in body.char_indices() {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(i);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(end) = end else {
            out.push_str(&rest[start..]);
            return out;
        };
        let expression = &body[..end];
        let name = expression.split(',').next().unwrap_or("").trim();
        if let Some(replacement) = theme_variable(name, fg, bg) {
            out.push_str(&replacement);
        } else {
            // Preserve unknown author-supplied variables and their fallbacks.
            out.push_str(&rest[start..start + 4 + end + 1]);
        }
        rest = &body[end + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standalone_svg_resolves_theme_without_changing_labels() {
        let input = r#"<svg style="font:16px monospace;color:var(--_fg)" fill="var(--_fg)"><text data-id="var(--_fg)">var(--_fg)</text><rect fill="var(--ondine-series-2,color-mix(in srgb,var(--_fg) 57%,var(--_bg)))"/></svg>"#;
        let svg = standalone_svg(input, MermaidTheme::Dark);
        assert!(svg.contains("font-size:16px"));
        assert!(svg.contains(r##"fill="#FAFAFA""##));
        assert!(svg.contains(r#"data-id="var(--_fg)">var(--_fg)</text>"#));
        assert!(!svg.contains("color-mix("));
        assert!(svg.contains(r##"<rect width="100%" height="100%" fill="#18181B""##));
    }

    #[test]
    fn ondine_svg_and_png_use_the_same_diagram() {
        let source = "flowchart LR\n A[Start] --> B[Finish]";
        let svg =
            render_svg_checked(source, MermaidTheme::Light, &RenderLimits::default()).unwrap();
        assert!(svg.contains("ondine-flowchart"));
        assert!(svg.contains("Start") && svg.contains("Finish"));
        assert!(!svg.contains("var(--_"));
        let rendered = OndineEngine
            .render(source, &RenderParams::default())
            .unwrap();
        assert!(rendered.png.starts_with(b"\x89PNG\r\n\x1a\n"));
    }
}
