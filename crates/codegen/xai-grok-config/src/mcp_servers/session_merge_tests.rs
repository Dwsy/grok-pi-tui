use std::path::PathBuf;

use agent_client_protocol as acp;

use super::{
    SessionMcpServer, SessionMcpTier, canonicalize_mcp_maps, mcp_server_definitions_equal,
};
use crate::compat::CompatConfig;
use crate::mcp_servers::server_list::UserMcpConfig;
use crate::mcp_servers::{
    McpServerOrigin, McpServerScope, McpServerSources, PluginMcpServers,
    admit_client_mcp_servers_at, mcp_server_name,
};
use crate::{ClaudeImport, McpPreferencesFile};

#[test]
fn client_list_replaces_only_a_different_definition_and_keeps_a_matching_source() {
    let workspace = Workspace::new();
    let project = workspace.write(
        ".grok/config.toml",
        "[mcp_servers.corp]\ncommand = \"echo\"\nargs = [\"ok\"]\n\n\
         [mcp_servers.corp-redefined]\ncommand = \"echo\"\nargs = [\"old\"]\n",
    );
    let plugin = PluginMcpServers {
        plugin_name: "native-plugin".to_owned(),
        root: workspace.path().join("native-plugin"),
        data_dir: workspace.path().join("native-plugin-data"),
        scope: McpServerScope::User,
        mcp_config_path: None,
        inline_mcp_servers: Some(serde_json::json!({
            "plugin-corp": { "command": "echo", "args": ["ok"] },
            "plugin-same": { "command": "echo", "args": ["ok"] }
        })),
    };
    let sources = workspace.sources(vec![project]);

    let list = user_config().session_servers_at(
        &sources,
        &[plugin],
        vec![
            stdio("corp", "echo", &["ok"]),
            stdio("corp-redefined", "echo", &["ok"]),
            stdio("plugin-corp", "other", &["no"]),
            stdio("plugin-same", "echo", &["ok"]),
            stdio("client-only", "true", &[]),
        ],
        Some(workspace.path()),
    );

    assert_eq!(
        vec![
            ("client-only", SessionMcpTier::Foreign),
            ("corp", SessionMcpTier::Native),
            ("corp-redefined", SessionMcpTier::Foreign),
            ("plugin-corp", SessionMcpTier::Foreign),
            ("plugin-same", SessionMcpTier::Native),
        ],
        list.iter()
            .map(|entry| (mcp_server_name(&entry.server), entry.tier))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        ("echo".to_owned(), vec!["ok".to_owned()]),
        stdio_command(server_named(&list, "corp-redefined"))
    );
    assert_eq!("other", stdio_command(server_named(&list, "plugin-corp")).0);
}

#[test]
fn vendor_mcps_off_skips_the_matching_client_server_and_keeps_the_toml_definition() {
    let workspace = Workspace::new();
    workspace.write(
        ".cursor/mcp.json",
        r#"{"mcpServers": {"killswitch-cache": {"command": "true"}}}"#,
    );
    let project = workspace.write(
        ".grok/config.toml",
        "[mcp_servers.killswitch-cache]\ncommand = \"echo\"\nargs = [\"ok\"]\n",
    );
    let mut sources = workspace.sources(vec![project]);

    let cases = [
        (
            false,
            vec![
                stdio("killswitch-cache", "true", &[]),
                stdio("client-only", "true", &[]),
            ],
            vec!["client-only".to_owned(), "killswitch-cache".to_owned()],
            ("echo".to_owned(), vec!["ok".to_owned()]),
            SessionMcpTier::Native,
        ),
        (
            true,
            vec![stdio("killswitch-cache", "true", &[])],
            vec!["killswitch-cache".to_owned()],
            ("true".to_owned(), Vec::new()),
            SessionMcpTier::Foreign,
        ),
    ];

    for (mcps_on, clients, names, expected_stdio, expected_tier) in cases {
        sources.compat.cursor.mcps = mcps_on;

        let list = user_config().session_servers_at(&sources, &[], clients, Some(workspace.path()));

        assert_eq!(
            names,
            server_names_among(&list, &["killswitch-cache", "client-only"])
        );
        let kept = server_named(&list, "killswitch-cache");
        assert_eq!(expected_stdio, stdio_command(kept));
        assert_eq!(expected_tier, kept.tier);
    }
}

#[test]
fn session_tier_is_native_only_for_config_toml_and_plugin_origins() {
    let cases = [
        (
            SessionMcpTier::Native,
            McpServerOrigin::ConfigToml {
                path: PathBuf::from("/u/.grok/config.toml"),
            },
        ),
        (
            SessionMcpTier::Native,
            McpServerOrigin::Plugin {
                plugin_name: "p".to_owned(),
                path: PathBuf::from("/p/.mcp.json"),
            },
        ),
        (
            SessionMcpTier::Foreign,
            McpServerOrigin::ClaudeJson {
                path: PathBuf::from("/u/.claude.json"),
            },
        ),
        (
            SessionMcpTier::Foreign,
            McpServerOrigin::McpJson {
                path: PathBuf::from("/repo/.mcp.json"),
            },
        ),
    ];

    for (expected, origin) in cases {
        assert_eq!(expected, SessionMcpTier::from(&origin), "{origin:?}");
    }
}

#[test]
fn normalized_vendor_url_skips_a_differently_named_client_server() {
    let workspace = Workspace::new();
    workspace.write(
        ".cursor/mcp.json",
        r#"{"mcpServers": {"disk-name": {"url": "https://killswitch.example.test/mcp/"}}}"#,
    );
    let sources = workspace.sources(Vec::new());

    let list = user_config().session_servers_at(
        &sources,
        &[],
        vec![
            http("killswitch-http", "https://killswitch.example.test/mcp"),
            http("other-http", "https://other.example.test/mcp"),
        ],
        Some(workspace.path()),
    );

    assert_eq!(
        vec!["other-http".to_owned()],
        server_names_among(&list, &["other-http", "killswitch-http"])
    );
}

