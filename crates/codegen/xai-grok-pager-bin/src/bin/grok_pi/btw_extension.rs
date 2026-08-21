use anyhow::{Context, Result};
use std::{fs::File, io::Write};
use tempfile::NamedTempFile;

/// Inject a headless Pi extension that answers `/btw` side questions via
/// `streamSimple()`, persists BTW answers as Pi custom entries, and appends
/// `pi-grok-btw/v1` custom entries for the adapter.
pub(super) fn write_btw_extension() -> Result<NamedTempFile> {
    let mut file = tempfile::Builder::new()
        .prefix("pi-grok-btw-")
        .suffix(".ts")
        .tempfile()
        .context("create Pi btw extension tempfile")?;
    const SOURCE: &str = include_str!("../../../../../../extensions/pi-grok-btw/index.ts");
    file.write_all(SOURCE.as_bytes())
        .context("write Pi btw extension source")?;
    file.flush().context("flush Pi btw extension source")?;
    File::open(file.path()).and_then(|f| f.sync_all()).ok();
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn btw_extension_source_is_a_loadable_typescript_module() {
        let file = write_btw_extension().expect("temp extension");
        let source = std::fs::read_to_string(file.path()).expect("read extension");
        assert!(source.contains("__pi_grok_btw"));
        assert!(source.contains("registerCommand(COMMAND"));
        assert!(source.contains("pi-grok-btw/v1"));
        assert!(source.contains("from \"@earendil-works/pi-ai/compat\""));
        assert!(source.contains("All /btw models failed"));
        assert!(source.contains("modelChain"));
        assert!(source.contains("stripIncompleteTail"));
        assert!(file.path().extension().and_then(|e| e.to_str()) == Some("ts"));
    }

    #[test]
    fn btw_extension_keeps_bridge_traffic_out_of_the_agent_loop() {
        // Regression: emit used to deliver via sendMessage, which pushes the
        // delta/answer into agent.state.messages when idle (convertToLlm maps
        // it onto a user message) or steers the parent mid-turn when streaming.
        // Bridge traffic must be appended custom entries only.
        let file = write_btw_extension().expect("temp extension");
        let source = std::fs::read_to_string(file.path()).expect("read extension");
        assert!(source.contains("pi.appendEntry(BRIDGE_TYPE, {"));
        assert!(!source.contains("pi.sendMessage"));
        assert!(source.contains("pi.on(\"context\""));
    }
}
