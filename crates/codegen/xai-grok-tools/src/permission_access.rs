use xai_grok_permission_rules::types::AccessKind;
/// Classification is an allowlist of read-only tools: a tool is `Read`/`Grep`/`WebSearch` only when it
/// reads state or touches nothing but the session's own bookkeeping. Every other tool prompts as an
/// edit, a command, an MCP call, or a fetch, so a new `ToolInput` variant cannot run unasked.
impl From<&crate::types::ToolInput> for AccessKind {
    fn from(input: &crate::types::ToolInput) -> Self {
        use crate::types::ToolInput;
        match input {
            ToolInput::ReadFile(r) => AccessKind::Read(Some(r.path.clone())),
            ToolInput::ListDir(l) => AccessKind::Read(Some(l.target_directory.clone())),
            ToolInput::CodexReadFile(r) => AccessKind::Read(Some(r.file_path.clone())),
            ToolInput::CodexListDir(l) => AccessKind::Read(Some(l.dir_path.clone())),
            ToolInput::MemoryGet(m) => AccessKind::Read(Some(m.path.clone())),
            ToolInput::Lsp(l) => AccessKind::Read(l.file_path.clone()),
            ToolInput::Grep(g) => AccessKind::Grep {
                path: g.path.clone(),
                glob: g.glob.clone(),
            },
            ToolInput::CodexGrepFiles(g) => AccessKind::Grep {
                path: g.path.clone(),
                glob: g.include.clone(),
            },
            ToolInput::TodoWrite(_)
            | ToolInput::TaskOutput(_)
            | ToolInput::WaitTasks(_)
            | ToolInput::KillTask(_)
            | ToolInput::Skill(_)
            | ToolInput::MemorySearch(_)
            | ToolInput::SearchTool(_)
            | ToolInput::SchedulerList(_)
            | ToolInput::EnterPlanMode(_)
            | ToolInput::ExitPlanMode(_)
            | ToolInput::AskUserQuestion(_)
            | ToolInput::UpdateGoal(_) => AccessKind::Read(None),
            ToolInput::Task(_) => AccessKind::Tool("task".to_owned()),
            ToolInput::SchedulerCreate(_) => AccessKind::Tool("scheduler_create".to_owned()),
            ToolInput::SchedulerDelete(_) => AccessKind::Tool("scheduler_delete".to_owned()),
            ToolInput::Workflow(_) => AccessKind::Tool("workflow".to_owned()),
            ToolInput::ImageGen(_) => AccessKind::Tool("image_gen".to_owned()),
            ToolInput::ImageEdit(_) => AccessKind::Tool("image_edit".to_owned()),
            ToolInput::ImageToVideo(_) => AccessKind::Tool("image_to_video".to_owned()),
            ToolInput::ReferenceToVideo(_) => AccessKind::Tool("reference_to_video".to_owned()),
            ToolInput::SendSubagentMessage(message) => AccessKind::AgentMessage {
                subagent_id: message.subagent_id.clone(),
            },
            ToolInput::SendFeedback(_) => AccessKind::Tool("send_feedback".to_owned()),
            ToolInput::WebSearch(ws) => AccessKind::WebSearch(ws.query.clone()),
            ToolInput::SearchReplace(search_replace) => {
                AccessKind::Edit(search_replace.file_path.to_string())
            }
            ToolInput::ApplyPatch(_) => AccessKind::Tool("apply_patch".to_owned()),
            ToolInput::HashlineEdit(he) => AccessKind::Edit(he.file_path.to_string()),
            ToolInput::Write(w) => AccessKind::Edit(w.file_path.clone()),
            ToolInput::Bash(bash) => AccessKind::Bash(bash.command.to_string()),
            ToolInput::Monitor(m) => AccessKind::Bash(m.command.clone()),
            ToolInput::MCPTool(mcp) => AccessKind::MCPTool {
                name: mcp.tool_name.to_string(),
                input: mcp.tool_input.clone(),
            },
            ToolInput::UseTool(crate::implementations::UseToolInput::Inline(u)) => {
                AccessKind::MCPTool {
                    name: u.tool_name.clone(),
                    input: u.tool_input.clone(),
                }
            }
            ToolInput::UseTool(u) => {
                AccessKind::Read(u.source_path().map(|p| p.to_string_lossy().into_owned()))
            }
            ToolInput::WebFetch(wf) => AccessKind::WebFetch(wf.url.clone()),
            ToolInput::Dynamic(value) => access_kind_from_dynamic(value),
            #[allow(unreachable_patterns)]
            _ => AccessKind::Tool("unclassified_tool".to_owned()),
        }
    }
}
fn dynamic_string_field(value: &serde_json::Value, keys: &[&str]) -> Option<String> {
    let object = value.as_object()?;
    keys.iter()
        .find_map(|key| object.get(*key).and_then(serde_json::Value::as_str))
        .map(str::to_owned)
}
fn dynamic_has_field(value: &serde_json::Value, keys: &[&str]) -> bool {
    value
        .as_object()
        .is_some_and(|object| keys.iter().any(|key| object.contains_key(*key)))
}
fn access_kind_from_dynamic(value: &serde_json::Value) -> AccessKind {
    if let Some(name) = dynamic_string_field(value, &["name", "tool", "tool_name", "variant"])
        && (name == "send_feedback" || name == "SendFeedback")
    {
        return AccessKind::Tool("send_feedback".to_owned());
    }
    if let Some(path) = dynamic_string_field(value, &["filePath", "file_path", "path"]) {
        let is_mutation = dynamic_has_field(
            value,
            &[
                "oldString",
                "old_string",
                "newString",
                "new_string",
                "content",
                "edits",
                "replaceAll",
                "replace_all",
            ],
        );
        return if is_mutation {
            AccessKind::Edit(path)
        } else {
            AccessKind::Read(Some(path))
        };
    }
    if let Some(command) = dynamic_string_field(value, &["command"]) {
        return AccessKind::Bash(command);
    }
    AccessKind::Read(None)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hashline_edit_maps_to_edit_access() {
        use crate::implementations::grok_build_hashline::edit::types::HashlineEditInput;
        use crate::types::ToolInput;
        let input = ToolInput::HashlineEdit(HashlineEditInput {
            file_path: "src/main.rs".into(),
            edits: vec![],
        });
        let access = AccessKind::from(&input);
        assert!(
            matches!(access, AccessKind::Edit(ref p) if p == "src/main.rs"),
            "HashlineEdit should produce AccessKind::Edit with the file path, got {access:?}"
        );
    }
    #[test]
    fn bash_maps_to_bash_access() {
        use crate::implementations::grok_build::bash::BashToolInput;
        use crate::types::ToolInput;
        let input = ToolInput::Bash(BashToolInput {
            command: "cargo test".into(),
            timeout: None,
            description: "run tests".into(),
            is_background: false,
            block_until_ms: None,
        });
        let access = AccessKind::from(&input);
        assert!(
            matches!(access, AccessKind::Bash(ref cmd) if cmd == "cargo test"),
            "Bash should produce AccessKind::Bash with the command, got {access:?}"
        );
    }
    #[test]
    fn active_agent_message_maps_to_dedicated_access_without_text() {
        use crate::implementations::grok_build::send_subagent_message::SendSubagentMessageInput;
        use crate::types::ToolInput;
        let text = "private follow-up";
        let access = AccessKind::from(&ToolInput::SendSubagentMessage(SendSubagentMessageInput {
            subagent_id: "sub-1".into(),
            text: text.into(),
            delivery: None,
            queue: false,
        }));
        let AccessKind::AgentMessage { subagent_id } = access else {
            panic!("active agent messages must use dedicated access")
        };
        assert_eq!(subagent_id, "sub-1");
        assert!(!subagent_id.contains(text));
    }
    #[test]
    fn send_feedback_maps_to_tool_access() {
        use crate::types::ToolInput;
        let input: ToolInput = serde_json::from_value(serde_json::json!({
            "variant": "SendFeedback",
            "title": "Draft title",
            "details": "What happened:\n- The tool failed.",
            "type": "bug"
        }))
        .unwrap();
        assert!(matches!(
            AccessKind::from(&input),
            AccessKind::Tool(name) if name == "send_feedback"
        ));
        assert!(matches!(
            AccessKind::from(&ToolInput::Dynamic(serde_json::json!({
                "name": "send_feedback",
                "title": "Draft title",
                "details": "What happened",
            }))),
            AccessKind::Tool(name) if name == "send_feedback"
        ));
    }
    #[test]
    fn use_tool_maps_to_mcp_tool_access() {
        use crate::implementations::use_tool::UseToolInput;
        use crate::types::ToolInput;
        let input = ToolInput::UseTool(UseToolInput::Inline(
            crate::implementations::use_tool::InlineMcpInvocation {
                tool_name: "linear__save_issue".into(),
                tool_input: serde_json::json!({ "title" : "test" }),
            },
        ));
        let access = AccessKind::from(&input);
        assert!(
            matches!(
                access,
                AccessKind::MCPTool { ref name, ref input }
                    if name == "linear__save_issue"
                        && input.get("title").and_then(|v| v.as_str()) == Some("test")
            ),
            "UseTool should produce AccessKind::MCPTool carrying the inner tool name and args, got {access:?}"
        );
    }
    #[test]
    fn monitor_maps_to_bash_access() {
        use crate::implementations::grok_build::monitor::types::MonitorInput;
        use crate::types::ToolInput;
        let input = ToolInput::Monitor(MonitorInput {
            command: "tail -f /var/log/syslog".into(),
            description: "watch syslog".into(),
            timeout_ms: None,
            persistent: false,
        });
        let access = AccessKind::from(&input);
        assert!(
            matches!(access, AccessKind::Bash(ref cmd) if cmd == "tail -f /var/log/syslog"),
            "Monitor runs shell and must map to AccessKind::Bash (not Read), got {access:?}"
        );
    }
    #[test]
    fn search_replace_maps_to_edit_access() {
        use crate::implementations::grok_build::search_replace::SearchReplaceInput;
        use crate::types::ToolInput;
        let input = ToolInput::SearchReplace(SearchReplaceInput {
            file_path: "lib.rs".into(),
            old_string: "old".into(),
            new_string: "new".into(),
            replace_all: false,
        });
        let access = AccessKind::from(&input);
        assert!(
            matches!(access, AccessKind::Edit(ref p) if p == "lib.rs"),
            "SearchReplace should produce AccessKind::Edit, got {access:?}"
        );
    }
    #[test]
    fn web_fetch_maps_to_web_fetch_access() {
        use crate::implementations::grok_build::web_fetch::WebFetchInput;
        use crate::types::ToolInput;
        let input = ToolInput::WebFetch(WebFetchInput {
            url: "https://custom.example.com/api".into(),
        });
        let access = AccessKind::from(&input);
        assert!(
            matches!(access, AccessKind::WebFetch(ref u) if u == "https://custom.example.com/api"),
            "WebFetch should produce AccessKind::WebFetch with the URL, got {access:?}"
        );
    }
    #[test]
    fn web_search_maps_to_web_search_access() {
        use crate::implementations::grok_build::web_search::WebSearchInput;
        use crate::types::ToolInput;
        let input = ToolInput::WebSearch(WebSearchInput {
            query: "rust lang".into(),
            allowed_domains: None,
        });
        let access = AccessKind::from(&input);
        assert!(
            matches!(access, AccessKind::WebSearch(ref q) if q == "rust lang"),
            "WebSearch should produce AccessKind::WebSearch with the query, got {access:?}"
        );
    }
    /// The patch text names its files; no grant scope can vouch for them, so a patch prompts every time.
    #[test]
    fn apply_patch_maps_to_tool_access() {
        use crate::implementations::codex::apply_patch::ApplyPatchInput;
        use crate::types::ToolInput;
        let input = ToolInput::ApplyPatch(ApplyPatchInput {
            patch: "*** Begin Patch\n*** Update File: /home/user/.grok/mcp.json\n*** End Patch"
                .to_owned(),
        });
        assert!(matches!(
            AccessKind::from(&input),
            AccessKind::Tool(name) if name == "apply_patch"
        ));
    }
    #[test]
    fn write_tool_maps_to_edit_access() {
        use crate::implementations::opencode::write::WriteInput;
        use crate::types::ToolInput;
        let input = ToolInput::Write(WriteInput {
            file_path: "/tmp/secret.txt".into(),
            content: "overwritten".into(),
        });
        let access = AccessKind::from(&input);
        assert!(
            matches!(access, AccessKind::Edit(ref p) if p == "/tmp/secret.txt"),
            "Write should produce AccessKind::Edit with the file path, got {access:?}"
        );
    }
    #[test]
    fn write_scoped_and_dynamic_inputs_map_to_edit_not_read() {
        use crate::implementations::opencode::edit::EditInput;
        use crate::types::ToolInput;
        use xai_tool_types::TaskToolInput;
        let edit = ToolInput::from(EditInput {
            file_path: "/tmp/denied.txt".into(),
            old_string: "ORIGINAL".into(),
            new_string: "BYPASS".into(),
            replace_all: false,
        });
        assert!(matches!(
            &edit,
            ToolInput::SearchReplace(sr) if sr.file_path == "/tmp/denied.txt"
        ));
        assert!(matches!(
            AccessKind::from(&edit),
            AccessKind::Edit(p) if p == "/tmp/denied.txt"
        ));
        assert!(matches!(
            AccessKind::from(&ToolInput::Task(TaskToolInput {
                prompt: "edit config.toml".into(),
                description: "spawn".into(),
                subagent_type: "general-purpose".into(),
                subagent_type_specified: false,
                run_in_background: false,
                capability_mode: None,
                isolation: None,
                resume_from: None,
                cwd: None,
                model: None,
                workspace: None,
                task_id: None,
            })),
            AccessKind::Tool(name) if name == "task"
        ));
        assert!(matches!(
            AccessKind::from(&ToolInput::Dynamic(serde_json::json!({
                "filePath": "/tmp/denied.txt",
                "oldString": "a",
                "newString": "b",
            }))),
            AccessKind::Edit(p) if p == "/tmp/denied.txt"
        ));
        assert!(matches!(
            AccessKind::from(&ToolInput::Dynamic(serde_json::json!({
                "filePath": "src/main.rs"
            }))),
            AccessKind::Read(Some(p)) if p == "src/main.rs"
        ));
        assert!(matches!(
            AccessKind::from(&ToolInput::Dynamic(serde_json::json!({
                "command": "rm -rf /"
            }))),
            AccessKind::Bash(c) if c == "rm -rf /"
        ));
    }
    #[test]
    fn write_scoped_access_respects_edit_deny_and_not_read_allow() {
        use crate::implementations::opencode::edit::EditInput;
        use crate::types::ToolInput;
        use xai_grok_permission_rules::policy::CompiledPolicy;
        use xai_grok_permission_rules::rules::parse_permission_rule;
        use xai_grok_permission_rules::types::{Decision, PermissionConfig, RuleAction};
        use xai_tool_types::TaskToolInput;
        let edit = AccessKind::from(&ToolInput::from(EditInput {
            file_path: "/tmp/denied.txt".into(),
            old_string: "ORIGINAL".into(),
            new_string: "BYPASS".into(),
            replace_all: false,
        }));
        let task = AccessKind::from(&ToolInput::Task(TaskToolInput {
            prompt: "edit config.toml".into(),
            description: "spawn".into(),
            subagent_type: "general-purpose".into(),
            subagent_type_specified: false,
            run_in_background: false,
            capability_mode: None,
            isolation: None,
            resume_from: None,
            cwd: None,
            model: None,
            workspace: None,
            task_id: None,
        }));
        let deny_edits = CompiledPolicy::new(PermissionConfig::new(vec![
            parse_permission_rule("Edit(*)", RuleAction::Deny).unwrap(),
        ]));
        assert!(matches!(
            deny_edits.evaluate(&edit),
            Some(Decision::Reject(_))
        ));
        assert!(matches!(
            deny_edits.evaluate(&task),
            Some(Decision::Reject(_))
        ));
        let allow_read = CompiledPolicy::new(PermissionConfig::new(vec![
            parse_permission_rule("Read", RuleAction::Allow).unwrap(),
        ]));
        assert!(allow_read.evaluate(&task).is_none());
        assert!(allow_read.evaluate(&edit).is_none());
    }
}
