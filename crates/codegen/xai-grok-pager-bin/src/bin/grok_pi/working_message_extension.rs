use anyhow::{Context, Result};
use std::{fs::File, io::Write};
use tempfile::NamedTempFile;

/// Materialize the working-message mirror extension.
///
/// Pi's RPC mode implements `ctx.ui.setWorkingMessage()` as a no-op, so this
/// always-on shim republishes the text through `setStatus` for the adapter to
/// forward as `pi/ui/working_message`. The F2 toggle lives in the extension's
/// `grok-pi.json`; it gates only Pager rendering, not the shim itself, so a
/// late toggle picks up whatever text Pi has already published.
///
/// Single-file source, so a `NamedTempFile` (rather than a `TempDir` bundle) is
/// enough — there are no relative imports to resolve.
pub(super) fn write_working_message_extension() -> Result<NamedTempFile> {
    let source = include_str!("../../../../../../extensions/pi-grok-working-message/index.ts");
    let mut file = tempfile::Builder::new()
        .prefix("pi-grok-working-message-")
        .suffix(".ts")
        .tempfile()
        .with_context(|| "create working-message extension tempfile")?;
    file.write_all(source.as_bytes())
        .with_context(|| "write working-message extension source")?;
    file.flush()
        .with_context(|| "flush working-message extension source")?;
    File::open(file.path())
        .and_then(|source| source.sync_all())
        .ok();
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn working_message_source_materializes_from_extension() {
        let file = write_working_message_extension().expect("write");
        let source = std::fs::read_to_string(file.path()).expect("read");
        assert!(source.contains("__pi_grok_working_message__"));
        assert!(source.contains("installWorkingMessageMirror"));
        assert!(source.contains("setWorkingMessage"));
    }
}
