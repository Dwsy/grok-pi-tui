use std::collections::BTreeMap;
use std::path::Path;

use super::*;
use crate::McpServerTransportConfig;

#[test]
fn parse_mcp_servers_skips_unparseable_entries() {
    let root = toml(
        r#"
mcp_servers.broken = "not-a-table"

[mcp_servers.also_broken]
command = "echo"
enabled = "yes"

[mcp_servers.ok]
command = "echo"
args = ["hi"]
"#,
    );

    let servers = parse_mcp_servers_from_toml(&root);

    assert_eq!(vec!["ok"], servers.keys().collect::<Vec<_>>());
}

#[test]
fn parse_mcp_server_config_reports_unknown_fields() {
    let value = toml(
        r#"
command = "echo"
enabeld = false
"#,
    );

    let (config, unknown_fields) =
        deserialize_mcp_server_config(&value).expect("config with a command deserializes");

    assert!(config.enabled);
    assert_eq!(vec!["enabeld"], unknown_fields);
}

#[test]
fn parse_mcp_servers_drops_an_entry_with_no_transport() {
    let root = toml(
        r#"
[mcp_servers.github]
enabled = false

[mcp_servers.linear]
command = "npx"
args = ["-y", "mcp-remote", "https://mcp.linear.app/mcp"]
"#,
    );

    let ParsedMcpServers { servers, problems } = parse_mcp_servers_with_problems(&root);

    assert_eq!(vec!["linear"], servers.keys().collect::<Vec<_>>());
    let problem = problems
        .iter()
        .find(|p| p.server == "github")
        .expect("github problem reported");
    assert_eq!(McpServerProblemSeverity::Error, problem.severity);
    assert!(
        problem.message.contains("disabled_mcp_servers"),
        "{problem:?}"
    );
}

#[test]
fn parse_mcp_servers_rejects_blank_transport() {
    let root = toml(
        r#"
[mcp_servers.blank_url]
url = "  "

[mcp_servers.blank_cmd]
command = ""
"#,
    );

    let ParsedMcpServers { servers, problems } = parse_mcp_servers_with_problems(&root);

    assert!(servers.is_empty());
    assert_eq!(
        2,
        problems
            .iter()
            .filter(|p| p.severity == McpServerProblemSeverity::Error)
            .count(),
        "{problems:?}"
    );
}

#[test]
fn parse_mcp_servers_of_an_empty_config_is_empty() {
    assert!(parse_mcp_servers_from_toml(&toml("")).is_empty());
}

#[test]
fn parse_mcp_servers_reads_a_stdio_server() {
    let root = toml(
        r#"
[mcp_servers.test_server]
command = "node"
args = ["server.js"]
"#,
    );

    let servers = parse_mcp_servers_from_toml(&root);

    let config = servers.get("test_server").expect("test_server loads");
    assert!(config.enabled);
    let McpServerTransportConfig::Stdio { command, args, .. } = &config.transport else {
        panic!("expected a stdio transport: {config:?}");
    };
    assert_eq!("node", command);
    assert_eq!(&["server.js"], args.as_slice());
}

#[test]
fn parse_mcp_servers_reads_tool_timeouts() {
    let root = toml(
        r#"
[mcp_servers.github]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
tool_timeout_sec = 60
tool_timeouts = { create_issue = 120, search_repositories = 30 }
"#,
    );

    let servers = parse_mcp_servers_from_toml(&root);

    let github = servers.get("github").expect("github loads");
    assert_eq!(Some(60), github.tool_timeout_sec);
    assert_eq!(
        Some(HashMap::from([
            ("create_issue".to_owned(), 120),
            ("search_repositories".to_owned(), 30),
        ])),
        github.tool_timeouts
    );
}

#[test]
fn parse_mcp_servers_leaves_unset_timeouts_and_image_base64_unset() {
    let root = toml(
        r#"
[mcp_servers.filesystem]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem"]
"#,
    );

    let servers = parse_mcp_servers_from_toml(&root);

    let fs = servers.get("filesystem").expect("filesystem loads");
    assert_eq!(None, fs.tool_timeouts);
    assert_eq!(None, fs.tool_timeout_sec);
    assert_eq!(None, fs.expose_image_base64);
}

