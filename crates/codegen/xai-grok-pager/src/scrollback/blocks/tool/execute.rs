use ratatui::style::Modifier;
use ratatui::text::{Line, Span, Text};

use super::TOOL_HEADER_RANGE;
use crate::appearance::{AppearanceConfig, ExecuteHeaderContent, ExecuteHeaderStyle};
use crate::render::wrapping::word_wrap_lines_with_joiners;
use crate::scrollback::block::BlockContent;
use crate::scrollback::types::{
    AccentStyle, BlockBackground, BlockContext, BlockLine, BlockOutput, DisplayMode, Selectable,
};
use crate::theme::Theme;

const EXECUTE_STDOUT_RANGE_BASE: u16 = 1;

/// Execute tool call: runs a shell command.
#[derive(Debug, Clone)]
pub struct ExecuteToolCallBlock {
    /// Full command that was run (the source of truth for search, copy_meta, and export).
    pub command: String,
    /// Error message if the command failed (None means success).
    pub error: Option<String>,
    /// Optional description of what the command does.
    pub description: Option<String>,
    /// The terminal output. Streamed incrementally.
    pub output: Option<String>,
    /// When the tool started running.
    pub started_at: Option<std::time::Instant>,
    /// Elapsed time in ms after completion.
    pub elapsed_ms: Option<i64>,
    /// Whether this is a user-initiated bash-mode (`!`) command.
    /// Streams as a truncated live tail, expands to full output on finish.
    pub bash_mode: bool,
    /// Peeled display form for the header; `command` stays the full source of truth.
    pub header_display: Option<String>,
    /// Optional bounded OMP execution trace for multi-command breakdown.
    pub shell_trace: Option<xai_grok_tools::types::output::ShellTraceSummary>,
}
impl ExecuteToolCallBlock {
    /// Create a new execute block. Pre-completed blocks never get timing; they show `"-"`.
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            error: None,
            description: None,
            output: None,
            started_at: None,
            elapsed_ms: None,
            bash_mode: false,
            header_display: None,
            shell_trace: None,
        }
    }

    /// Attach an OMP execution trace summary.
    pub fn with_shell_trace(
        mut self,
        trace: xai_grok_tools::types::output::ShellTraceSummary,
    ) -> Self {
        self.shell_trace = Some(trace);
        self
    }

    /// Set error (marks as failed).
    pub fn with_error(mut self, error: impl Into<String>) -> Self {
        self.error = Some(error.into());
        self
    }

    /// Set description.
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Set output.
    pub fn with_output(mut self, output: impl Into<String>) -> Self {
        self.output = Some(output.into());
        self
    }

    /// Push streaming output chunk.
    pub fn push_output(&mut self, chunk: &str) {
        match &mut self.output {
            Some(o) => o.push_str(chunk),
            None => self.output = Some(chunk.to_string()),
        }
    }

    /// Finalize elapsed time from `started_at`.
    ///
    /// Idempotent: no-op if `started_at` is `None` (pre-completed block) or if `elapsed_ms` is already set (already finalized).
    pub fn finish(&mut self) {
        if self.elapsed_ms.is_some() {
            return;
        }
        if let Some(start) = self.started_at {
            self.elapsed_ms = Some(start.elapsed().as_millis() as i64);
        }
    }

    /// Set error (mutable); compute elapsed time if not already set.
    pub fn set_error(&mut self, error: Option<String>) {
        if self.elapsed_ms.is_none()
            && let Some(start) = self.started_at
        {
            self.elapsed_ms = Some(start.elapsed().as_millis() as i64);
        }
        self.error = error;
    }

    /// Check if successful (no error).
    pub fn is_success(&self) -> bool {
        self.error.is_none()
    }

    /// Get elapsed time in ms. Returns current elapsed if still running, or stored value if finished.
    pub fn elapsed_ms(&self) -> Option<i64> {
        match self.elapsed_ms {
            Some(ms) => Some(ms),
            None => self
                .started_at
                .map(|start| start.elapsed().as_millis() as i64),
        }
    }

    /// Get copyable text for this block (stdout output).
    ///
    /// Returns the output string if available, or empty string if no output.
    pub fn copy_text(&self) -> String {
        self.output
            .as_deref()
            .map(crate::render::terminal_output::render_terminal_plain)
            .unwrap_or_default()
    }

    /// Display form of the command for the header (may peel `cd &&` prefix).
    /// Preserves physical newlines so soft-wrap / copy can keep line structure.
    /// Callers that need a single ratatui line must flatten themselves.
    fn command_display(&self) -> &str {
        self.header_display.as_deref().unwrap_or(&self.command)
    }

    /// Non-empty description for the header title, if the model supplied one.
    /// When `strip_run_prefix` is true (Label style already has a bold `Run ` prefix), a leading `Run` / `Running` on the description is dropped.
    /// This keeps `Run Run the tests` from ever rendering.
    fn description_display(&self, strip_run_prefix: bool) -> Option<String> {
        self.description.as_ref().and_then(|d| {
            let trimmed = d.trim();
            if trimmed.is_empty() {
                return None;
            }
            // Collapse newlines so the title stays one logical line (`wrap_header_hanging` may still soft-wrap for width)
            let mut text = trimmed.replace('\n', " ");
            if strip_run_prefix {
                text = strip_leading_run_word(&text);
                if text.is_empty() {
                    return None;
                }
            }
            Some(text)
        })
    }

    /// `$ command` line (shell prompt style). Sole header when there is no description. secondary line when there is
    /// one. Flattens newlines for single-line layouts (collapsed truncate). Expanded rendering uses soft-wrap via
    /// [`Self::push_shell_command_soft_wrap`].
    fn shell_command_line(&self, theme: &Theme, muted_command: bool) -> Line<'static> {
        let command = self.command_display().replace('\n', " ");
        let command = if command.trim().is_empty() {
            "\u{2026}".to_string()
        } else {
            command
        };
        let command_spans: Vec<Span<'static>> = if muted_command || command == "\u{2026}" {
            vec![Span::styled(command, theme.muted())]
        } else {
            crate::views::tasks_pane::highlight_bash_command(&command)
        };
        // `$` uses gray_dim: dimmer than muted, no bold
        let mut spans = vec![Span::styled("$ ", theme.dim())];
        spans.extend(command_spans);
        Line::from(spans)
    }

    /// Prefix spans for a soft-wrapped command header (`$ ` or `Run [(user) ]`).
    ///
    /// Returns `(prefix_spans, hang_width)`; hang is the display width of the first-row prefix so continuations indent under the command body.
    fn command_header_prefix(
        &self,
        theme: &Theme,
        header_style: ExecuteHeaderStyle,
        muted_command: bool,
    ) -> (Vec<Span<'static>>, usize) {
        use unicode_width::UnicodeWidthStr;
        match header_style {
            ExecuteHeaderStyle::Shell => {
                let prefix = "$ ";
                (
                    vec![Span::styled(prefix.to_string(), theme.dim())],
                    UnicodeWidthStr::width(prefix),
                )
            }
            ExecuteHeaderStyle::Label => {
                let label_style = if muted_command {
                    theme.muted().add_modifier(Modifier::BOLD)
                } else {
                    theme.primary().add_modifier(Modifier::BOLD)
                };
                let mut spans = vec![Span::styled("Run ".to_string(), label_style)];
                let mut hang = UnicodeWidthStr::width("Run ");
                if self.bash_mode {
                    spans.push(Span::styled("(user) ".to_string(), theme.muted()));
                    hang += UnicodeWidthStr::width("(user) ");
                }
                (spans, hang)
            }
        }
    }

    /// Multi-line command header using permission-panel soft-wrap (operators and quotes).
    ///
    /// Used for both Shell (`$ command`) and Label (`Run [(user) ]command`) so physical newlines / `\` continuations match the permission overlay.
    fn push_command_soft_wrap(
        &self,
        lines: &mut Vec<BlockLine>,
        theme: &Theme,
        header_style: ExecuteHeaderStyle,
        muted_command: bool,
        width: usize,
        extra_indent: usize,
    ) {
        let command = self.command_display();
        let command = if command.trim().is_empty() {
            "\u{2026}"
        } else {
            command
        };
        let (prefix_spans, prefix_w) =
            self.command_header_prefix(theme, header_style, muted_command);
        let prefix_span_count = prefix_spans.len();
        let hang = extra_indent.saturating_add(prefix_w);
        let cmd_width = width.saturating_sub(hang).max(1);

        let cmd_rows: Vec<Line<'static>> = if muted_command || command == "\u{2026}" {
            let style = theme.muted();
            let flat = command.replace('\n', " ");
            let line = Line::from(Span::styled(flat, style));
            crate::render::wrapping::word_wrap_lines(std::iter::once(line), cmd_width)
        } else {
            crate::views::permission_view::render_bash_command_display_lines(command, cmd_width)
        };

        let hang_indent: String = " ".repeat(hang);
        if cmd_rows.is_empty() {
            lines.push(BlockLine {
                selectable: Selectable::None,
                selection_range: Some(TOOL_HEADER_RANGE),
                content: Line::from(prefix_spans),
                ..Default::default()
            });
            return;
        }
        for (i, row) in cmd_rows.into_iter().enumerate() {
            let mut spans = if i == 0 {
                prefix_spans.clone()
            } else {
                vec![Span::raw(hang_indent.clone())]
            };
            spans.extend(row.spans);
            let total = spans.len();
            let select_start = if i == 0 {
                prefix_span_count.min(total)
            } else {
                0
            };
            lines.push(BlockLine {
                selectable: Selectable::Spans(select_start..total),
                selection_range: Some(TOOL_HEADER_RANGE),
                // Preserve physical/soft-wrap structure as newlines when copying.
                joiner: if i == 0 { None } else { Some("\n".to_string()) },
                content: Line::from(spans),
                ..Default::default()
            });
        }
    }

    /// Primary title line for Label style: `Run [(user) ]<description|command>`.
    ///
    /// When `title` is empty (eager placeholder before `raw_input.command` arrives), renders `Run …` so we never flash an internal tool id.
    fn label_title_line(
        &self,
        theme: &Theme,
        muted_command: bool,
        title: &str,
        highlight_as_command: bool,
    ) -> Line<'static> {
        let label_style = if muted_command {
            theme.muted().add_modifier(Modifier::BOLD)
        } else {
            theme.primary().add_modifier(Modifier::BOLD)
        };
        let mut spans = vec![Span::styled("Run ", label_style)];
        if self.bash_mode {
            // Same style as session event messages (e.g. "Worked for 2.3s")
            spans.push(Span::styled("(user) ", theme.muted()));
        }
        // Single ratatui Line: never pass raw newlines (callers that need multi-line command display use `push_command_soft_wrap`)
        let title_owned;
        let title = if title.trim().is_empty() {
            "\u{2026}" // …
        } else if title.contains('\n') {
            title_owned = title.replace('\n', " ");
            title_owned.as_str()
        } else {
            title
        };
        if highlight_as_command && !muted_command && title != "\u{2026}" {
            spans.extend(crate::views::tasks_pane::highlight_bash_command(title));
        } else if muted_command {
            spans.push(Span::styled(title.to_string(), theme.muted()));
        } else {
            // Description (or loading ellipsis): plain primary text.
            spans.push(Span::styled(title.to_string(), theme.primary()));
        }
        Line::from(spans)
    }

    /// Description-first when a description exists; collapsed mode omits the command line for density.
    /// When `muted`, command/description body uses muted gray. Prefix spans (`Run `, `(user) `, `$ `) are not selectable.
    fn header_lines(
        &self,
        theme: &Theme,
        header_style: ExecuteHeaderStyle,
        muted_command: bool,
        header_content: ExecuteHeaderContent,
    ) -> Vec<(Line<'static>, usize)> {
        let strip_run = matches!(header_style, ExecuteHeaderStyle::Label);
        let description = if header_content.shows_task_name() {
            self.description_display(strip_run)
        } else {
            None
        };
        match description {
            Some(desc) => {
                let title = match header_style {
                    ExecuteHeaderStyle::Label => {
                        let prefix_spans = if self.bash_mode { 2 } else { 1 };
                        (
                            self.label_title_line(theme, muted_command, &desc, false),
                            prefix_spans,
                        )
                    }
                    ExecuteHeaderStyle::Shell => {
                        // Description as plain title (no `$`); command follows when included.
                        let style = if muted_command {
                            theme.muted()
                        } else {
                            theme.primary()
                        };
                        (Line::from(Span::styled(desc, style)), 0)
                    }
                };
                if header_content.shows_command() {
                    let cmd = (self.shell_command_line(theme, muted_command), 1);
                    vec![title, cmd]
                } else {
                    vec![title]
                }
            }
            None => {
                // No description: single-line header (callers that soft-wrap the command as title use `push_command_soft_wrap` instead)
                // Flatten newlines; raw `\n` is invalid inside one ratatui Line
                let line = match header_style {
                    ExecuteHeaderStyle::Shell => self.shell_command_line(theme, muted_command),
                    ExecuteHeaderStyle::Label => {
                        let flat = self.command_display().replace('\n', " ");
                        self.label_title_line(theme, muted_command, &flat, true)
                    }
                };
                let prefix_spans = match header_style {
                    ExecuteHeaderStyle::Shell => 1,
                    ExecuteHeaderStyle::Label => {
                        if self.bash_mode {
                            2
                        } else {
                            1
                        }
                    }
                };
                vec![(line, prefix_spans)]
            }
        }
    }

    /// `include_command` is false in collapsed mode so a description title alone is shown without the command line.
    /// `width` must already be the block content width (bullet accounted for). Do not also pass bullet width as
    /// `extra_indent`. hang is only for the `$` / `Run` prefix.
    #[allow(clippy::too_many_arguments)]
    fn push_header_lines(
        &self,
        lines: &mut Vec<BlockLine>,
        theme: &Theme,
        header_style: ExecuteHeaderStyle,
        muted_command: bool,
        width: usize,
        extra_indent: usize,
        truncate_to_width: bool,
        header_content: ExecuteHeaderContent,
    ) {
        let strip_run = matches!(header_style, ExecuteHeaderStyle::Label);
        let description = if header_content.shows_task_name() {
            self.description_display(strip_run)
        } else {
            None
        };
        // Command is the primary title when task names are hidden or absent.
        if !truncate_to_width && description.is_none() {
            self.push_command_soft_wrap(
                lines,
                theme,
                header_style,
                muted_command,
                width,
                extra_indent,
            );
            return;
        }
        // Description title (word-wrap) then soft-wrapped `$ command` (both styles).
        if !truncate_to_width && description.is_some() && header_content.shows_command() {
            let headers = self.header_lines(
                theme,
                header_style,
                muted_command,
                ExecuteHeaderContent::TaskName,
            );
            for (line, prefix_spans) in headers {
                let wrapped =
                    crate::render::wrapping::wrap_header_hanging(line, width, extra_indent);
                for (i, wrapped_line) in wrapped.into_iter().enumerate() {
                    let total = wrapped_line.spans.len();
                    let start = if i == 0 { prefix_spans.min(total) } else { 0 };
                    lines.push(BlockLine {
                        selectable: Selectable::Spans(start..total),
                        selection_range: Some(TOOL_HEADER_RANGE),
                        joiner: if i == 0 { None } else { Some(" ".to_string()) },
                        content: wrapped_line,
                        ..Default::default()
                    });
                }
            }
            // Secondary command line is always shell-style (`$ …`).
            self.push_command_soft_wrap(
                lines,
                theme,
                ExecuteHeaderStyle::Shell,
                muted_command,
                width,
                extra_indent,
            );
            return;
        }

        let headers = self.header_lines(theme, header_style, muted_command, header_content);
        for (line, prefix_spans) in headers {
            if truncate_to_width {
                let line = crate::render::line_utils::truncate_line(line, width);
                let total = line.spans.len();
                let start = prefix_spans.min(total);
                lines.push(BlockLine {
                    selectable: Selectable::Spans(start..total),
                    selection_range: Some(TOOL_HEADER_RANGE),
                    content: line,
                    ..Default::default()
                });
            } else {
                let wrapped =
                    crate::render::wrapping::wrap_header_hanging(line, width, extra_indent);
                for (i, wrapped_line) in wrapped.into_iter().enumerate() {
                    let total = wrapped_line.spans.len();
                    // Only the first wrapped segment of each logical header line has the prefix (`Run ` / `$ `); continuations are fully selectable
                    let start = if i == 0 { prefix_spans.min(total) } else { 0 };
                    lines.push(BlockLine {
                        selectable: Selectable::Spans(start..total),
                        selection_range: Some(TOOL_HEADER_RANGE),
                        joiner: if i == 0 { None } else { Some(" ".to_string()) },
                        content: wrapped_line,
                        ..Default::default()
                    });
                }
            }
        }
    }

    fn has_meaningful_trace(&self) -> bool {
        self.shell_trace
            .as_ref()
            .is_some_and(|trace| normalize_shell_trace_rows(trace).len() > 1)
    }

    /// Render the full OMP execution trace for the fullscreen `BlockViewer` modal.
    /// Unlike the inline scrollback card, single-step traces are still shown here so
    /// pressing Enter always exposes OMP step status and timing.
    pub(crate) fn render_trace_viewer_lines(&self, theme: &Theme) -> Vec<(Line<'static>, String)> {
        let Some(trace) = self.shell_trace.as_ref() else {
            return Vec::new();
        };
        render_shell_trace_viewer_lines(trace, theme)
    }

    fn push_shell_trace_lines(
        &self,
        lines: &mut Vec<BlockLine>,
        theme: &Theme,
        width: usize,
        max_nodes: usize,
        result_pad: usize,
    ) {
        let Some(trace) = self.shell_trace.as_ref() else {
            return;
        };
        let rows = normalize_shell_trace_rows(trace);
        if rows.len() <= 1 {
            return;
        }
        lines.push(BlockLine::separator(Line::from("")));
        let pad = " ".repeat(result_pad);
        let summary_label = format!("{pad}{}", format_trace_header(trace, rows.len()));
        lines.push(
            BlockLine::separator(Line::from(Span::styled(summary_label, theme.muted())))
                .with_panel_background(theme.bg_dark),
        );

        let shown = rows.len().min(max_nodes);
        for row in rows.iter().take(shown) {
            let (line, _) = render_shell_trace_row_line(row, theme, result_pad, Some(width));
            lines.push(
                BlockLine::styled(line).with_panel_background(theme.bg_dark),
            );
        }
        if rows.len() > shown {
            let hidden = rows.len() - shown;
            lines.push(
                BlockLine::separator(Line::from(Span::styled(
                    format!("{pad}  … +{hidden} steps"),
                    theme.muted(),
                )))
                .with_panel_background(theme.bg_dark),
            );
        }
    }

    /// Render with optional truncation.
    /// If `truncate` is Some((first, last)), shows first N lines, ellipsis, last M lines.
    /// If `truncate` is None, shows all output lines.
    fn render_with_truncation(
        &self,
        theme: &Theme,
        width: usize,
        truncate: Option<(usize, usize)>,
        header_style: ExecuteHeaderStyle,
        extra_indent: usize,
        header_content: ExecuteHeaderContent,
        result_pad: usize,
    ) -> BlockOutput {
        let mut lines: Vec<BlockLine> = Vec::new();
        self.push_header_lines(
            &mut lines,
            theme,
            header_style,
            false,
            width,
            extra_indent,
            false,
            header_content,
        );

        if self.has_meaningful_trace() {
            let max_nodes = if truncate.is_some() { 6 } else { 24 };
            self.push_shell_trace_lines(&mut lines, theme, width, max_nodes, result_pad);
        }

        if self.output.is_none()
            && let Some(error) = &self.error
            && !error.is_empty()
        {
            lines.push(BlockLine::separator(Line::from("")));
            let error_style = ratatui::style::Style::default().fg(theme.accent_error);
            let err_pad = " ".repeat(result_pad);
            for line in error.lines() {
                lines.push(BlockLine::separator(Line::from(Span::styled(
                    format!("{err_pad}{line}"),
                    error_style,
                ))));
            }
        }

        if let Some(output) = &self.output
            && !output.is_empty()
        {
            lines.push(BlockLine::separator(Line::from("")));

            // Default foreground for the output body
            // Under the terminal-native (minimal) palette, muted used to collapse to ANSI bright black and washed out on many dark profiles
            // Content must track the terminal's default fg (`primary` / Reset); labels / chrome stay on `muted`/`dim`
            let styled_lines: Vec<Line<'static>> =
                crate::render::terminal_output::render_terminal_lines(output, theme.primary())
                    .into_iter()
                    .map(|rl| rl.line)
                    .collect();

            let (wrapped, joiners) = word_wrap_lines_with_joiners(
                styled_lines,
                width.saturating_sub(2 + result_pad).max(20),
            );
            let pad = " ".repeat(result_pad);
            let apply_pad = |mut line: BlockLine| -> BlockLine {
                if result_pad == 0 {
                    return line;
                }
                line.content.spans.insert(0, Span::raw(pad.clone()));
                crate::scrollback::types::shift_selection_metadata_for_prefix(&mut line, 1);
                line
            };
            let total = wrapped.len();

            // Apply truncation if specified and content exceeds limits
            if let Some((first, last)) = truncate {
                let threshold = first + last;
                if total > threshold {
                    // First N lines: stdout range base
                    for (wrapped_line, joiner) in wrapped.iter().zip(joiners.iter()).take(first) {
                        lines.push(apply_pad(
                            BlockLine::styled(wrapped_line.clone())
                                .with_panel_background(theme.bg_dark)
                                .with_selection_range(Some(EXECUTE_STDOUT_RANGE_BASE))
                                .with_joiner(joiner.clone()),
                        ));
                    }
                    let hidden = total - threshold;
                    lines.push(apply_pad(
                        BlockLine::separator(Line::from(Span::styled(
                            format!("\u{2026} +{hidden} lines"),
                            theme.muted(),
                        )))
                        .with_panel_background(theme.bg_dark),
                    ));
                    // Last M lines: range base + 1 (distinct from first chunk)
                    for (wrapped_line, joiner) in
                        wrapped.iter().zip(joiners.iter()).skip(total - last)
                    {
                        lines.push(apply_pad(
                            BlockLine::styled(wrapped_line.clone())
                                .with_panel_background(theme.bg_dark)
                                .with_selection_range(Some(EXECUTE_STDOUT_RANGE_BASE + 1))
                                .with_joiner(joiner.clone()),
                        ));
                    }
                } else {
                    // Content fits, show all with same range
                    for (wrapped_line, joiner) in wrapped.into_iter().zip(joiners) {
                        lines.push(apply_pad(
                            BlockLine::styled(wrapped_line)
                                .with_panel_background(theme.bg_dark)
                                .with_selection_range(Some(EXECUTE_STDOUT_RANGE_BASE))
                                .with_joiner(joiner),
                        ));
                    }
                }
            } else {
                // No truncation, show all with same range
                for (wrapped_line, joiner) in wrapped.into_iter().zip(joiners) {
                    lines.push(apply_pad(
                        BlockLine::styled(wrapped_line)
                            .with_panel_background(theme.bg_dark)
                            .with_selection_range(Some(EXECUTE_STDOUT_RANGE_BASE))
                            .with_joiner(joiner),
                    ));
                }
            }
        }

        BlockOutput { lines }
    }
}

