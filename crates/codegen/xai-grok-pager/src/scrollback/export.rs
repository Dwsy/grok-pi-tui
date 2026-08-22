//! Pure functions for exporting a conversation transcript as human-readable Markdown.
//!
//! Used by the `/export` slash command (and its dispatch handler). The converter walks
//! `RenderBlock`s and produces `## User` / `## Assistant` / `## Tools` sections with
//! compact one-line tool summaries. Non-conversation blocks (system chrome, thinking,
//! subagent lifecycle, etc.) are intentionally skipped so the output is useful for
//! "continue elsewhere" or archival.

use super::{RenderBlock, ToolCallBlock};

/// Convert an iterator of `RenderBlock` references into a Markdown transcript.
///
/// The output is a clean, readable document suitable for saving or clipboard:
/// - `## User` for user prompts (raw text)
/// - `## Assistant` for agent responses (prefers raw source Markdown via `copy_text(true)`)
/// - `## Tools` section with one-line summaries for every tool call kind
///
/// Consecutive assistant messages are coalesced under a single header.
/// Thinking / system / subagent / credit / etc. blocks are skipped.
///
/// This function is pure and easily unit-testable with synthetic blocks (including `Stub`).
pub fn render_blocks_to_markdown<'a>(blocks: impl IntoIterator<Item = &'a RenderBlock>) -> String {
    let mut out = String::new();
    let mut last_was_agent = false;
    let mut in_tools_section = false;

    for b in blocks {
        match b {
            RenderBlock::UserPrompt(u) => {
                if in_tools_section {
                    out.push('\n');
                    in_tools_section = false;
                }
                out.push_str("## User\n\n");
                out.push_str(&u.copy_text());
                out.push_str("\n\n");
                last_was_agent = false;
            }
            RenderBlock::AgentMessage(a) => {
                if !last_was_agent {
                    if in_tools_section {
                        out.push('\n');
                        in_tools_section = false;
                    }
                    out.push_str("## Assistant\n\n");
                }
                // Prefer raw source Markdown for fidelity in the exported .md
                out.push_str(&a.copy_text(true));
                out.push_str("\n\n");
                last_was_agent = true;
            }
            RenderBlock::ToolCall(tc) => {
                if matches!(tc, ToolCallBlock::Eval(eval) if eval.effects_first()) {
                    continue;
                }
                if !in_tools_section {
                    out.push_str("## Tools\n\n");
                    in_tools_section = true;
                }
                out.push_str("- ");
                out.push_str(&tool_summary(tc));
                out.push('\n');
                last_was_agent = false;
            }
            // Skip all non-conversation chrome: Thinking, System, SessionEvent, BgTask,
            // Subagent, Btw, CreditLimit, Stub, etc. Thinking blocks are
            // treated as intra-Assistant glue (no new header).
            _ => {}
        }
    }

    let trimmed_len = out.trim_end().len();
    out.truncate(trimmed_len);
    out
}

fn tool_summary(tc: &ToolCallBlock) -> String {
    match tc {
        ToolCallBlock::Read(r) => {
            let range = r
                .line_range
                .as_ref()
                .map_or(String::new(), |lr| format!(" ({})", lr));
            format!("Read: {}{}", r.path, range)
        }
        ToolCallBlock::Edit(e) => format!("Edit: {}", e.path),
        ToolCallBlock::Execute(ex) => {
            let desc = ex
                .description
                .as_deref()
                .map_or(String::new(), |d| format!(" ({})", d));
            format!("Execute: {}{}", ex.command, desc)
        }
        ToolCallBlock::Eval(eval) => {
            let label = eval.title.as_deref().unwrap_or(eval.language.as_str());
            format!("Eval: {label}")
        }
        ToolCallBlock::ListDir(l) => format!("ListDir: {}", l.path),
        ToolCallBlock::Search(s) => format!("Search: {}", s.pattern),
        ToolCallBlock::WebFetch(w) => format!("WebFetch: {}", w.url),
        ToolCallBlock::WebSearch(w) => format!("WebSearch: {}", w.query),
        ToolCallBlock::UseTool(u) => format!("UseTool: {}", u.tool_name),
        ToolCallBlock::IntegrationSearch(_) => "IntegrationSearch (MCP tool discovery)".into(),
        ToolCallBlock::MemorySearch(_) => "MemorySearch".into(),
        ToolCallBlock::Skill(o) | ToolCallBlock::Other(o) => format!("Tool: {}", o.name),
        ToolCallBlock::Lifecycle(_) => "Lifecycle event".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_blocks_yield_empty_string() {
        let out = render_blocks_to_markdown(std::iter::empty::<&RenderBlock>());
        assert!(out.is_empty());
    }

    #[test]
    fn effects_first_omits_v2_eval_but_keeps_nested_tool_entries() {
        crate::appearance::cache::set_pi_eval_v2_effects_first(true);
        let blocks = vec![
            RenderBlock::ToolCall(ToolCallBlock::Eval(
                crate::scrollback::blocks::tool::EvalToolCallBlock::new(
                    "js",
                    "await tool.read({ path: 'README.md' })",
                )
                .with_bridge_version("v2"),
            )),
            RenderBlock::read("README.md", None),
        ];
        let out = render_blocks_to_markdown(blocks.iter());
        assert!(!out.contains("Eval:"));
        assert!(!out.contains("tool.read"));
        assert!(out.contains("Read: README.md"));
    }

    #[test]
    fn legacy_mode_exports_v2_eval_summary() {
        crate::appearance::cache::set_pi_eval_v2_effects_first(false);
        let blocks = [RenderBlock::ToolCall(ToolCallBlock::Eval(
            crate::scrollback::blocks::tool::EvalToolCallBlock::new("js", "1 + 1")
                .with_bridge_version("v2"),
        ))];
        let out = render_blocks_to_markdown(blocks.iter());
        assert!(out.contains("Eval: js"));
        crate::appearance::cache::set_pi_eval_v2_effects_first(true);
    }
}
