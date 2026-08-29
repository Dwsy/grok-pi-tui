use super::*;
use serde_json::json;

#[test]
fn parse_session_tree_flattens_depth_and_marks_current_leaf() {
    let tree = parse_session_tree(&json!({
        "leafId": "a2",
        "tree": [{
            "entry": {
                "type": "message",
                "id": "u1",
                "message": { "role": "user", "content": "first\nline" }
            },
            "children": [{
                "entry": {
                    "type": "message",
                    "id": "a2",
                    "message": {
                        "role": "assistant",
                        "content": [{ "type": "text", "text": "reply" }]
                    }
                },
                "children": [],
                "label": "checkpoint"
            }]
        }]
    }));
    assert_eq!(tree.leaf_id.as_deref(), Some("a2"));
    assert_eq!(tree.rows.len(), 2);
    assert_eq!(tree.rows[0].id, "u1");
    assert_eq!(tree.rows[0].depth, 0);
    assert_eq!(tree.rows[0].role, "user");
    assert_eq!(tree.rows[0].preview, "first line");
    assert!(!tree.rows[0].is_current);
    assert!(tree.rows[0].on_active_path);
    assert_eq!(tree.rows[0].child_ids, vec!["a2".to_string()]);
    assert_eq!(tree.rows[1].id, "a2");
    assert_eq!(tree.rows[1].parent_id.as_deref(), Some("u1"));
    assert_eq!(tree.rows[1].depth, 1);
    assert!(tree.rows[1].is_current);
    assert!(tree.rows[1].on_active_path);
    assert_eq!(tree.rows[1].label.as_deref(), Some("checkpoint"));
    assert_eq!(tree.rows[1].role, "assistant");
    assert_eq!(tree.rows[1].preview, "reply");
    assert_eq!(tree.rows[1].detail, "reply");
    assert!(tree.rows[1].has_text);
}

#[test]
fn tree_entry_editor_text_preserves_full_user_message() {
    let tree = json!({
        "tree": [{
            "entry": {
                "type": "message",
                "id": "u1",
                "message": {
                    "role": "user",
                    "content": [
                        { "type": "text", "text": "first line\n" },
                        { "type": "image", "data": "ignored" },
                        { "type": "text", "text": "second line" }
                    ]
                }
            },
            "children": [{
                "entry": {
                    "type": "custom_message",
                    "id": "c1",
                    "content": "custom text"
                },
                "children": []
            }]
        }]
    });

    assert_eq!(
        tree_entry_editor_text(&tree, "u1").as_deref(),
        Some("first line\nsecond line")
    );
    assert_eq!(
        tree_entry_editor_text(&tree, "c1").as_deref(),
        Some("custom text")
    );
    assert_eq!(tree_entry_editor_text(&tree, "missing"), None);
}

#[test]
fn parse_session_tree_formats_tool_results_from_tool_calls() {
    let tree = parse_session_tree(&json!({
        "leafId": "tr1",
        "tree": [{
            "entry": {
                "type": "message",
                "id": "a1",
                "message": {
                    "role": "assistant",
                    "content": [{
                        "type": "toolCall",
                        "id": "call-1",
                        "name": "bash",
                        "arguments": { "command": "echo hi" }
                    }]
                }
            },
            "children": [{
                "entry": {
                    "type": "message",
                    "id": "tr1",
                    "message": {
                        "role": "toolResult",
                        "toolCallId": "call-1",
                        "toolName": "bash",
                        "content": [{ "type": "text", "text": "hi" }]
                    }
                },
                "children": []
            }]
        }]
    }));
    assert_eq!(tree.rows.len(), 2);
    assert!(!tree.rows[0].has_text);
    assert_eq!(tree.rows[0].preview, "(no content)");
    assert_eq!(tree.rows[1].preview, "[bash: echo hi]");
}

