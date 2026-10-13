use super::*;
#[test]
fn worktree_auto_gc_partial_object_and_round_trip() {
    let json = r#"{"worktree_auto_gc":{"enabled":false}}"#;
    let s: RemoteSettings = serde_json::from_str(json).unwrap();
    let agc = s.worktree_auto_gc.as_ref().expect("present");
    assert_eq!(agc.enabled, Some(false));
    assert_eq!(agc.max_age_secs, None);
    assert_eq!(agc.min_interval_secs, None);
    assert_eq!(agc.dry_run, None);
    assert_eq!(agc.include_orphan_snapshots, None);
    assert_eq!(agc.max_age_by_kind, None);
    let full = r#"{
        "worktree_auto_gc": {
            "enabled": true,
            "max_age_secs": 604800,
            "min_interval_secs": 21600,
            "dry_run": true,
            "include_orphan_snapshots": false,
            "max_age_by_kind": {
                "session": 604800,
                "subagent": 86400,
                "manual": "never"
            }
        }
    }"#;
    let s: RemoteSettings = serde_json::from_str(full).unwrap();
    let agc = s.worktree_auto_gc.clone().unwrap();
    let mut kind_map = std::collections::BTreeMap::new();
    kind_map.insert("session".into(), WorktreeKindMaxAge::Secs(604800));
    kind_map.insert("subagent".into(), WorktreeKindMaxAge::Secs(86400));
    kind_map.insert("manual".into(), WorktreeKindMaxAge::Never);
    assert_eq!(
        agc,
        WorktreeAutoGcSettings {
            enabled: Some(true),
            max_age_secs: Some(604800),
            min_interval_secs: Some(21600),
            dry_run: Some(true),
            include_orphan_snapshots: Some(false),
            max_age_by_kind: Some(kind_map),
            include_rebuild: None,
            rebuild_min_interval_secs: None,
        }
    );
    let out = serde_json::to_string(&s).unwrap();
    let s2: RemoteSettings = serde_json::from_str(&out).unwrap();
    assert_eq!(s2.worktree_auto_gc, s.worktree_auto_gc);
    let absent: RemoteSettings = serde_json::from_str("{}").unwrap();
    assert_eq!(absent.worktree_auto_gc, None);
    let extra = r#"{"worktree_auto_gc":{"enabled":true,"future_knob":1}}"#;
    let s: RemoteSettings = serde_json::from_str(extra).unwrap();
    assert_eq!(s.worktree_auto_gc.unwrap().enabled, Some(true));
    let partial_bad = r#"{"worktree_auto_gc":{"enabled":false,"max_age_secs":"nope"}}"#;
    let s: RemoteSettings = serde_json::from_str(partial_bad).unwrap();
    let agc = s.worktree_auto_gc.as_ref().expect("object still present");
    assert_eq!(agc.enabled, Some(false));
    assert_eq!(agc.max_age_secs, None);
    let kind_partial = r#"{
        "worktree_auto_gc": {
            "max_age_by_kind": {
                "subagent": 86400,
                "session": {"nested": true},
                "manual": "never"
            }
        }
    }"#;
    let s: RemoteSettings = serde_json::from_str(kind_partial).unwrap();
    let map = s
        .worktree_auto_gc
        .as_ref()
        .and_then(|a| a.max_age_by_kind.as_ref())
        .expect("map present");
    assert_eq!(map.get("subagent"), Some(&WorktreeKindMaxAge::Secs(86400)));
    assert_eq!(map.get("manual"), Some(&WorktreeKindMaxAge::Never));
    assert!(!map.contains_key("session"));
    let null_never = r#"{"worktree_auto_gc":{"max_age_by_kind":{"manual":null,"pool":172800}}}"#;
    let s: RemoteSettings = serde_json::from_str(null_never).unwrap();
    let map = s.worktree_auto_gc.unwrap().max_age_by_kind.unwrap();
    assert_eq!(map.get("manual"), Some(&WorktreeKindMaxAge::Never));
    assert_eq!(map.get("pool"), Some(&WorktreeKindMaxAge::Secs(172800)));
    assert_eq!(
        serde_json::to_value(&WorktreeKindMaxAge::Never).unwrap(),
        serde_json::Value::String("never".into())
    );
    let nested_bad = r#"{"leader_mode":true,"worktree_auto_gc":"not-an-object"}"#;
    let s: RemoteSettings = serde_json::from_str(nested_bad).unwrap();
    assert_eq!(s.leader_mode, Some(true));
    assert_eq!(s.worktree_auto_gc, None);
}
#[test]
fn remote_settings_turn_transient_retry_malformed_value_does_not_poison_siblings() {
    let s: RemoteSettings =
        serde_json::from_str(r#"{"turn_transient_retry": "false", "leader_mode": true}"#).unwrap();
    assert_eq!(
        s.turn_transient_retry, None,
        "malformed value drops to None"
    );
    assert_eq!(s.leader_mode, Some(true), "siblings survive");
}
#[test]
fn tolerant_bool_swallows_array_and_object_shapes() {
    for bad in [
        r#"{"turn_transient_retry": [1,2], "leader_mode": true}"#,
        r#"{"turn_transient_retry": {"a": 1}, "leader_mode": true}"#,
    ] {
        let s: RemoteSettings = serde_json::from_str(bad).unwrap();
        assert_eq!(s.turn_transient_retry, None);
        assert_eq!(s.leader_mode, Some(true), "siblings survive: {bad}");
    }
}
#[test]
fn remote_settings_turn_transient_retry_round_trip_and_default_absent() {
    let s: RemoteSettings = serde_json::from_str(r#"{"turn_transient_retry": false}"#).unwrap();
    assert_eq!(s.turn_transient_retry, Some(false));
    let round_trip: RemoteSettings =
        serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
    assert_eq!(round_trip.turn_transient_retry, Some(false));
    let absent: RemoteSettings = serde_json::from_str("{}").unwrap();
    assert_eq!(absent.turn_transient_retry, None);
}
#[test]
fn remote_settings_uncharged_401_park_round_trip_and_default_absent() {
    let s: RemoteSettings = serde_json::from_str(r#"{"uncharged_401_park": false}"#).unwrap();
    assert_eq!(s.uncharged_401_park, Some(false));
    let round_trip: RemoteSettings =
        serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
    assert_eq!(round_trip.uncharged_401_park, Some(false));
    let absent: RemoteSettings = serde_json::from_str("{}").unwrap();
    assert_eq!(absent.uncharged_401_park, None);
}
#[test]
fn remote_settings_uncharged_401_park_malformed_value_does_not_poison_siblings() {
    for bad in [
        r#"{"uncharged_401_park": "false", "leader_mode": true}"#,
        r#"{"uncharged_401_park": [1], "leader_mode": true}"#,
    ] {
        let s: RemoteSettings = serde_json::from_str(bad).unwrap();
        assert_eq!(s.uncharged_401_park, None, "malformed value drops: {bad}");
        assert_eq!(s.leader_mode, Some(true), "siblings survive: {bad}");
    }
}
#[test]
fn remote_settings_vendor_sessions_round_trip_and_default_absent() {
    let session_flags = |settings: &RemoteSettings| {
        (
            settings.cursor_sessions_enabled,
            settings.claude_sessions_enabled,
            settings.codex_sessions_enabled,
        )
    };
    let json = r#"{
        "cursor_sessions_enabled": true,
        "claude_sessions_enabled": false,
        "codex_sessions_enabled": true
    }"#;
    let settings: RemoteSettings = serde_json::from_str(json).unwrap();
    assert_eq!(
        session_flags(&settings),
        (Some(true), Some(false), Some(true))
    );
    let serialized = serde_json::to_string(&settings).unwrap();
    let round_trip: RemoteSettings = serde_json::from_str(&serialized).unwrap();
    assert_eq!(
        session_flags(&round_trip),
        (Some(true), Some(false), Some(true))
    );
    let absent: RemoteSettings = serde_json::from_str("{}").unwrap();
    assert_eq!(session_flags(&absent), (None, None, None));
}
#[test]
fn remote_settings_image_description_model_round_trip() {
    let json = r#"{"image_description_model": "grok-4.6"}"#;
    let s: RemoteSettings = serde_json::from_str(json).unwrap();
    assert_eq!(s.image_description_model.as_deref(), Some("grok-4.6"));
    let out = serde_json::to_string(&s).unwrap();
    let s2: RemoteSettings = serde_json::from_str(&out).unwrap();
    assert_eq!(s2.image_description_model, s.image_description_model);
}
#[test]
fn remote_settings_prompt_suggestion_model_round_trip() {
    let json = r#"{"prompt_suggestion_model": "grok-4.6"}"#;
    let s: RemoteSettings = serde_json::from_str(json).unwrap();
    assert_eq!(s.prompt_suggestion_model.as_deref(), Some("grok-4.6"));
    let out = serde_json::to_string(&s).unwrap();
    let s2: RemoteSettings = serde_json::from_str(&out).unwrap();
    assert_eq!(s2.prompt_suggestion_model, s.prompt_suggestion_model);
    let s3: RemoteSettings = serde_json::from_str("{}").unwrap();
    assert_eq!(s3.prompt_suggestion_model, None);
}
#[test]
fn remote_settings_announcements_absent() {
    let json = r#"{}"#;
    let settings: RemoteSettings = serde_json::from_str(json).unwrap();
    assert_eq!(settings.announcements, None);
}
#[test]
fn remote_settings_announcements_populated() {
    let json = r#"{"announcements": [{"id": "a", "message": "m"}]}"#;
    let settings: RemoteSettings = serde_json::from_str(json).unwrap();
    assert_eq!(
        settings.announcements,
        Some(vec![RemoteAnnouncement {
            id: Some("a".to_string()),
            message: Some("m".to_string()),
            severity: None,
            title: None,
            cta: None,
            updated_at: None,
            expires_at: None,
            dismissible: None,
            persistent: None,
        }])
    );
}
#[test]
fn remote_settings_announcements_one_bad_item_does_not_poison() {
    let json = r#"{
        "announcements": [
            {"id": "good", "message": "ok"},
            {"id": 999, "message": "bad-id-type"}
        ]
    }"#;
    let settings: RemoteSettings = serde_json::from_str(json).unwrap();
    assert_eq!(
        settings.announcements,
        Some(vec![RemoteAnnouncement {
            id: Some("good".to_string()),
            message: Some("ok".to_string()),
            severity: None,
            title: None,
            cta: None,
            updated_at: None,
            expires_at: None,
            dismissible: None,
            persistent: None,
        }])
    );
}
#[test]
fn remote_settings_consent_gate_round_trip() {
    let json = r#"{
        "consent_gate": {
            "id": "tos-2026-08",
            "version": 3,
            "title": "Updated terms",
            "body": "Review our [Terms of Service](https://x.ai/legal/tos) before continuing.",
            "accept_label": "Accept and continue"
        }
    }"#;
    let settings: RemoteSettings = serde_json::from_str(json).unwrap();
    let gate = settings.consent_gate.expect("gate present");
    assert_eq!(gate.id, "tos-2026-08");
    assert_eq!(gate.version, Some(3));
    assert!(
        gate.body
            .as_deref()
            .is_some_and(|b| b.contains("https://x.ai/legal/tos"))
    );
}
/// A poisoned response would leave `zdr_access_enabled` false and hard-block ZDR users.
#[test]
fn remote_settings_malformed_consent_gate_does_not_poison() {
    for gate in [
        r#"{"id": "tos", "version": "not-a-number"}"#,
        r#"{"version": 3, "body": "Review our terms."}"#,
    ] {
        let json = format!(r#"{{"consent_gate": {gate}, "tips": ["still parsed"]}}"#);
        let settings: RemoteSettings = serde_json::from_str(&json).unwrap();
        assert!(settings.consent_gate.is_none(), "{gate} must be dropped");
        assert_eq!(settings.tips, Some(vec!["still parsed".to_string()]));
    }
}
#[test]
fn remote_settings_goal_planner_model_round_trip() {
    let json = r#"{"goal_planner_model": {"model": "grok-4", "agent_type": "general-purpose"}}"#;
    let s: RemoteSettings = serde_json::from_str(json).unwrap();
    assert_eq!(
        s.goal_planner_model,
        Some(GoalRoleModel {
            model: "grok-4".to_string(),
            agent_type: "general-purpose".to_string(),
        })
    );
    let out = serde_json::to_string(&s).unwrap();
    let s2: RemoteSettings = serde_json::from_str(&out).unwrap();
    assert_eq!(s2.goal_planner_model, s.goal_planner_model);
}
#[test]
fn remote_settings_goal_skeptic_models_fully_valid_pool_round_trips() {
    let json = r#"{"goal_skeptic_models": [
        {"model": "grok-4", "agent_type": "general-purpose"},
        {"model": "grok-3", "agent_type": "cursor"}
    ]}"#;
    let s: RemoteSettings = serde_json::from_str(json).unwrap();
    assert_eq!(
        s.goal_skeptic_models,
        vec![
            GoalRoleModel {
                model: "grok-4".to_string(),
                agent_type: "general-purpose".to_string(),
            },
            GoalRoleModel {
                model: "grok-3".to_string(),
                agent_type: "cursor".to_string(),
            },
        ]
    );
    let out = serde_json::to_string(&s).unwrap();
    let s2: RemoteSettings = serde_json::from_str(&out).unwrap();
    assert_eq!(s2.goal_skeptic_models, s.goal_skeptic_models);
}
#[test]
fn remote_settings_goal_skeptic_models_one_bad_item_does_not_poison_pool() {
    let json = r#"{"goal_skeptic_models": [
        {"model": "grok-4", "agent_type": "general-purpose"},
        {"model": "grok-broken"},
        {"model": "grok-3", "agent_type": "cursor"}
    ]}"#;
    let s: RemoteSettings = serde_json::from_str(json).unwrap();
    assert_eq!(
        s.goal_skeptic_models,
        vec![
            GoalRoleModel {
                model: "grok-4".to_string(),
                agent_type: "general-purpose".to_string(),
            },
            GoalRoleModel {
                model: "grok-3".to_string(),
                agent_type: "cursor".to_string(),
            },
        ]
    );
}
#[test]
fn remote_settings_goal_skeptic_models_all_entries_bad_yields_empty() {
    let json = r#"{"goal_skeptic_models": [
        {"model": "only-model"},
        {"agent_type": "only-agent-type"},
        "scalar-not-an-object",
        42
    ]}"#;
    let s: RemoteSettings = serde_json::from_str(json).unwrap();
    assert!(s.goal_skeptic_models.is_empty());
}
#[test]
fn remote_settings_goal_skeptic_models_non_array_yields_empty() {
    for json in [
        r#"{"goal_skeptic_models": {"model": "x", "agent_type": "y"}}"#,
        r#"{"goal_skeptic_models": "not-an-array"}"#,
        r#"{"goal_skeptic_models": 7}"#,
    ] {
        let s: RemoteSettings = serde_json::from_str(json).unwrap();
        assert!(
            s.goal_skeptic_models.is_empty(),
            "non-array pool must yield empty for {json}"
        );
    }
}
#[test]
fn remote_settings_goal_skeptic_models_missing_model_entry_dropped() {
    let json = r#"{"goal_skeptic_models": [
        {"agent_type": "general-purpose"},
        {"model": "grok-3", "agent_type": "cursor"}
    ]}"#;
    let s: RemoteSettings = serde_json::from_str(json).unwrap();
    assert_eq!(
        s.goal_skeptic_models,
        vec![GoalRoleModel {
            model: "grok-3".to_string(),
            agent_type: "cursor".to_string(),
        }]
    );
}
#[test]
fn remote_settings_goal_skeptic_models_wrong_typed_scalar_dropped() {
    let json = r#"{"goal_skeptic_models": [
        {"model": 123, "agent_type": "general-purpose"},
        {"model": "grok-3", "agent_type": ["cursor"]},
        {"model": "grok-4", "agent_type": "general-purpose"}
    ]}"#;
    let s: RemoteSettings = serde_json::from_str(json).unwrap();
    assert_eq!(
        s.goal_skeptic_models,
        vec![GoalRoleModel {
            model: "grok-4".to_string(),
            agent_type: "general-purpose".to_string(),
        }]
    );
}
#[test]
fn remote_settings_goal_skeptic_models_extra_unknown_fields_kept() {
    let json = r#"{"goal_skeptic_models": [
        {"model": "grok-4", "agent_type": "general-purpose", "reasoning_effort": "high"}
    ]}"#;
    let s: RemoteSettings = serde_json::from_str(json).unwrap();
    assert_eq!(
        s.goal_skeptic_models,
        vec![GoalRoleModel {
            model: "grok-4".to_string(),
            agent_type: "general-purpose".to_string(),
        }]
    );
}
#[test]
fn remote_settings_goal_skeptic_models_survivor_order_preserved() {
    let json = r#"{"goal_skeptic_models": [
        {"model": "first", "agent_type": "general-purpose"},
        {"model": "bad"},
        {"model": "second", "agent_type": "cursor"},
        "garbage",
        {"model": "third", "agent_type": "general-purpose"}
    ]}"#;
    let s: RemoteSettings = serde_json::from_str(json).unwrap();
    let order: Vec<&str> = s
        .goal_skeptic_models
        .iter()
        .map(|m| m.model.as_str())
        .collect();
    assert_eq!(order, vec!["first", "second", "third"]);
}
#[test]
fn remote_settings_goal_planner_model_malformed_yields_none() {
    for json in [
        r#"{"goal_planner_model": {"model": "only-model"}}"#,
        r#"{"goal_planner_model": {"agent_type": "only-agent-type"}}"#,
        r#"{"goal_planner_model": {"model": 1, "agent_type": "x"}}"#,
        r#"{"goal_planner_model": "scalar"}"#,
        r#"{"goal_planner_model": null}"#,
    ] {
        let s: RemoteSettings = serde_json::from_str(json)
            .unwrap_or_else(|e| panic!("must not hard-error for {json}: {e}"));
        assert_eq!(s.goal_planner_model, None, "for {json}");
    }
}
#[test]
fn remote_settings_goal_role_models_malformed_pair_does_not_drop_other_fields() {
    let json = r#"{
        "goal_planner_model": {"model": "broken"},
        "goal_strategist_model": {"model": "grok-4.5", "agent_type": "cursor"},
        "default_model": "grok-4"
    }"#;
    let s: RemoteSettings = serde_json::from_str(json).unwrap();
    assert_eq!(s.goal_planner_model, None);
    assert_eq!(
        s.goal_strategist_model,
        Some(GoalRoleModel {
            model: "grok-4.5".to_string(),
            agent_type: "cursor".to_string(),
        })
    );
    assert_eq!(s.default_model.as_deref(), Some("grok-4"));
}
#[test]
fn remote_settings_goal_role_model_extra_unknown_fields_kept_single_pair() {
    let json = r#"{"goal_planner_model": {"model": "grok-4", "agent_type": "general-purpose", "future": true}}"#;
    let s: RemoteSettings = serde_json::from_str(json).unwrap();
    assert_eq!(
        s.goal_planner_model,
        Some(GoalRoleModel {
            model: "grok-4".to_string(),
            agent_type: "general-purpose".to_string(),
        })
    );
}
#[test]
fn remote_settings_inference_idle_timeout_present() {
    let json = r#"{"inference_idle_timeout_secs": 180}"#;
    let settings: RemoteSettings = serde_json::from_str(json).unwrap();
    assert_eq!(settings.inference_idle_timeout_secs, Some(180));
}
#[test]
fn remote_settings_initial_injection_deserialize_present() {
    let json = r#"{"memory_initial_injection_enabled": false, "memory_initial_injection_min_score": 0.66}"#;
    let settings: RemoteSettings = serde_json::from_str(json).unwrap();
    assert_eq!(settings.memory_initial_injection_enabled, Some(false));
    assert_eq!(settings.memory_initial_injection_min_score, Some(0.66));
}
#[test]
fn remote_settings_memory_v2_round_trips_and_defaults_absent() {
    for expected in [true, false] {
        let settings: RemoteSettings = serde_json::from_value(serde_json::json!({
            "memory_v2": {
                "enabled": expected,
                "capture_status_enabled": expected,
            },
        }))
        .unwrap();
        let memory_v2 = settings.memory_v2.as_ref().unwrap();
        assert_eq!(memory_v2.enabled, Some(expected));
        assert_eq!(memory_v2.capture_status_enabled, Some(expected));
        let round_trip: RemoteSettings =
            serde_json::from_value(serde_json::to_value(&settings).unwrap()).unwrap();
        let memory_v2 = round_trip.memory_v2.as_ref().unwrap();
        assert_eq!(memory_v2.enabled, Some(expected));
        assert_eq!(memory_v2.capture_status_enabled, Some(expected));
    }
    let absent: RemoteSettings = serde_json::from_str("{}").unwrap();
    assert_eq!(absent.memory_v2, None);
}
#[test]
fn remote_settings_initial_injection_deserialize_absent() {
    let json = r#"{"memory_enabled": true}"#;
    let settings: RemoteSettings = serde_json::from_str(json).unwrap();
    assert_eq!(settings.memory_initial_injection_enabled, None);
    assert_eq!(settings.memory_initial_injection_min_score, None);
}
#[test]
fn remote_settings_inference_idle_timeout_absent() {
    let json = r#"{"memory_enabled": true}"#;
    let settings: RemoteSettings = serde_json::from_str(json).unwrap();
    assert_eq!(settings.inference_idle_timeout_secs, None);
}
#[test]
fn remote_settings_unknown_fields_tolerated() {
    let json = r#"{
        "inference_idle_timeout_secs": 120,
        "future_remote_field": 42,
        "verification_staleness_enabled": true
    }"#;
    let settings: RemoteSettings = serde_json::from_str(json).unwrap();
    assert_eq!(settings.inference_idle_timeout_secs, Some(120));
}
#[test]
fn remote_settings_display_refresh_present_partial() {
    let json = r#"{"display_refresh": {"auto_cadence_enabled": true, "floor_ms": 7}}"#;
    let s: RemoteSettings = serde_json::from_str(json).unwrap();
    let dr = s.display_refresh.expect("display_refresh present");
    assert_eq!(dr.auto_cadence_enabled, Some(true));
    assert_eq!(dr.floor_ms, Some(7));
    assert_eq!(dr.probe_enabled, None);
    assert_eq!(dr.ceiling_ms, None);
    assert_eq!(dr.min_hz, None);
    assert_eq!(dr.max_hz, None);
}
#[test]
fn remote_settings_display_refresh_unknown_keys_preserved() {
    let json =
        r#"{"display_refresh": {"probe_enabled": true, "future_knob": 42, "floor_ms": "bad"}}"#;
    let s: RemoteSettings = serde_json::from_str(json).unwrap();
    let dr = s.display_refresh.expect("display_refresh present");
    assert_eq!(dr.probe_enabled, Some(true));
    assert_eq!(dr.floor_ms, None, "wrong-typed floor_ms ignored");
    assert_eq!(dr.extra.get("future_knob"), Some(&serde_json::json!(42)));
    let out = serde_json::to_value(&dr).unwrap();
    assert_eq!(out.get("future_knob"), Some(&serde_json::json!(42)));
    assert_eq!(out.get("probe_enabled"), Some(&serde_json::json!(true)));
}
#[test]
fn remote_settings_accept_request_encodings_unknown_tolerated_and_default_empty() {
    let s: RemoteSettings =
        serde_json::from_str(r#"{"accept_request_encodings": ["zstd", "br"]}"#).unwrap();
    assert_eq!(
        vec![RemoteRequestEncoding::Zstd, RemoteRequestEncoding::Unknown],
        s.accept_request_encodings
    );
    let absent: RemoteSettings = serde_json::from_str("{}").unwrap();
    assert!(absent.accept_request_encodings.is_empty());
}
type JemallocFields<'a> = (Option<bool>, Option<&'a [u64]>, Option<u64>);
fn jemalloc_fields(s: &RemoteSettings) -> JemallocFields<'_> {
    (
        s.jemalloc_heap_profile_enabled,
        s.jemalloc_heap_profile_thresholds_bytes.as_deref(),
        s.jemalloc_heap_profile_poll_interval_secs,
    )
}
fn parse_remote(json: &str) -> RemoteSettings {
    serde_json::from_str(json).unwrap_or_else(|e| panic!("parse failed for {json}: {e}"))
}
fn round_trip_remote(s: &RemoteSettings) -> RemoteSettings {
    let out = serde_json::to_string(s).unwrap();
    parse_remote(&out)
}
fn assert_jemalloc_round_trip(json: &str, expected: JemallocFields<'_>) {
    let s = parse_remote(json);
    assert_eq!(jemalloc_fields(&s), expected);
    assert_eq!(jemalloc_fields(&round_trip_remote(&s)), expected);
}
fn assert_remote_parse_err(json: &str) {
    assert!(
        serde_json::from_str::<RemoteSettings>(json).is_err(),
        "expected parse error for {json}"
    );
}
#[test]
fn remote_settings_jemalloc_heap_profile_thresholds_populated_and_empty_round_trip() {
    assert_jemalloc_round_trip(
        r#"{
            "jemalloc_heap_profile_thresholds_bytes": [2147483648, 5368709120, 10737418240]
        }"#,
        (
            None,
            Some(&[2_147_483_648, 5_368_709_120, 10_737_418_240]),
            None,
        ),
    );
    assert_jemalloc_round_trip(
        r#"{"jemalloc_heap_profile_thresholds_bytes": []}"#,
        (None, Some(&[]), None),
    );
    assert_jemalloc_round_trip(
        r#"{
            "jemalloc_heap_profile_enabled": true,
            "jemalloc_heap_profile_thresholds_bytes": []
        }"#,
        (Some(true), Some(&[]), None),
    );
}
#[test]
fn remote_settings_jemalloc_heap_profile_malformed_fails_whole_parse() {
    for json in [
        r#"{"jemalloc_heap_profile_enabled": "yes"}"#,
        r#"{"jemalloc_heap_profile_enabled": 1}"#,
        r#"{"jemalloc_heap_profile_enabled": []}"#,
        r#"{"jemalloc_heap_profile_enabled": {}}"#,
        r#"{"jemalloc_heap_profile_thresholds_bytes": "2G"}"#,
        r#"{"jemalloc_heap_profile_thresholds_bytes": {"bytes": 1}}"#,
        r#"{"jemalloc_heap_profile_thresholds_bytes": [true]}"#,
        r#"{"jemalloc_heap_profile_thresholds_bytes": ["2G"]}"#,
        r#"{"jemalloc_heap_profile_thresholds_bytes": [-1]}"#,
        r#"{"jemalloc_heap_profile_thresholds_bytes": [null]}"#,
        r#"{"jemalloc_heap_profile_thresholds_bytes": [1, null, 2]}"#,
        r#"{"jemalloc_heap_profile_poll_interval_secs": "30"}"#,
        r#"{"jemalloc_heap_profile_poll_interval_secs": true}"#,
        r#"{"jemalloc_heap_profile_poll_interval_secs": -1}"#,
        r#"{
            "jemalloc_heap_profile_enabled": true,
            "jemalloc_heap_profile_thresholds_bytes": "bad",
            "workspace_command_enabled": true
        }"#,
    ] {
        assert_remote_parse_err(json);
    }
}
