use super::*;

#[test]
fn pi_read_maps_to_native_read_card() {
    assert_eq!(tool_kind("read"), acp::ToolKind::Read);
    assert_eq!(tool_kind("use_skill"), acp::ToolKind::Other);
}

#[test]
fn pi_read_result_projects_native_readfile_raw_output() {
    let raw = normalize_tool_raw_output(
        "read",
        Some(&json!({ "path": "src/lib.rs", "offset": 10, "limit": 20 })),
        &json!({
            "content": [{ "type": "text", "text": "fn main() {}\n// end\n\n[Showing lines 10-11 of 42. Use offset=12 to continue.]" }],
        }),
        false,
    );
    assert_eq!(raw.get("type").and_then(Value::as_str), Some("ReadFile"));
    let file = raw.get("FileContent").expect("FileContent variant");
    assert_eq!(
        file.get("absolute_path").and_then(Value::as_str),
        Some("src/lib.rs")
    );
    assert_eq!(file.get("offset").and_then(Value::as_u64), Some(9));
    assert_eq!(file.get("limit").and_then(Value::as_u64), Some(20));
    assert_eq!(file.get("total_lines").and_then(Value::as_u64), Some(42));
    assert!(
        file.get("raw_output")
            .and_then(Value::as_str)
            .is_some_and(|text| text.contains("fn main()"))
    );
}

#[test]
fn pi_bash_result_projects_native_bash_raw_output() {
    let raw = normalize_tool_raw_output(
        "bash",
        Some(&json!({ "command": "ls -la", "task_name": "列出目录" })),
        &json!({
            "content": [{ "type": "text", "text": "total 48\nREADME.md\n" }],
            "details": { "fullOutputPath": null },
        }),
        false,
    );
    assert_eq!(raw.get("type").and_then(Value::as_str), Some("Bash"));
    assert_eq!(raw.get("command").and_then(Value::as_str), Some("ls -la"));
    assert_eq!(
        raw.get("description").and_then(Value::as_str),
        Some("列出目录")
    );
    assert_eq!(raw.get("exit_code").and_then(Value::as_i64), Some(0));
    let output = raw
        .get("output_for_prompt")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(output.contains("README.md"));

    let direct = bash_tool_output(
        "echo hi",
        Some("print greeting"),
        &json!({
            "output": "hi\n",
            "exitCode": 0,
            "truncated": false,
        }),
        false,
    );
    assert_eq!(direct.get("type").and_then(Value::as_str), Some("Bash"));
    assert_eq!(
        direct.get("output_for_prompt").and_then(Value::as_str),
        Some("hi\n")
    );
    assert_eq!(
        direct.get("description").and_then(Value::as_str),
        Some("print greeting")
    );
}

#[test]
fn pi_bash_task_name_aliases_to_description_for_pager() {
    let args = normalize_tool_raw_input(
        "bash",
        Some(json!({
            "command": "cargo test -p pi-grok-adapter -- --nocapture",
            "task_name": "运行 adapter 测试",
        })),
    )
    .unwrap();
    assert_eq!(
        args.get("description").and_then(Value::as_str),
        Some("运行 adapter 测试"),
        "Pager Execute cards read raw_input.description"
    );
    // Keep original field for debuggability / round-trips.
    assert_eq!(
        args.get("task_name").and_then(Value::as_str),
        Some("运行 adapter 测试")
    );

    // Existing description wins over task_name.
    let preferred = normalize_tool_raw_input(
        "bash",
        Some(json!({
            "command": "true",
            "description": "already set",
            "task_name": "ignored",
        })),
    )
    .unwrap();
    assert_eq!(
        preferred.get("description").and_then(Value::as_str),
        Some("already set")
    );
}

