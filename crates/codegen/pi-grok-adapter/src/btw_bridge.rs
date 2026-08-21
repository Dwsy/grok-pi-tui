//! Project Pi BTW bridge messages into streamed review deltas and ACP x.ai/btw answers;
//! parse the separate persisted custom entities used by BTW history.

use serde_json::{Value, json};

const BRIDGE_TYPE: &str = "pi-grok-btw/v1";
pub(crate) const HISTORY_ENTRY_TYPE: &str = "pi-grok-btw/history/v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BtwHistoryEntry {
    pub(crate) id: String,
    pub(crate) question: String,
    pub(crate) answer: String,
    pub(crate) created_at_ms: Option<i64>,
    pub(crate) model_used: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BtwProjection {
    Delta {
        request_id: String,
        delta: String,
    },
    Complete {
        request_id: String,
        result: Result<String, String>,
        model_used: Option<String>,
    },
}

/// Parse a persisted Pi custom entity into a BTW history record.
///
/// Returns `None` for unrelated entries or malformed history data. The Pi
/// entry id is used as the stable identity because `appendEntry()` does not
/// expose an id to the extension.
pub(crate) fn parse_btw_history_entry(value: &Value) -> Option<BtwHistoryEntry> {
    let entry = value.get("entry").unwrap_or(value);
    if entry.get("type").and_then(Value::as_str) != Some("custom")
        || entry.get("customType").and_then(Value::as_str) != Some(HISTORY_ENTRY_TYPE)
    {
        return None;
    }
    let data = entry.get("data")?;
    if data.get("version").and_then(Value::as_u64) != Some(1) {
        return None;
    }
    let question = data.get("question").and_then(Value::as_str)?;
    let answer = data.get("answer").and_then(Value::as_str)?;
    if question.trim().is_empty() || answer.trim().is_empty() {
        return None;
    }
    let id = entry
        .get("id")
        .and_then(Value::as_str)
        .or_else(|| data.get("requestId").and_then(Value::as_str))?
        .to_owned();
    let created_at_ms = data.get("createdAt").and_then(Value::as_i64).or_else(|| {
        data.get("createdAt")
            .and_then(Value::as_u64)
            .map(|v| v as i64)
    });
    Some(BtwHistoryEntry {
        id,
        question: question.to_owned(),
        answer: answer.to_owned(),
        created_at_ms,
        model_used: data
            .get("modelUsed")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

/// Parse a btw bridge event into a streamed delta or final projection.
///
/// Returns `None` when the event is not a btw bridge message.
pub(crate) fn parse_btw_message(event: &Value) -> Option<BtwProjection> {
    // Live traffic arrives via appendEntry (`entry_appended` with a custom
    // entry carrying `data`); the extension keeps deltas/answers out of the
    // agent context that way. Older builds delivered display:false custom
    // messages (`message_end` with `details`) — keep accepting both shapes.
    let (message, details) = if let Some(entry) = event.get("entry").filter(|entry| {
        field_str(entry, "type") == Some("custom")
            && field_str(entry, "customType") == Some(BRIDGE_TYPE)
    }) {
        (entry, entry.get("data").unwrap_or(entry))
    } else {
        let message = event
            .get("message")
            .or_else(|| event.get("entry").and_then(|e| e.get("message")))
            .unwrap_or(event);
        let custom_type = message
            .get("customType")
            .or_else(|| message.get("custom_type"))
            .and_then(Value::as_str)?;
        if custom_type != BRIDGE_TYPE {
            return None;
        }
        (message, message.get("details").unwrap_or(&Value::Null))
    };
    let request_id = details
        .get("requestId")
        .or_else(|| details.get("request_id"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if details.get("phase").and_then(Value::as_str) == Some("delta") {
        let delta = details.get("delta").and_then(Value::as_str)?.to_string();
        return (!delta.is_empty()).then_some(BtwProjection::Delta { request_id, delta });
    }
    let model_used = details
        .get("modelUsed")
        .or_else(|| details.get("model_used"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let ok = details.get("ok").and_then(Value::as_bool).unwrap_or(false);
    if ok {
        let answer = details
            .get("answer")
            .and_then(Value::as_str)
            .or_else(|| message.get("content").and_then(Value::as_str))
            .unwrap_or("")
            .to_string();
        if answer.is_empty() {
            return Some(BtwProjection::Complete {
                request_id,
                result: Err("Empty side question response".into()),
                model_used,
            });
        }
        Some(BtwProjection::Complete {
            request_id,
            result: Ok(answer),
            model_used,
        })
    } else {
        let error = details
            .get("error")
            .and_then(Value::as_str)
            .or_else(|| message.get("content").and_then(Value::as_str))
            .unwrap_or("side question failed")
            .to_string();
        Some(BtwProjection::Complete {
            request_id,
            result: Err(error),
            model_used,
        })
    }
}

#[allow(dead_code)]
pub(crate) fn btw_answer_payload(projection: &BtwProjection) -> Value {
    match projection {
        BtwProjection::Delta { delta, .. } => json!({ "delta": delta }),
        BtwProjection::Complete {
            result, model_used, ..
        } => match result {
            Ok(answer) => {
                let mut body = json!({ "answer": answer });
                if let Some(model) = model_used {
                    body["modelUsed"] = json!(model);
                }
                body
            }
            Err(error) => json!({ "error": error }),
        },
    }
}

fn field_str<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_success() {
        let event = json!({
            "message": {
                "role": "custom",
                "customType": "pi-grok-btw/v1",
                "content": "42",
                "details": {
                    "ok": true,
                    "requestId": "r1",
                    "answer": "42",
                    "modelUsed": "openai::gpt"
                }
            }
        });
        let p = parse_btw_message(&event).expect("projection");
        assert_eq!(
            p,
            BtwProjection::Complete {
                request_id: "r1".into(),
                result: Ok("42".into()),
                model_used: Some("openai::gpt".into()),
            }
        );
    }

    #[test]
    fn parses_delta() {
        let event = json!({
            "message": {
                "customType": "pi-grok-btw/v1",
                "details": {
                    "ok": true,
                    "phase": "delta",
                    "requestId": "r1",
                    "delta": "partial"
                }
            }
        });
        assert_eq!(
            parse_btw_message(&event),
            Some(BtwProjection::Delta {
                request_id: "r1".into(),
                delta: "partial".into(),
            })
        );
    }

    #[test]
    fn parses_error() {
        let event = json!({
            "message": {
                "customType": "pi-grok-btw/v1",
                "details": {
                    "ok": false,
                    "requestId": "r2",
                    "error": "All /btw models failed"
                }
            }
        });
        let p = parse_btw_message(&event).expect("projection");
        assert!(matches!(
            p,
            BtwProjection::Complete {
                request_id,
                result: Err(error),
                ..
            } if request_id == "r2" && error.contains("failed")
        ));
    }

    #[test]
    fn parses_delta_from_append_entry() {
        let event = json!({
            "type": "entry_appended",
            "entry": {
                "id": "e1",
                "type": "custom",
                "customType": "pi-grok-btw/v1",
                "data": {
                    "version": 1,
                    "requestId": "r1",
                    "ok": true,
                    "phase": "delta",
                    "delta": "partial"
                },
                "timestamp": "2026-08-21T00:00:00.000Z"
            }
        });
        assert_eq!(
            parse_btw_message(&event),
            Some(BtwProjection::Delta {
                request_id: "r1".into(),
                delta: "partial".into(),
            })
        );
    }

    #[test]
    fn parses_complete_from_append_entry() {
        let event = json!({
            "type": "entry_appended",
            "entry": {
                "type": "custom",
                "customType": "pi-grok-btw/v1",
                "data": {
                    "version": 1,
                    "requestId": "r3",
                    "ok": true,
                    "phase": "complete",
                    "answer": "42",
                    "modelUsed": "openai::gpt"
                }
            }
        });
        assert_eq!(
            parse_btw_message(&event),
            Some(BtwProjection::Complete {
                request_id: "r3".into(),
                result: Ok("42".into()),
                model_used: Some("openai::gpt".into()),
            })
        );
    }

    #[test]
    fn ignores_history_and_other_custom_entries() {
        // /btw-history entries use a dedicated custom type.
        let history = json!({
            "type": "entry_appended",
            "entry": {
                "type": "custom",
                "customType": "pi-grok-btw/history/v1",
                "data": { "version": 1, "answer": "saved" }
            }
        });
        assert!(parse_btw_message(&history).is_none());

        let other = json!({
            "type": "entry_appended",
            "entry": {
                "type": "custom",
                "customType": "pi-grok-recap/v1",
                "data": { "version": 1 }
            }
        });
        assert!(parse_btw_message(&other).is_none());
    }

    #[test]
    fn ignores_other_types() {
        let event = json!({ "message": { "customType": "pi-grok-recap/v1" } });
        assert!(parse_btw_message(&event).is_none());
    }
}
