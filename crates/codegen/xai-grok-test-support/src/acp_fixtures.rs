//! A protocol bump changes these constructors once instead of every test call site.

use std::sync::Arc;

use agent_client_protocol as acp;
use serde::Serialize;
use serde_json::value::RawValue;

pub fn session_id(id: &str) -> acp::SessionId {
    acp::SessionId::new(id)
}

pub fn tool_call_id(id: &str) -> acp::ToolCallId {
    acp::ToolCallId::new(id)
}

pub fn model_id(id: &str) -> acp::ModelId {
    acp::ModelId::new(id)
}

pub fn text_block(text: &str) -> acp::ContentBlock {
    acp::ContentBlock::Text(acp::TextContent::new(text))
}

fn raw_params(params: &impl Serialize) -> Arc<RawValue> {
    Arc::from(serde_json::value::to_raw_value(params).expect("fixture params serialize"))
}

pub fn ext_notification(method: &str, params: &impl Serialize) -> acp::ExtNotification {
    acp::ExtNotification::new(method, raw_params(params))
}

pub fn ext_request(method: &str, params: &impl Serialize) -> acp::ExtRequest {
    acp::ExtRequest::new(method, raw_params(params))
}

pub fn session_notification(id: &str, update: acp::SessionUpdate) -> acp::SessionNotification {
    acp::SessionNotification::new(session_id(id), update)
}

pub fn model_info(id: &str, name: &str) -> acp::ModelInfo {
    acp::ModelInfo::new(model_id(id), name)
}

/// Separate from `model_info` so a caller cannot pass `None` for meta.
pub fn model_info_with_meta(id: &str, name: &str, meta: serde_json::Value) -> acp::ModelInfo {
    let serde_json::Value::Object(meta_object) = meta else {
        panic!("model meta fixture must be a JSON object");
    };
    model_info(id, name).meta(meta_object)
}

pub fn tool_call_update(id: &str, fields: acp::ToolCallUpdateFields) -> acp::ToolCallUpdate {
    acp::ToolCallUpdate::new(tool_call_id(id), fields)
}

#[cfg(test)]
#[path = "acp_fixtures_tests.rs"]
mod tests;
