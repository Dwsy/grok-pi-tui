//! After each turn the shell generates an ultra-short one-line summary of the agent's reply for that turn (not a meta activity log).
//! The dashboard row shows it as its secondary line.
//! Like recap, it is display-only and never mutates the conversation.
//! The request carries only the last user message and the agent's visible text after it, so its cost does not grow with the session.

use crate::sampling::ConversationItem;
use crate::session::helpers::chat::floor_char_boundary;

/// The instruction targets 5-12 words; this only guards against runaway output.
/// Rows truncate to width on render.
pub(crate) const TURN_SUMMARY_MAX_CHARS: usize = 200;

/// The message that opened the latest prompt turn and the agent's visible text after it.
/// The opener can be a real prompt or a server-initiated wake (scheduler, task or subagent completion, agent message).
/// Reasoning, tool calls, tool results, and mid-turn injected user-role items are dropped.
/// The user message keeps its first `user_max_chars`, the reply its last `reply_max_chars`.
/// `None` when no turn has started or the agent wrote no text in it.
pub(crate) fn last_turn(
    conversation: &[ConversationItem],
    user_max_chars: usize,
    reply_max_chars: usize,
) -> Option<(String, String)> {
    let (start, opener) = conversation
        .iter()
        .enumerate()
        .rev()
        .find(|(_, item)| {
            matches!(item, ConversationItem::User(u) if u.synthetic_reason.starts_prompt_turn())
        })?;
    let user_text = opener.text_content();
    let user_text = match user_text.trim() {
        "" => "(no text; attachments only)",
        text => text,
    };
    let reply = conversation
        .iter()
        .skip(start + 1)
        .filter_map(|item| match item {
            ConversationItem::Assistant(a) if !a.content.trim().is_empty() => {
                Some(a.content.trim())
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    if reply.is_empty() {
        return None;
    }
    Some((
        keep_head(user_text, user_max_chars),
        keep_tail(&reply, reply_max_chars),
    ))
}

fn keep_head(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let head = text.get(..floor_char_boundary(text, max)).unwrap_or(text);
    format!("{head}\u{2026}")
}

fn keep_tail(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut start = text.len() - max;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    let tail = text.get(start..).unwrap_or(text);
    format!("\u{2026}{tail}")
}

pub(crate) const TURN_SUMMARY_SYSTEM: &str = "Write an ultra-short dashboard line that captures the AGENT'S REPLY \
     to the user's message. Focus on what the agent concluded, answered, recommended, or delivered, \
     not a meta description of the turn (avoid \"Explained…\", \"Answered…\", \"Greeted…\", \
     \"Reviewed…\").\n\n\
     Output ONLY the fragment: 5-12 words, plain text, glanceable on a status row. \
     Prefer the payload: answer, finding, change, or decision needed.\n\n\
     Synthetic examples (style only, do not copy):\n\
     `queue_worker` shutdown race fixed; suite green\n\
     Payment retries: exp backoff in `billing/retry.rs`, 5× on 429\n\
     Retry backoff wired into `billing/retry.rs`; tests pending\n\
     Need decision: keep or drop `sqlx` cache before refactor\n\
     Black — matches the terminal aesthetic\n\n\
     Bad (never):\n\
     - Lead with Explained / Answered / Greeted / Reviewed / Confirmed / Flagged / Summarized\n\
     - Labels, quotes, bullets, markdown, code fences, multi-sentence dumps\n\
     - Filler like \"no code changes\" or \"awaiting task\" unless that is the whole point\n\
     - Invent content not in the agent's reply";

pub(crate) fn turn_summary_user_message(user_text: &str, reply: &str) -> String {
    format!(
        "<user_message>\n{user_text}\n</user_message>\n\n<agent_reply>\n{reply}\n</agent_reply>"
    )
}

/// Clean the model's raw output into a one-line fragment.
/// Recap normalization (whitespace collapse, stray label/quote stripping) runs first, then the tighter [`TURN_SUMMARY_MAX_CHARS`] cap.
pub(crate) fn clean_turn_summary_text(raw: &str) -> String {
    let mut out = super::session_recap::clean_recap_text(raw);
    if out.len() > TURN_SUMMARY_MAX_CHARS {
        let cut = floor_char_boundary(&out, TURN_SUMMARY_MAX_CHARS);
        out.truncate(cut);
        out = out.trim_end().to_string();
        out.push('\u{2026}');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use xai_grok_sampling_types::{ContentPart, SyntheticReason, UserItem};

    fn user(text: &str) -> ConversationItem {
        ConversationItem::user(text.to_string())
    }

    const USER_MAX: usize = 4_000;
    const REPLY_MAX: usize = 32_000;

    #[test]
    fn last_turn_keeps_user_message_and_visible_agent_text() {
        let conv = vec![
            ConversationItem::system("sys"),
            user("old question"),
            ConversationItem::assistant("old answer"),
            user("fix the parser"),
            ConversationItem::assistant("Looking at the parser."),
            ConversationItem::tool_result("call-1", "tool output"),
            ConversationItem::system_reminder("injected"),
            ConversationItem::assistant(format!("{}Fixed the parser.", "x".repeat(REPLY_MAX))),
        ];
        let (user_text, reply) = last_turn(&conv, USER_MAX, REPLY_MAX).unwrap();
        assert_eq!(user_text, "fix the parser");
        assert!(reply.starts_with('\u{2026}'));
        assert!(reply.ends_with("Fixed the parser."));
        assert!(!reply.contains("tool output") && !reply.contains("injected"));

        assert_eq!(
            last_turn(
                &[user("fix it"), ConversationItem::tool_result("c", "out")],
                USER_MAX,
                REPLY_MAX
            ),
            None
        );
        assert_eq!(
            last_turn(
                &[ConversationItem::system_reminder("injected")],
                USER_MAX,
                REPLY_MAX
            ),
            None
        );

        let image_only = ConversationItem::user_with_parts(vec![ContentPart::Image {
            url: "data:image/png;base64,AA".into(),
        }]);
        let wake = ConversationItem::User(UserItem {
            content: vec![ContentPart::Text {
                text: "background task finished".into(),
            }],
            synthetic_reason: SyntheticReason::TaskCompleted,
            ..Default::default()
        });
        for (opener, expected) in [
            (image_only, "(no text; attachments only)"),
            (wake, "background task finished"),
        ] {
            let conv = vec![
                user("old question"),
                ConversationItem::assistant("old answer"),
                opener,
                ConversationItem::assistant("new answer"),
            ];
            let (user_text, reply) = last_turn(&conv, USER_MAX, REPLY_MAX).unwrap();
            assert_eq!(user_text, expected);
            assert_eq!(reply, "new answer");
        }
    }

    #[test]
    fn clean_normalizes_and_caps() {
        assert_eq!(
            clean_turn_summary_text("Summary: \"Fixed the\n\n  parser\""),
            "Fixed the parser"
        );
        let capped = clean_turn_summary_text(&"word ".repeat(100));
        assert!(capped.len() <= TURN_SUMMARY_MAX_CHARS + '\u{2026}'.len_utf8());
        assert!(capped.ends_with('\u{2026}'));
    }
}
