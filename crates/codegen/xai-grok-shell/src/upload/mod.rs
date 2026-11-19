pub(crate) mod artifact_tracker;
pub(crate) mod drain;
pub mod gcs;
#[cfg(feature = "stock-agent")]
pub(crate) mod manifest;
#[cfg(feature = "stock-agent")]
pub(crate) mod memory;
#[cfg(feature = "stock-agent")]
pub(crate) mod trace;
#[cfg(feature = "stock-agent")]
pub(crate) mod turn;
pub use drain::{drain_pending_uploads, drain_pending_uploads_at_exit};

pub(crate) const BLOCKING_ATTEMPT_CAP: std::time::Duration = std::time::Duration::from_secs(30);
