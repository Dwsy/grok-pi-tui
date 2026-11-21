use anyhow::{Context, Result};
use std::{fs::File, io::Write};
use tempfile::NamedTempFile;

/// Materialize the built-in `@` object-mentions extension (`@skill`, `@thread`, `@mcp`, `@file`).
///
/// Single-file TypeScript source (zero relative imports), so a `NamedTempFile`
/// is sufficient. Publishes its catalog via `setStatus("__pi_grok_mentions__", ...)`
/// which `pi-grok-adapter` forwards as `pi/ui/mentions_catalog`.
pub(super) fn write_mentions_extension() -> Result<NamedTempFile> {
    let source = include_str!("../../../../../../extensions/pi-grok-mentions/index.ts");
    let mut file = tempfile::Builder::new()
        .prefix("pi-grok-mentions-")
        .suffix(".ts")
        .tempfile()
        .with_context(|| "create mentions extension tempfile")?;
    file.write_all(source.as_bytes())
        .with_context(|| "write mentions extension source")?;
    file.flush()
        .with_context(|| "flush mentions extension source")?;
    File::open(file.path())
        .and_then(|source| source.sync_all())
        .ok();
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mentions_source_materializes_from_extension() {
        let file = write_mentions_extension().expect("write");
        let source = std::fs::read_to_string(file.path()).expect("read");
        assert!(source.contains("__pi_grok_mentions__"));
        assert!(source.contains("__piGrokRegisterMentionProvider"));
        assert!(source.contains("grok-pi://skill/"));
        assert!(source.contains("grok-pi://thread/"));
        assert!(source.contains("grok-pi://mcp/"));
    }
}
