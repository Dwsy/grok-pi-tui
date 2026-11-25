use anyhow::Result;
use std::{fs::File, io::Read, path::Path};

const MANAGED_PI_MARKER: &str = "HERDR_INTEGRATION_ID=pi";
const MANAGED_FILE_NAME: &str = "herdr-agent-state.ts";

/// Materialize the default-on, headless Herdr lifecycle bridge for grok-pi.
pub(super) fn write_herdr_extension() -> Result<super::tui_kit::CachedExtensionFile> {
    const SOURCE: &str = include_str!("../../../../../../extensions/pi-grok-herdr/index.ts");
    super::tui_kit::materialize_cached_file("pi-grok-herdr", SOURCE)
}

/// Detect Herdr's managed stock-Pi integration in auto-discovered resources.
///
/// When the built-in bridge is active, loading both would create two writers for
/// the same `herdr:pi` lifecycle authority. Explicit `--extension` arguments are
/// intentionally not filtered; only the host's auto-discovered launch plan uses
/// this predicate.
pub(super) fn is_managed_pi_integration(path: &Path) -> bool {
    if path.file_name().and_then(|name| name.to_str()) != Some(MANAGED_FILE_NAME) {
        return false;
    }

    let Ok(file) = File::open(path) else {
        return false;
    };
    let mut prefix = Vec::with_capacity(2048);
    if file.take(2048).read_to_end(&mut prefix).is_err() {
        return false;
    }
    String::from_utf8_lossy(&prefix).contains(MANAGED_PI_MARKER)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn herdr_extension_source_uses_current_authoritative_lifecycle_contract() {
        let file = write_herdr_extension().expect("write extension");
        let source = std::fs::read_to_string(file.path()).expect("read extension");
        assert!(source.contains("source: SOURCE"));
        assert!(source.contains("const SOURCE = \"herdr:pi\""));
        assert!(source.contains("pane.report_agent_session"));
        assert!(source.contains("pane.report_agent"));
        assert!(source.contains("session_start_source"));
        assert!(source.contains("pi.on(\"agent_settled\""));
        assert!(source.contains("ctx?.isIdle?.() !== true"));
        assert!(source.contains("pi.events.on(\"herdr:blocked\""));
        assert!(!source.contains("pane.release_agent"));
        assert!(!source.contains("pi.on(\"agent_end\""));
        assert_eq!(
            file.path().extension().and_then(|value| value.to_str()),
            Some("ts")
        );
    }

    #[test]
    fn managed_integration_detection_requires_name_and_marker() {
        let mut managed = tempfile::Builder::new()
            .prefix("managed-")
            .suffix("-herdr-agent-state.ts")
            .tempfile()
            .expect("managed file");
        writeln!(managed, "// {MANAGED_PI_MARKER}").expect("write marker");
        assert!(!is_managed_pi_integration(managed.path()));

        let dir = tempfile::tempdir().expect("temp dir");
        let managed_path = dir.path().join(MANAGED_FILE_NAME);
        std::fs::write(&managed_path, format!("// {MANAGED_PI_MARKER}\n"))
            .expect("write managed integration");
        assert!(is_managed_pi_integration(&managed_path));

        std::fs::write(&managed_path, "// user extension\n").expect("write custom extension");
        assert!(!is_managed_pi_integration(&managed_path));
    }
}
