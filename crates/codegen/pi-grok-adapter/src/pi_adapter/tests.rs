use super::*;

#[test]
fn utc_now_ms_is_positive() {
    assert!(utc_now_ms() > 0);
}

#[test]
fn recap_capability_requires_an_enabled_bridge_extension() {
    assert!(!env_flag_enabled(None));
    assert!(!env_flag_enabled(Some("false")));
    assert!(env_flag_enabled(Some("1")));
    assert!(env_flag_enabled(Some("on")));
}

#[test]
fn bridge_commands_require_registration_and_recap_is_not_reentrant() {
    let commands = vec![PiCommand {
        name: RECAP_COMMAND.into(),
        ..Default::default()
    }];
    assert!(bridge_command_is_registered(&commands, RECAP_COMMAND));
    assert!(!bridge_command_is_registered(&commands, BTW_COMMAND));

    let mut in_flight = false;
    assert!(reserve_recap_request(&mut in_flight));
    assert!(!reserve_recap_request(&mut in_flight));
    in_flight = false;
    assert!(reserve_recap_request(&mut in_flight));

    let mut reload_in_flight = false;
    assert!(reserve_reload_request(&mut reload_in_flight));
    assert!(!reserve_reload_request(&mut reload_in_flight));
    reload_in_flight = false;
    assert!(reserve_reload_request(&mut reload_in_flight));
}

#[test]
fn subagent_sequence_restarts_after_transport_state_reset() {
    let mut sequences = HashMap::new();
    assert!(accept_subagent_sequence(&mut sequences, "run-1", 5, false));
    assert!(!accept_subagent_sequence(&mut sequences, "run-1", 1, false));
    sequences.clear();
    assert!(accept_subagent_sequence(&mut sequences, "run-1", 1, false));
}

#[test]
fn subagent_child_cancel_routes_to_the_extension_run_id() {
    let routes = HashMap::from([("child-run".to_string(), "subagent-run".to_string())]);
    assert_eq!(
        subagent_cancel_target(&routes, "child-run").as_deref(),
        Some("subagent-run")
    );
    assert_eq!(subagent_cancel_target(&routes, "root-session"), None);
}

#[test]
fn appends_background_control_only_for_an_active_tool() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let control_path = directory.path().join("control.jsonl");
    let meta_path = directory.path().join("control.json");
    std::fs::write(&control_path, "").expect("control file");
    std::fs::write(
        &meta_path,
        json!({
            "controlPath": control_path,
            "activeToolCallIds": ["tool-1"],
        })
        .to_string(),
    )
    .expect("metadata file");

    append_bash_background_control(&meta_path, "tool-1").expect("append control event");
    assert_eq!(
        std::fs::read_to_string(&control_path).expect("read control file"),
        "{\"op\":\"background\",\"toolCallId\":\"tool-1\"}\n"
    );
    assert!(append_bash_background_control(&meta_path, "tool-2").is_err());
}

#[test]
fn appends_kill_control_only_for_a_running_task() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let control_path = directory.path().join("control.jsonl");
    let meta_path = directory.path().join("control.json");
    std::fs::write(&control_path, "").expect("control file");
    std::fs::write(
        &meta_path,
        json!({
            "controlPath": control_path,
            "activeToolCallIds": [],
            "runningTaskIds": ["bash-1"],
        })
        .to_string(),
    )
    .expect("metadata file");

    assert_eq!(
        append_bash_kill_control(&meta_path, "bash-1").expect("kill running task"),
        "killed"
    );
    assert_eq!(
        std::fs::read_to_string(&control_path).expect("read control file"),
        "{\"op\":\"kill\",\"taskId\":\"bash-1\"}\n"
    );
    assert_eq!(
        append_bash_kill_control(&meta_path, "bash-missing").expect("unknown task"),
        "not_found"
    );
    assert_eq!(
        std::fs::read_to_string(&control_path).expect("read control file after not_found"),
        "{\"op\":\"kill\",\"taskId\":\"bash-1\"}\n"
    );
}

#[test]
fn session_file_discovers_a_settings_configured_session_directory() {
    let fallback = Path::new("/home/user/.pi/agent/sessions");
    let state = PiState {
        session_file: Some("/data/pi-sessions/current.jsonl".to_string()),
        ..PiState::default()
    };
    assert_eq!(
        catalog_session_dir(&state, fallback),
        PathBuf::from("/data/pi-sessions")
    );

    let default_state = PiState {
        session_file: Some("/home/user/.pi/agent/sessions/project/current.jsonl".to_string()),
        ..PiState::default()
    };
    assert_eq!(catalog_session_dir(&default_state, fallback), fallback);
}

