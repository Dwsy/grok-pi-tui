use anyhow::Result;
use std::path::{Path, PathBuf};

/// Materialized Remote TUI extension bundle in a content-addressed cache
/// directory so `jiti`'s `fsCache` hits across launches.
pub(super) struct RemoteTuiExtension {
    source_path: PathBuf,
}

impl RemoteTuiExtension {
    pub(super) fn source_path(&self) -> &Path {
        &self.source_path
    }
}

/// Materialize the experimental Remote TUI probe as a standalone extension.
/// Only loaded when PI_GROK_REMOTE_TUI=1.
///
/// Every authored module must be materialized here — the injector owns the
/// transitive closure of `index.ts`'s relative imports (see AGENTS.md
/// "Diagnosing Pi RPC bootstrap / extension failures").
pub(super) fn write_remote_tui_extension() -> Result<RemoteTuiExtension> {
    let root = super::tui_kit::materialize_cached_bundle(
        "pi-grok-remote-tui",
        &[
            (
                "index.ts",
                include_str!("../../../../../../extensions/pi-grok-remote-tui/index.ts"),
            ),
            (
                "shared.ts",
                include_str!("../../../../../../extensions/pi-grok-remote-tui/shared.ts"),
            ),
            (
                "env.ts",
                include_str!("../../../../../../extensions/pi-grok-remote-tui/env.ts"),
            ),
            (
                "layout.ts",
                include_str!("../../../../../../extensions/pi-grok-remote-tui/layout.ts"),
            ),
            (
                "transport.ts",
                include_str!("../../../../../../extensions/pi-grok-remote-tui/transport.ts"),
            ),
            (
                "host.ts",
                include_str!("../../../../../../extensions/pi-grok-remote-tui/host.ts"),
            ),
            (
                "demo.ts",
                include_str!("../../../../../../extensions/pi-grok-remote-tui/demo.ts"),
            ),
            (
                "timeline.ts",
                include_str!("../../../../../../extensions/pi-grok-remote-tui/timeline.ts"),
            ),
            (
                "timeline-format.ts",
                include_str!("../../../../../../extensions/pi-grok-remote-tui/timeline-format.ts"),
            ),
        ],
    )?;
    Ok(RemoteTuiExtension {
        source_path: root.join("index.ts"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Bundle {
        index: String,
        shared: String,
        env: String,
        layout: String,
        transport: String,
        host: String,
        demo: String,
        timeline: String,
        timeline_format: String,
    }

    fn write_bundle() -> (RemoteTuiExtension, Bundle) {
        let extension = write_remote_tui_extension().expect("write extension");
        let dir = extension.source_path().parent().expect("source dir");
        let read = |name: &str| {
            std::fs::read_to_string(dir.join(name)).unwrap_or_else(|e| panic!("read {name}: {e}"))
        };
        let bundle = Bundle {
            index: read("index.ts"),
            shared: read("shared.ts"),
            env: read("env.ts"),
            layout: read("layout.ts"),
            transport: read("transport.ts"),
            host: read("host.ts"),
            demo: read("demo.ts"),
            timeline: read("timeline.ts"),
            timeline_format: read("timeline-format.ts"),
        };
        (extension, bundle)
    }

    #[test]
    fn remote_tui_extension_materializes_every_module_of_the_entry_closure() {
        let (extension, bundle) = write_bundle();
        // Entry registers the session_start hook and /remote-tui command.
        assert!(bundle.index.contains("session_start"));
        assert!(bundle.index.contains("registerCommand(\"remote-tui\""));
        assert!(bundle.index.contains("createDemoSelector"));
        assert!(bundle.index.contains("applyDemoCapabilities"));
        // Every relative import of the entry closure is materialized.
        assert!(bundle.index.contains("from \"./host.ts\""));
        assert!(bundle.index.contains("from \"./env.ts\""));
        assert!(bundle.index.contains("from \"./shared.ts\""));
        assert!(bundle.host.contains("from \"./env.ts\""));
        assert!(bundle.host.contains("from \"./layout.ts\""));
        assert!(bundle.host.contains("from \"./transport.ts\""));
        assert!(bundle.host.contains("from \"./shared.ts\""));
        assert!(bundle.layout.contains("from \"./shared.ts\""));
        assert!(bundle.transport.contains("from \"./shared.ts\""));
        assert!(bundle.demo.contains("from \"./shared.ts\""));
        assert!(bundle.index.contains("from \"./timeline.ts\""));
        assert!(bundle.timeline.contains("from \"./host.ts\""));
        assert!(bundle.timeline.contains("timeline-config"));
        assert!(bundle.timeline.contains("from \"./timeline-format.ts\""));
        assert!(bundle.timeline_format.contains("registerTimelineRenderer"));
        // Per-module load-bearing symbols.
        assert!(bundle.shared.contains("__pi_grok_remote_tui_layout__"));
        assert!(bundle.env.contains("PI_GROK_REMOTE_TUI"));
        assert!(bundle.env.contains("shouldInstallRemoteHost"));
        assert!(bundle.env.contains("ensurePiTheme"));
        assert!(bundle.env.contains("initTheme"));
        assert!(bundle.layout.contains("resolveRemoteTuiLayout"));
        assert!(bundle.layout.contains("resolveViewport"));
        assert!(bundle.shared.contains("pi-grok-remote-tui-active.json"));
        assert!(bundle.transport.contains("writeMeta"));
        assert!(bundle.transport.contains("drainKeys"));
        assert!(bundle.host.contains("__piGrokEnsureRemoteTuiHost"));
        assert!(bundle.host.contains("installCustomPatch"));
        assert!(bundle.demo.contains("SettingsList"));
        assert_eq!(
            extension
                .source_path()
                .extension()
                .and_then(|value| value.to_str()),
            Some("ts")
        );
    }
}
