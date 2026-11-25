use anyhow::Result;

/// Materialize default-on Pi session export/share (`/export-html` / `/pi-share`).
///
/// Reuses Pi host export-html + gh gist paths. Grok `/export` stays Markdown.
pub(super) fn write_export_extension() -> Result<super::tui_kit::CachedExtensionFile> {
    const SOURCE: &str = include_str!("../../../../../../extensions/pi-grok-export/index.ts");
    super::tui_kit::materialize_cached_file("pi-grok-export", SOURCE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_extension_source_is_a_loadable_typescript_module() {
        let file = write_export_extension().expect("write extension");
        let source = std::fs::read_to_string(file.path()).expect("read extension");
        assert!(source.contains("registerCommand(\"export-html\""));
        assert!(source.contains("registerCommand(\"pi-share\""));
        assert!(source.contains("exportSessionToHtml"));
        assert!(source.contains("getShareViewerUrl"));
        assert!(source.contains("mkdtempSync"));
        assert!(source.contains("path.join(tmpDir, \"session.html\")"));
        assert_eq!(
            file.path().extension().and_then(|value| value.to_str()),
            Some("ts")
        );
    }
}