#[test]
fn model_catalog_includes_provider_and_detail_description() {
    let models = vec![PiModel {
        provider: "anthropic".into(),
        id: "claude-haiku-4-5".into(),
        label: "Claude Haiku 4.5".into(),
        context_window: Some(200_000),
        max_tokens: Some(64_000),
        api: Some("anthropic-messages".into()),
        base_url: Some("https://api.anthropic.com".into()),
        reasoning: true,
        accepts_images: true,
        input: vec!["text".into(), "image".into()],
        cost_input: Some(1.0),
        cost_output: Some(5.0),
        cost_cache_read: Some(0.1),
        cost_cache_write: Some(1.25),
        thinking_levels: vec!["off".into(), "low".into(), "medium".into(), "high".into()],
        thinking_level_efforts: std::collections::HashMap::from([
            ("off".into(), "none".into()),
            ("low".into(), "low".into()),
            ("medium".into(), "medium".into()),
            ("high".into(), "high".into()),
        ]),
    }];
    let (available, current) = build_model_catalog(&models, models.first(), "medium");
    assert!(current.is_some());
    let id = acp::ModelId::new("anthropic::claude-haiku-4-5");
    let info = available.get(&id).expect("catalog entry");
    assert_eq!(info.name, "Claude Haiku 4.5");
    let description = info.description.as_deref().unwrap_or("");
    assert!(
        !description.contains("[anthropic]"),
        "provider stays on left: {description}"
    );
    assert!(description.contains("ctx 200k"), "{description}");
    assert!(description.contains("out 64k"), "{description}");
    assert!(description.contains("api anth"), "{description}");
    assert!(description.contains("in txt+img"), "{description}");
    assert!(description.contains("⚡"), "{description}");
    assert!(description.contains("$1 / $5"), "{description}");
    let meta = info.meta.as_ref().expect("meta");
    assert_eq!(
        meta.get("provider").and_then(|v| v.as_str()),
        Some("anthropic")
    );
    assert_eq!(
        meta.get("modelId").and_then(|v| v.as_str()),
        Some("claude-haiku-4-5")
    );
    assert_eq!(
        meta.get("api").and_then(|v| v.as_str()),
        Some("anthropic-messages")
    );
    assert_eq!(
        meta.get("totalContextTokens").and_then(|v| v.as_u64()),
        Some(200_000)
    );
    assert_eq!(meta.get("maxTokens").and_then(|v| v.as_u64()), Some(64_000));
}

#[test]
fn model_catalog_uses_runtime_thinking_level_ids_and_mappings() {
    let models = vec![PiModel {
        provider: "demo".into(),
        id: "reasoning-model".into(),
        label: "Reasoning Model".into(),
        reasoning: true,
        thinking_levels: vec!["balanced".into(), "deep_mode".into()],
        thinking_level_efforts: std::collections::HashMap::from([
            ("balanced".into(), "medium".into()),
            ("deep_mode".into(), "max".into()),
        ]),
        ..PiModel::default()
    }];

    let (available, _) = build_model_catalog(&models, models.first(), "deep_mode");
    let info = available
        .get(&acp::ModelId::new("demo::reasoning-model"))
        .expect("catalog entry");
    let meta = info.meta.as_ref().expect("meta");
    assert_eq!(
        meta.get("reasoningEffort").and_then(Value::as_str),
        Some("max")
    );
    assert_eq!(
        meta.get("reasoningEfforts"),
        Some(&json!([
            { "id": "balanced", "value": "medium", "label": "Balanced" },
            { "id": "deep_mode", "value": "max", "label": "Deep Mode" }
        ]))
    );
}

