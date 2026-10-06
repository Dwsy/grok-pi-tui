//! CodemodeToolCallBlock - Pi codemode script orchestration.
//!
//! Mirrors Pi's own presentation (`extensions/codemode/renderer.ts`): the call
//! shows the script; the result lists the nested tool calls with their status as
//! they run plus the cost of the script's model calls, followed by the script
//! output without the `Script completed` header. Nested calls are not separate
//! tool rows because they never reach the model as tool calls.

use ratatui::text::{Line, Span, Text};
use serde_json::Value;

use crate::appearance::AppearanceConfig;
use crate::render::wrapping::word_wrap_lines;
use crate::scrollback::block::BlockContent;
use crate::scrollback::types::{
    AccentStyle, BlockBackground, BlockContext, BlockLine, BlockOutput, DisplayMode,
};
use crate::theme::Theme;

/// Script source lines shown before the expand hint in the preview (Truncated) mode.
const CODE_PREVIEW_LINES: usize = 10;
/// Nested call rows shown in the preview (Truncated) mode; earlier rows hide.
const CALL_PREVIEW_COUNT: usize = 8;
/// Script output lines shown in the preview (Truncated) mode.
const OUTPUT_PREVIEW_LINES: usize = 5;
/// Nested call args shown per row before truncation outside Expanded mode.
const COLLAPSED_ARGS_CHARS: usize = 80;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodemodeCallStatus {
    Running,
    Ok,
    Error,
    Cancelled,
}

impl CodemodeCallStatus {
    fn parse(value: Option<&str>) -> Self {
        match value {
            Some("ok") => Self::Ok,
            Some("error") => Self::Error,
            Some("cancelled") => Self::Cancelled,
            _ => Self::Running,
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            Self::Running => "…",
            Self::Ok => "✓",
            Self::Error => "✗",
            Self::Cancelled => "⊘",
        }
    }
}

/// One nested call row of a codemode script (Pi `CodemodeNestedCall`).
#[derive(Debug, Clone)]
pub struct CodemodeCallRow {
    pub name: String,
    pub args: String,
    pub status: CodemodeCallStatus,
    pub duration_ms: Option<u64>,
    pub error: Option<String>,
    pub cost: Option<f64>,
}

impl CodemodeCallRow {
    fn from_value(value: &Value) -> Option<Self> {
        let name = value.get("name").and_then(Value::as_str)?.to_string();
        let args = value
            .get("args")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let status = CodemodeCallStatus::parse(value.get("status").and_then(Value::as_str));
        // Pi reports `performance.now()` deltas, so `durationMs` is fractional
        // (e.g. 12.942792…); an integer-only parse would drop every row.
        let duration_ms = value
            .get("durationMs")
            .and_then(Value::as_f64)
            .map(|ms| ms.round() as u64);
        let error = value
            .get("error")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(str::to_string);
        let cost = value.get("cost").and_then(Value::as_f64);
        Some(Self {
            name,
            args,
            status,
            duration_ms,
            error,
            cost,
        })
    }
}

#[derive(Debug, Clone)]
pub struct CodemodeToolCallBlock {
    pub code: String,
    pub calls: Vec<CodemodeCallRow>,
    /// Script output with the `Script completed` header already stripped.
    pub output: Option<String>,
    /// Temp file with the full text output, when the output was truncated.
    pub full_output_path: Option<String>,
    pub error: Option<String>,
    pub started_at: Option<std::time::Instant>,
    pub elapsed_ms: Option<i64>,
}

