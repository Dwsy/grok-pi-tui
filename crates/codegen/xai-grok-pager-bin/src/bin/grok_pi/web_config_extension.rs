use anyhow::{Context, Result};
use std::{
    fs::File,
    io::Write,
    path::{Path, PathBuf},
};
use tempfile::TempDir;

/// Materialized web-config extension bundle. `_source_dir` must stay alive for
/// the Pi process lifetime so relative imports between the TypeScript modules
/// keep resolving.
pub(super) struct WebConfigExtension {
    _source_dir: TempDir,
    source_path: PathBuf,
    ui_path: PathBuf,
    catalog_path: PathBuf,
}

impl WebConfigExtension {
    pub(super) fn source_path(&self) -> &Path {
        &self.source_path
    }

    /// Absolute path of the single-page UI handed to `PI_GROK_WEB_CONFIG_UI`.
    pub(super) fn ui_path(&self) -> &Path {
        &self.ui_path
    }

    /// Absolute path of the baked F2 settings catalog handed to
    /// `PI_GROK_WEB_CONFIG_CATALOG`.
    pub(super) fn catalog_path(&self) -> &Path {
        &self.catalog_path
    }
}

fn write_source_file(dir: &Path, name: &str, source: &str) -> Result<PathBuf> {
    let path = dir.join(name);
    let mut file = File::create(&path)
        .with_context(|| format!("create Pi web config extension module {name}"))?;
    file.write_all(source.as_bytes())
        .with_context(|| format!("write Pi web config extension module {name}"))?;
    file.flush()
        .with_context(|| format!("flush Pi web config extension module {name}"))?;
    file.sync_all().ok();
    Ok(path)
}

/// Materialize the `/pi-config web` + `/pi-models web` browser surface.
///
/// Every authored module must be materialized here — the injector owns the
/// transitive closure of `index.ts`'s relative imports (see AGENTS.md
/// "Diagnosing Pi RPC bootstrap / extension failures").
pub(super) fn write_web_config_extension() -> Result<WebConfigExtension> {
    let source_dir = tempfile::Builder::new()
        .prefix("pi-grok-web-config-")
        .tempdir()
        .context("create Pi web config extension source directory")?;
    let source_path = write_source_file(
        source_dir.path(),
        "index.ts",
        include_str!("../../../../../../extensions/pi-grok-web-config/index.ts"),
    )?;
    write_source_file(
        source_dir.path(),
        "shared.ts",
        include_str!("../../../../../../extensions/pi-grok-web-config/shared.ts"),
    )?;
    write_source_file(
        source_dir.path(),
        "config-store.ts",
        include_str!("../../../../../../extensions/pi-grok-web-config/config-store.ts"),
    )?;
    write_source_file(
        source_dir.path(),
        "server.ts",
        include_str!("../../../../../../extensions/pi-grok-web-config/server.ts"),
    )?;
    let ui_path = write_source_file(
        source_dir.path(),
        "ui.html",
        include_str!("../../../../../../extensions/pi-grok-web-config/web/index.html"),
    )?;
    let catalog_path = write_source_file(
        source_dir.path(),
        "host-catalog.json",
        &host_catalog_json(),
    )?;
    Ok(WebConfigExtension {
        _source_dir: source_dir,
        source_path,
        ui_path,
        catalog_path,
    })
}

/// Serialize the same `grok-pi.json` manifests build.rs bakes into
/// `BUNDLED_HOST_UI_SOURCES`, so the web F2 surface maps the registered
/// extension settings one-to-one with the native F2 modal.
fn host_catalog_json() -> String {
    let items = crate::bundled_host_ui::BUNDLED_HOST_UI_SOURCES
        .iter()
        .map(|(source, json)| {
            let escaped = source.replace('\\', "\\\\").replace('"', "\\\"");
            format!("{{\"source\": \"{escaped}\", \"manifest\": {json}}}")
        })
        .collect::<Vec<_>>()
        .join(",\n");
    format!("[\n{items}\n]\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Bundle {
        index: String,
        shared: String,
        config_store: String,
        server: String,
        ui: String,
        catalog: String,
    }

    fn write_bundle() -> (WebConfigExtension, Bundle) {
        let extension = write_web_config_extension().expect("write extension");
        let dir = extension.source_path().parent().expect("source dir");
        let read = |name: &str| {
            std::fs::read_to_string(dir.join(name)).unwrap_or_else(|e| panic!("read {name}: {e}"))
        };
        let bundle = Bundle {
            index: read("index.ts"),
            shared: read("shared.ts"),
            config_store: read("config-store.ts"),
            server: read("server.ts"),
            ui: read("ui.html"),
            catalog: read("host-catalog.json"),
        };
        (extension, bundle)
    }

    #[test]
    fn web_config_extension_materializes_every_module_of_the_entry_closure() {
        let (extension, bundle) = write_bundle();
        assert!(bundle.index.contains("from \"./server.ts\""));
        assert!(bundle.index.contains("from \"./config-store.ts\""));
        assert!(bundle.index.contains("from \"./shared.ts\""));
        assert!(bundle.config_store.contains("from \"./shared.ts\""));
        // Entry registers both web commands.
        assert!(bundle.index.contains("registerCommand(\"pi-config-web\""));
        assert!(bundle.index.contains("registerCommand(\"pi-models-web\""));
        // Per-module load-bearing symbols.
        assert!(bundle.shared.contains("PI_GROK_WEB_CONFIG_UI"));
        assert!(bundle.server.contains("startWebConfigServer"));
        assert!(bundle.server.contains("x-pi-token"));
        assert!(bundle.config_store.contains("collectState"));
        assert!(bundle.ui.contains("__PI_GROK_WEB_CONFIG_TOKEN__"));
        // The F2 catalog mirrors the baked grok-pi.json manifests.
        let parsed: serde_json::Value =
            serde_json::from_str(&bundle.catalog).expect("host catalog is valid JSON");
        let sources = parsed.as_array().expect("catalog array");
        assert!(
            sources
                .iter()
                .any(|item| item["source"]
                    .as_str()
                    .is_some_and(|value| value.contains("pi-grok-loop"))),
            "catalog must include the pi-grok-loop manifest"
        );
        assert!(
            sources.iter().all(|item| item["manifest"]["settings"].is_array()),
            "every catalog entry carries a settings array"
        );
        assert_eq!(
            extension
                .source_path()
                .extension()
                .and_then(|value| value.to_str()),
            Some("ts")
        );
        assert_eq!(
            extension.ui_path().extension().and_then(|value| value.to_str()),
            Some("html")
        );
        assert!(extension.ui_path().exists());
        assert!(extension.catalog_path().exists());
    }
}
