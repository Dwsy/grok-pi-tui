use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use agent_client_protocol as acp;

use crate::compat::CompatVendor;
use crate::mcp_bearer_token_file::bearer_token_file_meta;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerDetails {
    pub name: String,
    pub transport: McpServerTransport,
    pub source: McpServerSource,
    /// Only a `config.toml` entry with the same name sets these timeouts.
    pub timeouts: McpServerTimeouts,
    pub disabled_tools: BTreeSet<String>,
    pub oauth: Option<McpServerOAuth>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpServerTransport {
    Stdio {
        command: PathBuf,
        args: Vec<String>,
        env: Vec<(String, String)>,
        cwd: Option<PathBuf>,
    },
    Http {
        url: String,
        /// The `Authorization` header from `bearer_token_env_var` is the last entry.
        headers: Vec<(String, String)>,
        /// As configured; the MCP client parses it when the server starts.
        bearer_token_file: Option<String>,
    },
    Sse {
        url: String,
        headers: Vec<(String, String)>,
        bearer_token_file: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpServerSource {
    UserConfig {
        path: PathBuf,
    },
    ProjectConfig {
        path: PathBuf,
    },
    /// `scope` is the scope the plugin is installed at.
    Plugin {
        plugin_name: String,
        path: PathBuf,
        scope: McpServerScope,
    },
    ClaudeUser {
        path: PathBuf,
    },
    ClaudeProject {
        path: PathBuf,
    },
    CursorUser {
        path: PathBuf,
    },
    CursorProject {
        path: PathBuf,
    },
    McpJson {
        path: PathBuf,
    },
    Client,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpServerScope {
    User,
    Project,
}

impl McpServerSource {
    pub fn scope(&self) -> McpServerScope {
        match self {
            McpServerSource::UserConfig { .. }
            | McpServerSource::ClaudeUser { .. }
            | McpServerSource::CursorUser { .. }
            | McpServerSource::Client => McpServerScope::User,
            McpServerSource::ProjectConfig { .. }
            | McpServerSource::ClaudeProject { .. }
            | McpServerSource::CursorProject { .. }
            | McpServerSource::McpJson { .. } => McpServerScope::Project,
            McpServerSource::Plugin { scope, .. } => *scope,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpServerTimeouts {
    pub startup_timeout_sec: Option<u64>,
    pub tool_timeout_sec: Option<u64>,
    pub tool_timeouts: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerOAuth {
    pub client_id: String,
    pub client_secret_env_var: Option<String>,
    pub scopes: Option<Vec<String>>,
    pub callback_port: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedMcpServer {
    pub name: String,
    pub source: McpServerSource,
    pub reason: McpServerSkipReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpServerSkipReason {
    DisabledInConfig,
    ListedInDisabledMcpServers,
    VendorMcpsOff { vendor: CompatVendor },
    SetupRequired,
    InvalidSetup { reason: String },
    InvalidEntry { reason: String },
}

impl McpServerTransport {
    pub(crate) fn into_acp_mcp_server(self, name: String) -> acp::McpServer {
        let acp_headers = |headers: Vec<(String, String)>| {
            headers
                .into_iter()
                .map(|(header, value)| acp::HttpHeader::new(header, value))
                .collect()
        };

        match self {
            // The ACP stdio config has no cwd field
            McpServerTransport::Stdio {
                command,
                args,
                env,
                cwd: _,
            } => acp::McpServer::Stdio(
                acp::McpServerStdio::new(name, command).args(args).env(
                    env.into_iter()
                        .map(|(variable, value)| acp::EnvVariable::new(variable, value))
                        .collect(),
                ),
            ),
            McpServerTransport::Http {
                url,
                headers,
                bearer_token_file,
            } => acp::McpServer::Http(
                acp::McpServerHttp::new(name, url)
                    .headers(acp_headers(headers))
                    .meta(bearer_token_file.as_deref().map(bearer_token_file_meta)),
            ),
            McpServerTransport::Sse {
                url,
                headers,
                bearer_token_file,
            } => acp::McpServer::Sse(
                acp::McpServerSse::new(name, url)
                    .headers(acp_headers(headers))
                    .meta(bearer_token_file.as_deref().map(bearer_token_file_meta)),
            ),
        }
    }
}

impl From<McpServerDetails> for acp::McpServer {
    fn from(server: McpServerDetails) -> acp::McpServer {
        server.transport.into_acp_mcp_server(server.name)
    }
}