#[derive(Debug, Clone, Copy)]
struct TraceDisplayRow<'a> {
    node: &'a xai_grok_tools::types::output::ShellTraceNode,
    depth: usize,
    pipe_continuation: bool,
}

/// Unwrap redundant OMP AST container nodes (`and_or`, single/multi-stage `pipeline`
/// wrappers, empty/`word` nodes, and identical single-child wrappers) so each executed
/// statement renders once with clean nesting — matching `oh-my-pi`'s shell visualizer.
fn normalize_shell_trace_rows(
    trace: &xai_grok_tools::types::output::ShellTraceSummary,
) -> Vec<TraceDisplayRow<'_>> {
    use std::collections::HashMap;

    if trace.nodes.is_empty() {
        return Vec::new();
    }

    let mut id_to_idx: HashMap<u32, usize> = HashMap::with_capacity(trace.nodes.len());
    for (idx, node) in trace.nodes.iter().enumerate() {
        id_to_idx.insert(node.id, idx);
    }

    let mut children_map: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut roots: Vec<usize> = Vec::new();
    for (idx, node) in trace.nodes.iter().enumerate() {
        if let Some(parent_idx) = node.parent_id.and_then(|pid| id_to_idx.get(&pid).copied())
            && parent_idx != idx
        {
            children_map.entry(parent_idx).or_default().push(idx);
        } else {
            roots.push(idx);
        }
    }

    fn walk<'a>(
        idx: usize,
        depth: usize,
        pipe_continuation: bool,
        nodes: &'a [xai_grok_tools::types::output::ShellTraceNode],
        children_map: &HashMap<usize, Vec<usize>>,
        visited: &mut [bool],
        out: &mut Vec<TraceDisplayRow<'a>>,
    ) {
        if visited[idx] {
            return;
        }
        visited[idx] = true;

        let node = &nodes[idx];
        let children = children_map
            .get(&idx)
            .map(Vec::as_slice)
            .unwrap_or(&[]);

        let trimmed = node.command.trim();
        if trimmed.is_empty() || node.kind == "word" {
            for &child in children {
                walk(child, depth, pipe_continuation, nodes, children_map, visited, out);
            }
            return;
        }

        // Unwrap `and_or` (`a && b || c`) directly into its child statements.
        if node.kind == "and_or" && !children.is_empty() {
            for &child in children {
                walk(child, depth, false, nodes, children_map, visited, out);
            }
            return;
        }

        // Unwrap `pipeline` into its stages; stages 2..n carry a `| ` continuation prefix.
        if node.kind == "pipeline" && !children.is_empty() {
            for (stage_idx, &child) in children.iter().enumerate() {
                walk(
                    child,
                    depth,
                    pipe_continuation || stage_idx > 0,
                    nodes,
                    children_map,
                    visited,
                    out,
                );
            }
            return;
        }

        // Drop any single-child wrapper whose command text is identical to its child.
        if children.len() == 1 && nodes[children[0]].command.trim() == trimmed {
            walk(
                children[0],
                depth,
                pipe_continuation,
                nodes,
                children_map,
                visited,
                out,
            );
            return;
        }

        out.push(TraceDisplayRow {
            node,
            depth,
            pipe_continuation,
        });
        for &child in children {
            walk(child, depth + 1, false, nodes, children_map, visited, out);
        }
    }

    let mut visited = vec![false; trace.nodes.len()];
    let mut out = Vec::with_capacity(trace.nodes.len());
    for root in roots {
        walk(root, 0, false, &trace.nodes, &children_map, &mut visited, &mut out);
    }
    out
}