#[test]
fn parse_mcp_servers_reads_expose_image_base64() {
    let root = toml(
        r#"
[mcp_servers.grafana]
url = "https://grafana.example/mcp"
expose_image_base64 = true
"#,
    );

    let servers = parse_mcp_servers_from_toml(&root);

    assert_eq!(
        Some(true),
        servers
            .get("grafana")
            .expect("grafana loads")
            .expose_image_base64
    );
}

#[test]
fn project_definition_replaces_user_definition_entirely() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let project = write_project_config(
        dir.path(),
        r#"
[mcp_servers.linear]
command = "npx"
args = ["-y", "mcp-remote", "https://mcp.linear.app/mcp"]
"#,
    );

    let servers = toml_mcp_server_configs_from(&toml(USER_CONFIG_TOML), &[project]);

    let (linear, scope) = servers.get("linear").expect("linear loads");
    assert_eq!(MCP_SCOPE_PROJECT, *scope);
    assert_eq!(
        (None, None),
        (linear.startup_timeout_sec, linear.tool_timeout_sec)
    );
    assert_eq!(
        MCP_SCOPE_USER,
        servers.get("buildkite").expect("buildkite loads").1
    );
}

#[test]
fn project_definition_can_disable_a_user_server() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let project = write_project_config(
        dir.path(),
        r#"
[mcp_servers.linear]
command = "npx"
enabled = false
"#,
    );

    let servers = toml_mcp_server_configs_from(&toml(USER_CONFIG_TOML), &[project]);

    let enabled: BTreeMap<&str, bool> = servers
        .iter()
        .map(|(name, (config, _))| (name.as_str(), config.enabled))
        .collect();
    assert_eq!(
        BTreeMap::from([("buildkite", true), ("linear", false)]),
        enabled
    );
}

#[test]
fn nearer_project_file_wins_a_shared_name() {
    let root = tempfile::tempdir().expect("create temp dir");
    let nested = root.path().join("pkg");
    let farther = write_project_config(root.path(), "[mcp_servers.svc]\ncommand = \"far\"\n");
    let nearer = write_project_config(&nested, "[mcp_servers.svc]\ncommand = \"near\"\n");

    let servers = toml_mcp_server_configs_from(&toml(""), &[farther, nearer]);

    let McpServerTransportConfig::Stdio { command, .. } =
        &servers.get("svc").expect("svc loads").0.transport
    else {
        panic!("expected a stdio transport: {servers:?}");
    };
    assert_eq!("near", command);
}

#[test]
fn toml_server_names_include_disabled_project_servers() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let project = write_project_config(
        dir.path(),
        r#"
[mcp_servers.mcp-servers-test-enabled]
url = "https://example.com"

[mcp_servers.mcp-servers-test-disabled]
command = "/ignored"
enabled = false
"#,
    );

    let names = all_toml_mcp_server_names(&[project]);

    assert!(names.contains("mcp-servers-test-enabled"));
    assert!(names.contains("mcp-servers-test-disabled"));
}

fn toml(source: &str) -> toml::Value {
    toml::from_str(source).expect("test TOML parses")
}

fn write_project_config(dir: &Path, source: &str) -> PathBuf {
    let path = dir.join(".grok").join("config.toml");
    std::fs::create_dir_all(path.parent().expect("config path has a parent"))
        .expect("create .grok dir");
    std::fs::write(&path, source).expect("write project config");
    path
}

const USER_CONFIG_TOML: &str = r#"
[mcp_servers.linear]
command = "npx"
args = ["-y", "mcp-remote", "https://mcp.linear.app/mcp"]
startup_timeout_sec = 10
tool_timeout_sec = 60

[mcp_servers.buildkite]
command = "npx"
args = ["-y", "mcp-remote", "https://mcp.buildkite.com/mcp"]
"#;