#[test]
fn command_catalog_is_pi_owned_and_deduplicated() {
    let commands = vec![
        PiCommand {
            name: "/review".into(),
            description: "Review changes".into(),
            source: "extension".into(),
            ..Default::default()
        },
        PiCommand {
            name: "REVIEW".into(),
            description: "Duplicate".into(),
            source: "prompt".into(),
            ..Default::default()
        },
        PiCommand {
            name: "brief".into(),
            description: String::new(),
            source: "skill".into(),
            ..Default::default()
        },
        PiCommand {
            name: NAVIGATE_TREE_COMMAND.into(),
            description: "internal".into(),
            source: "extension".into(),
            ..Default::default()
        },
        PiCommand {
            name: LABEL_TREE_COMMAND.into(),
            description: "internal".into(),
            source: "extension".into(),
            ..Default::default()
        },
        PiCommand {
            name: RELOAD_COMMAND.into(),
            description: "internal".into(),
            source: "extension".into(),
            ..Default::default()
        },
    ];
    let serialized = serde_json::to_value(command_catalog(&commands, false)).unwrap();
    let text = serialized.to_string();
    assert_eq!(text.matches("review").count(), 1);
    assert!(text.contains("Review changes"));
    assert!(text.contains("brief"));
    assert!(text.contains("Pi skill command"));
    assert!(text.contains("piCommandSource"));
    assert!(text.contains("extension"));
    assert!(!text.contains(NAVIGATE_TREE_COMMAND));
    assert!(!text.contains(LABEL_TREE_COMMAND));
    assert!(!text.contains(RELOAD_COMMAND));
}

#[test]
fn command_catalog_carries_pi_argument_completions_in_meta() {
    let commands = vec![PiCommand {
        name: "gapp".into(),
        description: "Manage Glimpse-APPs".into(),
        source: "extension".into(),
        argument_hint: Some("<list|open|...>".into()),
        argument_completions: vec![crate::model::PiArgumentCompletion {
            value: "list".into(),
            label: "List apps".into(),
            description: String::new(),
        }],
    }];
    let catalog = command_catalog(&commands, false);
    // Workflows may inject extra slash entries when enabled; assert the gapp row.
    let cmd = catalog
        .iter()
        .find(|c| c.name == "gapp")
        .expect("gapp command present");
    match cmd.input.as_ref() {
        Some(acp::AvailableCommandInput::Unstructured(u)) => {
            assert_eq!(u.hint, "<list|open|...>");
        }
        other => panic!("expected unstructured input, got {other:?}"),
    }
    let comps = cmd
        .meta
        .as_ref()
        .and_then(|m| m.get("piArgumentCompletions"))
        .and_then(|v| v.as_array())
        .expect("piArgumentCompletions meta");
    assert_eq!(comps.len(), 1);
    assert_eq!(comps[0]["value"], "list");
    assert_eq!(comps[0]["label"], "List apps");
}

#[test]
fn pi_input_and_editor_prefer_native_freeform_annotations() {
    let result = json!({
        "answers": { "pi-question": ["Other"] },
        "annotations": { "pi-question": { "notes": "typed in Grok PromptWidget" } },
        "value": "fallback",
    });
    assert_eq!(
        extension_answer("input", &result).as_deref(),
        Some("typed in Grok PromptWidget")
    );
    assert_eq!(
        extension_answer("editor", &result).as_deref(),
        Some("typed in Grok PromptWidget")
    );
}

#[test]
fn pi_select_and_confirm_prefer_native_selected_option() {
    let result = json!({
        "answers": { "pi-question": ["Yes"] },
        "annotations": { "pi-question": { "notes": "ignored freeform" } },
        "value": "fallback",
    });
    assert_eq!(extension_answer("select", &result).as_deref(), Some("Yes"));
    assert_eq!(extension_answer("confirm", &result).as_deref(), Some("Yes"));
}

#[test]
fn normalize_ask_user_questions_maps_multi_select_and_header() {
    let args = json!({
        "questions": [{
            "header": "Auth",
            "question": "Which method?",
            "multi_select": true,
            "options": [
                { "label": "JWT (Recommended)", "description": "stateless" },
                { "label": "Session", "description": "cookie" }
            ]
        }]
    });
    let questions = normalize_ask_user_questions(Some(&args));
    assert_eq!(questions.len(), 1);
    assert_eq!(questions[0]["question"], "Auth: Which method?");
    assert_eq!(questions[0]["multiSelect"], true);
    assert_eq!(questions[0]["options"].as_array().unwrap().len(), 2);
}