fn format_trace_duration(ms: u64) -> String {
    if ms < 1_000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{:.1}s", (ms as f64) / 1_000.0)
    } else {
        let total_secs = ms / 1_000;
        let mins = total_secs / 60;
        let secs = total_secs % 60;
        format!("{mins}m {secs}s")
    }
}

fn format_trace_header(
    trace: &xai_grok_tools::types::output::ShellTraceSummary,
    step_count: usize,
) -> String {
    let step_word = if step_count == 1 { "step" } else { "steps" };
    let mut parts = vec![
        "Trace".to_string(),
        format!("{step_count} {step_word}"),
        format_trace_duration(trace.total_duration_ms),
    ];
    if let Some(code) = trace.exit_code
        && code != 0
    {
        parts.push(format!("exit {code}"));
    }
    if trace.truncated {
        parts.push("truncated".to_string());
    }
    parts.join(" · ")
}

fn format_trace_row_meta(node: &xai_grok_tools::types::output::ShellTraceNode) -> String {
    use xai_grok_tools::types::output::ShellTraceNodeState;

    let mut parts: Vec<String> = Vec::new();
    if node.state == ShellTraceNodeState::Fail
        && let Some(code) = node.exit_code
        && code != 0
    {
        parts.push(format!("exit {code}"));
    }
    if node.runs > 1 {
        parts.push(format!("×{}", node.runs));
    }
    if node.background {
        parts.push("bg".to_string());
    }
    if node.duration_ms > 0 {
        parts.push(format_trace_duration(node.duration_ms));
    }
    parts.join(" · ")
}