impl CodemodeToolCallBlock {
    pub fn new(code: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            calls: Vec::new(),
            output: None,
            full_output_path: None,
            error: None,
            started_at: None,
            elapsed_ms: None,
        }
    }

    /// Parse the adapter's canonical `{"type":"Codemode", calls, output, full_output_path}`
    /// raw output. Missing/malformed payloads degrade to an empty projection.
    pub fn with_raw_output(mut self, raw: Option<&Value>) -> Self {
        let Some(raw) = raw else {
            return self;
        };
        if raw.get("type").and_then(Value::as_str) != Some("Codemode") {
            return self;
        }
        if let Some(calls) = raw.get("calls").and_then(Value::as_array) {
            self.calls = calls
                .iter()
                .filter_map(CodemodeCallRow::from_value)
                .collect();
        }
        if let Some(output) = raw.get("output").and_then(Value::as_str)
            && !output.is_empty()
        {
            self.output = Some(output.to_string());
        }
        if let Some(path) = raw.get("full_output_path").and_then(Value::as_str)
            && !path.is_empty()
        {
            self.full_output_path = Some(path.to_string());
        }
        self
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

    fn expanded(ctx: &BlockContext) -> bool {
        ctx.mode == DisplayMode::Expanded
    }

    fn highlighted_code(&self, theme: &Theme) -> Vec<Line<'static>> {
        let syntect = crate::syntax::get_syntect();
        let mut highlighter = syntect.highlight_lines_for_token("javascript");
        let fallback = theme.fg(theme.md_code);
        let code = self.code.replace("\r\n", "\n").replace('\r', "\n");

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

    fn header_line(&self, theme: &Theme, muted: bool) -> Line<'static> {
        let style = if muted {
            theme.muted()
        } else {
            theme.primary()
        };
        let bold = style.add_modifier(ratatui::style::Modifier::BOLD);
        let mut spans = vec![Span::styled("Codemode".to_string(), bold)];

        let mut summary = Vec::new();
        if !self.calls.is_empty() {
            summary.push(format!(
                "{} call{}",
                self.calls.len(),
                if self.calls.len() == 1 { "" } else { "s" }
            ));
        }
        let total_cost = self.total_model_cost();
        if let Some(cost) = total_cost {
            summary.push(format_cost(cost));
        }
        if let Some(ms) = self.elapsed_ms() {
            summary.push(format_duration(ms.max(0) as u64));
        }
        if !summary.is_empty() {
            spans.push(Span::styled(format!("  {}", summary.join(" · ")), style));
        }
        Line::from(spans)
    }

    /// Sum of the nested `models.*` call costs, when the script reported usage.
    fn total_model_cost(&self) -> Option<f64> {
        let priced = self.calls.iter().filter_map(|call| call.cost).sum::<f64>();
        (priced > 0.0).then_some(priced)
    }

    fn call_row(
        &self,
        theme: &Theme,
        call: &CodemodeCallRow,
        expanded: bool,
    ) -> Vec<Line<'static>> {
        let args = if !expanded && call.args.chars().count() > COLLAPSED_ARGS_CHARS {
            let cut: String = call.args.chars().take(COLLAPSED_ARGS_CHARS - 3).collect();
            format!("{cut}...")
        } else {
            call.args.clone()
        };
        let mut spans = vec![Span::styled(
            call.status.glyph().to_string(),
            status_style(call.status, theme),
        )];
        spans.push(Span::styled(
            format!(" {}", call.name),
            theme.fg(theme.accent_tool),
        ));
        if !args.is_empty() {
            spans.push(Span::styled(format!(" {args}"), theme.muted()));
        }
        if let Some(ms) = call.duration_ms {
            spans.push(Span::styled(
                format!(" {}", format_duration(ms)),
                theme.muted(),
            ));
        }
        if let Some(cost) = call.cost {
            spans.push(Span::styled(
                format!(" {}", format_cost(cost)),
                theme.muted(),
            ));
        }
        let mut lines = vec![Line::from(spans)];
        if expanded && let Some(error) = &call.error {
            for line in error.lines() {
                lines.push(Line::from(Span::styled(
                    format!("    {line}"),
                    theme.fg(theme.accent_error),
                )));
            }
        }
        lines
    }

    fn calls_section(&self, theme: &Theme, expanded: bool) -> Vec<Line<'static>> {
        if self.calls.is_empty() {
            return Vec::new();
        }
        let (shown, hidden_label): (Vec<&CodemodeCallRow>, Option<usize>) = if expanded {
            (self.calls.iter().collect(), None)
        } else {
            let start = self.calls.len().saturating_sub(CALL_PREVIEW_COUNT);
            let hidden = start;
            (
                self.calls[start..].iter().collect(),
                (hidden > 0).then_some(hidden),
            )
        };
        let mut lines = Vec::new();
        if let Some(hidden) = hidden_label {
            let noun = if hidden == 1 { "call" } else { "calls" };
            lines.push(Line::from(Span::styled(
                format!("... ({hidden} earlier {noun}, press Enter to view)"),
                theme.muted(),
            )));
        }
        for call in shown {
            lines.extend(self.call_row(theme, call, expanded));
        }
        if let Some(cost) = self.total_model_cost()
            && self.calls.iter().filter(|call| call.cost.is_some()).count() > 1
        {
            lines.push(Line::from(Span::styled(
                format!("Model calls: {}", format_cost(cost)),
                theme.muted(),
            )));
        }
        lines
    }

    fn output_section(&self, theme: &Theme, expanded: bool) -> Vec<Line<'static>> {
        let Some(output) = self.output.as_deref().filter(|text| !text.is_empty()) else {
            return Vec::new();
        };
        let style = if self.error.is_some() {
            theme.fg(theme.accent_error)
        } else {
            theme.muted()
        };
        let lines: Vec<Line<'static>> = output
            .lines()
            .map(|line| Line::from(Span::styled(line.to_string(), style)))
            .collect();
        let mut shown = lines;
        let mut more = None;
        if !expanded && shown.len() > OUTPUT_PREVIEW_LINES {
            more = Some(shown.len() - OUTPUT_PREVIEW_LINES);
            shown = shown[..OUTPUT_PREVIEW_LINES].to_vec();
        }
        let mut result = shown;
        if let Some(more) = more {
            result.push(Line::from(Span::styled(
                format!("... ({more} more lines, press Enter to view)"),
                theme.muted(),
            )));
        }
        if !expanded && let Some(path) = &self.full_output_path {
            result.push(Line::from(Span::styled(
                format!("Full output: {path}"),
                theme.muted(),
            )));
        }
        result
    }

    fn render_body(&self, ctx: &BlockContext) -> BlockOutput {
        let theme = Theme::current();
        let expanded = Self::expanded(ctx);
        let width = ctx.content_width().max(20);
        let mut lines: Vec<BlockLine> = vec![
            self.header_line(
                &theme,
                ctx.mute_when_collapsed(ctx.appearance.scrollback.blocks.tool.muted_collapsed),
            )
            .into(),
        ];

        if !self.code.is_empty() {
            lines.push(Line::from("").into());
            let mut code_lines = self.highlighted_code(&theme);
            if !expanded && code_lines.len() > CODE_PREVIEW_LINES {
                let hidden = code_lines.len() - CODE_PREVIEW_LINES;
                code_lines.truncate(CODE_PREVIEW_LINES);
                code_lines.push(Line::from(Span::styled(
                    format!("... ({hidden} more lines, press Enter to view)"),
                    theme.muted(),
                )));
            }
            for line in word_wrap_lines(code_lines, width) {
                lines.push(BlockLine::styled(line));
            }
        }

        let calls = self.calls_section(&theme, expanded);
        if !calls.is_empty() {
            lines.push(Line::from("").into());
            for line in calls {
                lines.push(BlockLine::styled(line));
            }
        }

        let output = self.output_section(&theme, expanded);
        if !output.is_empty() {
            lines.push(Line::from("").into());
            for line in word_wrap_lines(output, width) {
                lines.push(BlockLine::styled(line));
            }
        }

        if let Some(error) = &self.error {
            lines.push(Line::from("").into());
            for line in error.lines() {
                lines.push(BlockLine::styled(Line::from(Span::styled(
                    line.to_string(),
                    theme.fg(theme.accent_error),
                ))));
            }
        }

        BlockOutput { lines }
    }
}

