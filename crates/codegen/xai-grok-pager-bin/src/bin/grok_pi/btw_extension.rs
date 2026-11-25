use anyhow::Result;
use std::path::{Path, PathBuf};

/// Materialized /btw extension bundle in a content-addressed cache directory
/// so `jiti`'s `fsCache` hits across launches.
pub(super) struct BtwExtension {
    source_path: PathBuf,
}

impl BtwExtension {
    pub(super) fn source_path(&self) -> &Path {
        &self.source_path
    }
}

/// Inject a headless Pi extension that answers `/btw` side questions via
/// `streamSimple()`, persists BTW answers as Pi custom entries, and appends
/// `pi-grok-btw/v1` custom entries for the adapter.
///
/// Every authored module must be materialized here — the injector owns the
/// transitive closure of `index.ts`'s relative imports (see AGENTS.md
/// "Diagnosing Pi RPC bootstrap / extension failures").
pub(super) fn write_btw_extension() -> Result<BtwExtension> {
    let root = super::tui_kit::materialize_cached_bundle(
        "pi-grok-btw",
        &[
            (
                "index.ts",
                include_str!("../../../../../../extensions/pi-grok-btw/index.ts"),
            ),
            (
                "shared.ts",
                include_str!("../../../../../../extensions/pi-grok-btw/shared.ts"),
            ),
            (
                "context.ts",
                include_str!("../../../../../../extensions/pi-grok-btw/context.ts"),
            ),
            (
                "models.ts",
                include_str!("../../../../../../extensions/pi-grok-btw/models.ts"),
            ),
            (
                "bridge.ts",
                include_str!("../../../../../../extensions/pi-grok-btw/bridge.ts"),
            ),
        ],
    )?;
    Ok(BtwExtension {
        source_path: root.join("index.ts"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Bundle {
        index: String,
        shared: String,
        context: String,
        models: String,
        bridge: String,
    }

    fn write_bundle() -> (BtwExtension, Bundle) {
        let extension = write_btw_extension().expect("write extension");
        let dir = extension.source_path().parent().expect("source dir");
        let read = |name: &str| {
            std::fs::read_to_string(dir.join(name)).unwrap_or_else(|e| panic!("read {name}: {e}"))
        };
        let bundle = Bundle {
            index: read("index.ts"),
            shared: read("shared.ts"),
            context: read("context.ts"),
            models: read("models.ts"),
            bridge: read("bridge.ts"),
        };
        (extension, bundle)
    }

    #[test]
    fn btw_extension_materializes_every_module_of_the_entry_closure() {
        let (extension, bundle) = write_bundle();
        // Entry registers both commands and the legacy context filter.
        assert!(bundle.index.contains("registerCommand(COMMAND"));
        assert!(bundle.index.contains("registerCommand(HISTORY_COMMAND"));
        assert!(bundle.index.contains("pi.on(\"context\""));
        // Every relative import of the entry closure is materialized.
        assert!(bundle.index.contains("from \"./bridge.ts\""));
        assert!(bundle.index.contains("from \"./shared.ts\""));
        assert!(bundle.bridge.contains("from \"./context.ts\""));
        assert!(bundle.bridge.contains("from \"./models.ts\""));
        assert!(bundle.bridge.contains("from \"./shared.ts\""));
        assert!(bundle.context.contains("from \"./shared.ts\""));
        assert!(bundle.models.contains("from \"./shared.ts\""));
        // Per-module load-bearing symbols.
        assert!(bundle.shared.contains("pi-grok-btw/v1"));
        assert!(bundle.shared.contains("pi-grok-btw/history/v1"));
        assert!(bundle.context.contains("stripIncompleteTail"));
        assert!(bundle.context.contains("buildSideContext"));
        assert!(bundle.models.contains("resolveModel"));
        assert!(bundle.models.contains("modelChain"));
        assert!(
            bundle
                .bridge
                .contains("from \"@earendil-works/pi-ai/compat\"")
        );
        assert!(bundle.bridge.contains("All /btw models failed"));
        assert!(bundle.bridge.contains("handleBtwCommand"));
        assert_eq!(
            extension
                .source_path()
                .extension()
                .and_then(|value| value.to_str()),
            Some("ts")
        );
    }

    #[test]
    fn btw_extension_keeps_bridge_traffic_out_of_the_agent_loop() {
        // Regression: emit used to deliver via sendMessage, which pushes the
        // delta/answer into agent.state.messages when idle (convertToLlm maps
        // it onto a user message) or steers the parent mid-turn when streaming.
        // Bridge traffic must be appended custom entries only.
        let (_, bundle) = write_bundle();
        assert!(bundle.bridge.contains("pi.appendEntry(BRIDGE_TYPE, {"));
        assert!(!bundle.bridge.contains("pi.sendMessage"));
        assert!(!bundle.index.contains("pi.sendMessage"));
        assert!(!bundle.context.contains("pi.sendMessage"));
        assert!(!bundle.models.contains("pi.sendMessage"));
        assert!(bundle.index.contains("pi.on(\"context\""));
    }
}
