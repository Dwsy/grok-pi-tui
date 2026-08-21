//! Project Pi custom `pi-grok-recap/v1` messages into Grok SessionRecap updates.

use serde_json::{Value, json};

const BRIDGE_TYPE: &str = "pi-grok-recap/v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RecapProjection {
    Available { summary: String, auto: bool },
    Unavailable { auto: bool },
}

/// Parse a recap bridge event into a recap projection.
///
/// Returns `Ok(None)` when the event is not a recap bridge message.
pub(crate) fn parse_recap_message(event: &Value) -> Option<RecapProjection> {
    // Live traffic arrives via appendEntry (`entry_appended` with a custom
    // entry carrying `data`); the extension keeps summaries out of the agent
    // context that way. Older builds delivered a display:false custom message
    // (`message_end` with `details`) — keep accepting both shapes.
    let (message, details) = if let Some(entry) = event.get("entry").filter(|entry| {
        field_str(entry, "type") == Some("custom")
            && field_str(entry, "customType") == Some(BRIDGE_TYPE)
    }) {
        (entry, entry.get("data").unwrap_or(entry))
    } else {
        let message = event.get("message").unwrap_or(event);
        if field_str(message, "role") != Some("custom")
            || field_str(message, "customType") != Some(BRIDGE_TYPE)
        {
            return None;
        }
        (message, message.get("details").unwrap_or(message))
    };
    let auto = details
        .get("auto")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let ok = details.get("ok").and_then(Value::as_bool).unwrap_or(false);
    if !ok {
        return Some(RecapProjection::Unavailable { auto });
    }
    let summary = details
        .get("summary")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            // Fallback to message content when details.summary is empty.
            content_text(message)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        });
    match summary {
        Some(summary) => Some(RecapProjection::Available { summary, auto }),
        None => Some(RecapProjection::Unavailable { auto }),
    }
}

pub(crate) fn session_recap_notification(session_id: &str, projection: &RecapProjection) -> Value {
    match projection {
        RecapProjection::Available { summary, auto } => json!({
            "sessionId": session_id,
            "update": {
                "sessionUpdate": "session_recap",
                "summary": summary,
                "auto": auto,
            }
        }),
        RecapProjection::Unavailable { auto: _ } => json!({
            "sessionId": session_id,
            "update": {
                "sessionUpdate": "session_recap_unavailable",
            }
        }),
    }
}

fn field_str<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn content_text(message: &Value) -> Option<String> {
    if let Some(text) = message.get("content").and_then(Value::as_str) {
        return Some(text.to_string());
    }
    let items = message.get("content")?.as_array()?;
    let mut out = String::new();
    for item in items {
        if field_str(item, "type") == Some("text")
            && let Some(text) = item.get("text").and_then(Value::as_str)
        {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(text);
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_success_recap() {
        let event = json!({
            "message": {
                "role": "custom",
                "customType": "pi-grok-recap/v1",
                "content": "ignored body",
                "details": {
                    "version": 1,
                    "ok": true,
                    "auto": false,
                    "summary": "We fixed the recap bridge."
                }
            }
        });
        assert_eq!(
            parse_recap_message(&event),
            Some(RecapProjection::Available {
                summary: "We fixed the recap bridge.".into(),
                auto: false,
            })
        );
    }

    #[test]
    fn parses_unavailable_recap() {
        let event = json!({
            "message": {
                "role": "custom",
                "customType": "pi-grok-recap/v1",
                "details": {
                    "ok": false,
                    "auto": true,
                    "reason": "no main turns yet"
                }
            }
        });
        assert_eq!(
            parse_recap_message(&event),
            Some(RecapProjection::Unavailable { auto: true })
        );
    }

    #[test]
    fn parses_success_recap_from_append_entry() {
        let event = json!({
            "type": "entry_appended",
            "entry": {
                "id": "e1",
                "type": "custom",
                "customType": "pi-grok-recap/v1",
                "data": {
                    "version": 1,
                    "ok": true,
                    "auto": true,
                    "summary": "We fixed the recap bridge via appendEntry."
                },
                "timestamp": "2026-08-21T00:00:00.000Z"
            }
        });
        assert_eq!(
            parse_recap_message(&event),
            Some(RecapProjection::Available {
                summary: "We fixed the recap bridge via appendEntry.".into(),
                auto: true,
            })
        );
    }

    #[test]
    fn ignores_other_custom_entries() {
        let event = json!({
            "type": "entry_appended",
            "entry": {
                "type": "custom",
                "customType": "pi-grok-btw/v1",
                "data": { "version": 1, "answer": "side answer" }
            }
        });
        assert!(parse_recap_message(&event).is_none());
    }

    #[test]
    fn ignores_other_custom_messages() {
        let event = json!({
            "message": {
                "role": "custom",
                "customType": "pi-grok-subagent/v1",
                "details": { "version": 1 }
            }
        });
        assert!(parse_recap_message(&event).is_none());
    }

    #[test]
    fn builds_session_recap_payloads() {
        let ok = session_recap_notification(
            "sess-1",
            &RecapProjection::Available {
                summary: "hello".into(),
                auto: true,
            },
        );
        assert_eq!(ok["sessionId"], "sess-1");
        assert_eq!(ok["update"]["sessionUpdate"], "session_recap");
        assert_eq!(ok["update"]["summary"], "hello");
        assert_eq!(ok["update"]["auto"], true);

        let miss =
            session_recap_notification("sess-1", &RecapProjection::Unavailable { auto: false });
        assert_eq!(miss["update"]["sessionUpdate"], "session_recap_unavailable");
    }
}
