use anyhow::{Context, Result};
use std::{
    fs::File,
    io::Write,
    path::{Path, PathBuf},
};
use tempfile::TempDir;

/// Materialized pi-grok-recap extension bundle. The source directory remains
/// alive for the Pi process lifetime so relative imports between the authored
/// TypeScript modules continue to resolve.
pub(super) struct RecapExtension {
    _source_dir: TempDir,
    source_path: PathBuf,
}

impl RecapExtension {
    pub(super) fn source_path(&self) -> &Path {
        &self.source_path
    }
}

fn write_source_file(dir: &Path, name: &str, source: &str) -> Result<PathBuf> {
    let path = dir.join(name);
    let mut file =
        File::create(&path).with_context(|| format!("create Pi recap extension module {name}"))?;
    file.write_all(source.as_bytes())
        .with_context(|| format!("write Pi recap extension module {name}"))?;
    file.flush()
        .with_context(|| format!("flush Pi recap extension module {name}"))?;
    file.sync_all().ok();
    Ok(path)
}

/// Inject a headless Pi extension that generates display-only session recaps
/// via `complete()` and appends `pi-grok-recap/v1` custom entries for the adapter.
/// Every authored module must be materialized: Pi loads the entry `index.ts`
/// from the temp directory and resolves its relative imports there.
pub(super) fn write_recap_extension() -> Result<RecapExtension> {
    let source_dir = tempfile::Builder::new()
        .prefix("pi-grok-recap-")
        .tempdir()
        .context("create Pi recap extension source directory")?;
    let source_path = write_source_file(
        source_dir.path(),
        "index.ts",
        include_str!("../../../../../../extensions/pi-grok-recap/index.ts"),
    )?;
    write_source_file(
        source_dir.path(),
        "shared.ts",
        include_str!("../../../../../../extensions/pi-grok-recap/shared.ts"),
    )?;
    write_source_file(
        source_dir.path(),
        "args.ts",
        include_str!("../../../../../../extensions/pi-grok-recap/args.ts"),
    )?;
    write_source_file(
        source_dir.path(),
        "prompt.ts",
        include_str!("../../../../../../extensions/pi-grok-recap/prompt.ts"),
    )?;
    write_source_file(
        source_dir.path(),
        "clean.ts",
        include_str!("../../../../../../extensions/pi-grok-recap/clean.ts"),
    )?;
    write_source_file(
        source_dir.path(),
        "session.ts",
        include_str!("../../../../../../extensions/pi-grok-recap/session.ts"),
    )?;
    write_source_file(
        source_dir.path(),
        "model.ts",
        include_str!("../../../../../../extensions/pi-grok-recap/model.ts"),
    )?;
    Ok(RecapExtension {
        _source_dir: source_dir,
        source_path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Bundle {
        index: String,
        shared: String,
        args: String,
        prompt: String,
        clean: String,
        session: String,
        model: String,
    }

    fn write_bundle() -> (RecapExtension, Bundle) {
        let extension = write_recap_extension().expect("write extension");
        let dir = extension.source_path().parent().expect("source dir");
        let read = |name: &str| {
            std::fs::read_to_string(dir.join(name)).unwrap_or_else(|_| panic!("read {name} module"))
        };
        let bundle = Bundle {
            index: read("index.ts"),
            shared: read("shared.ts"),
            args: read("args.ts"),
            prompt: read("prompt.ts"),
            clean: read("clean.ts"),
            session: read("session.ts"),
            model: read("model.ts"),
        };
        (extension, bundle)
    }

    #[test]
    fn recap_extension_materializes_every_relative_import() {
        // Regression guard (mirrors the pi-grok-bash bundle): a module added to
        // the authored extension but not to the injector breaks Pi bootstrap
        // with `Cannot find module './<name>.ts'` at RPC startup.
        let (extension, bundle) = write_bundle();
        let dir = extension.source_path().parent().expect("source dir");
        let sources = [
            ("index.ts", &bundle.index),
            ("shared.ts", &bundle.shared),
            ("args.ts", &bundle.args),
            ("prompt.ts", &bundle.prompt),
            ("clean.ts", &bundle.clean),
            ("session.ts", &bundle.session),
            ("model.ts", &bundle.model),
        ];
        for (name, source) in sources {
            for caps in source.match_indices("./") {
                let rest = &source[caps.0 + 2..];
                let end = rest.find('"').expect("relative import closing quote");
                let target = &rest[..end];
                assert!(
                    target.ends_with(".ts"),
                    "{name} imports {target:?} without an explicit .ts extension"
                );
                assert!(
                    dir.join(target).is_file(),
                    "{name} imports ./{target}; injector must materialize it"
                );
            }
        }
        assert_eq!(
            extension
                .source_path()
                .extension()
                .and_then(|value| value.to_str()),
            Some("ts")
        );
    }

    #[test]
    fn recap_extension_source_is_a_loadable_typescript_module() {
        let (_, bundle) = write_bundle();
        assert!(bundle.index.contains("registerCommand(COMMAND"));
        assert!(bundle.shared.contains("pi-grok-recap/v1"));
        assert!(
            bundle
                .index
                .contains("from \"@earendil-works/pi-ai/compat\"")
        );
        assert!(
            bundle
                .session
                .contains("entry.type === \"message\" && entry.message")
        );
        assert!(bundle.shared.contains("AUTO_MIN_TURNS = 3"));
        assert!(bundle.shared.contains("AUTO_MIN_IDLE_MS = 3 * 60 * 1000"));
        assert!(bundle.shared.contains("MAX_RECAP_CONTEXT_CHARS = 12_000"));
        assert!(bundle.session.contains("lastSuccessfulRecapTurnCount"));
        assert!(
            bundle
                .model
                .contains("if (!modelRef || !modelRef.trim()) return undefined")
        );
        assert!(bundle.model.contains("function modelChain"));
        assert!(bundle.index.contains("for (const modelRef of chain)"));
        assert!(
            bundle
                .model
                .contains("const canonicalSeparator = raw.indexOf(\"::\")")
        );
        assert!(bundle.model.contains("`${m.provider}::${m.id}` === raw"));
        assert!(bundle.index.contains("{ messages: [userMessage] }"));
        assert!(bundle.index.contains("parsed.thinkingLevel"));
        assert!(bundle.index.contains("reasoning:"));
        assert!(
            bundle.index.contains(
                "response.stopReason === \"aborted\" || response.stopReason === \"error\""
            )
        );
        assert!(bundle.prompt.contains("operating-system language"));
        assert!(bundle.prompt.contains("Do not switch to English"));
        assert!(!bundle.index.contains("serializeConversation"));
        assert!(!bundle.shared.contains("serializeConversation"));
        assert!(!bundle.session.contains("serializeConversation"));
        assert!(!bundle.model.contains("serializeConversation"));
    }

    #[test]
    fn recap_extension_only_persists_successful_summaries() {
        let (_, bundle) = write_bundle();
        assert!(
            bundle
                .index
                .contains("function emitSummary(summary: string, auto: boolean)")
        );
        assert!(bundle.index.contains("ok: true,"));
        assert!(!bundle.index.contains("ok: false"));
        assert!(!bundle.index.contains("reason: payload.reason"));
    }

    #[test]
    fn recap_extension_keeps_bridge_traffic_out_of_the_agent_loop() {
        // Regression: emitSummary used to deliver via sendMessage, which pushes
        // the summary into agent.state.messages when idle (convertToLlm maps it
        // onto a user message) or steers the parent mid-turn when streaming.
        // Bridge traffic must be appended custom entries only.
        let (_, bundle) = write_bundle();
        assert!(bundle.index.contains("pi.appendEntry(BRIDGE_TYPE, {"));
        assert!(!bundle.index.contains("pi.sendMessage"));
        assert!(bundle.index.contains("pi.on(\"context\""));
    }

    #[test]
    fn recap_extension_picks_mermaid_layout_from_terminal_width() {
        let (_, bundle) = write_bundle();
        assert!(bundle.shared.contains("MERMAID_LR_MIN_COLS = 110"));
        assert!(bundle.prompt.contains("function mermaidLayoutInstruction"));
        assert!(bundle.prompt.contains("terminalWidth"));
        assert!(bundle.prompt.contains("`flowchart LR`"));
        assert!(bundle.prompt.contains("`flowchart TD`"));
    }

    #[test]
    fn recap_extension_keeps_mermaid_closing_fence() {
        // Regression: cleanRecapMarkdown must strip ONLY the outer
        // ```markdown wrapper, never the closing ``` of a mermaid block —
        // an unclosed fence falls back to a raw code block instead of the
        // native diagram art.
        let (_, bundle) = write_bundle();
        // The trailing-fence strip is guarded by a ```markdown wrapper check,
        // so a bare trailing ``` (a mermaid closing fence) is never removed.
        assert!(bundle.clean.contains("/^```markdown\\s/i.test(text)"));
        assert!(
            bundle
                .clean
                .contains("Only strip a trailing fence when the response was wrapped")
        );
    }
}