#[test]
fn pi_edit_and_write_inputs_produce_native_diff_content() {
    let edit = edit_diff_content(
        "edit",
        Some(&json!({
            "path": "README.md",
            "oldText": "before\n",
            "newText": "after\n",
        })),
        None,
    )
    .expect("edit input must become a diff");
    let acp::ToolCallContent::Diff(diff) = &edit[0] else {
        panic!("edit input must produce ACP Diff content");
    };
    assert_eq!(diff.path.to_string_lossy(), "README.md");
    assert_eq!(diff.old_text.as_deref(), Some("before\n"));
    assert_eq!(diff.new_text, "after\n");

    let current_edit = edit_diff_content(
        "edit",
        Some(&json!({
            "path": "README.md",
            "edits": [
                { "oldText": "before\n", "newText": "after\n" },
                { "oldText": "first\n", "newText": "second\n" },
            ],
        })),
        None,
    )
    .expect("current edit input must become diffs");
    assert_eq!(current_edit.len(), 2);

    let write = edit_diff_content(
        "write",
        Some(&json!({ "path": "README.md", "content": "new file\n" })),
        None,
    )
    .expect("write input must become a diff");
    let acp::ToolCallContent::Diff(diff) = &write[0] else {
        panic!("write input must produce ACP Diff content");
    };
    assert_eq!(diff.old_text, None);
    assert_eq!(diff.new_text, "new file\n");
}

#[test]
fn pi_multiregion_edit_projects_patch_line_numbers() {
    let content = edit_diff_content(
        "edit",
        Some(&json!({
            "path": "src/example.rs",
            "edits": [
                { "oldText": "before alpha", "newText": "after alpha" },
                { "oldText": "before beta", "newText": "after beta" },
            ],
        })),
        Some(&json!({
            "details": {
                "patch": "--- src/example.rs\n+++ src/example.rs\n@@ -7,2 +7,2 @@\n after alpha\n-before alpha\n+after alpha\n@@ -31,1 +31,1 @@\n-before beta\n+after beta\n"
            },
        })),
    )
    .expect("edit input must become diffs");

    let lines = content
        .iter()
        .map(|content| {
            let acp::ToolCallContent::Diff(diff) = content else {
                panic!("edit input must produce ACP Diff content");
            };
            diff.meta
                .as_ref()
                .and_then(|meta| meta.get("new_line"))
                .and_then(Value::as_u64)
        })
        .collect::<Vec<_>>();
    assert_eq!(lines, vec![Some(8), Some(31)]);
}

#[test]
fn pi_history_edit_projects_direct_patch_line_number() {
    let content = edit_diff_content(
        "edit",
        Some(&json!({
            "path": "src/example.rs",
            "oldText": "before alpha",
            "newText": "after alpha",
        })),
        Some(&json!({
            "patch": "--- src/example.rs\n+++ src/example.rs\n@@ -42,1 +42,1 @@\n-before alpha\n+after alpha\n"
        })),
    )
    .expect("edit input must become a diff");
    let acp::ToolCallContent::Diff(diff) = &content[0] else {
        panic!("edit input must produce ACP Diff content");
    };
    assert_eq!(
        diff.meta
            .as_ref()
            .and_then(|meta| meta.get("new_line"))
            .and_then(Value::as_u64),
        Some(42)
    );
}

#[test]
fn pi_edit_without_matching_patch_keeps_line_metadata_empty() {
    let content = edit_diff_content(
        "edit",
        Some(&json!({
            "path": "src/example.rs",
            "oldText": "before alpha",
            "newText": "after alpha",
        })),
        Some(&json!({
            "details": {
                "patch": "--- src/example.rs\n+++ src/example.rs\n@@ -7,1 +7,1 @@\n-before beta\n+after beta\n"
            },
        })),
    )
    .expect("edit input must become a diff");
    let acp::ToolCallContent::Diff(diff) = &content[0] else {
        panic!("edit input must produce ACP Diff content");
    };
    assert!(diff.meta.is_none());
}

#[test]
fn pi_builtin_tool_kinds() {
    assert_eq!(tool_kind("read"), acp::ToolKind::Read);
    assert_eq!(tool_kind("bash"), acp::ToolKind::Execute);
    assert_eq!(tool_kind("edit"), acp::ToolKind::Edit);
    assert_eq!(tool_kind("write"), acp::ToolKind::Edit);
    assert_eq!(tool_kind("grep"), acp::ToolKind::Search);
    assert_eq!(tool_kind("find"), acp::ToolKind::Search);
    assert_eq!(tool_kind("ls"), acp::ToolKind::Other);
    assert_eq!(tool_kind("eval"), acp::ToolKind::Other);
}