fn is_shell_keyword(token: &str) -> bool {
    matches!(
        token,
        "if" | "then"
            | "else"
            | "elif"
            | "fi"
            | "for"
            | "in"
            | "do"
            | "done"
            | "while"
            | "until"
            | "case"
            | "esac"
            | "select"
            | "function"
    )
}

fn is_shell_operator(token: &str) -> bool {
    matches!(
        token,
        "&&" | "||" | "|" | ";" | ">" | ">>" | "<" | "<<" | "2>&1" | "&>" | "&"
    )
}

fn is_env_assignment(token: &str) -> bool {
    let Some((name, _)) = token.split_once('=') else {
        return false;
    };
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Lightweight OMP-inspired syntax highlighting for a single shell trace statement.
fn push_highlighted_shell_command(
    spans: &mut Vec<Span<'static>>,
    command: &str,
    inactive: bool,
    theme: &Theme,
) {
    if inactive {
        spans.push(Span::styled(
            command.to_string(),
            ratatui::style::Style::default().fg(theme.gray_dim),
        ));
        return;
    }

    let mut expect_command = true;
    let bytes = command.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let rest = &command[i..];
        let ch = rest.chars().next().unwrap();
        if ch.is_whitespace() {
            let start = i;
            while i < bytes.len() {
                let c = command[i..].chars().next().unwrap();
                if !c.is_whitespace() {
                    break;
                }
                i += c.len_utf8();
            }
            spans.push(Span::raw(command[start..i].to_string()));
            continue;
        }

        // Quoted string literal
        if ch == '"' || ch == '\'' {
            let quote = ch;
            let start = i;
            i += ch.len_utf8();
            while i < bytes.len() {
                let c = command[i..].chars().next().unwrap();
                i += c.len_utf8();
                if c == '\\' && quote == '"' && i < bytes.len() {
                    let next = command[i..].chars().next().unwrap();
                    i += next.len_utf8();
                    continue;
                }
                if c == quote {
                    break;
                }
            }
            spans.push(Span::styled(
                command[start..i].to_string(),
                ratatui::style::Style::default().fg(theme.path),
            ));
            expect_command = false;
            continue;
        }

        // Non-whitespace token
        let start = i;
        while i < bytes.len() {
            let c = command[i..].chars().next().unwrap();
            if c.is_whitespace() {
                break;
            }
            i += c.len_utf8();
        }
        let token = &command[start..i];
        let style = if is_shell_operator(token) {
            expect_command = matches!(token, "&&" | "||" | "|" | ";");
            ratatui::style::Style::default().fg(theme.gray)
        } else if is_shell_keyword(token) {
            expect_command = matches!(token, "then" | "else" | "do");
            ratatui::style::Style::default().fg(theme.running)
        } else if expect_command && is_env_assignment(token) {
            ratatui::style::Style::default().fg(theme.accent_verify)
        } else if expect_command {
            expect_command = false;
            ratatui::style::Style::default().fg(theme.command)
        } else if token.starts_with('-') {
            ratatui::style::Style::default().fg(theme.text_secondary)
        } else if token.contains('/') {
            ratatui::style::Style::default().fg(theme.path)
        } else {
            theme.primary()
        };
        spans.push(Span::styled(token.to_string(), style));
    }
}

