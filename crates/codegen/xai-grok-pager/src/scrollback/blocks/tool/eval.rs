//! EvalToolCallBlock - persistent-kernel code evaluation.

use ratatui::text::{Line, Span, Text};

use crate::appearance::AppearanceConfig;
use crate::render::wrapping::word_wrap_lines;
use crate::scrollback::block::BlockContent;
use crate::scrollback::types::{
    AccentStyle, BlockBackground, BlockContext, BlockLine, BlockOutput, DisplayMode,
};
use crate::theme::Theme;

#[derive(Debug, Clone)]
pub struct EvalToolCallBlock {
    pub language: String,
    pub code: String,
    pub title: Option<String>,
    pub bridge_version: Option<String>,
    pub output: Option<String>,
    pub error: Option<String>,
    pub started_at: Option<std::time::Instant>,
    pub elapsed_ms: Option<i64>,
}

impl EvalToolCallBlock {
    pub fn new(language: impl Into<String>, code: impl Into<String>) -> Self {
        Self {
            language: language.into(),
            code: code.into(),
            title: None,
            bridge_version: None,
            output: None,
            error: None,
            started_at: None,
            elapsed_ms: None,
        }
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        let title = title.into();
        self.title = (!title.trim().is_empty()).then_some(title);
        self
    }

    pub fn with_bridge_version(mut self, version: impl Into<String>) -> Self {
        let version = version.into();
        self.bridge_version = (!version.trim().is_empty()).then_some(version);
        self
    }

    pub fn effects_first(&self) -> bool {
        self.bridge_version.as_deref() == Some("v2")
            && crate::appearance::cache::load_pi_eval_v2_effects_first()
    }

    pub fn with_output(mut self, output: impl Into<String>) -> Self {
        let output = output.into();
        self.output = (!output.is_empty()).then_some(output);
        self
    }

    pub fn with_error(mut self, error: impl Into<String>) -> Self {
        self.error = Some(error.into());
        self
    }

    pub fn is_success(&self) -> bool {
        self.error.is_none()
    }

    pub fn finish(&mut self) {
        if self.elapsed_ms.is_none()
            && let Some(start) = self.started_at
        {
            self.elapsed_ms = Some(start.elapsed().as_millis() as i64);
        }
    }

    pub fn elapsed_ms(&self) -> Option<i64> {
        self.elapsed_ms.or_else(|| {
            self.started_at
                .map(|start| start.elapsed().as_millis() as i64)
        })
    }

    fn highlighted_code(&self, theme: &Theme) -> Vec<Line<'static>> {
        let syntect = crate::syntax::get_syntect();
        let mut highlighter = eval_syntax_token(&self.language)
            .and_then(|token| syntect.highlight_lines_for_token(token));
        let fallback = theme.fg(theme.md_code);
        let code = self.display_code();

        code.lines()
            .map(|line| {
                Line::from(crate::syntax::highlight_line(
                    line,
                    &mut highlighter,
                    syntect,
                    fallback,
                ))
            })
            .collect()
    }

    fn display_code(&self) -> String {
        let code = prepare_eval_display_text(&self.code);
        if crate::appearance::cache::load_pi_bash_command_format() {
            format_eval_code_for_display(&code)
        } else {
            code
        }
    }

    fn header_line(&self, theme: &Theme, muted: bool) -> Line<'static> {
        let style = if muted {
            theme.muted()
        } else {
            theme.primary()
        };
        let bold = style.add_modifier(ratatui::style::Modifier::BOLD);
        let mut spans = vec![Span::styled("Eval".to_string(), bold)];
        let label = self
            .title
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| (!self.language.trim().is_empty()).then_some(self.language.as_str()));
        if let Some(label) = label {
            spans.push(Span::styled(format!("  {label}"), style));
        }
        Line::from(spans)
    }

    fn render_body(&self, ctx: &BlockContext, include_output: bool) -> BlockOutput {
        let theme = Theme::current();
        let width = ctx.content_width().max(20);
        let effects_first = self.effects_first();
        let mut lines: Vec<BlockLine> = if effects_first {
            Vec::new()
        } else {
            vec![self.header_line(&theme, false).into()]
        };

        if !effects_first && !self.code.is_empty() {
            lines.push(Line::from("").into());
            for line in word_wrap_lines(self.highlighted_code(&theme), width) {
                lines.push(BlockLine::styled(line));
            }
        }

        if include_output {
            if let Some(output) = &self.output {
                if !lines.is_empty() {
                    lines.push(Line::from("").into());
                }
                let output_lines: Vec<Line<'static>> = output
                    .lines()
                    .map(|line| Line::from(Span::styled(line.to_string(), theme.muted())))
                    .collect();
                for line in word_wrap_lines(output_lines, width) {
                    lines.push(BlockLine::styled(line));
                }
            }
            if let Some(error) = &self.error {
                if !lines.is_empty() {
                    lines.push(Line::from("").into());
                }
                for line in error.lines() {
                    lines.push(BlockLine::styled(Line::from(Span::styled(
                        line.to_string(),
                        theme.fg(theme.accent_error),
                    ))));
                }
            }
        }

        BlockOutput { lines }
    }
}

