use std::collections::BTreeMap;
use std::path::Path;

use super::*;
use crate::ClaudeImport;
use crate::compat::{CompatConfig, CompatVendor};
use crate::mcp_servers::{McpServerOAuth, McpServerScope, PluginMcpServers};

#[test]
fn stdio_server_carries_its_cwd_timeouts_and_disabled_tools() {
    let workspace = Workspace::new();
    let project_config = workspace.write(
        ".grok/config.toml",
        r#"
[mcp_servers.docs]
command = "docs-server"
args = ["--stdio"]
cwd = "/srv/docs"
env = { MODE = "read" }
startup_timeout_sec = 5
tool_timeout_sec = 90
tool_timeouts = { search = 30 }
"#,
    );

    let list = user_config("[disabled_mcp_tools]\ndocs = [\"delete\"]\n").server_list(
        &workspace.sources(vec![project_config.clone()], Vec::new()),
        &[],
        Vec::new(),
    );

    assert_eq!(
        vec![McpServerDetails {
            name: "docs".to_owned(),
            transport: McpServerTransport::Stdio {
                command: PathBuf::from("docs-server"),
                args: vec!["--stdio".to_owned()],
                env: vec![("MODE".to_owned(), "read".to_owned())],
                cwd: Some(PathBuf::from("/srv/docs")),
            },
            source: McpServerSource::ProjectConfig {
                path: project_config
            },
            timeouts: McpServerTimeouts {
                startup_timeout_sec: Some(5),
                tool_timeout_sec: Some(90),
                tool_timeouts: BTreeMap::from([("search".to_owned(), 30)]),
            },
            disabled_tools: BTreeSet::from(["delete".to_owned()]),
            oauth: None,
        }],
        list.servers
    );
}

#[test]
fn url_server_is_sse_by_type_or_path_and_http_otherwise() {
    let headers = vec![("X-Team".to_owned(), "grok".to_owned())];
    let cases = [
        (
            "url = \"https://docs.example.test/mcp\"",
            McpServerTransport::Http {
                url: "https://docs.example.test/mcp".to_owned(),
                headers: headers.clone(),
                bearer_token_file: None,
            },
        ),
        (
            "url = \"https://docs.example.test/mcp\"\ntype = \"sse\"",
            McpServerTransport::Sse {
                url: "https://docs.example.test/mcp".to_owned(),
                headers: headers.clone(),
                bearer_token_file: None,
            },
        ),
        (
            "url = \"https://docs.example.test/sse\"",
            McpServerTransport::Sse {
                url: "https://docs.example.test/sse".to_owned(),
                headers: headers.clone(),
                bearer_token_file: None,
            },
        ),
    ];
    for (entry, expected) in cases {
        let workspace = Workspace::new();
        let list = user_config(&format!(
            "[mcp_servers.docs]\n{entry}\nheaders = {{ X-Team = \"grok\" }}\n"
        ))
        .server_list(&workspace.sources(Vec::new(), Vec::new()), &[], Vec::new());

        assert_eq!(
            vec![expected],
            list.servers
                .into_iter()
                .map(|server| server.transport)
                .collect::<Vec<_>>(),
            "{entry}"
        );
    }
}

#[test]
fn each_server_names_the_file_that_defined_it() {
    let workspace = Workspace::new();
    let root_mcp_json = workspace.write(
        ".mcp.json",
        r#"{"mcpServers": {"from-root": {"command": "a"}}}"#,
    );
    let pkg_mcp_json = workspace.write(
        "pkg/.mcp.json",
        r#"{"mcpServers": {"from-pkg": {"command": "b"}}}"#,
    );
    let cursor_mcp_json = workspace.write(
        "pkg/.cursor/mcp.json",
        r#"{"mcpServers": {"from-cursor": {"command": "c"}}}"#,
    );
    let project_config = workspace.write(
        "pkg/.grok/config.toml",
        "[mcp_servers.from-project]\ncommand = \"d\"\n",
    );
    let mut sources = workspace.sources(
        vec![project_config.clone()],
        vec![root_mcp_json.clone(), pkg_mcp_json.clone()],
    );
    sources.cwd = workspace.path().join("pkg");
    sources.compat.cursor.mcps = true;

    let list = user_config("").server_list(&sources, &[], Vec::new());

    let mut sources_by_name: Vec<(String, McpServerSource)> = list
        .servers
        .into_iter()
        .filter(|server| server.name.starts_with("from-"))
        .map(|server| (server.name, server.source))
        .collect();
    sources_by_name.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        vec![
            (
                "from-cursor".to_owned(),
                McpServerSource::CursorProject {
                    path: cursor_mcp_json
                }
            ),
            (
                "from-pkg".to_owned(),
                McpServerSource::McpJson { path: pkg_mcp_json }
            ),
            (
                "from-project".to_owned(),
                McpServerSource::ProjectConfig {
                    path: project_config
                }
            ),
            (
                "from-root".to_owned(),
                McpServerSource::McpJson {
                    path: root_mcp_json
                }
            ),
        ],
        sources_by_name
    );
}