#[test]
fn history_preserves_reasoning_tools_and_results() {
    let items = parse_messages(&json!({
        "messages": [
            { "role": "user", "content": "hello" },
            {
                "role": "assistant",
                "usage": { "input": 101, "output": 23, "cacheRead": 17, "cost": 0.01 },
                "content": [
                    { "type": "thinking", "thinking": "plan" },
                    { "type": "toolCall", "id": "tool-1", "name": "read", "arguments": { "path": "README.md" } },
                    { "type": "text", "text": "done" }
                ]
            },
            {
                "role": "toolResult",
                "toolCallId": "tool-1",
                "toolName": "read",
                "content": [{ "type": "text", "text": "file" }],
                "isError": false
            }
        ]
    }));
    assert!(matches!(items[0].item, PiHistoryItem::UserText(ref text) if text == "hello"));
    assert!(matches!(items[1].item, PiHistoryItem::AgentThought(ref text) if text == "plan"));
    assert!(matches!(
        items[2].item,
        PiHistoryItem::ToolStart { ref id, ref usage, .. }
            if id == "tool-1"
                && usage.as_ref().and_then(|u| u.get("input")).and_then(Value::as_u64)
                    == Some(101)
    ));
    assert!(matches!(items[3].item, PiHistoryItem::AgentText(ref text) if text == "done"));
    assert!(matches!(items[4].item, PiHistoryItem::ToolEnd { ref id, .. } if id == "tool-1"));
}

#[test]
fn entries_replay_preserves_messages_across_compaction() {
    let items = parse_entries(&json!({
        "entries": [
            {
                "type": "message",
                "timestamp": "2026-07-01T00:00:01Z",
                "message": { "role": "user", "content": "before compaction" }
            },
            {
                "type": "compaction",
                "timestamp": "2026-07-01T00:00:02Z",
                "summary": "older context summary"
            },
            {
                "type": "message",
                "timestamp": "2026-07-01T00:00:03Z",
                "message": { "role": "assistant", "content": "after compaction" }
            },
            {
                "type": "custom_message",
                "display": false,
                "content": "hidden extension bookkeeping"
            }
        ]
    }));

    assert_eq!(items.len(), 3);
    assert!(matches!(
        items[0].item,
        PiHistoryItem::UserText(ref text) if text == "before compaction"
    ));
    assert!(matches!(
        items[1].item,
        PiHistoryItem::CompactionSummary(ref text) if text == "older context summary"
    ));
    assert!(matches!(
        items[2].item,
        PiHistoryItem::AgentText(ref text) if text == "after compaction"
    ));
    assert_eq!(items[0].timestamp_ms, Some(1_782_864_001_000));
    assert_eq!(items[2].timestamp_ms, Some(1_782_864_003_000));
}

#[test]
fn entries_replay_selects_only_the_active_parent_chain() {
    let items = parse_entries(&json!({
        "entries": [
            {
                "type": "message",
                "id": "root",
                "parentId": null,
                "message": { "role": "user", "content": "root" }
            },
            {
                "type": "message",
                "id": "main",
                "parentId": "root",
                "message": { "role": "assistant", "content": "main sibling" }
            },
            {
                "type": "message",
                "id": "branch-user",
                "parentId": "root",
                "message": { "role": "user", "content": "branch" }
            },
            {
                "type": "model_change",
                "id": "branch-model",
                "parentId": "branch-user",
                "provider": "openai",
                "modelId": "gpt-test"
            },
            {
                "type": "message",
                "id": "branch-leaf",
                "parentId": "branch-model",
                "message": { "role": "assistant", "content": "selected leaf" }
            }
        ],
        "leafId": "branch-leaf"
    }));

    assert_eq!(items.len(), 3);
    assert!(matches!(
        items[0].item,
        PiHistoryItem::UserText(ref text) if text == "root"
    ));
    assert!(matches!(
        items[1].item,
        PiHistoryItem::UserText(ref text) if text == "branch"
    ));
    assert!(matches!(
        items[2].item,
        PiHistoryItem::AgentText(ref text) if text == "selected leaf"
    ));
}