fn prepare_eval_display_text(code: &str) -> String {
    let normalized = code.replace("\r\n", "\n").replace('\r', "\n");
    let mut out = String::with_capacity(normalized.len());
    for (i, line) in normalized.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(line.trim_end());
    }
    while out.ends_with('\n') {
        out.pop();
    }
    out
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EvalFormatMode {
    Normal,
    SingleQuoted,
    DoubleQuoted,
    Template,
    LineComment,
    BlockComment,
}

fn format_eval_code_for_display(code: &str) -> String {
    if code.contains('\n') || code.trim().is_empty() {
        return code.to_string();
    }

    let chars: Vec<char> = code.chars().collect();
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut mode = EvalFormatMode::Normal;
    let mut escaped = false;
    let mut indent = 0usize;
    let mut brace_depth = 0usize;
    let mut bracket_depth = 0usize;
    let mut skip_spaces = false;
    let mut i = 0usize;

    while i < chars.len() {
        let ch = chars[i];
        match mode {
            EvalFormatMode::Normal => {
                if skip_spaces && ch.is_whitespace() {
                    i += 1;
                    continue;
                }
                skip_spaces = false;

                if ch.is_whitespace() {
                    if !current.trim().is_empty() && !current.ends_with(' ') {
                        current.push(' ');
                    }
                } else if ch == '/' && chars.get(i + 1) == Some(&'/') {
                    push_eval_char(&mut current, indent, '/');
                    push_eval_char(&mut current, indent, '/');
                    mode = EvalFormatMode::LineComment;
                    i += 1;
                } else if ch == '/' && chars.get(i + 1) == Some(&'*') {
                    push_eval_char(&mut current, indent, '/');
                    push_eval_char(&mut current, indent, '*');
                    mode = EvalFormatMode::BlockComment;
                    i += 1;
                } else if ch == '\'' {
                    push_eval_char(&mut current, indent, ch);
                    mode = EvalFormatMode::SingleQuoted;
                    escaped = false;
                } else if ch == '"' {
                    push_eval_char(&mut current, indent, ch);
                    mode = EvalFormatMode::DoubleQuoted;
                    escaped = false;
                } else if ch == '`' {
                    push_eval_char(&mut current, indent, ch);
                    mode = EvalFormatMode::Template;
                    escaped = false;
                } else {
                    match ch {
                        '{' => {
                            push_eval_char(&mut current, indent, ch);
                            flush_eval_line(&mut lines, &mut current);
                            indent += 1;
                            brace_depth += 1;
                            skip_spaces = true;
                        }
                        '}' => {
                            flush_eval_line(&mut lines, &mut current);
                            indent = indent.saturating_sub(1);
                            brace_depth = brace_depth.saturating_sub(1);
                            push_eval_char(&mut current, indent, ch);
                        }
                        ';' => {
                            push_eval_char(&mut current, indent, ch);
                            flush_eval_line(&mut lines, &mut current);
                            skip_spaces = true;
                        }
                        ',' if brace_depth > 0 || bracket_depth > 0 => {
                            push_eval_char(&mut current, indent, ch);
                            flush_eval_line(&mut lines, &mut current);
                            skip_spaces = true;
                        }
                        '[' => {
                            bracket_depth += 1;
                            push_eval_char(&mut current, indent, ch);
                        }
                        ']' => {
                            bracket_depth = bracket_depth.saturating_sub(1);
                            push_eval_char(&mut current, indent, ch);
                        }
                        _ => push_eval_char(&mut current, indent, ch),
                    }
                }
            }
            EvalFormatMode::SingleQuoted => {
                push_eval_char(&mut current, indent, ch);
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '\'' {
                    mode = EvalFormatMode::Normal;
                }
            }
            EvalFormatMode::DoubleQuoted => {
                push_eval_char(&mut current, indent, ch);
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    mode = EvalFormatMode::Normal;
                }
            }
            EvalFormatMode::Template => {
                push_eval_char(&mut current, indent, ch);
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '`' {
                    mode = EvalFormatMode::Normal;
                }
            }
            EvalFormatMode::LineComment => {
                push_eval_char(&mut current, indent, ch);
            }
            EvalFormatMode::BlockComment => {
                push_eval_char(&mut current, indent, ch);
                if ch == '*' && chars.get(i + 1) == Some(&'/') {
                    push_eval_char(&mut current, indent, '/');
                    mode = EvalFormatMode::Normal;
                    i += 1;
                }
            }
        }
        i += 1;
    }

    flush_eval_line(&mut lines, &mut current);
    if lines.len() <= 1 {
        code.to_string()
    } else {
        lines.join("\n")
    }
}