#[test]
fn servers_left_out_name_their_reason() {
    let one_field_setup = "[mcp_servers.pick.setup]\nfields = [{ id = \"region\", label = \"Region\", type = \"select\", options = [{ label = \"US\", value = \"us\" }] }]\n";
    let two_field_setup = "[mcp_servers.pick.setup]\nfields = [{ id = \"region\", label = \"Region\", type = \"select\", options = [{ label = \"US\", value = \"us\" }] }, { id = \"tier\", label = \"Tier\", type = \"select\", options = [{ label = \"Free\", value = \"free\" }] }]\n";
    let cases = [
        (
            "[mcp_servers.off]\ncommand = \"off-server\"\nenabled = false\n".to_owned(),
            Vec::new(),
            vec![skipped(
                "off",
                user_source(),
                McpServerSkipReason::DisabledInConfig,
            )],
        ),
        (
            "disabled_mcp_servers = [\"listed\"]\n[mcp_servers.listed]\ncommand = \"listed-server\"\n"
                .to_owned(),
            Vec::new(),
            vec![skipped(
                "listed",
                user_source(),
                McpServerSkipReason::ListedInDisabledMcpServers,
            )],
        ),
        (
            format!("[mcp_servers.pick]\ncommand = \"pick-server\"\n{one_field_setup}"),
            Vec::new(),
            vec![skipped(
                "pick",
                user_source(),
                McpServerSkipReason::SetupRequired,
            )],
        ),
        (
            format!("[mcp_servers.pick]\ncommand = \"pick-server\"\n{two_field_setup}"),
            Vec::new(),
            vec![skipped(
                "pick",
                user_source(),
                McpServerSkipReason::InvalidSetup {
                    reason: "setup schema must declare exactly one select field (v0)".to_owned(),
                },
            )],
        ),
        (
            "[mcp_servers.off]\ncommand = \"off-server\"\nenabled = false\n".to_owned(),
            vec![client_stdio("off")],
            vec![
                skipped(
                    "off",
                    user_source(),
                    McpServerSkipReason::DisabledInConfig,
                ),
                skipped(
                    "off",
                    McpServerSource::Client,
                    McpServerSkipReason::DisabledInConfig,
                ),
            ],
        ),
        (
            "disabled_mcp_servers = [\"listed\"]\n".to_owned(),
            vec![client_stdio("listed")],
            vec![skipped(
                "listed",
                McpServerSource::Client,
                McpServerSkipReason::ListedInDisabledMcpServers,
            )],
        ),
    ];
    for (user_toml, client_servers, expected) in cases {
        let workspace = Workspace::new();
        let list = user_config(&user_toml).server_list(
            &workspace.sources(Vec::new(), Vec::new()),
            &[],
            client_servers,
        );

        assert_eq!(
            (Vec::<McpServerDetails>::new(), expected),
            (list.servers, list.skipped),
            "{user_toml}"
        );
    }
}

#[test]
fn client_server_matching_a_disabled_vendor_config_is_skipped_for_that_vendor() {
    let workspace = Workspace::new();
    workspace.write(
        ".cursor/mcp.json",
        r#"{"mcpServers": {"disk-name": {"url": "https://killswitch.example.test/mcp/"}}}"#,
    );

    let list = user_config("").server_list(
        &workspace.sources(Vec::new(), Vec::new()),
        &[],
        vec![client_http(
            "forwarded",
            "https://killswitch.example.test/mcp",
        )],
    );

    assert_eq!(
        (
            Vec::<McpServerDetails>::new(),
            vec![skipped(
                "forwarded",
                McpServerSource::Client,
                McpServerSkipReason::VendorMcpsOff {
                    vendor: CompatVendor::Cursor
                },
            )]
        ),
        (list.servers, list.skipped)
    );
}

