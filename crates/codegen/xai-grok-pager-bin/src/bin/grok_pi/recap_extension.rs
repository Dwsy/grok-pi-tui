use anyhow::{Context, Result};
use std::{fs::File, io::Write};
use tempfile::NamedTempFile;

/// Inject a headless Pi extension that generates display-only session recaps
/// via `complete()` and appends `pi-grok-recap/v1` custom entries for the adapter.
pub(super) fn write_recap_extension() -> Result<NamedTempFile> {
    let mut file = tempfile::Builder::new()
        .prefix("pi-grok-recap-")
        .suffix(".ts")
        .tempfile()
        .context("create Pi recap extension tempfile")?;
    const SOURCE: &str = include_str!("../../../../../../extensions/pi-grok-recap/index.ts");
    file.write_all(SOURCE.as_bytes())
        .context("write Pi recap extension source")?;
    file.flush().context("flush Pi recap extension source")?;
    File::open(file.path()).and_then(|f| f.sync_all()).ok();
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recap_extension_source_is_a_loadable_typescript_module() {
        let file = write_recap_extension().expect("temp extension");
        let source = std::fs::read_to_string(file.path()).expect("read extension");
        assert!(source.contains("__pi_grok_recap"));
        assert!(source.contains("registerCommand(COMMAND"));
        assert!(source.contains("pi-grok-recap/v1"));
        assert!(source.contains("from \"@earendil-works/pi-ai/compat\""));
        assert!(source.contains("entry.type === \"message\" && entry.message"));
        assert!(source.contains("AUTO_MIN_TURNS = 3"));
        assert!(source.contains("AUTO_MIN_IDLE_MS = 3 * 60 * 1000"));
        assert!(source.contains("MAX_RECAP_CONTEXT_CHARS = 12_000"));
        assert!(source.contains("lastSuccessfulRecapTurnCount"));
        assert!(source.contains("if (!modelRef || !modelRef.trim()) return undefined"));
        assert!(source.contains("function modelChain"));
        assert!(source.contains("for (const modelRef of chain)"));
        assert!(source.contains("const canonicalSeparator = raw.indexOf(\"::\")"));
        assert!(source.contains("`${m.provider}::${m.id}` === raw"));
        assert!(source.contains("{ messages: [userMessage] }"));
        assert!(source.contains("parsed.thinkingLevel"));
        assert!(source.contains("reasoning:"));
        assert!(
            source.contains(
                "response.stopReason === \"aborted\" || response.stopReason === \"error\""
            )
        );
        assert!(source.contains("operating-system language"));
        assert!(source.contains("Do not switch to English"));
        assert!(!source.contains("serializeConversation"));
        assert!(file.path().extension().and_then(|e| e.to_str()) == Some("ts"));
    }

    #[test]
    fn recap_extension_only_persists_successful_summaries() {
        let file = write_recap_extension().expect("temp extension");
        let source = std::fs::read_to_string(file.path()).expect("read extension");
        assert!(source.contains("function emitSummary(summary: string, auto: boolean)"));
        assert!(source.contains("ok: true,"));
        assert!(!source.contains("ok: false"));
        assert!(!source.contains("reason: payload.reason"));
    }

    #[test]
    fn recap_extension_keeps_bridge_traffic_out_of_the_agent_loop() {
        // Regression: emitSummary used to deliver via sendMessage, which pushes
        // the summary into agent.state.messages when idle (convertToLlm maps it
        // onto a user message) or steers the parent mid-turn when streaming.
        // Bridge traffic must be appended custom entries only.
        let file = write_recap_extension().expect("temp extension");
        let source = std::fs::read_to_string(file.path()).expect("read extension");
        assert!(source.contains("pi.appendEntry(BRIDGE_TYPE, {"));
        assert!(!source.contains("pi.sendMessage"));
        assert!(source.contains("pi.on(\"context\""));
    }

    #[test]
    fn recap_extension_picks_mermaid_layout_from_terminal_width() {
        let file = write_recap_extension().expect("temp extension");
        let source = std::fs::read_to_string(file.path()).expect("read extension");
        assert!(source.contains("MERMAID_LR_MIN_COLS = 110"));
        assert!(source.contains("function mermaidLayoutInstruction"));
        assert!(source.contains("terminalWidth"));
        assert!(source.contains("`flowchart LR`"));
        assert!(source.contains("`flowchart TD`"));
    }

    #[test]
    fn recap_extension_keeps_mermaid_closing_fence() {
        // Regression: cleanRecapMarkdown must strip ONLY the outer
        // ```markdown wrapper, never the closing ``` of a mermaid block —
        // an unclosed fence falls back to a raw code block instead of the
        // native diagram art.
        let file = write_recap_extension().expect("temp extension");
        let source = std::fs::read_to_string(file.path()).expect("read extension");
        // The trailing-fence strip is guarded by a ```markdown wrapper check,
        // so a bare trailing ``` (a mermaid closing fence) is never removed.
        assert!(source.contains("/^```markdown\\s/i.test(text)"));
        assert!(source.contains("Only strip a trailing fence when the response was wrapped"));
    }
}