fn push_eval_char(line: &mut String, indent: usize, ch: char) {
    if line.is_empty() {
        if ch.is_whitespace() {
            return;
        }
        line.extend(std::iter::repeat(' ').take(indent.saturating_mul(2)));
    }
    line.push(ch);
}

fn flush_eval_line(lines: &mut Vec<String>, line: &mut String) {
    let trimmed = line.trim_end();
    if !trimmed.trim().is_empty() {
        lines.push(trimmed.to_string());
    }
    line.clear();
}

fn eval_syntax_token(language: &str) -> Option<&'static str> {
    let language = language.trim();
    if language.eq_ignore_ascii_case("py") || language.eq_ignore_ascii_case("python") {
        return Some("python");
    }
    if language.eq_ignore_ascii_case("js")
        || language.eq_ignore_ascii_case("javascript")
        || language.eq_ignore_ascii_case("node")
        || language.eq_ignore_ascii_case("nodejs")
    {
        return Some("javascript");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> BlockContext {
        BlockContext {
            width: 80,
            mode: DisplayMode::Expanded,
            is_running: false,
            raw: false,
            max_lines: None,
            appearance: Default::default(),
            is_selected: false,
            cwd: None,
        }
    }

    #[test]
    fn eval_display_format_breaks_statement_chain() {
        assert_eq!(
            format_eval_code_for_display("const x = 1; const y = 2;"),
            "const x = 1;\nconst y = 2;"
        );
    }

    #[test]
    fn eval_display_format_preserves_quoted_delimiters() {
        assert_eq!(
            format_eval_code_for_display("const s = '{;,'; console.log(s);"),
            "const s = '{;,';\nconsole.log(s);"
        );
    }

    #[test]
    fn eval_display_format_indents_object_literals() {
        assert_eq!(
            format_eval_code_for_display("const x = {a: 1, b: 2}; console.log(x);"),
            "const x = {\n  a: 1,\n  b: 2\n};\nconsole.log(x);"
        );
    }

    #[test]
    fn expanded_code_preserves_python_syntax_token_spans() {
        let block = EvalToolCallBlock::new("py", "def add(x):\n    return x + 1");
        let output = block.output(&ctx());
        let code_spans: usize = output.lines[2..4]
            .iter()
            .map(|line| line.content.spans.len())
            .sum();
        assert!(
            code_spans > 2,
            "Python code should contain multiple syntax spans, got {code_spans}"
        );
    }

    #[test]
    fn expanded_code_preserves_javascript_syntax_token_spans() {
        let block = EvalToolCallBlock::new(
            "js",
            "const values = [1, 2, 3];\nvalues.reduce((a, b) => a + b, 0);",
        );
        let output = block.output(&ctx());
        let code_spans: usize = output.lines[2..4]
            .iter()
            .map(|line| line.content.spans.len())
            .sum();
        assert!(
            code_spans > 2,
            "JavaScript code should contain multiple syntax spans, got {code_spans}"
        );
    }

    #[test]
    fn v2_effects_first_hides_source_but_keeps_output() {
        crate::appearance::cache::set_pi_eval_v2_effects_first(true);
        let block = EvalToolCallBlock::new("js", "await tool.read({ path: 'secret' })")
            .with_bridge_version("v2")
            .with_output("done");
        let output = block.output(&ctx());
        let text = output
            .lines
            .iter()
            .map(|line| line.content.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!text.contains("tool.read"));
        assert!(text.contains("done"));
    }

    #[test]
    fn v1_keeps_source_even_when_v2_effects_first_is_enabled() {
        crate::appearance::cache::set_pi_eval_v2_effects_first(true);
        let block = EvalToolCallBlock::new("js", "1 + 1").with_bridge_version("v1");
        let output = block.output(&ctx());
        let text = output
            .lines
            .iter()
            .map(|line| line.content.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("1 + 1"));
    }
}

