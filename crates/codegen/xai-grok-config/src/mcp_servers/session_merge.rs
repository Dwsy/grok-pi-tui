//! Merges configured and client MCP servers into one session list keyed by name alone.
//! Clients are re-admitted here, and a client keeps a configured tier only with an identical
//! definition, else it is foreign. Name order keeps a no-op reload equal, so MCP is not restarted.

use std::collections::HashMap;
use std::path::Path;

use agent_client_protocol as acp;

use crate::mcp_servers::server_list::UserMcpConfig;
use crate::mcp_servers::{McpServerOrigin, McpServerSources, PluginMcpServers, mcp_server_name};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionMcpTier {
    Native,
    Foreign,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionMcpServer {
    pub server: acp::McpServer,
    pub tier: SessionMcpTier,
}

pub fn resolve_session_mcp_servers(
    sources: &McpServerSources,
    plugins: &[PluginMcpServers],
    client_servers: Vec<acp::McpServer>,
) -> Vec<SessionMcpServer> {
    UserMcpConfig::load().session_servers_at(
        sources,
        plugins,
        client_servers,
        xai_dirs::home_dir().as_deref(),
    )
}

impl UserMcpConfig {
    fn session_servers_at(
        &self,
        sources: &McpServerSources,
        plugins: &[PluginMcpServers],
        client_servers: Vec<acp::McpServer>,
        home: Option<&Path>,
    ) -> Vec<SessionMcpServer> {
        let mut by_name: HashMap<String, SessionMcpServer> = HashMap::new();
        for details in self.configured_servers(sources, plugins, home) {
            let origin = McpServerOrigin::from_source(details.source.clone(), &sources.cwd);
            let tier = SessionMcpTier::from(&origin);
            let server = acp::McpServer::from(details);
            by_name.insert(
                mcp_server_name(&server).to_owned(),
                SessionMcpServer { server, tier },
            );
        }

        for server in crate::mcp_servers::admit_client_mcp_servers_at(
            client_servers,
            &sources.cwd,
            &sources.compat,
            home,
            &self.preferences,
        ) {
            let name = mcp_server_name(&server);
            if let Some(existing) = by_name.get_mut(name)
                && mcp_server_definitions_equal(&existing.server, &server)
            {
                existing.server = server;
                continue;
            }
            by_name.insert(
                name.to_owned(),
                SessionMcpServer {
                    server,
                    tier: SessionMcpTier::Foreign,
                },
            );
        }

        let mut servers: Vec<SessionMcpServer> = by_name.into_values().collect();
        servers.sort_by(|a, b| mcp_server_name(&a.server).cmp(mcp_server_name(&b.server)));
        servers
    }
}

impl From<&McpServerOrigin> for SessionMcpTier {
    fn from(origin: &McpServerOrigin) -> SessionMcpTier {
        match origin {
            McpServerOrigin::ConfigToml { .. } | McpServerOrigin::Plugin { .. } => {
                SessionMcpTier::Native
            }
            McpServerOrigin::ClaudeJson { .. } | McpServerOrigin::McpJson { .. } => {
                SessionMcpTier::Foreign
            }
        }
    }
}

/// `env` and `headers` come from map iteration, so their order is ignored; `args` order is not.
pub fn mcp_server_definitions_equal(a: &acp::McpServer, b: &acp::McpServer) -> bool {
    let mut left = a.clone();
    let mut right = b.clone();
    canonicalize_mcp_maps(&mut left);
    canonicalize_mcp_maps(&mut right);
    left == right
}

pub fn canonicalize_mcp_maps(server: &mut acp::McpServer) {
    match server {
        acp::McpServer::Stdio(stdio) => stdio
            .env
            .sort_by(|a, b| (&a.name, &a.value).cmp(&(&b.name, &b.value))),
        acp::McpServer::Http(acp::McpServerHttp { headers, .. })
        | acp::McpServer::Sse(acp::McpServerSse { headers, .. }) => {
            headers.sort_by(|a, b| (&a.name, &a.value).cmp(&(&b.name, &b.value)))
        }
        // `McpServer` is #[non_exhaustive]; unknown transports have nothing to canonicalize.
        _ => {}
    }
}

#[cfg(test)]
#[path = "session_merge_tests.rs"]
mod tests;