fn render_shell_trace_row_line(
    row: &TraceDisplayRow<'_>,
    theme: &Theme,
    result_pad: usize,
    max_width: Option<usize>,
) -> (Line<'static>, String) {
    use xai_grok_tools::types::output::ShellTraceNodeState;

    let pad = " ".repeat(result_pad);
    let indent = "  ".repeat(row.depth.min(4));
    let (gutter, gutter_style) = match row.node.state {
        ShellTraceNodeState::Fail => (
            "×",
            ratatui::style::Style::default().fg(theme.accent_error),
        ),
        ShellTraceNodeState::Running => (
            "▶",
            ratatui::style::Style::default().fg(theme.running),
        ),
        ShellTraceNodeState::Skipped => (
            "⊘",
            ratatui::style::Style::default().fg(theme.gray_dim),
        ),
        ShellTraceNodeState::Pending => (
            "·",
            ratatui::style::Style::default().fg(theme.gray_dim),
        ),
        ShellTraceNodeState::Ok => (" ", theme.muted()),
    };

    let pipe_prefix = if row.pipe_continuation { "| " } else { "" };
    let meta = format_trace_row_meta(row.node);
    let meta_suffix = if meta.is_empty() {
        String::new()
    } else {
        format!("  {meta}")
    };

    let raw_cmd = row.node.command.trim();
    let cmd_display = if let Some(width) = max_width {
        let lead_cols = result_pad + 2 + indent.len() + pipe_prefix.len();
        let meta_cols = meta_suffix.chars().count();
        let avail = width.saturating_sub(lead_cols + meta_cols + 1).max(16);
        if raw_cmd.chars().count() > avail {
            let cut: String = raw_cmd.chars().take(avail.saturating_sub(1)).collect();
            format!("{cut}…")
        } else {
            raw_cmd.to_string()
        }
    } else {
        raw_cmd.to_string()
    };

    let inactive = matches!(
        row.node.state,
        ShellTraceNodeState::Skipped | ShellTraceNodeState::Pending
    );
    let mut spans: Vec<Span<'static>> = Vec::new();
    if result_pad > 0 {
        spans.push(Span::raw(pad.clone()));
    }
    spans.push(Span::styled(gutter.to_string(), gutter_style));
    spans.push(Span::raw(format!(" {indent}")));
    if !pipe_prefix.is_empty() {
        spans.push(Span::styled(
            pipe_prefix.to_string(),
            ratatui::style::Style::default().fg(theme.gray_dim),
        ));
    }
    push_highlighted_shell_command(&mut spans, &cmd_display, inactive, theme);
    if !meta_suffix.is_empty() {
        let meta_style = if row.node.state == ShellTraceNodeState::Fail {
            ratatui::style::Style::default().fg(theme.accent_error)
        } else {
            theme.muted()
        };
        spans.push(Span::styled(meta_suffix.clone(), meta_style));
    }

    let plain = format!("{pad}{gutter} {indent}{pipe_prefix}{cmd_display}{meta_suffix}");
    (Line::from(spans), plain)
}

fn render_shell_trace_viewer_lines(
    trace: &xai_grok_tools::types::output::ShellTraceSummary,
    theme: &Theme,
) -> Vec<(Line<'static>, String)> {
    let rows = normalize_shell_trace_rows(trace);
    if rows.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(rows.len() + 1);
    let header = format_trace_header(trace, rows.len());
    out.push((
        Line::from(Span::styled(header.clone(), theme.muted())),
        header,
    ));
    for row in &rows {
        out.push(render_shell_trace_row_line(row, theme, 0, None));
    }
    out
}

/// Drop a leading `Run` / `Running` word (case-insensitive) plus following whitespace so Label headers do not read `Run Run the tests`.
fn strip_leading_run_word(s: &str) -> String {
    let lower = s.to_ascii_lowercase();
    let rest = if let Some(rest) = lower.strip_prefix("running") {
        rest
    } else if let Some(rest) = lower.strip_prefix("run") {
        rest
    } else {
        return s.to_string();
    };
    // Require a word boundary after Run/Running (space or end).
    if rest.is_empty() {
        return String::new();
    }
    if !rest.starts_with(|c: char| c.is_whitespace()) {
        return s.to_string();
    }
    // Map back to original casing via byte length of the prefix consumed (`to_ascii_lowercase` preserves length for ASCII prefixes)
    let prefix_len = s.len() - rest.len();
    s[prefix_len..].trim_start().to_string()
}

impl BlockContent for ExecuteToolCallBlock {
    fn output(&self, ctx: &BlockContext) -> BlockOutput {
        let theme = Theme::current();
        let config = &ctx.appearance.scrollback.blocks.execute;
        let header_style = config.header_style;
        let header_content = crate::appearance::cache::load_execute_header_content();

        // Content width already nets out the bullet; hang indent is only for `$` / `Run` prefix (adding the bullet again would double-count it)
        let content_width = ctx.content_width();
        // Minimal drops the accent gutter; pad results by `$ ` so they line up with the command.
        let result_pad = if ctx.appearance.scrollback.blocks.thinking.rail_under_bullet {
            2
        } else {
            0
        };

        match ctx.mode {
            DisplayMode::Collapsed => {
                let muted = ctx.mute_when_collapsed(config.muted_command_collapsed);
                let mut lines = Vec::new();
                // Collapsed: description title only (no `$ command`) for density; without description, still show the single-line command header
                self.push_header_lines(
                    &mut lines,
                    &theme,
                    header_style,
                    muted,
                    content_width,
                    0,
                    true,
                    header_content,
                );
                BlockOutput { lines }
            }
            DisplayMode::Truncated => self.render_with_truncation(
                &theme,
                content_width,
                Some((config.first_lines as usize, config.last_lines as usize)),
                header_style,
                0,
                header_content,
                result_pad,
            ),
            DisplayMode::Expanded => self.render_with_truncation(
                &theme,
                content_width,
                None,
                header_style,
                0,
                header_content,
                result_pad,
            ),
        }
    }

