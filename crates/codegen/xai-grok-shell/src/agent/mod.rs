#[cfg(feature = "stock-agent")]
pub mod activity;
#[cfg(feature = "stock-agent")]
pub mod app;
pub mod auth_method;
pub mod chat_modes;
pub mod config;
pub(crate) mod config_model_override_parse;
#[cfg(feature = "stock-agent")]
mod ext_parsers;
pub mod external_otel_pin;
#[cfg(feature = "stock-agent")]
pub mod feedback_client;
pub mod folder_trust;
#[cfg(feature = "stock-agent")]
pub(crate) mod handlers;
#[cfg(feature = "stock-agent")]
pub mod init;
#[cfg(feature = "stock-agent")]
pub mod model_providers;
#[cfg(feature = "stock-agent")]
pub mod mvp_agent;
#[cfg(feature = "stock-agent")]
pub(crate) mod otel_gate;
pub(crate) mod proxy;
#[cfg(feature = "stock-agent")]
pub mod relay;
#[cfg(feature = "stock-agent")]
pub mod remote_config;
#[cfg(feature = "stock-agent")]
pub(crate) mod restore_code;
#[cfg(feature = "stock-agent")]
pub mod roster;
#[cfg(feature = "stock-agent")]
pub mod server;
#[cfg(feature = "stock-agent")]
pub mod session_config;
#[cfg(feature = "stock-agent")]
pub(crate) mod session_metrics;
#[cfg(feature = "stock-agent")]
pub mod session_registry_client;
#[cfg(feature = "stock-agent")]
pub(crate) mod subagent;
#[cfg(feature = "stock-agent")]
pub(crate) mod subscription_check;
#[cfg(feature = "test-support")]
pub mod testkit;
#[cfg(feature = "stock-agent")]
pub(crate) mod update_chunk_merge;

#[cfg(feature = "stock-agent")]
pub use mvp_agent::MvpAgent;
pub use relay::{RelayConfig, RelayHandle, spawn_relay_connection};
#[cfg(feature = "stock-agent")]
pub use server::{ServerConfig, run_agent_server};

#[cfg(test)]
mod storage_client_tests;
