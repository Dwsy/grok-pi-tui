//! Shared policy contract for ordinary file operations targeting memory v2.
//! The stock Tools adapter owns resource dispatch and async blocking work;
//! Memory implements these traits without depending on the tool runtime.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryV2Write {
    Outside,
    Written { previous_content: Option<Vec<u8>> },
}

pub trait MemoryV2Access: std::fmt::Debug + Send + Sync {
    fn validate_read(&self, path: &Path) -> Result<bool, String>;
    fn record_read(&self, path: &Path, contents: &[u8]) -> Result<(), String>;
    fn preflight_write(&self, path: &Path, contents: &[u8]) -> Result<bool, String>;
    fn write_file(&self, path: &Path, contents: &[u8]) -> Result<MemoryV2Write, String>;
    fn scope_roots(&self) -> [PathBuf; 2];
}
