use super::*;

#[test]
fn json_map_skips_bad_entry_and_keeps_the_rest() {
    let value = serde_json::json!({
        "mcpServers": {
            "bad": { "enabled": false },
            "good": { "command": "npx", "args": ["-y", "pkg"] }
        }
    });

    let config = mcp_config_from_json_value(&value);
    assert_eq!(vec!["good"], config.mcp_servers.keys().collect::<Vec<_>>());
}

#[test]
fn mcp_json_oauth_block_parsed_into_oauth_config() {
    let json = r#"{
        "mcpServers": {
            "slack": {
                "type": "http",
                "url": "https://mcp.slack.example/mcp",
                "oauth": { "clientId": "slack-byo-client", "callbackPort": 3118 }
            }
        }
    }"#;

    let config: McpConfig = serde_json::from_str(json).unwrap();
    let slack = config.mcp_servers.get("slack").unwrap();
    let oauth = slack.oauth_config().unwrap();
    assert_eq!(
        (Some("slack-byo-client"), Some(3118)),
        (oauth.client_id.as_deref(), oauth.callback_port)
    );
}

#[test]
fn read_mcp_json_of_a_missing_file_is_none() {
    let dir = tempfile::tempdir().unwrap();
    assert!(read_mcp_json(&dir.path().join("mcp.json")).is_none());
}

#[test]
fn claude_json_loads_user_and_project_servers() {
    let dir = tempfile::tempdir().unwrap();
    let claude_json = dir.path().join("claude.json");
    let cwd = dir.path().join("proj");
    let cwd_key = cwd.to_string_lossy().into_owned();
    let fixture = serde_json::json!({
        "mcpServers": {
            "killswitch-claude": { "command": "true" }
        },
        "projects": {
            cwd_key: {
                "mcpServers": {
                    "killswitch-claude-proj": { "command": "true" }
                }
            }
        }
    });
    std::fs::write(&claude_json, serde_json::to_string(&fixture).unwrap()).unwrap();

    let servers = load_claude_json_mcp_servers_from(
        &claude_json,
        &cwd,
        &crate::McpPreferencesFile::default(),
    );
    assert_eq!(
        vec!["killswitch-claude-proj", "killswitch-claude"],
        servers
            .iter()
            .map(crate::mcp_servers::mcp_server_name)
            .collect::<Vec<_>>()
    );
}

#[test]
fn claude_attribution_keeps_both_urls_for_one_name() {
    let dir = tempfile::tempdir().unwrap();
    let claude_json = dir.path().join("claude.json");
    let cwd = dir.path().join("proj");
    let cwd_key = cwd.to_string_lossy().into_owned();
    let fixture = serde_json::json!({
        "mcpServers": {
            "shared": { "url": "https://user.example/mcp" }
        },
        "projects": {
            cwd_key: {
                "mcpServers": {
                    "shared": { "url": "https://project.example/mcp" }
                }
            }
        }
    });
    std::fs::write(&claude_json, serde_json::to_string(&fixture).unwrap()).unwrap();

    let servers = load_claude_json_mcp_servers_from(
        &claude_json,
        &cwd,
        &crate::McpPreferencesFile::default(),
    );

    let urls: Vec<&str> = servers
        .iter()
        .filter_map(|server| match server {
            acp::McpServer::Http(http) => Some(http.url.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        vec!["https://project.example/mcp", "https://user.example/mcp"],
        urls
    );
}

#[test]
fn mcp_json_expands_an_unset_variable_to_its_default() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".mcp.json");
    std::fs::write(
        &path,
        r#"{"mcpServers": {"api": {"url": "${GROK_TEST_MCP_UNSET_VAR_12345:-https://fallback.example.com}/mcp"}}}"#,
    )
    .unwrap();

    let servers = load_mcp_json_file(&path);
    let [acp::McpServer::Http(acp::McpServerHttp { url, .. })] = servers.as_slice() else {
        panic!("expected one http server: {servers:?}");
    };
    assert_eq!("https://fallback.example.com/mcp", url);
}

#[test]
fn nearest_mcp_json_wins_a_shared_name() {
    let root = tempfile::tempdir().unwrap();
    let farther = root.path().join(".mcp.json");
    let nearer = root.path().join("pkg").join(".mcp.json");
    std::fs::create_dir_all(nearer.parent().unwrap()).unwrap();
    std::fs::write(
        &farther,
        r#"{"mcpServers": {"svc": {"command": "far"}, "far-only": {"command": "far"}}}"#,
    )
    .unwrap();
    std::fs::write(&nearer, r#"{"mcpServers": {"svc": {"command": "near"}}}"#).unwrap();

    let servers = load_mcp_json_servers(&[farther, nearer], ClaudeImport::NotImported);
    let commands: Vec<(&str, String)> = servers
        .iter()
        .map(|server| {
            let acp::McpServer::Stdio(stdio) = server else {
                panic!("expected stdio servers: {servers:?}");
            };
            (stdio.name.as_str(), stdio.command.display().to_string())
        })
        .collect();
    assert_eq!(
        vec![("svc", "near".to_owned()), ("far-only", "far".to_owned())],
        commands
    );
}
