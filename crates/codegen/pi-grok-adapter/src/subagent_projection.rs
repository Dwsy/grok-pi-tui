use crate::tool_projection::{
    edit_diff_content, normalize_tool_raw_input, normalize_tool_raw_output, tool_content, tool_kind,
};
use agent_client_protocol as acp;
use anyhow::{Result, bail};
use serde_json::{Value, json};

const BRIDGE_TYPE: &str = "pi-grok-subagent/v1";

pub(crate) enum BridgeOperation {
    ParentTaskMetadata {
        tool_call_id: String,
        raw_input: Value,
    },
    ParentLifecycle(Value),
    ChildUpdate {
        child_session_id: String,
        update: acp::SessionUpdate,
    },
    ReplayComplete {
        request_id: String,
    },
}

pub(crate) struct BridgeProjection {
    pub sequence: u64,
    pub replay: bool,
    pub kind: String,
    pub subagent_id: String,
    pub child_session_id: String,
    pub operations: Vec<BridgeOperation>,
}

fn bridge_candidate(event: &Value) -> &Value {
    event
        .get("message")
        .or_else(|| event.get("entry"))
        .unwrap_or(event)
}

fn bridge_details(event: &Value) -> Result<Option<&Value>> {
    let message = bridge_candidate(event);
    if field_str(message, "customType") != Some(BRIDGE_TYPE) {
        return Ok(None);
    }
    let details_key = if field_str(message, "role") == Some("custom") {
        "details"
    } else if field_str(message, "type") == Some("custom") {
        "data"
    } else {
        return Ok(None);
    };
    message
        .get(details_key)
        .ok_or_else(|| anyhow::anyhow!("subagent bridge custom message has no {details_key}"))
        .map(Some)
}

pub(crate) fn bridge_parent_session_id(event: &Value) -> Result<Option<&str>> {
    let Some(details) = bridge_details(event)? else {
        return Ok(None);
    };
    Ok(Some(required_str(details, "parentSessionId")?))
}

