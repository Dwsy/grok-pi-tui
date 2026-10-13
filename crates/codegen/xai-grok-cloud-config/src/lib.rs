//! This crate fetches, caches, and refreshes the Grok config delivered from the cloud.
//! Parsing and composition stay in `xai-grok-config`.

#![deny(clippy::indexing_slicing)]

mod cached_config;
mod commit;
pub mod managed_config;
pub mod remote_settings;
mod settings_cache;
mod settings_endpoint;
mod settings_fetch;
pub mod settings_get;
mod settings_refresh;

pub use cached_config::{CachedConfigInputs, load_config_with_cached_remote};
pub use commit::{Commit, PolicyRepairPending, resolve_disk_auth};
pub use settings_cache::SettingsCacheMode;
pub(crate) use settings_endpoint::SettingsEndpoint;
pub use settings_endpoint::settings_cache_identity;
pub use settings_fetch::{SettingsFetch, fetch_settings_blocking};
pub use settings_refresh::SettingsRefresh;
