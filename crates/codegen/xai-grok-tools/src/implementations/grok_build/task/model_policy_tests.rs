use pretty_assertions::assert_eq;

use super::*;

#[test]
fn exported_schema_hides_model_only_under_inherited_selection() {
    let schema = task_input_schema();

    let selectable = exported_input_schema(&schema, &TaskParams::default());
    assert!(property(&selectable, "model").is_some());

    let inherited = TaskParams {
        model_selection: TaskModelSelection::Inherited,
        ..TaskParams::default()
    };
    let hidden = exported_input_schema(&schema, &inherited);
    assert!(property(&hidden, "model").is_none());
    assert!(property(&hidden, "prompt").is_some());
}

#[test]
fn exported_schema_has_no_subagent_type_without_selectable_types() {
    let schema = task_input_schema();

    let exported = exported_input_schema(&schema, &TaskParams::default());

    assert_eq!(schema, exported);
    assert!(property(&exported, SUBAGENT_TYPE_PARAM).is_none());
}

#[test]
fn exported_schema_offers_selectable_types_as_an_optional_enum() {
    let params = TaskParams {
        selectable_subagent_types: vec![
            descriptor("merlin:stash", "Searches Stash.\nUse for code history."),
            descriptor("reviewer", "Reviews diffs."),
        ],
        ..TaskParams::default()
    };

    let exported = exported_input_schema(&task_input_schema(), &params);

    assert_eq!(
        Some(&serde_json::json!({
            "type": "string",
            "enum": ["merlin:stash", "reviewer"],
            "description": "Omit for a general-purpose subagent. Set it to hand the task to one of these agents:\n\
                            - merlin:stash: Searches Stash. Use for code history.\n\
                            - reviewer: Reviews diffs.",
        })),
        property(&exported, SUBAGENT_TYPE_PARAM)
    );
    let required = exported
        .get("required")
        .and_then(serde_json::Value::as_array)
        .expect("required list");
    assert!(!required.iter().any(|name| name == SUBAGENT_TYPE_PARAM));
}

#[test]
fn exported_schema_caps_listed_types_and_descriptions() {
    let long = "x".repeat(MAX_SUBAGENT_TYPE_DESCRIPTION_BYTES * 2);
    let params = TaskParams {
        selectable_subagent_types: (0..MAX_SELECTABLE_SUBAGENT_TYPES + 3)
            .map(|i| descriptor(&format!("agent-{i:03}"), &long))
            .collect(),
        ..TaskParams::default()
    };

    let exported = exported_input_schema(&task_input_schema(), &params);

    let subagent_type = property(&exported, SUBAGENT_TYPE_PARAM).expect("subagent_type");
    let names = subagent_type
        .get("enum")
        .and_then(serde_json::Value::as_array)
        .expect("enum");
    assert_eq!(MAX_SELECTABLE_SUBAGENT_TYPES, names.len());
    let description = subagent_type
        .get("description")
        .and_then(serde_json::Value::as_str)
        .expect("description");
    let first = description.lines().nth(1).expect("first type line");
    assert!(first.len() <= "- agent-000: ".len() + MAX_SUBAGENT_TYPE_DESCRIPTION_BYTES);
}

#[test]
fn exported_schema_leaves_out_types_with_overlong_names() {
    let long_name = format!("plugin:{}", "n".repeat(MAX_SUBAGENT_TYPE_NAME_BYTES));
    let params = TaskParams {
        selectable_subagent_types: vec![
            descriptor(&long_name, "Too long to list."),
            descriptor("reviewer", "Reviews diffs."),
        ],
        ..TaskParams::default()
    };

    let exported = exported_input_schema(&task_input_schema(), &params);

    let subagent_type = property(&exported, SUBAGENT_TYPE_PARAM).expect("subagent_type");
    assert_eq!(
        Some(&serde_json::json!(["reviewer"])),
        subagent_type.get("enum")
    );
    let description = subagent_type
        .get("description")
        .and_then(serde_json::Value::as_str)
        .expect("description");
    assert!(!description.contains(&long_name));

    let only_long = TaskParams {
        selectable_subagent_types: vec![descriptor(&long_name, "Too long to list.")],
        ..TaskParams::default()
    };
    let schema = task_input_schema();
    assert_eq!(schema, exported_input_schema(&schema, &only_long));
}

fn task_input_schema() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(xai_tool_types::TaskToolInput))
        .expect("task input schema serializes")
}

fn descriptor(name: &str, description: &str) -> SubagentDescriptor {
    SubagentDescriptor {
        name: name.to_owned(),
        description: description.to_owned(),
        tools: None,
    }
}

fn property<'a>(schema: &'a serde_json::Value, name: &str) -> Option<&'a serde_json::Value> {
    schema.get("properties")?.get(name)
}