impl BlockContent for EvalToolCallBlock {
    fn output(&self, ctx: &BlockContext) -> BlockOutput {
        if self.effects_first() {
            return self.render_body(ctx, true);
        }
        let theme = Theme::current();
        match ctx.mode {
            DisplayMode::Collapsed => BlockOutput {
                lines: vec![
                    self.header_line(
                        &theme,
                        ctx.mute_when_collapsed(
                            ctx.appearance.scrollback.blocks.tool.muted_collapsed,
                        ),
                    )
                    .into(),
                ],
            },
            DisplayMode::Truncated | DisplayMode::Expanded => self.render_body(ctx, true),
        }
    }

    fn accent(&self, ctx: &BlockContext) -> Option<AccentStyle> {
        if ctx.mode == DisplayMode::Collapsed {
            return None;
        }
        let theme = Theme::current();
        if self.error.is_some() {
            Some(AccentStyle::static_color(theme.accent_error))
        } else if ctx.is_running {
            Some(AccentStyle::animated(theme.accent_running))
        } else {
            Some(AccentStyle::static_color(theme.accent_tool))
        }
    }

    fn bullet(&self, ctx: &BlockContext) -> Option<AccentStyle> {
        if self.error.is_some() {
            Some(AccentStyle::static_color(Theme::current().accent_error))
        } else if ctx.mode == DisplayMode::Collapsed {
            None
        } else {
            self.accent(ctx)
        }
    }

    fn has_vpad_for(&self, _appearance: &AppearanceConfig) -> bool {
        false
    }

    fn background(&self, _ctx: &BlockContext) -> BlockBackground {
        BlockBackground::None
    }

    fn is_foldable(&self) -> bool {
        !self.effects_first()
            && (!self.code.is_empty() || self.output.is_some() || self.error.is_some())
    }

    fn default_display_mode(&self) -> DisplayMode {
        DisplayMode::Collapsed
    }

    fn next_fold_mode(&self, current: DisplayMode, _is_running: bool) -> DisplayMode {
        match current {
            DisplayMode::Collapsed => DisplayMode::Expanded,
            DisplayMode::Truncated | DisplayMode::Expanded => DisplayMode::Collapsed,
        }
    }

    fn preamble(&self, _ctx: &BlockContext) -> Option<Text<'static>> {
        if self.effects_first() {
            None
        } else {
            Some(Text::from(vec![self.header_line(&Theme::current(), false)]))
        }
    }
}
