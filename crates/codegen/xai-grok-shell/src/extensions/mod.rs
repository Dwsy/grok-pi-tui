#[cfg(feature = "stock-agent")]
pub mod agent_runtime;
#[cfg(feature = "stock-agent")]
pub mod auth;
#[cfg(feature = "stock-agent")]
pub(crate) mod auth_gate;
#[cfg(feature = "stock-agent")]
pub(crate) mod background_task;
#[cfg(feature = "stock-agent")]
pub mod billing;
#[cfg(feature = "stock-agent")]
pub(crate) mod btw;
#[cfg(feature = "stock-agent")]
pub mod bundle;
#[cfg(feature = "stock-agent")]
pub(crate) mod chat_conversation_history;
#[cfg(feature = "stock-agent")]
pub mod code_nav;
#[cfg(feature = "stock-agent")]
pub mod consent;
#[cfg(feature = "stock-agent")]
pub mod debug;
#[cfg(feature = "stock-agent")]
pub mod feedback;
#[cfg(feature = "stock-agent")]
pub(crate) mod feedback_drafts;
#[cfg(feature = "stock-agent")]
pub(crate) mod feedback_trace;
#[cfg(feature = "stock-agent")]
pub mod fs;
#[cfg(feature = "stock-agent")]
pub mod git;
#[cfg(feature = "stock-agent")]
pub mod hooks;
#[cfg(feature = "stock-agent")]
pub mod hunk_tracker;
#[cfg(feature = "stock-agent")]
pub mod interject;
#[cfg(feature = "stock-agent")]
pub mod jj;
pub mod marketplace;
#[cfg(feature = "stock-mcp")]
pub mod mcp;
#[cfg(feature = "stock-agent")]
pub mod memory;
pub mod notification;
#[cfg(feature = "stock-agent")]
pub mod plugins;
#[cfg(feature = "stock-agent")]
pub mod pr;
#[cfg(feature = "stock-agent")]
pub mod privacy;
#[cfg(feature = "stock-agent")]
pub mod prompt_history;
pub mod prompt_meta;
#[cfg(feature = "stock-agent")]
pub mod recap;
#[cfg(feature = "stock-agent")]
pub mod repair;
#[cfg(feature = "stock-agent")]
pub(crate) mod review;
#[cfg(feature = "stock-agent")]
pub mod rewind;
#[cfg(feature = "stock-agent")]
pub mod rollout;
#[cfg(feature = "stock-agent")]
pub mod routing;
#[cfg(feature = "stock-agent")]
pub mod search;
#[cfg(feature = "stock-agent")]
pub(crate) mod session_admin;
pub mod session_search;
pub(crate) mod session_state;
pub mod session_updates;
#[cfg(feature = "stock-agent")]
pub mod share;
#[cfg(feature = "stock-agent")]
pub mod skills;
#[cfg(feature = "stock-agent")]
pub mod subagent_message;
#[cfg(feature = "stock-agent")]
pub mod suggest;
pub mod task;
#[cfg(feature = "stock-agent")]
pub mod terminal;
pub mod usage;
#[cfg(feature = "stock-agent")]
pub mod worktree;
use crate::session::ExtMethodResult;
use agent_client_protocol as acp;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::sync::Arc;
pub type ExtResult = Result<acp::ExtResponse, acp::Error>;
pub(crate) fn parse_params<T: DeserializeOwned>(args: &acp::ExtRequest) -> Result<T, acp::Error> {
    parse_params_str(args.params.get())
}
/// Deserialize ACP params from their raw JSON string, mapping a parse failure to `invalid_params`.
/// Used by [`parse_params`] and the bridge `encode` hooks, which hold the params `RawValue` directly.
pub(crate) fn parse_params_str<T: DeserializeOwned>(raw: &str) -> Result<T, acp::Error> {
    serde_json::from_str(raw)
        .map_err(|e| acp::Error::invalid_params().data(format!("invalid params: {}", e)))
}
pub fn parse_session_id(args: &acp::ExtRequest) -> Option<acp::SessionId> {
    let v: serde_json::Value = serde_json::from_str(args.params.get()).ok()?;
    let sid = v.get("sessionId")?.as_str()?;
    Some(acp::SessionId::new(sid))
}
pub fn to_ext_response<T: Serialize>(result: anyhow::Result<T>) -> ExtResult {
    ExtMethodResult::from_result(result)
        .to_ext_response()
        .map_err(|e| acp::Error::internal_error().data(e.to_string()))
}
/// Wrap a serializable value as an `ExtResponse` without the `ExtMethodResult` envelope.
pub(crate) fn to_raw_response<T: Serialize>(v: &T) -> ExtResult {
    serde_json::value::to_raw_value(v)
        .map(|raw| acp::ExtResponse::new(Arc::from(raw)))
        .map_err(|e| acp::Error::internal_error().data(e.to_string()))
}
pub(crate) fn to_ext_response_partial<T: Serialize>(
    result: anyhow::Result<T>,
    warning: Option<String>,
) -> ExtResult {
    let ext_result = match (result, warning) {
        (Ok(data), Some(warn)) => ExtMethodResult::partial(data, warn),
        (Ok(data), None) => ExtMethodResult::success(data),
        (Err(e), _) => ExtMethodResult::failure(e),
    };
    ext_result
        .to_ext_response()
        .map_err(|e| acp::Error::internal_error().data(e.to_string()))
}
/// Empty response for operations that return no data.
#[derive(Debug, Serialize)]
pub struct Empty {}