#[test]
fn client_keeps_oauth_and_timeouts_when_the_toml_server_is_skipped() {
    let workspace = Workspace::new();
    let setup = "[mcp_servers.linear.setup]\nfields = [{ id = \"region\", label = \"Region\", type = \"select\", options = [{ label = \"US\", value = \"us\" }] }]\n";
    let cases = [
        (
            McpServerSkipReason::SetupRequired,
            format!(
                "[mcp_servers.linear]\nurl = \"https://linear.example.test/mcp\"\noauth_client_id = \"grok-client\"\noauth_client_secret_env_var = \"LINEAR_CLIENT_SECRET\"\ntool_timeout_sec = 600\n{setup}"
            ),
        ),
        (
            McpServerSkipReason::InvalidSetup {
                reason: "setup schema must declare exactly one select field (v0)".to_owned(),
            },
            "[mcp_servers.linear]\nurl = \"https://linear.example.test/mcp\"\noauth_client_id = \"grok-client\"\noauth_client_secret_env_var = \"LINEAR_CLIENT_SECRET\"\ntool_timeout_sec = 600\n[mcp_servers.linear.setup]\nfields = [{ id = \"region\", label = \"Region\", type = \"select\", options = [{ label = \"US\", value = \"us\" }] }, { id = \"tier\", label = \"Tier\", type = \"select\", options = [{ label = \"Free\", value = \"free\" }] }]\n".to_owned(),
        ),
    ];
    for (reason, toml) in cases {
        let list = user_config(&toml).server_list(
            &workspace.sources(Vec::new(), Vec::new()),
            &[],
            vec![client_http("linear", "https://client.example.test/mcp")],
        );

        assert_eq!(
            (
                vec![McpServerDetails {
                    name: "linear".to_owned(),
                    transport: McpServerTransport::Http {
                        url: "https://client.example.test/mcp".to_owned(),
                        headers: Vec::new(),
                        bearer_token_file: None,
                    },
                    source: McpServerSource::Client,
                    timeouts: McpServerTimeouts {
                        startup_timeout_sec: None,
                        tool_timeout_sec: Some(600),
                        tool_timeouts: BTreeMap::new(),
                    },
                    disabled_tools: BTreeSet::new(),
                    oauth: Some(McpServerOAuth {
                        client_id: "grok-client".to_owned(),
                        client_secret_env_var: Some("LINEAR_CLIENT_SECRET".to_owned()),
                        scopes: None,
                        callback_port: None,
                    }),
                }],
                vec![skipped("linear", user_source(), reason)],
            ),
            (list.servers, list.skipped)
        );
    }
}

#[test]
fn client_server_replaces_the_configured_server_and_keeps_its_oauth_and_timeouts() {
    let workspace = Workspace::new();
    let user = user_config(
        r#"
[mcp_servers.linear]
url = "https://linear.example.test/mcp"
oauth_client_id = "grok-client"
oauth_client_secret_env_var = "LINEAR_CLIENT_SECRET"
tool_timeout_sec = 600
"#,
    );

    let list = user.server_list(
        &workspace.sources(Vec::new(), Vec::new()),
        &[],
        vec![client_http("linear", "https://client.example.test/mcp")],
    );

    assert_eq!(
        vec![McpServerDetails {
            name: "linear".to_owned(),
            transport: McpServerTransport::Http {
                url: "https://client.example.test/mcp".to_owned(),
                headers: Vec::new(),
                bearer_token_file: None,
            },
            source: McpServerSource::Client,
            timeouts: McpServerTimeouts {
                startup_timeout_sec: None,
                tool_timeout_sec: Some(600),
                tool_timeouts: BTreeMap::new(),
            },
            disabled_tools: BTreeSet::new(),
            oauth: Some(McpServerOAuth {
                client_id: "grok-client".to_owned(),
                client_secret_env_var: Some("LINEAR_CLIENT_SECRET".to_owned()),
                scopes: None,
                callback_port: None,
            }),
        }],
        list.servers
    );
}

#[test]
fn unparseable_entry_is_skipped_and_leaves_its_name_to_lower_sources() {
    let workspace = Workspace::new();
    let mcp_json = workspace.write(
        ".mcp.json",
        r#"{"mcpServers": {"svc": {"command": "json-server"}, "bad": {"enabled": false}}}"#,
    );

    let list = user_config("[mcp_servers.svc]\nenabled = true\n").server_list(
        &workspace.sources(Vec::new(), vec![mcp_json.clone()]),
        &[],
        Vec::new(),
    );

    assert_eq!(
        (
            vec![(
                "svc".to_owned(),
                McpServerSource::McpJson {
                    path: mcp_json.clone()
                }
            )],
            vec![
                ("svc".to_owned(), user_source(), true),
                (
                    "bad".to_owned(),
                    McpServerSource::McpJson { path: mcp_json },
                    true
                ),
            ]
        ),
        (
            list.servers
                .into_iter()
                .map(|server| (server.name, server.source))
                .collect::<Vec<_>>(),
            list.skipped
                .into_iter()
                .map(|skipped| (
                    skipped.name,
                    skipped.source,
                    matches!(skipped.reason, McpServerSkipReason::InvalidEntry { .. })
                ))
                .collect::<Vec<_>>()
        )
    );
}