#[test]
fn format_ask_user_tool_result_accepted_and_cancel() {
    let accepted = format_ask_user_tool_result(&json!({
        "outcome": "accepted",
        "answers": { "Which DB?": ["Postgres"] },
        "annotations": { "Which DB?": { "notes": "managed" } }
    }));
    assert_eq!(accepted["outcome"], "accepted");
    let message = accepted["message"].as_str().unwrap();
    assert!(message.contains("Which DB?"));
    assert!(message.contains("Postgres"));
    assert!(message.contains("user notes: managed"));

    let cancelled = format_ask_user_tool_result(&json!({ "outcome": "cancelled" }));
    assert_eq!(cancelled["outcome"], "cancelled");
    assert!(cancelled["message"].as_str().unwrap().contains("declined"));
}

#[test]
fn pi_extension_timeout_is_milliseconds_and_zero_means_no_timeout() {
    assert_eq!(
        extension_dialog_timeout(&json!({ "timeout": 2500 })),
        Some(Duration::from_millis(2500))
    );
    assert_eq!(extension_dialog_timeout(&json!({ "timeout": 0 })), None);
    assert_eq!(extension_dialog_timeout(&json!({})), None);
}

#[test]
fn extension_tool_call_ids_are_stable_and_namespaced() {
    assert_eq!(
        extension_tool_call_id(&json!("dialog-7")),
        "pi-extension-ui:dialog-7"
    );
    assert_eq!(extension_tool_call_id(&json!(17)), "pi-extension-ui:17");
}

#[test]
fn product_multi_select_envelope_uses_native_checkbox_answer_shape() {
    assert_eq!(
        extension_multi_select_title(
            "__pi_grok_multi_select_v1__:{\"title\":\"Built-in tools\",\"maxSelections\":3}"
        ),
        Some("Built-in tools".into())
    );
    assert_eq!(extension_multi_select_title("ordinary select"), None);
    assert_eq!(
        extension_multi_select_answer(&json!({
            "answers": { "pi-question": ["☐ read", "☑ bash"] }
        })),
        Some("[\"☐ read\",\"☑ bash\"]".into())
    );
}

#[test]
fn product_resource_picker_envelope_round_trips_selected_paths() {
    let picker = extension_resource_picker(
        "__pi_grok_resource_picker_v1__:{\"title\":\"Extensions\",\"resourceTypes\":[\"extensions\"],\"initialPaths\":[\"/tmp/a.ts\"]}",
    )
    .expect("valid resource-picker envelope");
    assert_eq!(picker["title"], "Extensions");
    assert!(extension_resource_picker("ordinary select").is_none());
    assert_eq!(
        extension_resource_picker_answer(&json!({
            "paths": ["/tmp/a.ts", "/tmp/b.ts"]
        })),
        Some("[\"/tmp/a.ts\",\"/tmp/b.ts\"]".into())
    );
}

#[test]
fn product_subagent_history_envelope_round_trips_selected_id() {
    let picker = extension_subagent_history(
        "__pi_grok_subagent_history_v1__:{\"title\":\"Subagent history\",\"entries\":[{\"id\":\"abc123\",\"description\":\"Inspect parser\",\"status\":\"COMPLETED\",\"type\":\"explore\",\"turnCount\":2,\"toolCallCount\":4,\"modelId\":\"provider/model\",\"background\":true}]}",
    )
    .expect("valid subagent-history envelope");
    assert_eq!(picker["entries"][0]["id"], "abc123");
    assert!(extension_subagent_history("ordinary select").is_none());
    assert_eq!(
        extension_subagent_history_answer(&json!({ "id": "abc123" })),
        Some("abc123".into())
    );
}

#[test]
fn normalizes_system_language_tags() {
    assert_eq!(normalize_language_tag("zh_CN.UTF-8"), Some("zh-CN".into()));
    assert_eq!(normalize_language_tag("\"en-US\""), Some("en-US".into()));
    assert_eq!(normalize_language_tag("C"), None);
    assert_eq!(normalize_language_tag("POSIX"), None);
}

#[test]
fn parses_first_macos_preferred_language() {
    assert_eq!(
        first_apple_language("(\n    \"zh-Hans-CN\",\n    \"en-CN\"\n)"),
        Some("zh-Hans-CN".into())
    );
    assert_eq!(first_apple_language("(\n)"), None);
}

#[test]
fn plan_sidecar_is_scoped_to_jsonl_session() {
    let state = PiState {
        session_id: "session-1".into(),
        session_file: Some("/tmp/pi/project/session.jsonl".into()),
        ..PiState::default()
    };
    let plan = plan_file_path(&state, Path::new("/tmp/pi"));
    assert_eq!(plan, PathBuf::from("/tmp/pi/project/plans/session.plan.md"));
    assert_eq!(
        plan_state_path(&plan),
        PathBuf::from("/tmp/pi/project/session.plan-mode.json")
    );
}