#[test]
fn entry_replay_cache_applies_deltas_and_leaf_only_switches() {
    let mut cache = PiEntryReplayCache::default();
    cache.reset(
        "session-1",
        &json!({
            "entries": [
                {
                    "type": "message",
                    "id": "root",
                    "parentId": null,
                    "message": { "role": "user", "content": "root editor" }
                },
                {
                    "type": "message",
                    "id": "main",
                    "parentId": "root",
                    "message": { "role": "assistant", "content": "main" }
                }
            ],
            "leafId": "main"
        }),
    );
    assert!(cache.matches_session("session-1"));
    assert_eq!(cache.since_id(), Some("main"));
    assert_eq!(cache.editor_text("root").as_deref(), Some("root editor"));

    cache.append(&json!({
        "entries": [
            {
                "type": "message",
                "id": "main",
                "parentId": "root",
                "message": { "role": "assistant", "content": "duplicate" }
            },
            {
                "type": "message",
                "id": "branch-user",
                "parentId": "root",
                "message": { "role": "user", "content": "branch" }
            },
            {
                "type": "message",
                "id": "branch-leaf",
                "parentId": "branch-user",
                "message": { "role": "assistant", "content": "branch leaf" }
            }
        ],
        "leafId": "branch-leaf"
    }));
    assert_eq!(
        cache.entry_count(),
        4,
        "duplicate ids must not grow the cache"
    );
    assert_eq!(cache.since_id(), Some("branch-leaf"));
    let branch = cache.replay_entries();
    assert_eq!(branch.len(), 3);
    assert!(matches!(
        branch[2].item,
        PiHistoryItem::AgentText(ref text) if text == "branch leaf"
    ));

    cache.append(&json!({ "entries": [], "leafId": "main" }));
    let main = cache.replay_entries();
    assert_eq!(main.len(), 2);
    assert!(matches!(
        main[1].item,
        PiHistoryItem::AgentText(ref text) if text == "main"
    ));
}

#[test]
fn entry_replay_cache_respects_explicit_empty_leaf() {
    let items = parse_entries(&json!({
        "entries": [{
            "type": "message",
            "id": "orphan",
            "parentId": null,
            "message": { "role": "user", "content": "not active" }
        }],
        "leafId": null
    }));
    assert!(items.is_empty());
}

#[test]
fn scans_pi_session_metadata_with_native_selector_fields() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("sessions/project");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(
        project.join("session.jsonl"),
        concat!(
            "{\"type\":\"session\",\"id\":\"session-1\",\"timestamp\":\"2026-07-01T00:00:00.000Z\",\"cwd\":\"/repo\"}\n",
            "{\"type\":\"message\",\"id\":\"1\",\"parentId\":null,\"timestamp\":\"2026-07-01T00:00:01.000Z\",\"message\":{\"role\":\"user\",\"content\":\"hello\"}}\n",
            "{\"type\":\"model_change\",\"id\":\"2\",\"parentId\":\"1\",\"timestamp\":\"2026-07-01T00:00:02.000Z\",\"provider\":\"openai\",\"modelId\":\"gpt-test\"}\n",
            "{\"type\":\"message\",\"id\":\"3\",\"parentId\":\"2\",\"timestamp\":\"2026-07-01T00:00:03.000Z\",\"message\":{\"role\":\"assistant\",\"provider\":\"openai\",\"model\":\"gpt-test\",\"usage\":{\"totalTokens\":1200,\"cost\":{\"total\":0.42}}}}\n",
            "{\"type\":\"session_info\",\"id\":\"4\",\"parentId\":\"3\",\"timestamp\":\"2026-07-01T00:00:04.000Z\",\"name\":\"Named session\"}\n"
        ),
    )
    .unwrap();
    std::fs::write(project.join("invalid.jsonl"), "not json\n").unwrap();

    let sessions = scan_local_sessions(&root.path().join("sessions"));
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, "session-1");
    assert_eq!(sessions[0].cwd, "/repo");
    assert_eq!(sessions[0].name.as_deref(), Some("Named session"));
    assert_eq!(sessions[0].message_count, 2);
    assert_eq!(sessions[0].first_message, "hello");
    assert_eq!(sessions[0].model_id.as_deref(), Some("openai::gpt-test"));
    assert_eq!(sessions[0].total_tokens, Some(1200));
    assert_eq!(sessions[0].total_cost, Some(0.42));
    // Entry-level ISO becomes modified_at when message.timestamp is absent.
    assert_eq!(sessions[0].created_at, "2026-07-01T00:00:00.000Z");
    assert_eq!(sessions[0].modified_at, "2026-07-01T00:00:03.000Z");
}