fn status_style(status: CodemodeCallStatus, theme: &Theme) -> ratatui::style::Style {
    match status {
        CodemodeCallStatus::Running => theme.fg(theme.accent_running),
        CodemodeCallStatus::Ok => theme.fg(theme.accent_success),
        CodemodeCallStatus::Error => theme.fg(theme.accent_error),
        CodemodeCallStatus::Cancelled => theme.muted(),
    }
}

fn format_duration(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms}ms")
    } else {
        format!("{:.1}s", ms as f64 / 1000.0)
    }
}

/// Cents for larger amounts, two significant digits for the fractions of a cent
/// classifier calls cost (mirrors Pi's renderer).
fn format_cost(cost: f64) -> String {
    if cost >= 0.01 {
        return format!("${cost:.2}");
    }
    if cost == 0.0 {
        return "$0.00".to_string();
    }
    // Two significant digits: 0.0012 → "$0.0012".
    let magnitude = cost.abs().log10().floor() as i32;
    let decimals = (2 - 1 - magnitude).clamp(2, 6) as usize;
    format!("${cost:.decimals$}")
}

impl BlockContent for CodemodeToolCallBlock {
    fn output(&self, ctx: &BlockContext) -> BlockOutput {
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
            DisplayMode::Truncated | DisplayMode::Expanded => self.render_body(ctx),
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
        !self.code.is_empty()
            || self.output.is_some()
            || self.error.is_some()
            || !self.calls.is_empty()
    }

