use super::*;

fn tool(id: &str, kind: ToolKind) -> ToolConfig {
    ToolConfig {
        kind: Some(kind),
        ..ToolConfig::from_id(id)
    }
}

fn entries(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

fn ids<'a>(listed: &ListedTools<'a>) -> Vec<&'a str> {
    listed.tools.iter().map(|tool| tool.id.as_str()).collect()
}

#[test]
fn denylist_matches_tool_ids_only() {
    let toolset = [
        tool("GrokBuild:read_file", ToolKind::Read),
        tool("GrokBuild:web_fetch", ToolKind::WebFetch),
    ];
    let denied = entries(&["web_fetch", "GrokBuild:read_file", "Read"]);

    let listed = listed_tools(ToolList::Denylist, &denied, &toolset);

    assert_eq!(
        (Vec::<&str>::new(), vec!["Read"]),
        (ids(&listed), listed.unmatched)
    );
}

#[test]
fn allowlist_reads_a_claude_name_as_its_kind() {
    let toolset = [
        tool("GrokBuild:read_file", ToolKind::Read),
        tool("GrokBuild:grep", ToolKind::Search),
    ];
    let allowed = entries(&["Grep"]);

    let listed = listed_tools(
        ToolList::Allowlist {
            known_tool_ids: &HashSet::new(),
        },
        &allowed,
        &toolset,
    );

    assert_eq!(vec!["GrokBuild:grep"], ids(&listed));
}

#[test]
fn allowlist_keeps_mcp_access() {
    let toolset = [
        tool("GrokBuild:read_file", ToolKind::Read),
        tool("GrokBuild:grep", ToolKind::Search),
        tool("GrokBuild:search_tool", ToolKind::SearchTool),
        tool("GrokBuild:use_tool", ToolKind::UseTool),
    ];
    let allowed = entries(&["read_file"]);

    let listed = listed_tools(
        ToolList::Allowlist {
            known_tool_ids: &HashSet::new(),
        },
        &allowed,
        &toolset,
    );

    assert_eq!(
        vec![
            "GrokBuild:read_file",
            "GrokBuild:search_tool",
            "GrokBuild:use_tool"
        ],
        ids(&listed)
    );
}

#[test]
fn allowlist_mcp_entry_restricts_and_absent_kind_is_unmatched() {
    let toolset = [
        tool("GrokBuild:read_file", ToolKind::Read),
        tool("GrokBuild:grep", ToolKind::Search),
    ];
    for (expected_ids, expected_unmatched, expected_unknown, names) in [
        (
            vec!["GrokBuild:read_file"],
            Vec::<&str>::new(),
            Vec::<&str>::new(),
            &["read_file", "mcp__server__tool"][..],
        ),
        (
            vec!["GrokBuild:read_file"],
            vec!["WebFetch"],
            Vec::<&str>::new(),
            &["read_file", "WebFetch"][..],
        ),
    ] {
        let allowed = entries(names);
        let listed = listed_tools(
            ToolList::Allowlist {
                known_tool_ids: &HashSet::new(),
            },
            &allowed,
            &toolset,
        );
        assert_eq!(
            (expected_ids, expected_unmatched, expected_unknown),
            (ids(&listed), listed.unmatched, listed.unknown),
            "{names:?}"
        );
    }
}

#[test]
fn allowlist_with_an_unknown_name_keeps_the_whole_toolset() {
    let toolset = [
        tool("GrokBuild:read_file", ToolKind::Read),
        tool("GrokBuild:grep", ToolKind::Search),
    ];
    let allowed = entries(&["read_file", "Frobnicate"]);

    let listed = listed_tools(
        ToolList::Allowlist {
            known_tool_ids: &HashSet::new(),
        },
        &allowed,
        &toolset,
    );

    assert_eq!(
        (
            vec!["GrokBuild:read_file", "GrokBuild:grep"],
            vec!["Frobnicate"]
        ),
        (ids(&listed), listed.unknown)
    );
}

#[test]
fn known_tool_the_toolset_lacks_is_unmatched_not_unknown() {
    let toolset = [tool("GrokBuild:read_file", ToolKind::Read)];
    let known_tool_ids = HashSet::from(["GrokBuild:web_search".to_owned()]);
    let allowed = entries(&["read_file", "web_search"]);

    let listed = listed_tools(
        ToolList::Allowlist {
            known_tool_ids: &known_tool_ids,
        },
        &allowed,
        &toolset,
    );

    assert_eq!(
        (
            vec!["GrokBuild:read_file"],
            vec!["web_search"],
            Vec::<&str>::new()
        ),
        (ids(&listed), listed.unmatched, listed.unknown)
    );
}

#[test]
fn spawn_directive_keeps_the_task_tool_and_its_companions() {
    let toolset = [
        tool("GrokBuild:task", ToolKind::Task),
        tool("GrokBuild:get_task_output", ToolKind::BackgroundTaskAction),
        tool("GrokBuild:read_file", ToolKind::Read),
    ];
    let allowed = entries(&["Agent"]);

    let listed = listed_tools(
        ToolList::Allowlist {
            known_tool_ids: &HashSet::new(),
        },
        &allowed,
        &toolset,
    );

    assert_eq!(
        (
            vec!["GrokBuild:task", "GrokBuild:get_task_output"],
            vec!["Agent"]
        ),
        (ids(&listed), listed.spawn_directives)
    );
}

#[test]
fn spawn_directive_names_its_subagent_types_lowercased() {
    for (entry, expected) in [
        ("Agent", Vec::<&str>::new()),
        ("Task()", Vec::new()),
        ("Agent( , )", Vec::new()),
        ("Agent(Explore, plan)", vec!["explore", "plan"]),
        ("read_file", Vec::new()),
    ] {
        assert_eq!(
            expected,
            subagent_types(entry).collect::<Vec<_>>(),
            "{entry}"
        );
    }
}

#[test]
fn shell_lsp_ask_and_task_tool_names_map() {
    assert_eq!(claude_tool_kind("PowerShell"), Some(ToolKind::Execute));
    assert_eq!(claude_tool_kind("LSP"), Some(ToolKind::Lsp));
    assert_eq!(claude_tool_kind("AskUserQuestion"), Some(ToolKind::AskUser));
    for name in ["TaskOutput", "BashOutputTool", "AgentOutputTool"] {
        assert_eq!(claude_tool_kind(name), Some(ToolKind::BackgroundTaskAction));
    }
    assert_eq!(claude_tool_kind("TaskStop"), Some(ToolKind::KillTaskAction));
    assert_eq!(claude_tool_kind("EnterPlanMode"), None);
    assert_eq!(claude_tool_kind("ExitPlanMode"), None);
}