pub(crate) fn parse_bridge_message(
    event: &Value,
    root_session_id: &str,
) -> Result<Option<BridgeProjection>> {
    let Some(details) = bridge_details(event)? else {
        return Ok(None);
    };
    if details.get("version").and_then(Value::as_u64) != Some(1) {
        bail!("unsupported subagent bridge version");
    }
    let sequence = required_u64(details, "sequence")?;
    let replay = details
        .get("replay")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let parent_session_id = bridge_parent_session_id(event)?.expect("validated subagent bridge");
    if parent_session_id != root_session_id {
        bail!("subagent bridge parentSessionId does not match active Pi session");
    }
    let subagent_id = required_str(details, "subagentId")?.to_string();
    let child_session_id = required_str(details, "childSessionId")?.to_string();
    let payload = details
        .get("payload")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow::anyhow!("subagent bridge payload must be an object"))?;
    let kind = required_str(details, "kind")?;

    let operations = match kind {
        "spawned" => {
            let parent_tool_call_id = required_str_value(payload, "parentToolCallId")?.to_string();
            let background = payload
                .get("background")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let description = required_str_value(payload, "description")?;
            let subagent_type = required_str_value(payload, "subagentType")?;
            let mut update = json!({
                "sessionUpdate": "subagent_spawned",
                "subagent_id": subagent_id,
                "parent_session_id": parent_session_id,
                "child_session_id": child_session_id,
                "subagent_type": subagent_type,
                "description": description,
            });
            for (source, target) in [
                ("capabilityMode", "capability_mode"),
                ("persona", "persona"),
                ("role", "role"),
                ("model", "model"),
            ] {
                if let Some(value) = payload.get(source).filter(|value| !value.is_null()) {
                    update[target] = value.clone();
                }
            }
            let mut lifecycle =
                session_update_envelope(root_session_id, update, replay, &subagent_id, sequence);
            lifecycle["_meta"]["subagentBackground"] = Value::Bool(background);
            vec![
                BridgeOperation::ParentTaskMetadata {
                    tool_call_id: parent_tool_call_id,
                    raw_input: json!({
                        "variant": "Task",
                        "task_id": subagent_id,
                        "run_in_background": background,
                        "reconcile": payload.get("reconcile").and_then(Value::as_bool).unwrap_or(false),
                    }),
                },
                BridgeOperation::ParentLifecycle(lifecycle),
            ]
        }
        "progress" => {
            let update = json!({
                "sessionUpdate": "subagent_progress",
                "subagent_id": subagent_id,
                "parent_session_id": parent_session_id,
                "child_session_id": child_session_id,
                "duration_ms": required_u64_value(payload, "durationMs")?,
                "turn_count": required_u64_value(payload, "turnCount")?,
                "tool_call_count": required_u64_value(payload, "toolCallCount")?,
                "tokens_used": payload.get("tokensUsed").and_then(Value::as_u64).unwrap_or(0),
                "context_window_tokens": payload.get("contextWindowTokens").and_then(Value::as_u64).unwrap_or(0),
                "context_usage_pct": payload.get("contextUsagePct").and_then(Value::as_u64).unwrap_or(0),
                "tools_used": payload.get("toolsUsed").cloned().unwrap_or_else(|| json!([])),
                "error_count": required_u64_value(payload, "errorCount")?,
            });
            vec![BridgeOperation::ParentLifecycle(session_update_envelope(
                root_session_id,
                update,
                replay,
                &subagent_id,
                sequence,
            ))]
        }
        "finished" => {
            let status = required_str_value(payload, "status")?;
            if !matches!(status, "completed" | "failed" | "cancelled") {
                bail!("invalid subagent terminal status: {status}");
            }
            let mut update = json!({
                "sessionUpdate": "subagent_finished",
                "subagent_id": subagent_id,
                "child_session_id": child_session_id,
                "status": status,
                "tool_calls": required_u64_value(payload, "toolCalls")?,
                "turns": required_u64_value(payload, "turns")?,
                "duration_ms": required_u64_value(payload, "durationMs")?,
                "tokens_used": payload.get("tokensUsed").and_then(Value::as_u64).unwrap_or(0),
                "will_wake": false,
            });
            if let Some(error) = payload.get("error").filter(|value| !value.is_null()) {
                update["error"] = error.clone();
            }
            if let Some(output) = payload.get("output").filter(|value| !value.is_null()) {
                update["output"] = output.clone();
            }
            vec![BridgeOperation::ParentLifecycle(session_update_envelope(
                root_session_id,
                update,
                replay,
                &subagent_id,
                sequence,
            ))]
        }
        "child_update" => {
            let update = parse_child_update(payload.get("update"))?;
            vec![BridgeOperation::ChildUpdate {
                child_session_id: child_session_id.clone(),
                update,
            }]
        }
        "replay_complete" => vec![BridgeOperation::ReplayComplete {
            request_id: required_str_value(payload, "requestId")?.to_string(),
        }],
        other => bail!("unknown subagent bridge event: {other}"),
    };

    Ok(Some(BridgeProjection {
        sequence,
        replay,
        kind: kind.to_string(),
        subagent_id,
        child_session_id,
        operations,
    }))
}

fn parse_child_update(value: Option<&Value>) -> Result<acp::SessionUpdate> {
    let update = value
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow::anyhow!("subagent child update must be an object"))?;
    match required_str_value(update, "type")? {
        "user" => Ok(acp::SessionUpdate::UserMessageChunk(text_chunk(
            required_str_value(update, "text")?,
        ))),
        "assistant_delta" => Ok(acp::SessionUpdate::AgentMessageChunk(text_chunk(
            required_str_value(update, "text")?,
        ))),
        "thinking_delta" => Ok(acp::SessionUpdate::AgentThoughtChunk(text_chunk(
            required_str_value(update, "text")?,
        ))),
        "tool_call" => {
            let id = required_str_value(update, "toolCallId")?;
            let name = required_str_value(update, "toolName")?;
            let args = normalize_tool_raw_input(name, update.get("args").cloned());
            let content = edit_diff_content(name, args.as_ref(), None).unwrap_or_default();
            Ok(acp::SessionUpdate::ToolCall(
                acp::ToolCall::new(acp::ToolCallId::new(id.to_string()), name.to_string())
                    .kind(tool_kind(name))
                    .status(acp::ToolCallStatus::InProgress)
                    .content(content)
                    .locations(Vec::new())
                    .raw_input(args),
            ))
        }
        "tool_update" => {
            let name = required_str_value(update, "toolName")?;
            let args = normalize_tool_raw_input(name, update.get("args").cloned());
            let result = update.get("partialResult").cloned().unwrap_or(Value::Null);
            let mut fields = acp::ToolCallUpdateFields::new()
                .status(Some(acp::ToolCallStatus::InProgress))
                .raw_output(Some(normalize_tool_raw_output(
                    name,
                    args.as_ref(),
                    &result,
                    false,
                )));
            if tool_kind(name) != acp::ToolKind::Edit {
                fields = fields.content(Some(tool_content(&result)));
            }
            Ok(acp::SessionUpdate::ToolCallUpdate(
                acp::ToolCallUpdate::new(
                    acp::ToolCallId::new(required_str_value(update, "toolCallId")?.to_string()),
                    fields,
                ),
            ))
        }
        "tool_result" => {
            let name = required_str_value(update, "toolName")?;
            let args = normalize_tool_raw_input(name, update.get("args").cloned());
            let result = update.get("result").cloned().unwrap_or(Value::Null);
            let is_error = update.get("isError").and_then(Value::as_bool) == Some(true);
            let status = if is_error {
                acp::ToolCallStatus::Failed
            } else {
                acp::ToolCallStatus::Completed
            };
            let mut fields = acp::ToolCallUpdateFields::new()
                .status(Some(status))
                .raw_output(Some(normalize_tool_raw_output(
                    name,
                    args.as_ref(),
                    &result,
                    is_error,
                )));
            if tool_kind(name) == acp::ToolKind::Edit {
                fields = fields.content(edit_diff_content(name, args.as_ref(), Some(&result)));
            } else {
                fields = fields.content(Some(tool_content(&result)));
            }
            Ok(acp::SessionUpdate::ToolCallUpdate(
                acp::ToolCallUpdate::new(
                    acp::ToolCallId::new(required_str_value(update, "toolCallId")?.to_string()),
                    fields,
                ),
            ))
        }
        other => bail!("unknown child update type: {other}"),
    }
}

