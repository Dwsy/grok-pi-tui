use super::{apply_hooks_to_dir, extract_hooks_from_settings_file};
use crate::ImportableItem;

#[test]
fn extract_hooks_basic_command() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    std::fs::write(
        &path,
        r#"{
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": "Bash",
                        "hooks": [
                            { "type": "command", "command": "echo hi", "timeout": 7 }
                        ]
                    }
                ]
            }
        }"#,
    )
    .unwrap();

    let items = extract_hooks_from_settings_file(&path);

    assert_eq!(items.len(), 1);
    let Some(ImportableItem::Hook {
        event,
        matcher,
        command,
        timeout,
    }) = items.first()
    else {
        panic!("expected Hook variant: {items:?}");
    };
    assert_eq!(event, "PreToolUse");
    assert_eq!(matcher.as_deref(), Some("Bash"));
    assert_eq!(command, "echo hi");
    assert_eq!(*timeout, Some(7));
}

#[test]
fn extract_hooks_skips_non_command_handlers() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    std::fs::write(
        &path,
        r#"{
            "hooks": {
                "PostToolUse": [
                    {
                        "hooks": [
                            { "type": "http", "url": "https://example.com" },
                            { "type": "command", "command": "true" }
                        ]
                    }
                ]
            }
        }"#,
    )
    .unwrap();

    let items = extract_hooks_from_settings_file(&path);

    assert_eq!(items.len(), 1, "only command handler should be imported");
}

#[test]
fn extract_hooks_empty_when_no_hooks_key() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    std::fs::write(&path, r#"{ "permissions": { "allow": [] } }"#).unwrap();
    assert!(extract_hooks_from_settings_file(&path).is_empty());
}

#[test]
fn extract_hooks_missing_file_returns_empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing.json");
    assert!(extract_hooks_from_settings_file(&path).is_empty());
}

#[test]
fn apply_hooks_to_dir_writes_new_file() {
    let dir = tempfile::tempdir().unwrap();
    let hooks_dir = dir.path().join("hooks");
    let items = vec![ImportableItem::Hook {
        event: "PreToolUse".to_string(),
        matcher: Some("Bash".to_string()),
        command: "echo x".to_string(),
        timeout: None,
    }];

    let count = apply_hooks_to_dir(&hooks_dir, &items).unwrap();

    assert_eq!(count, 1);

    let target = hooks_dir.join("imported-from-claude.json");
    let groups = read_pre_tool_use_groups(&target);
    let [group] = groups.as_slice() else {
        panic!("expected one group: {groups:?}");
    };
    assert_eq!(group.get("matcher").and_then(|v| v.as_str()), Some("Bash"));
    assert_eq!(
        group
            .get("hooks")
            .and_then(|h| h.get(0))
            .and_then(|h| h.get("command"))
            .and_then(|v| v.as_str()),
        Some("echo x")
    );
}

#[test]
fn apply_hooks_to_dir_dedup_existing() {
    let dir = tempfile::tempdir().unwrap();
    let hooks_dir = dir.path().join("hooks");
    std::fs::create_dir_all(&hooks_dir).unwrap();
    let target = hooks_dir.join("imported-from-claude.json");
    std::fs::write(
        &target,
        r#"{
            "hooks": {
                "PreToolUse": [
                    { "matcher": "Bash", "hooks": [{ "type": "command", "command": "echo x" }] }
                ]
            }
        }"#,
    )
    .unwrap();

    let items = vec![
        ImportableItem::Hook {
            event: "PreToolUse".to_string(),
            matcher: Some("Bash".to_string()),
            command: "echo x".to_string(),
            timeout: None,
        },
        ImportableItem::Hook {
            event: "PreToolUse".to_string(),
            matcher: Some("Bash".to_string()),
            command: "echo y".to_string(),
            timeout: None,
        },
    ];

    let count = apply_hooks_to_dir(&hooks_dir, &items).unwrap();

    assert_eq!(count, 1, "only the new hook should be added");

    let groups = read_pre_tool_use_groups(&target);
    assert_eq!(groups.len(), 2);
}

#[test]
fn apply_hooks_to_dir_no_hooks_returns_zero() {
    let dir = tempfile::tempdir().unwrap();
    let hooks_dir = dir.path().join("hooks");
    let items = vec![ImportableItem::EnvVar {
        key: "X".into(),
        value: "y".into(),
    }];

    let count = apply_hooks_to_dir(&hooks_dir, &items).unwrap();

    assert_eq!(count, 0);
    assert!(!hooks_dir.exists());
}