    fn accent(&self, ctx: &BlockContext) -> Option<AccentStyle> {
        if !ctx.appearance.scrollback.blocks.execute.accent_enabled {
            return None;
        }
        let theme = Theme::current();
        if self.error.is_some() {
            Some(AccentStyle::static_color(theme.accent_error))
        } else if ctx.is_running {
            Some(AccentStyle::animated(
                ctx.appearance.scrollback.blocks.execute.running_accent,
            ))
        } else {
            Some(AccentStyle::static_color(theme.accent_success))
        }
    }

    fn has_vpad_for(&self, _appearance: &AppearanceConfig) -> bool {
        false
    }

    fn background(&self, _ctx: &BlockContext) -> BlockBackground {
        BlockBackground::None
    }

    fn has_raw_mode(&self) -> bool {
        false
    }

    fn is_foldable(&self) -> bool {
        // Collapsed with a description hides `$ command`; expand reveals it (and output/error when present)
        // A bare "Run"/"Running" description strips to empty under Label-style stripping
        // Its collapsed and expanded headers are then identical, so it must not claim a fold
        self.description_display(true).is_some()
            || self.output.is_some()
            || self.error.is_some()
            || self.has_meaningful_trace()
    }

    /// Minimum fold mode used by collapse and the running expand chevron. Agent tools default to Collapsed (title
    /// only), no auto-expand. When finished, Collapsed is always the true minimum (user can fold to title-only).
    fn collapse_mode(&self, is_running: bool) -> DisplayMode {
        // Every density below Detailed folds shells to their one-line row, custom `!` bash included.
        if crate::appearance::cache::load_tool_call_density().shells_collapsed() {
            return DisplayMode::Collapsed;
        }
        if self.bash_mode && is_running {
            DisplayMode::Truncated
        } else {
            DisplayMode::Collapsed
        }
    }

    /// At Detailed ("shells with output") every command starts on the output preview.
    /// Compact densities start folded to the one-line row and stay there.
    fn default_display_mode(&self) -> DisplayMode {
        if crate::appearance::cache::load_tool_call_density().shells_collapsed() {
            DisplayMode::Collapsed
        } else {
            DisplayMode::Truncated
        }
    }

    fn finished_display_mode(&self) -> Option<DisplayMode> {
        if crate::appearance::cache::load_tool_call_density().shells_collapsed() {
            // Compact densities never snap open: a folded row stays folded.
            return None;
        }
        if self.bash_mode {
            // Interactive bash: full output on finish, like a terminal; the streaming preview would silently drop the middle lines
            Some(DisplayMode::Expanded)
        } else {
            // Agent tools keep the preview row; the user can still expand for the full output.
            Some(DisplayMode::Truncated)
        }
    }

    fn preamble(&self, ctx: &BlockContext) -> Option<Text<'static>> {
        let theme = Theme::current();
        let header_style = ctx.appearance.scrollback.blocks.execute.header_style;
        let header_content = crate::appearance::cache::load_execute_header_content();

        // `header_lines` flattens `\n` to spaces, which smashes multi-line commands into one run-on row in the block viewer
        // The viewer draws no bullet, so `ctx.width` (not `content_width()`) is the row width
        let mut header_rows = Vec::new();
        self.push_header_lines(
            &mut header_rows,
            &theme,
            header_style,
            /*muted_command*/ false,
            ctx.width as usize,
            /*extra_indent*/ 0,
            /*truncate_to_width*/ false,
            header_content,
        );
        let mut lines: Vec<Line<'static>> =
            header_rows.into_iter().map(|row| row.content).collect();

        if self.output.is_none()
            && let Some(error) = &self.error
            && !error.is_empty()
        {
            lines.push(Line::from(""));
            let error_style = ratatui::style::Style::default().fg(theme.accent_error);
            for line in error.lines() {
                lines.push(Line::from(Span::styled(line.to_string(), error_style)));
            }
        }

        Some(Text::from(lines))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::appearance::ExecuteHeaderStyle;

    fn line_text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn header_line_uses_header_display_when_set_command_stays_full() {
        let mut block = ExecuteToolCallBlock::new("cd /proj && echo hi");
        block.header_display = Some("echo hi".into());
        assert_eq!(block.command, "cd /proj && echo hi");
        let theme = Theme::current();
        let headers = block.header_lines(
            &theme,
            ExecuteHeaderStyle::Label,
            false,
            ExecuteHeaderContent::TaskNameAndCommand,
        );
        assert_eq!(headers.len(), 1);
        let text = line_text(&headers[0].0);
        assert!(text.contains("echo hi"), "header={text:?}");
        assert!(
            !text.contains("cd /proj"),
            "header must use header_display: {text:?}"
        );
    }

    #[test]
    fn label_header_with_description_shows_title_then_command() {
        let block = ExecuteToolCallBlock::new("cargo test --lib")
            .with_description("Run the unit test suite");
        let theme = Theme::current();
        let headers = block.header_lines(
            &theme,
            ExecuteHeaderStyle::Label,
            false,
            ExecuteHeaderContent::TaskNameAndCommand,
        );
        assert_eq!(headers.len(), 2);
        let title = line_text(&headers[0].0);
        let cmd = line_text(&headers[1].0);
        // Leading "Run " on the description is stripped (Label already has it).
        assert_eq!(title, "Run the unit test suite");
        assert!(cmd.starts_with("$ "), "cmd={cmd:?}");
        assert!(cmd.contains("cargo test --lib"), "cmd={cmd:?}");
        // Prefix span counts: "Run " only on title; "$ " on command.
        assert_eq!(headers[0].1, 1);
        assert_eq!(headers[1].1, 1);
    }

    #[test]
    fn collapsed_header_with_description_hides_command() {
        let block = ExecuteToolCallBlock::new("cargo test --lib")
            .with_description("Run the unit test suite");
        let theme = Theme::current();
        let headers = block.header_lines(
            &theme,
            ExecuteHeaderStyle::Label,
            true,
            ExecuteHeaderContent::TaskName,
        );
        assert_eq!(headers.len(), 1);
        assert_eq!(line_text(&headers[0].0), "Run the unit test suite");
    }

    #[test]
    fn strip_leading_run_word_handles_run_and_running() {
        assert_eq!(strip_leading_run_word("Run the tests"), "the tests");
        assert_eq!(strip_leading_run_word("running checks"), "checks");
        assert_eq!(strip_leading_run_word("runtime config"), "runtime config");
        assert_eq!(strip_leading_run_word("Check status"), "Check status");
    }

    #[test]
    fn shell_header_with_description_shows_title_then_command() {
        let block =
            ExecuteToolCallBlock::new("git status -sb").with_description("Check git status");
        let theme = Theme::current();
        let headers = block.header_lines(
            &theme,
            ExecuteHeaderStyle::Shell,
            false,
            ExecuteHeaderContent::TaskNameAndCommand,
        );
        assert_eq!(headers.len(), 2);
        assert_eq!(line_text(&headers[0].0), "Check git status");
        assert_eq!(headers[0].1, 0);
        let cmd = line_text(&headers[1].0);
        assert!(cmd.starts_with("$ "), "cmd={cmd:?}");
        assert!(cmd.contains("git status -sb"), "cmd={cmd:?}");
    }

