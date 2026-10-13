use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

#[test]
fn ext_notification_serializes_to_its_params_alone() {
    let notification = ext_notification("x.ai/session/update", &json!({ "sessionId": "s1" }));

    assert_eq!(
        json!({ "sessionId": "s1" }),
        serde_json::to_value(&notification).expect("notification serializes")
    );
}

#[test]
fn model_info_with_meta_serializes_id_name_and_meta() {
    let meta = json!({
        "supportsReasoningEffort": true,
        "reasoningEfforts": [{ "value": "none", "label": "None", "default": true }],
    });

    let info = model_info_with_meta("voice-dual", "Voice Dual", meta.clone());

    assert_eq!(
        json!({
            "modelId": "voice-dual",
            "name": "Voice Dual",
            "_meta": meta,
        }),
        serde_json::to_value(&info).expect("model info serializes")
    );
}

#[test]
fn session_notification_serializes_session_id_and_tagged_update() {
    let notification = session_notification(
        "sess-1",
        acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(text_block("hi"))),
    );

    assert_eq!(
        json!({
            "sessionId": "sess-1",
            "update": {
                "sessionUpdate": "agent_message_chunk",
                "content": { "type": "text", "text": "hi" },
            },
        }),
        serde_json::to_value(&notification).expect("session notification serializes")
    );
}

#[test]
fn tool_call_update_serializes_tool_call_id_and_fields() {
    let update = tool_call_update(
        "call-1",
        acp::ToolCallUpdateFields::new().raw_input(Some(json!({ "path": "a.rs" }))),
    );

    assert_eq!(
        json!({
            "toolCallId": "call-1",
            "rawInput": { "path": "a.rs" },
        }),
        serde_json::to_value(&update).expect("tool call update serializes")
    );
}