#[test]
fn extract_hooks_multiple_events_and_matchers() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    std::fs::write(
        &path,
        r#"{
            "hooks": {
                "PreToolUse": [
                    { "matcher": "Bash", "hooks": [{ "type": "command", "command": "a" }] },
                    { "matcher": "Edit", "hooks": [{ "type": "command", "command": "b" }] }
                ],
                "PostToolUse": [
                    { "hooks": [
                        { "type": "command", "command": "c" },
                        { "type": "command", "command": "d" }
                    ] }
                ]
            }
        }"#,
    )
    .unwrap();

    let items = extract_hooks_from_settings_file(&path);

    assert_eq!(items.len(), 4, "4 hooks across 2 events / 3 matchers");
    let events: std::collections::HashSet<&str> = items
        .iter()
        .filter_map(|i| match i {
            ImportableItem::Hook { event, .. } => Some(event.as_str()),
            _ => None,
        })
        .collect();
    assert!(events.contains("PreToolUse"));
    assert!(events.contains("PostToolUse"));
}

#[test]
fn extract_hooks_malformed_hooks_field_is_silent_skip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    std::fs::write(&path, r#"{ "hooks": "oops" }"#).unwrap();
    assert!(extract_hooks_from_settings_file(&path).is_empty());
}

#[test]
fn extract_hooks_empty_command_string_is_imported_as_is() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    std::fs::write(
        &path,
        r#"{ "hooks": { "PreToolUse": [
            { "matcher": "Bash", "hooks": [{ "type": "command", "command": "" }] }
        ] } }"#,
    )
    .unwrap();

    let items = extract_hooks_from_settings_file(&path);

    assert_eq!(items.len(), 1);
    if let Some(ImportableItem::Hook { command, .. }) = items.first() {
        assert_eq!(command, "");
    } else {
        panic!("expected Hook variant");
    }
}

#[test]
fn apply_hooks_to_dir_updates_timeout_on_dedup() {
    let dir = tempfile::tempdir().unwrap();
    let hooks_dir = dir.path().join("hooks");
    std::fs::create_dir_all(&hooks_dir).unwrap();
    let target = hooks_dir.join("imported-from-claude.json");
    std::fs::write(
        &target,
        r#"{ "hooks": { "PreToolUse": [
            { "matcher": "Bash", "hooks": [
                { "type": "command", "command": "echo x", "timeout": 5 }
            ] }
        ] } }"#,
    )
    .unwrap();

    let items = vec![ImportableItem::Hook {
        event: "PreToolUse".to_string(),
        matcher: Some("Bash".to_string()),
        command: "echo x".to_string(),
        timeout: Some(60),
    }];

    let count = apply_hooks_to_dir(&hooks_dir, &items).unwrap();

    assert_eq!(count, 0, "no new entry added; timeout updated in place");

    let groups = read_pre_tool_use_groups(&target);
    let [group] = groups.as_slice() else {
        panic!("expected one group: {groups:?}");
    };
    let Some(handlers) = group.get("hooks").and_then(|h| h.as_array()) else {
        panic!("expected handlers: {group:?}");
    };
    assert_eq!(
        handlers
            .first()
            .and_then(|h| h.get("timeout"))
            .and_then(|v| v.as_u64()),
        Some(60)
    );
}

#[test]
fn apply_hooks_to_dir_removes_timeout_when_new_has_none() {
    let dir = tempfile::tempdir().unwrap();
    let hooks_dir = dir.path().join("hooks");
    std::fs::create_dir_all(&hooks_dir).unwrap();
    let target = hooks_dir.join("imported-from-claude.json");
    std::fs::write(
        &target,
        r#"{ "hooks": { "PreToolUse": [
            { "matcher": "Bash", "hooks": [
                { "type": "command", "command": "echo x", "timeout": 5 }
            ] }
        ] } }"#,
    )
    .unwrap();

    let items = vec![ImportableItem::Hook {
        event: "PreToolUse".to_string(),
        matcher: Some("Bash".to_string()),
        command: "echo x".to_string(),
        timeout: None,
    }];

    let count = apply_hooks_to_dir(&hooks_dir, &items).unwrap();

    assert_eq!(count, 0);

    let groups = read_pre_tool_use_groups(&target);
    let Some(handlers) = groups
        .first()
        .and_then(|g| g.get("hooks"))
        .and_then(|h| h.as_array())
    else {
        panic!("expected handlers: {groups:?}");
    };
    assert!(handlers.first().is_some_and(|h| h.get("timeout").is_none()));
}

fn read_pre_tool_use_groups(target: &std::path::Path) -> Vec<serde_json::Value> {
    let content = std::fs::read_to_string(target).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
    let Some(groups) = parsed
        .get("hooks")
        .and_then(|h| h.get("PreToolUse"))
        .and_then(|v| v.as_array())
    else {
        panic!("expected PreToolUse hooks: {parsed:?}");
    };
    groups.clone()
}
