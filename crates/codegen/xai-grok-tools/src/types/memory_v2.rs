//! Adapter that links the shared memory-v2 policy contract to stock tool resources.

use std::path::Path;
use std::sync::Arc;

pub use xai_tool_types::memory_v2::{MemoryV2Access, MemoryV2Write};

/// Ephemeral ToolBridge resource shared by all ordinary file tools in a session.
#[derive(Clone)]
pub struct MemoryV2AccessResource(pub Arc<dyn MemoryV2Access>);

impl std::fmt::Debug for MemoryV2AccessResource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("MemoryV2AccessResource").finish()
    }
}

/// Validate a path when memory v2 is active, preserving normal behavior outside its roots.
pub async fn validate_memory_v2_read(
    resources: &crate::types::resources::SharedResources,
    path: &Path,
) -> Result<bool, String> {
    let access = resources
        .lock()
        .await
        .get::<MemoryV2AccessResource>()
        .cloned();
    let Some(access) = access else {
        return Ok(false);
    };
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || access.0.validate_read(&path))
        .await
        .map_err(|error| format!("memory v2 read validation task failed: {error}"))?
}

/// Record a successful ordinary read for optimistic edit concurrency.
pub async fn record_memory_v2_read(
    resources: &crate::types::resources::SharedResources,
    path: &Path,
    contents: &[u8],
) -> Result<(), String> {
    let access = resources
        .lock()
        .await
        .get::<MemoryV2AccessResource>()
        .cloned();
    if let Some(access) = access {
        let path = path.to_path_buf();
        let contents = contents.to_vec();
        tokio::task::spawn_blocking(move || access.0.record_read(&path, &contents))
            .await
            .map_err(|error| format!("memory v2 read recording task failed: {error}"))??;
    }
    Ok(())
}

/// Validate a write through memory v2 without persisting it.
pub async fn preflight_memory_v2_write(
    resources: &crate::types::resources::SharedResources,
    path: &Path,
    contents: &[u8],
) -> Result<bool, String> {
    let access = resources
        .lock()
        .await
        .get::<MemoryV2AccessResource>()
        .cloned();
    let Some(access) = access else {
        return Ok(false);
    };
    let path = path.to_path_buf();
    let contents = contents.to_vec();
    tokio::task::spawn_blocking(move || access.0.preflight_write(&path, &contents))
        .await
        .map_err(|error| format!("memory v2 write preflight task failed: {error}"))?
}

/// Route a write through memory v2 when the path belongs to one of its scopes.
pub async fn write_memory_v2_file(
    resources: &crate::types::resources::SharedResources,
    path: &Path,
    contents: &[u8],
) -> Result<MemoryV2Write, String> {
    let access = resources
        .lock()
        .await
        .get::<MemoryV2AccessResource>()
        .cloned();
    let Some(access) = access else {
        return Ok(MemoryV2Write::Outside);
    };
    let path = path.to_path_buf();
    let contents = contents.to_vec();
    tokio::task::spawn_blocking(move || access.0.write_file(&path, &contents))
        .await
        .map_err(|error| format!("memory v2 write task failed: {error}"))?
}
