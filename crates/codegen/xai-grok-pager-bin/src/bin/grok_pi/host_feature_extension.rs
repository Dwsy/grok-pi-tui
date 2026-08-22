use anyhow::{Context, Result, bail};
use std::{fs::File, io::Write};
use tempfile::NamedTempFile;
use xai_grok_shell::host_features::HostFeatureSpec;

/// Materialize a single-file Pi extension declared by a host-feature spec.
pub(super) fn write_host_feature_extension(spec: &HostFeatureSpec) -> Result<NamedTempFile> {
    let Some(source) = spec.extension_source else {
        bail!("host feature {} has no extension source", spec.key.as_str());
    };
    let prefix = format!("pi-grok-feature-{}-", spec.key.as_str());
    let mut file = tempfile::Builder::new()
        .prefix(&prefix)
        .suffix(".ts")
        .tempfile()
        .with_context(|| format!("create {} extension tempfile", spec.key.as_str()))?;
    file.write_all(source.as_bytes())
        .with_context(|| format!("write {} extension source", spec.key.as_str()))?;
    file.flush()
        .with_context(|| format!("flush {} extension source", spec.key.as_str()))?;
    File::open(file.path())
        .and_then(|source| source.sync_all())
        .ok();
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use xai_grok_shell::host_features::PI_WORKFLOWS_SPEC;

    #[test]
    fn workflow_feature_source_materializes_from_manifest() {
        let file = write_host_feature_extension(&PI_WORKFLOWS_SPEC).expect("write");
        let source = std::fs::read_to_string(file.path()).expect("read");
        assert!(source.contains("__pi_workflow_spawn"));
        assert!(source.contains("createAgentSession"));
        assert!(source.contains("pi-grok-workflow/v1"));
        assert!(source.contains("responsePath"));
    }
}
