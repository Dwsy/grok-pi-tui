//! Conservative Pi text-branch reconciliation through the native ACP tracker.
//! Stateful histories continue to use session/load. A content generation guard
//! invalidates the mapping after live output or any other transcript mutation.
use crate::acp::{meta::NotificationMeta, tracker::AcpUpdateTracker};
use crate::scrollback::state::ScrollbackState;
use agent_client_protocol as acp;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct Message {
    id: String,
    role: String,
    timestamp_ms: Option<i64>,
    parts: Vec<Part>,
}
#[derive(Debug, Clone, Deserialize, PartialEq)]
struct Part {
    kind: String,
    text: String,
}
#[derive(Deserialize)]
struct Projection {
    version: u32,
    messages: Vec<Message>,
}
#[derive(Debug)]
pub(crate) struct TextProjectionCache {
    session: String,
    messages: Vec<Message>,
    /// Scrollback offset BEFORE each message. Only user boundaries are reusable.
    offsets: Vec<usize>,
    generation: u64,
    thinking_visible: bool,
}

/// Validate the entire response before mutating scrollback. Returns None to
/// request the established full replay path for malformed/unsupported payloads.
pub(crate) fn apply(
    scrollback: &mut ScrollbackState,
    cache: Option<TextProjectionCache>,
    session: &str,
    payload: serde_json::Value,
) -> Option<TextProjectionCache> {
    let Projection { version, messages } = serde_json::from_value(payload).ok()?;
    let mut ids = std::collections::HashSet::new();
    if version != 1
        || messages.iter().any(|m| {
            m.id.is_empty()
                || !ids.insert(m.id.as_str())
                || m.parts.is_empty()
                || !matches!(m.role.as_str(), "user" | "assistant")
                || m.parts.iter().any(|p| {
                    p.text.is_empty()
                        || match m.role.as_str() {
                            "user" => p.kind != "user",
                            _ => !matches!(p.kind.as_str(), "assistant" | "thinking"),
                        }
                })
        })
    {
        return None;
    }
    let thinking_visible = crate::appearance::cache::load_show_thinking_blocks();
    let cache = cache.filter(|c| {
        c.session == session
            && c.generation == scrollback.content_generation()
            && c.thinking_visible == thinking_visible
    });
    let common = cache.as_ref().map_or(0, |c| {
        c.messages
            .iter()
            .zip(&messages)
            .take_while(|(old, new)| old == new)
            .count()
    });
    // An exact snapshot is a no-op, including the current viewport and folds.
    if let Some(c) = cache.as_ref()
        && common == messages.len()
        && common == c.messages.len()
    {
        return cache;
    }
    // User messages reset ACP text/thinking state. Replaying from an arbitrary
    // assistant boundary could append to a retained block and corrupt its cache.
    let boundary = (0..common)
        .rev()
        .find(|&i| messages[i].role == "user")
        .unwrap_or(0);
    let cut = cache
        .as_ref()
        .filter(|_| boundary > 0)
        .map_or(0, |c| c.offsets[boundary]);
    let mut offsets = cache
        .as_ref()
        .map_or_else(Vec::new, |c| c.offsets[..boundary].to_vec());
    scrollback.begin_batch();
    scrollback.remove_from(cut);
    let mut tracker = AcpUpdateTracker::new();
    for message in &messages[boundary..] {
        if message.role == "user" {
            tracker.finish_turn(scrollback);
        }
        offsets.push(scrollback.len());
        let meta = NotificationMeta {
            is_replay: true,
            agent_timestamp_ms: message.timestamp_ms,
            ..Default::default()
        };
        for part in &message.parts {
            let chunk = acp::ContentChunk::new(acp::ContentBlock::Text(acp::TextContent::new(
                part.text.clone(),
            )));
            let update = match part.kind.as_str() {
                "user" => acp::SessionUpdate::UserMessageChunk(chunk),
                "thinking" => acp::SessionUpdate::AgentThoughtChunk(chunk),
                _ => acp::SessionUpdate::AgentMessageChunk(chunk),
            };
            tracker.handle_update(update, &meta, scrollback);
        }
    }
    tracker.finish_turn(scrollback);
    scrollback.end_batch();
    tracing::debug!(
        retained_messages = boundary,
        rebuilt_messages = messages.len() - boundary,
        "Pi tree text projection reconciled"
    );
    Some(TextProjectionCache {
        session: session.to_owned(),
        messages,
        offsets,
        generation: scrollback.content_generation(),
        thinking_visible,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn message(id: &str, role: &str, text: &str) -> serde_json::Value {
        json!({"id": id, "role": role, "timestampMs": 1000,
            "parts": [{"kind": role, "text": text}]})
    }
    fn project(messages: Vec<serde_json::Value>) -> serde_json::Value {
        json!({"version": 1, "messages": messages})
    }
    #[test]
    fn sibling_switch_retains_prefix_ids_and_matches_fresh_projection() {
        let prefix = vec![
            message("u1", "user", "one"),
            message("a1", "assistant", "answer"),
            message("u2", "user", "two"),
            message("a2", "assistant", "second"),
        ];
        let mut old = prefix.clone();
        old.push(message("old", "user", "old branch"));
        let mut sb = ScrollbackState::new();
        let cache = apply(&mut sb, None, "s", project(old)).unwrap();
        let id = sb.get(0).unwrap().id;
        let mut target = prefix;
        target.push(message("new", "user", "new branch"));
        target.push(message("new-a", "assistant", "**new** answer"));
        let cache = apply(&mut sb, Some(cache), "s", project(target.clone())).unwrap();
        assert_eq!(sb.get(0).unwrap().id, id);
        let mut fresh = ScrollbackState::new();
        apply(&mut fresh, None, "s", project(target)).unwrap();
        assert_eq!(sb.len(), fresh.len());
        for i in 0..sb.len() {
            assert_eq!(
                format!("{:?}", sb.get(i).unwrap().block),
                format!("{:?}", fresh.get(i).unwrap().block)
            );
        }
        let generation = sb.content_generation();
        let payload = project(cache.messages.iter().map(|m| json!({
            "id": m.id, "role": m.role, "timestampMs": m.timestamp_ms,
            "parts": m.parts.iter().map(|p| json!({"kind": p.kind, "text": p.text})).collect::<Vec<_>>()
        })).collect());
        apply(&mut sb, Some(cache), "s", payload).unwrap();
        assert_eq!(sb.content_generation(), generation);
    }
    #[test]
    fn content_changes_and_identical_text_with_new_ids_cannot_reuse_mapping() {
        let target = project(vec![
            message("u", "user", "same"),
            message("a", "assistant", "text"),
        ]);
        let mut sb = ScrollbackState::new();
        let cache = apply(&mut sb, None, "s", target.clone()).unwrap();
        let id = sb.get(0).unwrap().id;
        sb.push_block(crate::scrollback::block::RenderBlock::system("live"));
        let cache = apply(&mut sb, Some(cache), "s", target).unwrap();
        assert_ne!(sb.get(0).unwrap().id, id);
        let id = sb.get(0).unwrap().id;
        apply(
            &mut sb,
            Some(cache),
            "s",
            project(vec![message("different", "user", "same")]),
        )
        .unwrap();
        assert_ne!(sb.get(0).unwrap().id, id);
    }
    #[test]
    fn malformed_projection_is_rejected_before_mutation() {
        let mut sb = ScrollbackState::new();
        sb.push_block(crate::scrollback::block::RenderBlock::system("keep"));
        let generation = sb.content_generation();
        assert!(
            apply(
                &mut sb,
                None,
                "s",
                project(vec![message("x", "toolResult", "bad")])
            )
            .is_none()
        );
        assert_eq!(sb.len(), 1);
        assert_eq!(sb.content_generation(), generation);
    }
    #[test]
    fn ancestor_and_empty_branch_match_full_projection() {
        let mut sb = ScrollbackState::new();
        let first = vec![
            message("u1", "user", "first"),
            message("a1", "assistant", "answer"),
        ];
        let mut longer = first.clone();
        longer.extend([
            message("u2", "user", "next"),
            message("a2", "assistant", "answer two"),
        ]);
        let cache = apply(&mut sb, None, "s", project(longer)).unwrap();
        let cache = apply(&mut sb, Some(cache), "s", project(first)).unwrap();
        assert_eq!(sb.len(), 2);
        apply(&mut sb, Some(cache), "s", project(vec![])).unwrap();
        assert_eq!(sb.len(), 0);
    }

    #[test]
    #[ignore = "manual native projection timing"]
    fn text_tree_projection_benchmark() {
        let text = "**Markdown** code and text\n\n".repeat(200);
        let mut base = Vec::new();
        for i in 0..200 {
            base.push(message(&format!("u{i}"), "user", "prompt"));
            base.push(message(&format!("a{i}"), "assistant", &text));
        }
        let mut old = base.clone();
        old.push(message("old", "user", "old branch"));
        let mut target = base;
        target.push(message("new", "user", "new branch"));
        target.push(message("answer", "assistant", &text));
        let target = project(target);
        let mut full_times = Vec::new();
        let mut delta_times = Vec::new();
        for _ in 0..5 {
            let mut sb = ScrollbackState::new();
            let cache = apply(&mut sb, None, "s", project(old.clone())).unwrap();
            let start = std::time::Instant::now();
            apply(&mut sb, Some(cache), "s", target.clone()).unwrap();
            delta_times.push(start.elapsed().as_micros());
            let mut fresh = ScrollbackState::new();
            let start = std::time::Instant::now();
            apply(&mut fresh, None, "s", target.clone()).unwrap();
            full_times.push(start.elapsed().as_micros());
            assert_eq!(sb.len(), fresh.len());
        }
        full_times.sort();
        delta_times.sort();
        eprintln!(
            "text projection 200 shared turns, ~1MB markdown, debug build: full median={}us; incremental median={}us; includes snapshot clone/decode; excludes RPC/terminal",
            full_times[2], delta_times[2]
        );
    }
    #[test]
    fn projection_matches_standard_acp_replay_with_thinking() {
        let payload = project(vec![
            message("u1", "user", "question"),
            json!({"id":"a1","role":"assistant","timestampMs":1000,"parts":[
                {"kind":"thinking","text":"reason"},{"kind":"assistant","text":"answer"}]}),
            message("u2", "user", "next"),
            message("a2", "assistant", "next answer"),
        ]);
        let parsed: Projection = serde_json::from_value(payload.clone()).unwrap();
        let mut normal = ScrollbackState::new();
        let mut tracker = AcpUpdateTracker::new();
        normal.begin_batch();
        for m in parsed.messages {
            for p in m.parts {
                let chunk =
                    acp::ContentChunk::new(acp::ContentBlock::Text(acp::TextContent::new(p.text)));
                let update = match p.kind.as_str() {
                    "user" => acp::SessionUpdate::UserMessageChunk(chunk),
                    "thinking" => acp::SessionUpdate::AgentThoughtChunk(chunk),
                    _ => acp::SessionUpdate::AgentMessageChunk(chunk),
                };
                tracker.handle_update(
                    update,
                    &NotificationMeta {
                        is_replay: true,
                        agent_timestamp_ms: m.timestamp_ms,
                        ..Default::default()
                    },
                    &mut normal,
                );
            }
        }
        tracker.finish_turn(&mut normal);
        normal.end_batch();
        let mut optimized = ScrollbackState::new();
        apply(&mut optimized, None, "s", payload).unwrap();
        assert_eq!(optimized.len(), normal.len());
        for i in 0..normal.len() {
            assert_eq!(
                format!("{:?}", optimized.get(i).unwrap().block),
                format!("{:?}", normal.get(i).unwrap().block)
            );
            // Native replay does not stamp Thinking blocks with the original
            // message time; their default created_at is Local::now().
            if !matches!(
                normal.get(i).unwrap().block,
                crate::scrollback::block::RenderBlock::Thinking(_)
            ) {
                assert_eq!(
                    optimized.get(i).unwrap().created_at,
                    normal.get(i).unwrap().created_at
                );
            }
        }
    }
}