#[test]
fn session_modified_at_prefers_message_timestamp_ms_as_rfc3339() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("sessions/project");
    std::fs::create_dir_all(&project).unwrap();
    // 2026-07-01T00:00:02.000Z == 1782864002000 ms
    std::fs::write(
        project.join("session.jsonl"),
        concat!(
            "{\"type\":\"session\",\"id\":\"session-ms\",\"timestamp\":\"2026-07-01T00:00:00.000Z\",\"cwd\":\"/repo\"}\n",
            "{\"type\":\"message\",\"id\":\"1\",\"parentId\":null,\"timestamp\":\"2026-07-01T00:00:01.000Z\",\"message\":{\"role\":\"user\",\"content\":\"hello\",\"timestamp\":1782864002000}}\n"
        ),
    )
    .unwrap();

    let sessions = scan_local_sessions(&root.path().join("sessions"));
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].modified_at, "2026-07-01T00:00:02.000Z");
    // Must be parseable RFC3339, never a bare millis digit string.
    assert!(
        sessions[0]
            .modified_at
            .parse::<chrono::DateTime<chrono::Utc>>()
            .is_ok()
    );
    assert!(!sessions[0].modified_at.chars().all(|c| c.is_ascii_digit()));
}

#[test]
fn scans_custom_session_directory_without_default_project_nesting() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("session.jsonl"),
        concat!(
            "{\"type\":\"session\",\"id\":\"custom-session\",\"timestamp\":\"2026-07-01T00:00:00.000Z\",\"cwd\":\"/repo\"}\n",
            "{\"type\":\"message\",\"id\":\"1\",\"parentId\":null,\"timestamp\":\"2026-07-01T00:00:01.000Z\",\"message\":{\"role\":\"user\",\"content\":\"hello\"}}\n"
        ),
    )
    .unwrap();

    let sessions = scan_local_sessions(root.path());
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, "custom-session");
}

#[test]
fn scans_only_current_cwd_from_default_session_store() {
    let root = tempfile::tempdir().unwrap();
    let sessions = root.path().join("sessions");
    let current_cwd = Path::new("/workspace/current");
    let current_dir = sessions.join("--workspace-current--");
    let other_dir = sessions.join("--workspace-other--");
    std::fs::create_dir_all(&current_dir).unwrap();
    std::fs::create_dir_all(&other_dir).unwrap();
    std::fs::write(
        current_dir.join("current.jsonl"),
        "{\"type\":\"session\",\"id\":\"current\",\"timestamp\":\"2026-07-01T00:00:00.000Z\",\"cwd\":\"/workspace/current\"}\n",
    )
    .unwrap();
    std::fs::write(
        other_dir.join("other.jsonl"),
        "{\"type\":\"session\",\"id\":\"other\",\"timestamp\":\"2026-07-01T00:00:00.000Z\",\"cwd\":\"/workspace/other\"}\n",
    )
    .unwrap();

    let current = scan_local_sessions_for_cwd(&sessions, current_cwd);
    assert_eq!(current.len(), 1);
    assert_eq!(current[0].id, "current");
}

#[test]
fn parses_cancelled_session_switch_without_state_mutation_signal() {
    assert_eq!(
        parse_session_switch(&json!({ "cancelled": true })),
        PiSessionSwitch { cancelled: true }
    );
    assert_eq!(
        parse_session_switch(&json!({ "cancelled": false })),
        PiSessionSwitch { cancelled: false }
    );
}

#[test]
fn assistant_errors_are_preserved_without_content() {
    let items = parse_messages(&json!({
        "messages": [{ "role": "assistant", "errorMessage": "request failed" }]
    }));
    assert!(
        matches!(items.as_slice(), [PiReplayEntry { item: PiHistoryItem::AgentText(text), .. }] if text == "**Pi error:** request failed")
    );
}

#[test]
fn delta_parser_ignores_toolcall_stream_fragments() {
    assert_eq!(
        extract_delta(&json!({
            "assistantMessageEvent": { "type": "toolcall_delta", "delta": "{\\\"path\\\":" }
        })),
        (String::new(), String::new())
    );
}