#[test]
fn definition_hidden_by_a_config_toml_name_is_not_listed_as_skipped() {
    let workspace = Workspace::new();
    let mcp_json = workspace.write(
        ".mcp.json",
        r#"{"mcpServers": {"svc": {"command": "json-server", "enabled": false}}}"#,
    );

    let list = user_config("[mcp_servers.svc]\ncommand = \"toml-server\"\nenabled = false\n")
        .server_list(
            &workspace.sources(Vec::new(), vec![mcp_json]),
            &[],
            Vec::new(),
        );

    assert_eq!(
        vec![skipped(
            "svc",
            user_source(),
            McpServerSkipReason::DisabledInConfig
        )],
        list.skipped
    );
}

#[test]
fn a_skipped_plugin_file_name_does_not_stay_skipped_when_inline_defines_it() {
    let workspace = Workspace::new();
    let root = workspace.path().join("acme");
    std::fs::create_dir_all(&root).expect("plugin root");
    let file = root.join(".mcp.json");
    let cases = [
        r#"{"mcpServers":{"sentry":{"type":"http","url":"https://file.example/mcp","enabled":false}}}"#,
        r#"{"mcpServers":{"sentry":{"type":"http"}}}"#,
    ];
    for file_json in cases {
        std::fs::write(&file, file_json).expect("plugin mcp.json");
        let plugin = PluginMcpServers {
            plugin_name: "acme".to_owned(),
            root: root.clone(),
            data_dir: workspace.path().join("acme-data"),
            scope: McpServerScope::Project,
            mcp_config_path: Some(file.clone()),
            inline_mcp_servers: Some(serde_json::json!({
                "sentry": { "type": "http", "url": "https://inline.example/mcp" }
            })),
        };

        let list = user_config("").server_list(
            &workspace.sources(Vec::new(), Vec::new()),
            &[plugin],
            Vec::new(),
        );

        let servers = list
            .servers
            .into_iter()
            .filter(|server| server.name == "sentry")
            .collect::<Vec<_>>();
        let skipped = list
            .skipped
            .into_iter()
            .filter(|skipped| skipped.name == "sentry")
            .collect::<Vec<_>>();
        assert_eq!(
            (
                vec![McpServerDetails {
                    name: "sentry".to_owned(),
                    transport: McpServerTransport::Http {
                        url: "https://inline.example/mcp".to_owned(),
                        headers: Vec::new(),
                        bearer_token_file: None,
                    },
                    source: McpServerSource::Plugin {
                        plugin_name: "acme".to_owned(),
                        path: root.clone(),
                        scope: McpServerScope::Project,
                    },
                    timeouts: McpServerTimeouts::default(),
                    disabled_tools: BTreeSet::new(),
                    oauth: None,
                }],
                Vec::<SkippedMcpServer>::new(),
            ),
            (servers, skipped),
            "{file_json}"
        );
    }
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

    fn sources(
        &self,
        project_configs: Vec<PathBuf>,
        mcp_json_files: Vec<PathBuf>,
    ) -> McpServerSources {
        let mut compat = CompatConfig::default();
        compat.claude.mcps = false;
        compat.cursor.mcps = false;
        McpServerSources {
            cwd: self.path().to_path_buf(),
            project_configs,
            mcp_json_files,
            compat,
            claude_import: ClaudeImport::NotImported,
        }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }
}

fn user_config(effective_config: &str) -> UserMcpConfig {
    UserMcpConfig {
        effective_config: toml::from_str(effective_config).expect("user config parses"),
        preferences: McpPreferencesFile::default(),
    }
}

fn user_source() -> McpServerSource {
    McpServerSource::UserConfig {
        path: crate::grok_home().join(crate::USER_CONFIG_FILENAME),
    }
}

fn skipped(name: &str, source: McpServerSource, reason: McpServerSkipReason) -> SkippedMcpServer {
    SkippedMcpServer {
        name: name.to_owned(),
        source,
        reason,
    }
}

fn client_stdio(name: &str) -> acp::McpServer {
    acp::McpServer::Stdio(acp::McpServerStdio::new(name, "client-server"))
}

fn client_http(name: &str, url: &str) -> acp::McpServer {
    acp::McpServer::Http(acp::McpServerHttp::new(name, url))
}
