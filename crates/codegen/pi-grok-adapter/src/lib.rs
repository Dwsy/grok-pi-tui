//! Pi core integration boundary for Grok Build.
//!
//! This crate deliberately contains no terminal, widget, drawing, markdown,
//! input, keybinding, or scrollback implementation. It translates Pi's JSONL
//! RPC protocol into standard ACP messages. The production `xai-grok-pager`
//! consumes those messages in the `grok-pi` composition binary.

mod background_bash_bridge;
mod btw_bridge;
mod cache_metrics;
mod context_projection;
mod goal_host;
mod loop_host;
mod model;
mod pi_adapter;
mod pi_rpc;
mod pi_workflow_backend;
pub mod plan_mode;
mod prompt_bridge;
mod psm_session_catalog;
mod queue_bridge;
mod recap_bridge;
mod subagent_projection;
mod subagent_transport;
mod todo_bridge;
mod tool_projection;
mod workflow_host;

pub use model::{PiSessionInfo, PiSessionSwitch, scan_local_sessions};
pub use pi_adapter::{PiAgent, PiBootstrap};
pub use pi_rpc::{PiProcess, PiRpc, SpawnConfig};
pub use subagent_transport::SubagentEventTransport;