#[test]
fn eval_raw_input_marks_dedicated_variant_and_preserves_fields() {
    let args = normalize_tool_raw_input(
        "eval",
        Some(json!({
            "language": "py",
            "code": "x = 40\nx + 2",
            "title": "compute",
        })),
    )
    .unwrap();
    assert_eq!(args.get("variant").and_then(Value::as_str), Some("Eval"));
    assert_eq!(args.get("language").and_then(Value::as_str), Some("py"));
    assert_eq!(
        args.get("code").and_then(Value::as_str),
        Some("x = 40\nx + 2")
    );
    assert_eq!(args.get("title").and_then(Value::as_str), Some("compute"));
    assert!(args.get("command").is_none());
    assert!(args.get("description").is_none());
}

#[test]
fn eval_raw_output_stays_native_instead_of_projecting_bash() {
    let result = json!({"content": [{"type": "text", "text": "42"}]});
    let raw = normalize_tool_raw_output(
        "eval",
        Some(&json!({"language": "py", "code": "x = 40\nx + 2"})),
        &result,
        false,
    );
    assert_eq!(raw, result);
}

#[test]
fn pi_write_raw_input_gets_write_variant() {
    let args =
        normalize_tool_raw_input("write", Some(json!({ "path": "a.rs", "content": "x" }))).unwrap();
    assert_eq!(args.get("variant").and_then(Value::as_str), Some("Write"));
}

#[test]
fn fabric_exec_raw_input_passes_through_for_other_card() {
    let args = normalize_tool_raw_input(
        "fabric_exec",
        Some(json!({
            "code": "return 1;",
            "display": { "name": "scan" },
        })),
    )
    .unwrap();
    // Must not rewrite to UseTool — F2 show_other_tool_args owns Other-card args.
    assert!(args.get("variant").is_none());
    assert_eq!(args.get("code").and_then(Value::as_str), Some("return 1;"));
}

#[test]
fn pi_ls_raw_input_gets_target_directory() {
    let args = normalize_tool_raw_input("ls", Some(json!({ "path": "src" }))).unwrap();
    assert_eq!(
        args.get("target_directory").and_then(Value::as_str),
        Some("src")
    );
}

#[test]
fn pi_grep_result_projects_native_grepsearch() {
    let raw = normalize_tool_raw_output(
        "grep",
        Some(&json!({ "pattern": "fn main", "path": "." })),
        &json!({
            "content": [{
                "type": "text",
                "text": "src/main.rs:10: fn main() {\nsrc/lib.rs:3: fn main_helper() {\n"
            }],
        }),
        false,
    );
    assert_eq!(raw.get("type").and_then(Value::as_str), Some("GrepSearch"));
    assert_eq!(raw.get("match_count").and_then(Value::as_u64), Some(2));
    let files = raw.get("file_matches").and_then(Value::as_array).unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(
        files[0].get("path").and_then(Value::as_str),
        Some("src/main.rs")
    );
    assert_eq!(
        files[0].get("matches").and_then(Value::as_array).unwrap()[0]
            .get("line_number")
            .and_then(Value::as_u64),
        Some(10)
    );
}

#[test]
fn pi_find_result_projects_files_with_matches() {
    let raw = normalize_tool_raw_output(
        "find",
        Some(&json!({ "pattern": "*.rs" })),
        &json!({
            "content": [{ "type": "text", "text": "src/a.rs\nsrc/b.rs\n" }],
        }),
        false,
    );
    assert_eq!(raw.get("type").and_then(Value::as_str), Some("GrepSearch"));
    assert_eq!(raw.get("match_count").and_then(Value::as_u64), Some(2));
}