fn session_update_envelope(
    session_id: &str,
    update: Value,
    replay: bool,
    subagent_id: &str,
    sequence: u64,
) -> Value {
    json!({
        "sessionId": session_id,
        "update": update,
        "_meta": {
            "isReplay": replay,
            "eventId": format!("pi-grok-subagent:{subagent_id}:{sequence}"),
        },
    })
}

fn text_chunk(text: &str) -> acp::ContentChunk {
    acp::ContentChunk::new(acp::ContentBlock::Text(acp::TextContent::new(
        text.to_string(),
    )))
}

fn field_str<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn required_str<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    field_str(value, key).ok_or_else(|| anyhow::anyhow!("subagent bridge {key} is required"))
}

fn required_str_value<'a>(value: &'a serde_json::Map<String, Value>, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("subagent bridge payload.{key} is required"))
}

fn required_u64(value: &Value, key: &str) -> Result<u64> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow::anyhow!("subagent bridge {key} is required"))
}

fn required_u64_value(value: &serde_json::Map<String, Value>, key: &str) -> Result<u64> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow::anyhow!("subagent bridge payload.{key} is required"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bridge_message(kind: &str, payload: Value) -> Value {
        json!({
            "message": {
                "role": "custom",
                "customType": BRIDGE_TYPE,
                "display": false,
                "details": {
                    "version": 1,
                    "sequence": 7,
                    "replay": false,
                    "kind": kind,
                    "parentSessionId": "parent-1",
                    "subagentId": "subagent-1",
                    "childSessionId": "child-1",
                    "payload": payload,
                },
            },
        })
    }

    #[test]
    fn spawned_projection_stamps_native_task_metadata_before_lifecycle() {
        let event = bridge_message(
            "spawned",
            json!({
                "parentToolCallId": "call-1",
                "description": "Inspect auth",
                "subagentType": "explore",
                "background": true,
                "capabilityMode": "execute",
                "model": "test-model",
                "prompt": "Inspect authentication.",
            }),
        );
        let projection = parse_bridge_message(&event, "parent-1")
            .unwrap()
            .expect("bridge event");
        assert_eq!(projection.sequence, 7);
        assert_eq!(projection.operations.len(), 2);
        let BridgeOperation::ParentTaskMetadata {
            tool_call_id,
            raw_input,
        } = &projection.operations[0]
        else {
            panic!("spawn must start with parent task metadata");
        };
        assert_eq!(tool_call_id, "call-1");
        assert_eq!(raw_input["variant"], "Task");
        assert_eq!(raw_input["task_id"], "subagent-1");
        assert_eq!(raw_input["run_in_background"], true);
        let BridgeOperation::ParentLifecycle(lifecycle) = &projection.operations[1] else {
            panic!("spawn must project a parent lifecycle event");
        };
        assert_eq!(lifecycle["sessionId"], "parent-1");
        assert_eq!(lifecycle["update"]["sessionUpdate"], "subagent_spawned");
        assert_eq!(lifecycle["update"]["child_session_id"], "child-1");
        assert_eq!(lifecycle["_meta"]["subagentBackground"], true);
    }

    #[test]
    fn appended_bridge_entry_is_projected_like_a_live_custom_message() {
        let mut event = bridge_message(
            "child_update",
            json!({ "update": { "type": "assistant_delta", "text": "hello" } }),
        );
        let details = event["message"]["details"].take();
        event = json!({
            "type": "entry_appended",
            "entry": {
                "type": "custom",
                "customType": BRIDGE_TYPE,
                "data": details,
            },
        });
        let projection = parse_bridge_message(&event, "parent-1")
            .unwrap()
            .expect("bridge entry");
        assert_eq!(projection.sequence, 7);
        assert!(matches!(
            projection.operations.as_slice(),
            [BridgeOperation::ChildUpdate { .. }]
        ));
    }

    #[test]
    fn child_text_delta_targets_the_declared_child_session() {
        let event = bridge_message(
            "child_update",
            json!({ "update": { "type": "assistant_delta", "text": "hello" } }),
        );
        let projection = parse_bridge_message(&event, "parent-1")
            .unwrap()
            .expect("bridge event");
        let [
            BridgeOperation::ChildUpdate {
                child_session_id,
                update,
            },
        ] = projection.operations.as_slice()
        else {
            panic!("expected one child update");
        };
        assert_eq!(child_session_id, "child-1");
        let acp::SessionUpdate::AgentMessageChunk(chunk) = update else {
            panic!("expected an assistant chunk");
        };
        let acp::ContentBlock::Text(text) = &chunk.content else {
            panic!("expected text content");
        };
        assert_eq!(text.text, "hello");
    }

    #[test]
    fn child_find_tool_projects_native_search_input_and_output() {
        let start = bridge_message(
            "child_update",
            json!({
                "update": {
                    "type": "tool_call",
                    "toolCallId": "tool-1",
                    "toolName": "find",
                    "args": { "pattern": "README.md", "path": ".", "limit": 20 },
                },
            }),
        );
        let projection = parse_bridge_message(&start, "parent-1")
            .unwrap()
            .expect("bridge event");
        let [BridgeOperation::ChildUpdate { update, .. }] = projection.operations.as_slice() else {
            panic!("expected one child update");
        };
        let acp::SessionUpdate::ToolCall(call) = update else {
            panic!("expected a tool call");
        };
        assert!(matches!(&call.kind, acp::ToolKind::Search));
        assert_eq!(
            call.raw_input.as_ref().unwrap()["output_mode"],
            "files_with_matches"
        );

        let end = bridge_message(
            "child_update",
            json!({
                "update": {
                    "type": "tool_result",
                    "toolCallId": "tool-1",
                    "toolName": "find",
                    "args": { "pattern": "README.md", "path": ".", "limit": 20 },
                    "result": { "content": [{ "type": "text", "text": "README.md\n" }] },
                    "isError": false,
                },
            }),
        );
        let projection = parse_bridge_message(&end, "parent-1")
            .unwrap()
            .expect("bridge event");
        let [BridgeOperation::ChildUpdate { update, .. }] = projection.operations.as_slice() else {
            panic!("expected one child update");
        };
        let acp::SessionUpdate::ToolCallUpdate(update) = update else {
            panic!("expected a tool update");
        };
        let output = update.fields.raw_output.as_ref().expect("native output");
        assert_eq!(output["type"], "GrepSearch");
        assert_eq!(output["match_count"], 1);
    }

    #[test]
    fn replay_complete_projects_an_adapter_barrier_marker() {
        let event = bridge_message("replay_complete", json!({ "requestId": "replay-1" }));
        let projection = parse_bridge_message(&event, "parent-1")
            .unwrap()
            .expect("bridge event");
        assert!(matches!(
            projection.operations.as_slice(),
            [BridgeOperation::ReplayComplete { request_id }] if request_id == "replay-1"
        ));
    }

    #[test]
    fn bridge_parent_session_id_identifies_only_subagent_events() {
        let event = bridge_message("progress", json!({}));
        assert_eq!(bridge_parent_session_id(&event).unwrap(), Some("parent-1"));
        assert_eq!(
            bridge_parent_session_id(&json!({ "type": "turn_end" })).unwrap(),
            None
        );
    }

    #[test]
    fn parent_session_mismatch_is_rejected() {
        let event = bridge_message(
            "progress",
            json!({
                "durationMs": 1,
                "turnCount": 0,
                "toolCallCount": 0,
                "errorCount": 0,
            }),
        );
        assert!(parse_bridge_message(&event, "another-parent").is_err());
    }
}