#[test]
fn a_client_server_dropped_at_admission_stays_out_after_its_vendor_file_is_removed() {
    let workspace = Workspace::new();
    let vendor_file = workspace.write(
        ".cursor/mcp.json",
        r#"{"mcpServers": {"killswitch-cache": {"command": "true"}}}"#,
    );
    let sources = workspace.sources(Vec::new());
    let admitted = admit_client_mcp_servers_at(
        vec![stdio("killswitch-cache", "true", &[])],
        &sources.cwd,
        &sources.compat,
        Some(workspace.path()),
        &McpPreferencesFile::default(),
    );
    std::fs::remove_file(vendor_file).expect("remove vendor file");

    let list = user_config().session_servers_at(&sources, &[], admitted, Some(workspace.path()));

    assert_eq!(Vec::<SessionMcpServer>::new(), list);
}

#[test]
fn definition_equality_ignores_env_and_header_order_but_not_args() {
    let cases = [
        (
            true,
            stdio_with_env(&["a", "b"], &[("A", "1"), ("B", "2")]),
            stdio_with_env(&["a", "b"], &[("B", "2"), ("A", "1")]),
        ),
        (
            true,
            http_with_headers(&[("X-A", "1"), ("X-B", "2")]),
            http_with_headers(&[("X-B", "2"), ("X-A", "1")]),
        ),
        (
            false,
            stdio_with_env(&["a", "b"], &[]),
            stdio_with_env(&["b", "a"], &[]),
        ),
        (
            false,
            stdio_with_env(&[], &[("A", "1")]),
            stdio_with_env(&[], &[("A", "2")]),
        ),
    ];

    for (equal, left, right) in cases {
        assert_eq!(equal, mcp_server_definitions_equal(&left, &right));
    }
}

#[test]
fn canonicalize_mcp_maps_sorts_env_by_name_then_value() {
    let mut server = stdio_with_env(&[], &[("B", "2"), ("A", "1")]);

    canonicalize_mcp_maps(&mut server);

    let acp::McpServer::Stdio(stdio) = server else {
        panic!("stdio");
    };
    assert_eq!(
        vec![
            ("A".to_owned(), "1".to_owned()),
            ("B".to_owned(), "2".to_owned())
        ],
        stdio
            .env
            .iter()
            .map(|variable| (variable.name.clone(), variable.value.clone()))
            .collect::<Vec<_>>()
    );
}

struct Workspace {
    dir: tempfile::TempDir,
}

impl Workspace {
    fn new() -> Workspace {
        Workspace {
            dir: tempfile::tempdir().expect("create workspace tempdir"),
        }
    }

    fn write(&self, relative: &str, contents: &str) -> PathBuf {
        let path = self.path().join(relative);
        std::fs::create_dir_all(path.parent().expect("workspace file has a parent"))
            .expect("create workspace file parent");
        std::fs::write(&path, contents).expect("write workspace file");
        path
    }

    fn sources(&self, project_configs: Vec<PathBuf>) -> McpServerSources {
        let mut compat = CompatConfig::default();
        compat.claude.mcps = false;
        compat.cursor.mcps = false;
        McpServerSources {
            cwd: self.path().to_path_buf(),
            project_configs,
            mcp_json_files: Vec::new(),
            compat,
            claude_import: ClaudeImport::NotImported,
        }
    }

    fn path(&self) -> &std::path::Path {
        self.dir.path()
    }
}

fn user_config() -> UserMcpConfig {
    UserMcpConfig {
        effective_config: toml::Value::Table(toml::map::Map::new()),
        preferences: McpPreferencesFile::default(),
    }
}

fn stdio(name: &str, command: &str, args: &[&str]) -> acp::McpServer {
    acp::McpServer::Stdio(
        acp::McpServerStdio::new(name, command)
            .args(args.iter().map(|arg| (*arg).to_owned()).collect()),
    )
}

fn http(name: &str, url: &str) -> acp::McpServer {
    acp::McpServer::Http(acp::McpServerHttp::new(name, url))
}

fn stdio_with_env(args: &[&str], env: &[(&str, &str)]) -> acp::McpServer {
    acp::McpServer::Stdio(
        acp::McpServerStdio::new("s", "cmd")
            .args(args.iter().map(|arg| (*arg).to_owned()).collect())
            .env(
                env.iter()
                    .map(|&(name, value)| acp::EnvVariable::new(name, value))
                    .collect(),
            ),
    )
}

fn http_with_headers(headers: &[(&str, &str)]) -> acp::McpServer {
    acp::McpServer::Http(
        acp::McpServerHttp::new("s", "https://x.example/mcp").headers(
            headers
                .iter()
                .map(|&(name, value)| acp::HttpHeader::new(name, value))
                .collect(),
        ),
    )
}

fn stdio_command(server: &SessionMcpServer) -> (String, Vec<String>) {
    match &server.server {
        acp::McpServer::Stdio(stdio) => (stdio.command.display().to_string(), stdio.args.clone()),
        other => panic!("expected stdio, got {other:?}"),
    }
}

fn server_named<'a>(list: &'a [SessionMcpServer], name: &str) -> &'a SessionMcpServer {
    list.iter()
        .find(|server| mcp_server_name(&server.server) == name)
        .unwrap_or_else(|| panic!("{name} missing"))
}

fn server_names_among<'a>(list: &'a [SessionMcpServer], kept: &[&str]) -> Vec<&'a str> {
    list.iter()
        .map(|server| mcp_server_name(&server.server))
        .filter(|name| kept.contains(name))
        .collect()
}