#[test]
fn parse_model_keeps_provider_and_api_separate_with_cost() {
    let model = parse_model(&json!({
        "id": "claude-haiku-4-5",
        "name": "Claude Haiku 4.5",
        "provider": "anthropic",
        "api": "anthropic-messages",
        "baseUrl": "https://api.anthropic.com",
        "contextWindow": 200000,
        "maxTokens": 64000,
        "reasoning": true,
        "input": ["text", "image"],
        "cost": { "input": 1.0, "output": 5.0, "cacheRead": 0.1, "cacheWrite": 1.25 }
    }))
    .expect("model");
    assert_eq!(model.provider, "anthropic");
    assert_eq!(model.id, "claude-haiku-4-5");
    assert_eq!(model.label, "Claude Haiku 4.5");
    assert_eq!(model.api.as_deref(), Some("anthropic-messages"));
    assert_eq!(model.base_url.as_deref(), Some("https://api.anthropic.com"));
    assert_eq!(model.context_window, Some(200_000));
    assert_eq!(model.max_tokens, Some(64_000));
    assert!(model.reasoning);
    assert!(model.accepts_images);
    assert_eq!(model.input, vec!["text".to_string(), "image".to_string()]);
    assert_eq!(model.cost_input, Some(1.0));
    assert_eq!(model.cost_output, Some(5.0));
    assert_eq!(model.cost_cache_read, Some(0.1));
    assert_eq!(model.cost_cache_write, Some(1.25));
}

#[test]
fn model_uses_dynamic_thinking_level_mappings() {
    let model = parse_model(&json!({
        "id": "reasoning-model",
        "provider": "demo",
        "reasoning": true,
        "thinkingLevelMap": {
            "off": null,
            "minimal": null,
            "low": null,
            "medium": null,
            "high": null,
            "xhigh": null,
            "max": null,
            "balanced": { "reasoning_effort": "medium" },
            "deep_mode": { "reasoning_effort": "max" }
        }
    }))
    .expect("model");

    assert!(
        model
            .thinking_levels
            .iter()
            .any(|level| level == "balanced")
    );
    assert!(
        model
            .thinking_levels
            .iter()
            .any(|level| level == "deep_mode")
    );
    assert_eq!(model.acp_effort_for_pi_level("deep_mode"), Some("max"));
    assert_eq!(model.pi_level_for_acp_effort("medium"), Some("balanced"));
    assert_eq!(model.pi_level_for_acp_effort("max"), Some("deep_mode"));
}

#[test]
fn parse_state_reads_streaming_and_compacting_flags() {
    let state = parse_state(&json!({
        "sessionId": "s1",
        "isStreaming": true,
        "isCompacting": true,
        "thinkingLevel": "high",
    }));
    assert_eq!(state.session_id, "s1");
    assert!(state.is_streaming);
    assert!(state.is_compacting);
    assert_eq!(state.thinking_level, "high");

    let idle = parse_state(&json!({ "sessionId": "s2" }));
    assert!(!idle.is_streaming);
    assert!(!idle.is_compacting);
}

#[test]
fn parse_commands_reads_argument_completions_snapshot() {
    let commands = parse_commands(&json!({
        "commands": [{
            "name": "gapp",
            "description": "Manage Glimpse-APPs",
            "source": "extension",
            "argumentHint": "<list|open>",
            "argumentCompletions": [
                { "value": "list", "label": "List apps" },
                { "value": "open ", "label": "Open app by id", "description": "id" }
            ]
        }]
    }));
    assert_eq!(commands.len(), 1);
    let cmd = &commands[0];
    assert_eq!(cmd.name, "gapp");
    assert_eq!(cmd.argument_hint.as_deref(), Some("<list|open>"));
    assert_eq!(cmd.argument_completions.len(), 2);
    assert_eq!(cmd.argument_completions[0].value, "list");
    assert_eq!(cmd.argument_completions[0].label, "List apps");
    assert_eq!(cmd.argument_completions[1].value, "open ");
    assert_eq!(cmd.argument_completions[1].description, "id");
}
