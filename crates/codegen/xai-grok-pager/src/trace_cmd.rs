use std::path::PathBuf;

#[derive(Debug, clap::Args, Clone)]
pub struct TraceArgs {
    /// Session ID to export/upload
    pub session_id: String,
    /// Save locally only, skip remote upload
    #[arg(long)]
    pub local: bool,
    /// Output path (default: $GROK_HOME/trace-exports/<session-id>.tar.gz)
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    /// Emit machine-readable JSON output
    #[arg(long)]
    pub json: bool,
}

#[cfg(feature = "stock-agent")]
#[path = "trace_cmd_imp.rs"]
mod imp;

#[cfg(feature = "stock-agent")]
pub use imp::{resolve_upload_method, run};