    #[test]
    fn label_header_without_description_is_single_run_command_line() {
        let block = ExecuteToolCallBlock::new("echo hi");
        let theme = Theme::current();
        let headers = block.header_lines(
            &theme,
            ExecuteHeaderStyle::Label,
            false,
            ExecuteHeaderContent::TaskName,
        );
        assert_eq!(headers.len(), 1);
        let text = line_text(&headers[0].0);
        assert!(text.starts_with("Run "), "header={text:?}");
        assert!(text.contains("echo hi"), "header={text:?}");
    }

    #[test]
    fn label_header_lines_flattens_command_newlines() {
        // Single-line header path must never embed raw `\n` (ratatui drops them).
        let block = ExecuteToolCallBlock::new("cargo test \\\n  --all");
        let theme = Theme::current();
        let headers = block.header_lines(
            &theme,
            ExecuteHeaderStyle::Label,
            false,
            ExecuteHeaderContent::TaskNameAndCommand,
        );
        assert_eq!(headers.len(), 1);
        let text = line_text(&headers[0].0);
        assert!(
            !text.contains('\n'),
            "label single-line header must flatten newlines: {text:?}"
        );
        assert!(text.contains("cargo test"), "header={text:?}");
        assert!(text.contains("--all"), "header={text:?}");
    }

    #[test]
    fn label_expanded_soft_wraps_multiline_command_like_permission_panel() {
        let block = ExecuteToolCallBlock::new(
            "git status --short --branch && cargo test --workspace --all-features",
        );
        let theme = Theme::current();
        let mut lines = Vec::new();
        block.push_header_lines(
            &mut lines,
            &theme,
            ExecuteHeaderStyle::Label,
            false,
            40,
            0,
            false, // expanded: soft-wrap
            ExecuteHeaderContent::TaskName,
        );
        assert!(
            lines.len() >= 2,
            "expected operator soft-wrap rows, got {}",
            lines.len()
        );
        let first = line_text(&lines[0].content);
        assert!(
            first.starts_with("Run "),
            "Label soft-wrap first row needs Run prefix: {first:?}"
        );
        assert!(
            first.contains("&&"),
            "first row should keep the operator: {first:?}"
        );
        assert!(
            !first.contains('\n'),
            "each BlockLine is one visual row: {first:?}"
        );
        let second = line_text(&lines[1].content);
        assert!(
            second.trim_start().starts_with("cargo"),
            "continuation under hang: {second:?}"
        );
        assert_eq!(
            lines[1].joiner.as_deref(),
            Some("\n"),
            "copy must preserve line breaks between soft-wrap rows"
        );
    }

    #[test]
    fn header_content_modes_choose_task_name_and_command_fields() {
        let block = ExecuteToolCallBlock::new("cargo test --workspace")
            .with_description("Run workspace tests");
        let theme = Theme::current();

        let command_only = block.header_lines(
            &theme,
            ExecuteHeaderStyle::Label,
            false,
            ExecuteHeaderContent::CommandOnly,
        );
        assert_eq!(command_only.len(), 1);
        let command_text = line_text(&command_only[0].0);
        assert!(command_text.contains("cargo test --workspace"));
        assert!(!command_text.contains("workspace tests"));

        let task_only = block.header_lines(
            &theme,
            ExecuteHeaderStyle::Label,
            false,
            ExecuteHeaderContent::TaskName,
        );
        assert_eq!(task_only.len(), 1);
        assert_eq!(line_text(&task_only[0].0), "Run workspace tests");

        let both = block.header_lines(
            &theme,
            ExecuteHeaderStyle::Label,
            false,
            ExecuteHeaderContent::TaskNameAndCommand,
        );
        assert_eq!(both.len(), 2);
        assert_eq!(line_text(&both[0].0), "Run workspace tests");
        assert!(line_text(&both[1].0).contains("cargo test --workspace"));
    }

    #[test]
    fn task_name_mode_falls_back_to_highlighted_command_without_description() {
        let block = ExecuteToolCallBlock::new("git status --short");
        let theme = Theme::current();
        let lines = block.header_lines(
            &theme,
            ExecuteHeaderStyle::Label,
            false,
            ExecuteHeaderContent::TaskName,
        );
        assert_eq!(lines.len(), 1);
        assert!(line_text(&lines[0].0).contains("git status --short"));
        assert!(
            lines[0].0.spans.len() > 2,
            "bash tokens should retain syntax spans"
        );
    }

    #[test]
    fn empty_description_treated_as_absent() {
        let block = ExecuteToolCallBlock::new("echo hi").with_description("   \n  ");
        let theme = Theme::current();
        let headers = block.header_lines(
            &theme,
            ExecuteHeaderStyle::Label,
            false,
            ExecuteHeaderContent::TaskName,
        );
        assert_eq!(headers.len(), 1);
        assert!(line_text(&headers[0].0).contains("echo hi"));
    }

    #[test]
    fn label_bash_mode_with_description_keeps_user_marker_on_title() {
        let mut block = ExecuteToolCallBlock::new("ls -la").with_description("List files");
        block.bash_mode = true;
        let theme = Theme::current();
        let headers = block.header_lines(
            &theme,
            ExecuteHeaderStyle::Label,
            false,
            ExecuteHeaderContent::TaskNameAndCommand,
        );
        assert_eq!(headers.len(), 2);
        let title = line_text(&headers[0].0);
        assert_eq!(title, "Run (user) List files");
        assert_eq!(headers[0].1, 2); // "Run " and "(user) "
    }

    #[test]
    fn description_makes_block_foldable_to_reveal_command() {
        let block = ExecuteToolCallBlock::new("true").with_description("no-op");
        assert!(block.is_foldable());
        let with_output = block.clone().with_output("ok\n");
        assert!(with_output.is_foldable());
        let no_desc = ExecuteToolCallBlock::new("true");
        assert!(!no_desc.is_foldable());
    }

    /// Density owns the shell default mode: Detailed shows the output preview,
    /// every compact level stores a one-line row and never auto-opens on finish.
    #[test]
    fn execute_display_modes_follow_tool_call_density() {
        use crate::appearance::cache::{ToolCallDensity, set_tool_call_density};

        set_tool_call_density(ToolCallDensity::Detailed);
        let agent = ExecuteToolCallBlock::new("echo hi").with_description("say hi");
        assert_eq!(agent.default_display_mode(), DisplayMode::Truncated);
        assert_eq!(agent.collapse_mode(true), DisplayMode::Collapsed);
        assert_eq!(agent.collapse_mode(false), DisplayMode::Collapsed);
        assert_eq!(agent.finished_display_mode(), Some(DisplayMode::Truncated));

        let mut bash = ExecuteToolCallBlock::new("ls");
        bash.bash_mode = true;
        // User ! streams truncated, finishes with the full output.
        assert_eq!(bash.default_display_mode(), DisplayMode::Truncated);
        assert_eq!(bash.finished_display_mode(), Some(DisplayMode::Expanded));
        assert_eq!(bash.collapse_mode(true), DisplayMode::Truncated);
        assert_eq!(bash.collapse_mode(false), DisplayMode::Collapsed);

        set_tool_call_density(ToolCallDensity::CompactShells);
        let agent = ExecuteToolCallBlock::new("echo hi").with_description("say hi");
        assert_eq!(agent.default_display_mode(), DisplayMode::Collapsed);
        assert_eq!(agent.collapse_mode(true), DisplayMode::Collapsed);
        assert_eq!(agent.finished_display_mode(), None);
        let mut bash = ExecuteToolCallBlock::new("ls");
        bash.bash_mode = true;
        assert_eq!(bash.default_display_mode(), DisplayMode::Collapsed);
        assert_eq!(bash.finished_display_mode(), None);
        assert_eq!(bash.collapse_mode(true), DisplayMode::Collapsed);

        let failed_bash = {
            let mut b = ExecuteToolCallBlock::new("pytest").with_error("exit 2");
            b.bash_mode = true;
            b
        };
        assert_eq!(failed_bash.default_display_mode(), DisplayMode::Collapsed);
        assert_eq!(failed_bash.finished_display_mode(), None);

        set_tool_call_density(ToolCallDensity::CompactAllGrouped);
    }