    fn default_display_mode(&self) -> DisplayMode {
        DisplayMode::Collapsed
    }

    fn next_fold_mode(&self, current: DisplayMode, _is_running: bool) -> DisplayMode {
        match current {
            DisplayMode::Collapsed => DisplayMode::Truncated,
            DisplayMode::Truncated => DisplayMode::Expanded,
            DisplayMode::Expanded => DisplayMode::Collapsed,
        }
    }

    fn preamble(&self, _ctx: &BlockContext) -> Option<ratatui::text::Text<'static>> {
        Some(Text::from(vec![self.header_line(&Theme::current(), false)]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ctx_mode(mode: DisplayMode) -> BlockContext {
        BlockContext {
            width: 80,
            mode,
            is_running: false,
            raw: false,
            max_lines: None,
            appearance: Default::default(),
            is_selected: false,
            cwd: None,
        }
    }

    fn sample_block() -> CodemodeToolCallBlock {
        CodemodeToolCallBlock::new("const a = await tools.read({ path: 'a.rs' });")
            .with_raw_output(Some(&json!({
                "type": "Codemode",
                "calls": [
                    { "id": "c/1", "name": "read", "args": "{ \"path\": \"a.rs\" }", "status": "ok", "durationMs": 120 },
                    { "id": "c/2", "name": "models.classify", "args": "{}", "status": "ok", "durationMs": 400, "cost": 0.0012 }
                ],
                "output": "a.rs: fn main() {}",
                "full_output_path": "/tmp/pi-codemode-abc.txt"
            })))
    }

    fn lines_text(output: &BlockOutput) -> Vec<String> {
        output
            .lines
            .iter()
            .map(|line| line.content.to_string())
            .collect()
    }

    #[test]
    fn raw_output_parses_calls_output_and_full_path() {
        let block = sample_block();
        assert_eq!(block.calls.len(), 2);
        assert_eq!(block.calls[0].name, "read");
        assert_eq!(block.calls[0].status, CodemodeCallStatus::Ok);
        assert_eq!(block.calls[0].duration_ms, Some(120));
        assert_eq!(block.calls[1].cost, Some(0.0012));
        assert_eq!(block.output.as_deref(), Some("a.rs: fn main() {}"));
        assert_eq!(
            block.full_output_path.as_deref(),
            Some("/tmp/pi-codemode-abc.txt")
        );
    }

    #[test]
    fn fractional_nested_call_durations_round_to_millis() {
        let block = CodemodeToolCallBlock::new("return 1;").with_raw_output(Some(&json!({
            "type": "Codemode",
            "calls": [
                { "id": "c/1", "name": "ffind", "args": "{}", "status": "ok", "durationMs": 12.942792000001646 }
            ],
            "output": ""
        })));
        assert_eq!(block.calls[0].duration_ms, Some(13));
    }

    #[test]
    fn malformed_raw_output_degrades_to_empty_projection() {
        let block = CodemodeToolCallBlock::new("return 1;").with_raw_output(Some(&json!({
            "type": "Bash",
            "output": "unrelated"
        })));
        assert!(block.calls.is_empty());
        assert!(block.output.is_none());
        assert!(block.full_output_path.is_none());

        let none = CodemodeToolCallBlock::new("return 1;").with_raw_output(None);
        assert!(none.calls.is_empty());
    }

    #[test]
    fn collapsed_shows_header_only() {
        let output = sample_block().output(&ctx_mode(DisplayMode::Collapsed));
        assert_eq!(output.lines.len(), 1);
        assert!(lines_text(&output)[0].contains("Codemode"));
        assert!(lines_text(&output)[0].contains("2 calls"));
        assert!(!lines_text(&output)[0].contains("read"));
    }

    #[test]
    fn truncated_previews_code_output_and_hides_early_calls() {
        let long_code = (0..14)
            .map(|i| format!("const v{i} = {i};"))
            .collect::<Vec<_>>()
            .join("\n");
        let long_output = (0..7)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut block = CodemodeToolCallBlock::new(long_code).with_output(long_output);
        for i in 0..9 {
            block.calls.push(CodemodeCallRow {
                name: format!("tool{i}"),
                args: "{}".to_string(),
                status: CodemodeCallStatus::Ok,
                duration_ms: Some(10),
                error: None,
                cost: None,
            });
        }
        let text = lines_text(&block.output(&ctx_mode(DisplayMode::Truncated))).join("\n");

        // Code preview keeps the first lines and names the hidden count.
        assert!(text.contains("const v0 = 0;"));
        assert!(text.contains("const v9 = 9;"));
        assert!(!text.contains("const v10 = 10;"));
        assert!(text.contains("... (4 more lines"));
        // Calls preview keeps the tail and hides the head.
        assert!(text.contains("... (1 earlier call,"));
        assert!(text.contains("tool8"));
        assert!(!text.contains("tool0 "));
        // Output preview truncates and names the spilled file.
        assert!(text.contains("line 0"));
        assert!(text.contains("line 4"));
        assert!(!text.contains("line 5"));
        assert!(text.contains("... (2 more lines"));
        // No spilled file on this block, so no Full output hint.
        assert!(!text.contains("Full output:"));
    }

    #[test]
    fn truncated_sample_names_spilled_full_output_file() {
        let text = lines_text(&sample_block().output(&ctx_mode(DisplayMode::Truncated))).join("\n");
        assert!(text.contains("Full output: /tmp/pi-codemode-abc.txt"));
    }

    #[test]
    fn expanded_shows_all_calls_errors_and_full_output() {
        let mut block = sample_block();
        block.calls.push(CodemodeCallRow {
            name: "grep",
            args: "{ \"pattern\": \"todo\" }".to_string(),
            status: CodemodeCallStatus::Error,
            duration_ms: Some(2100),
            error: Some("boom\nboom2".to_string()),
            cost: Some(1.5),
        });
        let text = lines_text(&block.output(&ctx_mode(DisplayMode::Expanded))).join("\n");

        assert!(text.contains("const a = await tools.read"));
        assert!(text.contains("Model calls: $1.50"));
        assert!(text.contains("grep"));
        assert!(text.contains("{ \"pattern\": \"todo\" }"));
        assert!(text.contains("2.1s"));
        assert!(text.contains("    boom"));
        assert!(text.contains("a.rs: fn main() {}"));
        assert!(!text.contains("Full output:"));
        assert!(text.contains("✗"));
        assert!(text.contains("✓"));
    }

    #[test]
    fn error_result_renders_error_accent_and_text() {
        let mut block = sample_block();
        block.error = Some("Script error:\nError: out of memory".to_string());
        let text = lines_text(&block.output(&ctx_mode(DisplayMode::Truncated))).join("\n");
        assert!(text.contains("Script error:"));
        assert!(text.contains("Error: out of memory"));
        assert!(!block.is_success());
    }

    #[test]
    fn cost_formatting_uses_two_significant_digits_for_small_amounts() {
        assert_eq!(format_cost(1.5), "$1.50");
        assert_eq!(format_cost(0.05), "$0.05");
        assert_eq!(format_cost(0.0012), "$0.0012");
        assert_eq!(format_cost(0.0), "$0.00");
    }

    #[test]
    fn duration_formatting_splits_seconds_and_millis() {
        assert_eq!(format_duration(120), "120ms");
        assert_eq!(format_duration(2100), "2.1s");
    }

    #[test]
    fn fold_mode_cycles_through_preview_and_expanded() {
        let block = CodemodeToolCallBlock::new("return 1;");
        assert_eq!(block.default_display_mode(), DisplayMode::Collapsed);
        assert_eq!(
            block.next_fold_mode(DisplayMode::Collapsed, false),
            DisplayMode::Truncated
        );
        assert_eq!(
            block.next_fold_mode(DisplayMode::Truncated, false),
            DisplayMode::Expanded
        );
        assert_eq!(
            block.next_fold_mode(DisplayMode::Expanded, false),
            DisplayMode::Collapsed
        );
    }

    #[test]
    fn args_are_truncated_outside_expanded_mode() {
        let block = CodemodeToolCallBlock::new("x").with_raw_output(Some(&json!({
            "type": "Codemode",
            "calls": [{
                "id": "c/1",
                "name": "read",
                "args": format!("\"{}\"", "x".repeat(200)),
                "status": "ok"
            }],
            "output": ""
        })));
        let collapsed = lines_text(&block.output(&ctx_mode(DisplayMode::Truncated))).join("\n");
        let expanded = lines_text(&block.output(&ctx_mode(DisplayMode::Expanded))).join("\n");
        assert!(collapsed.contains("..."));
        assert!(!collapsed.contains(&"x".repeat(200)));
        assert!(expanded.contains(&"x".repeat(200)));
    }
}