#[test]
fn pi_ls_result_projects_native_listdir() {
    let raw = normalize_tool_raw_output(
        "ls",
        Some(&json!({ "path": "src" })),
        &json!({
            "content": [{ "type": "text", "text": "main.rs\nlib.rs\n" }],
        }),
        false,
    );
    assert_eq!(raw.get("type").and_then(Value::as_str), Some("ListDir"));
    let content = raw.get("Content").expect("ListDir Content");
    assert_eq!(
        content.get("absolute_root_path").and_then(Value::as_str),
        Some("src")
    );
    assert!(
        content
            .get("content")
            .and_then(Value::as_str)
            .is_some_and(|t| t.contains("main.rs"))
    );
}

#[test]
fn pi_grep_match_line_parser() {
    let (path, line, content) =
        split_pi_grep_match_line("crates/foo/bar.rs:42: let x = 1;").unwrap();
    assert_eq!(path, "crates/foo/bar.rs");
    assert_eq!(line, 42);
    assert_eq!(content, "let x = 1;");
    assert!(split_pi_grep_match_line("crates/foo/bar.rs-42- context").is_none());
}

#[test]
fn pi_rtk_grep_output_projects_matches_under_file_header() {
    let raw = normalize_tool_raw_output(
        "grep",
        Some(&json!({ "pattern": "pi-grok-adapter", "path": "Cargo.toml" })),
        &json!({
            "content": [{
                "type": "text",
                "text": "1 matches in 1 files:\n\n> Cargo.toml (1 matches):\n    6: \"crates/codegen/pi-grok-adapter\",\n"
            }]
        }),
        false,
    );
    assert_eq!(raw.get("match_count").and_then(Value::as_u64), Some(1));
    let file = &raw.get("file_matches").and_then(Value::as_array).unwrap()[0];
    assert_eq!(file.get("path").and_then(Value::as_str), Some("Cargo.toml"));
    assert_eq!(
        file.get("matches").and_then(Value::as_array).unwrap()[0]
            .get("line_number")
            .and_then(Value::as_u64),
        Some(6)
    );
}

#[test]
fn search_like_tools_are_not_misclassified_as_grep() {
    // Extension / MCP / Pi tools whose names merely contain "search" must not
    // be treated as grep searches, or their results would be run through the
    // `path:line: content` parser and rendered as a broken Search card.
    for name in [
        "web_search",
        "memory_search",
        "session_search",
        "codebase_search",
        "x_search",
        "grafana__search",
        "pi/session/search",
    ] {
        assert_eq!(
            tool_kind(name),
            acp::ToolKind::Other,
            "{name} must stay Other, not be classed as Search"
        );
    }
}

#[test]
fn search_like_tool_output_is_not_grep_parsed() {
    // A `web_search` result is plain prose, not grep `path:line` matches.
    let result = json!({
        "content": [{ "type": "text", "text": "results for query\n- https://example.com" }],
    });
    let raw = normalize_tool_raw_output(
        "web_search",
        Some(&json!({ "query": "rust" })),
        &result,
        false,
    );
    assert_eq!(
        raw, result,
        "web_search must pass through unchanged, not be rewritten to GrepSearch"
    );
    assert_ne!(raw.get("type").and_then(Value::as_str), Some("GrepSearch"));
}

#[test]
fn write_like_and_exec_like_tools_stay_other() {
    // `TodoWrite` must not be treated as an Edit, and `fabric_exec` must not be
    // treated as a bash Execute — they are distinct extension tools.
    assert_eq!(tool_kind("TodoWrite"), acp::ToolKind::Other);
    assert_eq!(tool_kind("fabric_exec"), acp::ToolKind::Other);
}

#[test]
fn write_like_tools_do_not_get_write_variant_or_bash_description() {
    let todo = normalize_tool_raw_input(
        "TodoWrite",
        Some(json!({ "todo": "x", "task_name": "label" })),
    )
    .unwrap();
    assert!(
        todo.get("variant").is_none(),
        "TodoWrite must not be marked as a Write card"
    );
    assert!(
        todo.get("description").is_none(),
        "TodoWrite must not be aliased to a bash description"
    );

    let fabric = normalize_tool_raw_input(
        "fabric_exec",
        Some(json!({ "code": "return 1;", "task_name": "scan" })),
    )
    .unwrap();
    assert!(
        fabric.get("description").is_none(),
        "fabric_exec must not be aliased to a bash description"
    );
}