    #[test]
    fn bare_run_description_is_not_foldable() {
        let bare = ExecuteToolCallBlock::new("echo hi").with_description("Run");
        assert!(
            !bare.is_foldable(),
            "Label strips bare Run; headers identical → not foldable"
        );
        let real = ExecuteToolCallBlock::new("echo hi").with_description("say hi");
        assert!(real.is_foldable());
    }

    #[test]
    fn test_push_output_basic() {
        let mut block = ExecuteToolCallBlock::new("echo test");
        block.push_output("hello ");
        block.push_output("world");
        block.finish();
        assert_eq!(block.output, Some("hello world".to_string()));
    }

    #[test]
    fn test_push_output_multiple_chunks() {
        let mut block = ExecuteToolCallBlock::new("cargo build");
        block.push_output("Com");
        block.push_output("pili");
        block.push_output("ng ");
        block.push_output("crat");
        block.push_output("e\n");
        block.finish();
        assert_eq!(block.output, Some("Compiling crate\n".to_string()));
    }

    #[test]
    fn test_with_output() {
        let block = ExecuteToolCallBlock::new("echo test").with_output("plain text output");
        assert_eq!(block.output, Some("plain text output".to_string()));
    }

    #[test]
    fn preamble_multiline_command_keeps_separate_lines() {
        // Regression: the block viewer preamble flattened `\n` to spaces, smashing multi-line commands into one row
        let block = ExecuteToolCallBlock::new("export XAI_ROOT=/tmp\ncd /tmp\necho start");
        let mut appearance = AppearanceConfig::default();
        appearance.scrollback.blocks.execute.header_style = ExecuteHeaderStyle::Shell;
        let ctx = BlockContext {
            mode: DisplayMode::Expanded,
            is_running: false,
            width: 120,
            raw: false,
            max_lines: None,
            appearance,
            is_selected: false,
            cwd: None,
        };
        let plain: Vec<String> = block
            .preamble(&ctx)
            .expect("execute preamble is always present")
            .lines
            .iter()
            .map(line_text)
            .collect();
        assert_eq!(
            plain,
            vec!["$ export XAI_ROOT=/tmp", "  cd /tmp", "  echo start"]
        );
    }

    #[test]
    fn shell_trace_renders_per_command_timing_and_preserves_collapsed_compat() {
        use xai_grok_tools::types::output::{
            ShellTraceNode, ShellTraceNodeState, ShellTraceSummary,
        };

        let trace = ShellTraceSummary {
            version: 1,
            total_duration_ms: 42,
            exit_code: Some(1),
            truncated: false,
            nodes: vec![
                ShellTraceNode {
                    id: 0,
                    parent_id: None,
                    depth: 0,
                    kind: "and_or".to_string(),
                    command: "echo ok && false && echo skipped".to_string(),
                    state: ShellTraceNodeState::Fail,
                    exit_code: Some(1),
                    duration_ms: 42,
                    runs: 1,
                    background: false,
                },
                ShellTraceNode {
                    id: 1,
                    parent_id: Some(0),
                    depth: 1,
                    kind: "simple".to_string(),
                    command: "echo ok".to_string(),
                    state: ShellTraceNodeState::Ok,
                    exit_code: Some(0),
                    duration_ms: 5,
                    runs: 1,
                    background: false,
                },
                ShellTraceNode {
                    id: 2,
                    parent_id: Some(0),
                    depth: 1,
                    kind: "simple".to_string(),
                    command: "false".to_string(),
                    state: ShellTraceNodeState::Fail,
                    exit_code: Some(1),
                    duration_ms: 37,
                    runs: 1,
                    background: false,
                },
                ShellTraceNode {
                    id: 3,
                    parent_id: Some(0),
                    depth: 1,
                    kind: "simple".to_string(),
                    command: "echo skipped".to_string(),
                    state: ShellTraceNodeState::Skipped,
                    exit_code: None,
                    duration_ms: 0,
                    runs: 1,
                    background: false,
                },
            ],
        };

        let block = ExecuteToolCallBlock::new("echo ok && false && echo skipped")
            .with_output("ok\n")
            .with_shell_trace(trace);
        assert!(block.is_foldable());

        let make_ctx = |mode| BlockContext {
            mode,
            is_running: false,
            width: 100,
            raw: false,
            max_lines: None,
            appearance: AppearanceConfig::default(),
            is_selected: false,
            cwd: None,
        };

        let collapsed: Vec<String> = block
            .output(&make_ctx(DisplayMode::Collapsed))
            .lines
            .iter()
            .map(|l| line_text(&l.content))
            .collect();
        assert_eq!(collapsed.len(), 1);

        let expanded: Vec<String> = block
            .output(&make_ctx(DisplayMode::Expanded))
            .lines
            .iter()
            .map(|l| line_text(&l.content))
            .collect();
        let joined = expanded.join("\n");
        assert!(joined.contains("Trace · 3 steps · 42ms · exit 1"));
        assert!(joined.contains("  echo ok  5ms"));
        assert!(joined.contains("× false  exit 1 · 37ms"));
        assert!(joined.contains("⊘ echo skipped"));
        assert!(!joined.contains("(0ms)"));
    }

    #[test]
    fn shell_trace_deduplicates_single_command_pipeline_wrapper_in_scrollback() {
        use xai_grok_tools::types::output::{
            ShellTraceNode, ShellTraceNodeState, ShellTraceSummary,
        };

        let trace = ShellTraceSummary {
            version: 1,
            total_duration_ms: 8,
            exit_code: Some(0),
            truncated: false,
            nodes: vec![
                ShellTraceNode {
                    id: 0,
                    parent_id: None,
                    depth: 0,
                    kind: "pipeline".to_string(),
                    command: "echo hi".to_string(),
                    state: ShellTraceNodeState::Ok,
                    exit_code: Some(0),
                    duration_ms: 8,
                    runs: 1,
                    background: false,
                },
                ShellTraceNode {
                    id: 1,
                    parent_id: Some(0),
                    depth: 1,
                    kind: "simple".to_string(),
                    command: "echo hi".to_string(),
                    state: ShellTraceNodeState::Ok,
                    exit_code: Some(0),
                    duration_ms: 8,
                    runs: 1,
                    background: false,
                },
            ],
        };

        let block = ExecuteToolCallBlock::new("echo hi")
            .with_output("hi\n")
            .with_shell_trace(trace);
        assert!(!block.has_meaningful_trace());
        let viewer_lines = block.render_trace_viewer_lines(&Theme::current());
        assert_eq!(viewer_lines.len(), 2);
        assert!(viewer_lines[0].1.contains("Trace · 1 step · 8ms"));
        assert!(viewer_lines[1].1.contains("echo hi  8ms"));
    }
}