#[test]
fn completed_plan_gets_cursor_style_context_front_matter() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let session_file = directory.path().join("session.jsonl");
    std::fs::write(
        &session_file,
        "{\"type\":\"session\",\"id\":\"session-1\",\"timestamp\":\"2026-08-27T02:13:07.584Z\",\"cwd\":\"/repo\"}\n",
    )
    .unwrap();
    let state = PiState {
        session_id: "session-1".into(),
        session_file: Some(session_file.display().to_string()),
        session_name: Some("Fallback session title".into()),
        model: Some(PiModel {
            provider: "openai".into(),
            id: "gpt-test".into(),
            ..PiModel::default()
        }),
        ..PiState::default()
    };
    let plan = plan_file_path(&state, directory.path());
    std::fs::create_dir_all(plan.parent().unwrap()).unwrap();
    std::fs::write(
        &plan,
        "# Shipping Plan\n\n## Goal\nMake startup deterministic and fast.\n\n- Keep behavior stable.\n",
    )
    .unwrap();

    normalize_plan_document(&plan, &state).unwrap();
    let normalized = std::fs::read_to_string(&plan).unwrap();
    assert!(normalized.starts_with("---\nname: \"Shipping Plan\"\n"));
    assert!(normalized.contains("overview: \"Make startup deterministic and fast.\"\n"));
    assert!(normalized.contains("tags:\n  - plan\n"));
    assert!(normalized.contains("sessionId: \"session-1\"\n"));
    assert!(normalized.contains("sessionName: \"Fallback session title\"\n"));
    assert!(normalized.contains("createdAt: \"2026-08-27T02:13:07.584Z\"\n"));
    assert!(normalized.contains("cwd: \"/repo\"\n"));
    assert!(normalized.contains("model: \"openai::gpt-test\"\n"));
    assert!(normalized.contains("isProject: true\n---\n\n# Shipping Plan\n"));

    normalize_plan_document(&plan, &state).unwrap();
    assert_eq!(std::fs::read_to_string(&plan).unwrap(), normalized);
}

#[test]
fn plan_tracker_persists_and_restores_active_state() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let plan_file = directory.path().join("session.plan.md");
    let mut tracker = crate::plan_mode::PiPlanTracker::with_plan_file(plan_file.clone());
    tracker.enter_pending();
    tracker.build_reminder_for_prompt();
    atomic_write(
        &plan_state_path(&plan_file),
        &serde_json::to_vec(&tracker.snapshot()).expect("serialize snapshot"),
    )
    .expect("persist snapshot");

    let restored = load_plan_tracker(&plan_file).expect("restore snapshot");
    assert!(restored.is_active());
    assert_eq!(restored.plan_file_path(), plan_file.as_path());
}

#[test]
fn compaction_events_project_to_native_session_updates() {
    let start = compaction_start_notification(
        "session-1",
        &json!({ "reason": "threshold" }),
        85_000,
        100_000,
    );
    assert_eq!(start["update"]["sessionUpdate"], "auto_compact_started");
    assert_eq!(start["update"]["percentage"], 85);

    let success = compaction_end_notification(
        "session-1",
        &json!({
            "result": {
                "tokensBefore": 100_000,
                "estimatedTokensAfter": 20_000,
                "summary": "Retained recent work"
            }
        }),
        Some(500),
    )
    .expect("success projection");
    assert_eq!(success["sessionId"], "session-1");
    assert_eq!(success["update"]["sessionUpdate"], "auto_compact_completed");
    assert_eq!(success["update"]["tokens_before"], 100_000);
    assert_eq!(success["update"]["tokens_after"], 20_000);
    assert_eq!(success["update"]["elapsed_ms"], 500);

    let failure = compaction_end_notification(
        "session-1",
        &json!({ "errorMessage": "compaction failed" }),
        None,
    )
    .expect("failure projection");
    assert_eq!(failure["update"]["sessionUpdate"], "auto_compact_failed");

    let cancelled = compaction_end_notification(
        "session-1",
        &json!({ "aborted": true, "reason": "user" }),
        None,
    )
    .expect("cancelled projection");
    assert_eq!(
        cancelled["update"]["sessionUpdate"],
        "auto_compact_cancelled"
    );
}
