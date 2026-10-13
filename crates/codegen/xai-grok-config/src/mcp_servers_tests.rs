use super::*;

#[test]
fn each_name_comes_from_its_highest_precedence_source() {
    let workspace = Workspace::new();
    let project_config = workspace.write(
        ".grok/config.toml",
        "[mcp_servers.mcp-servers-test-shared]\ncommand = \"toml\"\n",
    );
    let mcp_json = workspace.write(
        ".mcp.json",
        r#"{"mcpServers": {
            "mcp-servers-test-shared": {"command": "mcp-json"},
            "mcp-servers-test-plugin": {"command": "mcp-json"},
            "mcp-servers-test-mcp-json": {"command": "mcp-json"}
        }}"#,
    );
    let plugin = workspace.plugin(&[
        r#"{"mcpServers": {
            "mcp-servers-test-shared": {"command": "plugin"},
            "mcp-servers-test-plugin": {"command": "plugin"}
        }}"#,
        r#"{"mcpServers": {"mcp-servers-test-plugin": {"command": "plugin-duplicate"}}}"#,
    ]);

    let resolved = resolved_commands(
        &workspace.sources(vec![project_config], vec![mcp_json.clone()]),
        &[plugin],
    );

    assert_eq!(
        vec![
            (
                "mcp-json".to_owned(),
                "mcp-json".to_owned(),
                McpServerOrigin::McpJson { path: mcp_json }
            ),
            (
                "plugin".to_owned(),
                "plugin".to_owned(),
                McpServerOrigin::Plugin {
                    plugin_name: "acme".to_owned(),
                    path: workspace.path().join("acme"),
                },
            ),
            (
                "shared".to_owned(),
                "toml".to_owned(),
                McpServerOrigin::ConfigToml {
                    path: crate::grok_home().join(crate::USER_CONFIG_FILENAME),
                },
            ),
        ],
        resolved
    );
}

#[test]
fn disabled_toml_definition_hides_lower_precedence_sources() {
    let workspace = Workspace::new();
    let project_config = workspace.write(
        ".grok/config.toml",
        "[mcp_servers.mcp-servers-test-hidden]\ncommand = \"toml\"\nenabled = false\n",
    );
    let mcp_json = workspace.write(
        ".mcp.json",
        r#"{"mcpServers": {"mcp-servers-test-hidden": {"command": "mcp-json"}}}"#,
    );
    let plugin = workspace
        .plugin(&[r#"{"mcpServers": {"mcp-servers-test-hidden": {"command": "plugin"}}}"#]);

    let resolved = resolved_commands(
        &workspace.sources(vec![project_config], vec![mcp_json]),
        &[plugin],
    );

    assert_eq!(Vec::<(String, String, McpServerOrigin)>::new(), resolved);
}

const SERVER_NAME_PREFIX: &str = "mcp-servers-test-";

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

    fn plugin(&self, configs: &[&str]) -> PluginMcpServers {
        let root = self.path().join("acme");
        let mcp_config_path = configs
            .first()
            .map(|config| self.write("acme/.mcp.json", config));
        let inline_mcp_servers = configs
            .get(1)
            .map(|config| serde_json::from_str(config).expect("plugin inline config parses"));
        PluginMcpServers {
            plugin_name: "acme".to_owned(),
            root,
            data_dir: self.path().join("acme-data"),
            scope: McpServerScope::User,
            mcp_config_path,
            inline_mcp_servers,
        }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }
}

fn resolved_commands(
    sources: &McpServerSources,
    plugins: &[PluginMcpServers],
) -> Vec<(String, String, McpServerOrigin)> {
    let mut resolved: Vec<(String, String, McpServerOrigin)> =
        load_mcp_servers_with_origin(sources, plugins)
            .into_iter()
            .filter_map(|(server, origin)| {
                let acp::McpServer::Stdio(stdio) = server else {
                    return None;
                };
                let name = stdio.name.strip_prefix(SERVER_NAME_PREFIX)?.to_owned();
                Some((name, stdio.command.display().to_string(), origin))
            })
            .collect();
    resolved.sort_by(|a, b| a.0.cmp(&b.0));
    resolved
}

#[test]
fn plugin_inline_stdio_expands_plugin_root() {
    let workspace = Workspace::new();
    let root = workspace.path().join("team-tool");
    let plugin = PluginMcpServers {
        plugin_name: "team-tool".to_owned(),
        root: root.clone(),
        data_dir: workspace.path().join("team-tool-data"),
        scope: McpServerScope::User,
        mcp_config_path: None,
        inline_mcp_servers: Some(serde_json::json!({
            "mcpServers": {
                "echo-mcp": {
                    "command": "python3",
                    "args": ["${GROK_PLUGIN_ROOT}/mcp-echo-server.py"]
                }
            }
        })),
    };

    let servers = load_mcp_servers_with_origin(&workspace.sources(vec![], vec![]), &[plugin]);

    let stdio = servers.iter().find_map(|(server, _)| match server {
        acp::McpServer::Stdio(stdio) if stdio.name == "echo-mcp" => Some(stdio),
        _ => None,
    });
    let Some(stdio) = stdio else {
        panic!("expected a stdio server");
    };
    assert_eq!(
        vec![root.join("mcp-echo-server.py").display().to_string()],
        stdio.args
    );
}

#[test]
fn plugin_inline_direct_map_loads_http_server() {
    let workspace = Workspace::new();
    let plugin = PluginMcpServers {
        plugin_name: "sentry".to_owned(),
        root: workspace.path().join("sentry"),
        data_dir: workspace.path().join("sentry-data"),
        scope: McpServerScope::User,
        mcp_config_path: None,
        inline_mcp_servers: Some(serde_json::json!({
            "sentry": { "type": "http", "url": "https://mcp.sentry.dev/mcp" }
        })),
    };

    let servers = load_mcp_servers_with_origin(&workspace.sources(vec![], vec![]), &[plugin]);

    let http = servers.iter().find_map(|(server, _)| match server {
        acp::McpServer::Http(http) if http.name == "sentry" => Some(http),
        _ => None,
    });
    let Some(http) = http else {
        panic!("expected an http server");
    };
    assert_eq!("https://mcp.sentry.dev/mcp", http.url);
}
