use anyhow::{Context, Result};
use std::{fs::File, io::Write};
use tempfile::NamedTempFile;
/// Materialize the bundled workflow extension. UI registration lives in
/// `extensions/pi-grok-workflows/grok-pi.json`; executable source remains the
/// extension's own `index.ts`.
pub(super) fn write_host_feature_extension() -> Result<NamedTempFile> {
    let source = include_str!("../../../../../../extensions/pi-grok-workflows/index.ts");
    let prefix = "pi-grok-feature-pi_workflows-";
    let mut file = tempfile::Builder::new()
        .prefix(&prefix)
        .suffix(".ts")
        .tempfile()
        .with_context(|| "create pi_workflows extension tempfile")?;
    file.write_all(source.as_bytes())
        .with_context(|| "write pi_workflows extension source")?;
    file.flush()
        .with_context(|| "flush pi_workflows extension source")?;
    File::open(file.path())
        .and_then(|source| source.sync_all())
        .ok();
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn workflow_feature_source_materializes_from_extension() {
        let file = write_host_feature_extension().expect("write");
        let source = std::fs::read_to_string(file.path()).expect("read");
        assert!(source.contains("__pi_workflow_spawn"));
        assert!(source.contains("createAgentSession"));
        assert!(source.contains("pi-grok-workflow/v1"));
        assert!(source.contains("responsePath"));
    }
}
