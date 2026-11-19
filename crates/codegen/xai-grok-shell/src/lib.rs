#![allow(
    unused_imports,
    unused_variables,
    unused_mut,
    unreachable_code,
    dead_code
)]
#![warn(unreachable_pub)]
#[cfg(all(test, feature = "dhat-heap"))]
#[global_allocator]
static DHAT_ALLOC: dhat::Alloc = dhat::Alloc;
pub(crate) use xai_grok_telemetry::unified_log;
pub use xai_tracing_macros::{teprintln, timed, tprintln};
pub mod agent;
#[cfg(feature = "stock-agent")]
pub mod auth {
    pub use crate::agent::init::run_cli_logout;
    pub use crate::credential_factory::{
        build_bootstrap_otel_credentials, build_storage_client_for_proxy,
    };
    pub use xai_grok_login::*;
}
pub mod builtin;
pub use xai_grok_bundle as bundle;
pub mod claude_import;
pub mod claude_import_state;
#[cfg(feature = "stock-agent")]
pub mod cli_models;
pub mod config;
#[cfg(all(test, feature = "config-docs"))]
pub mod config_docs;
#[cfg(feature = "stock-agent")]
pub mod credential_factory;
pub use xai_grok_shell_base::cpu_profile;
pub use xai_grok_shell_base::env;
pub mod extensions;
pub use xai_grok_foreign_sessions as foreign_sessions;
#[cfg(feature = "stock-agent")]
pub mod heap_profile;
pub mod host_features;
pub use xai_grok_http as http;
#[cfg(feature = "stock-agent")]
pub mod inspect;
pub mod instrumentation;
// The 13k-line stock Leader IPC runtime is unrelated to Pi JSONL RPC.
#[cfg(feature = "stock-agent")]
pub mod leader;
// Small wire contracts remain available to the Pi pager and Stock Leader.
#[path = "leader/protocol.rs"]
pub mod leader_protocol;
pub mod leader_status;
pub mod managed_config;
#[cfg(feature = "stock-mcp")]
pub mod mcp_doctor;
pub use xai_grok_models as models;
pub mod plugin;
pub mod relay;
#[cfg(feature = "stock-agent")]
pub mod remote;
pub mod sampling;
pub mod session;
pub use xai_grok_shell_terminal as terminal;
#[cfg(test)]
pub(crate) mod test_support;
pub mod tier;
pub mod tools;
pub mod upload;
pub mod util;
#[doc(hidden)]
pub mod waterfall;
