//! Managed connectors exist only via the gateway catalog (`GET /v1/mcp/tools/list`), not as injected `grok_com_*` HTTP servers.

pub use xai_grok_shell_session_support::managed_mcp::*;

use std::collections::HashMap;

use agent_client_protocol as acp;
use xai_grok_config::mcp_servers::{McpServerOrigin, SessionMcpTier};
use xai_grok_workspace::permission::resolution::{
    McpBlockReason, McpSubject, McpVerdict, PolicySubjectOrigin,
};

pub(crate) use xai_grok_config::mcp_servers::{admit_client_mcp_servers, mcp_server_name};

pub(crate) fn merge_managed_mcp_servers(
    client_mcp_servers: Vec<acp::McpServer>,
    cwd: &std::path::Path,
    plugin_registry: Option<&xai_grok_agent::plugins::PluginRegistry>,
    compat: &xai_grok_tools::types::compat::CompatConfig,
) -> Vec<acp::McpServer> {
    merge_managed_mcp_servers_with_policy(client_mcp_servers, cwd, plugin_registry, compat)
        .into_iter()
        .filter(|s| s.disabled_reason.is_none())
        .map(|s| s.server)
        .collect()
}

/// Returns `true` if the command was enqueued (session still alive).
pub(crate) fn merge_and_send_managed_mcp_update(
    cmd_tx: &tokio::sync::mpsc::UnboundedSender<crate::session::SessionCommand>,
    cwd: &std::path::Path,
    initial_client_mcp_servers: Vec<acp::McpServer>,
    plugin_registry: Option<&xai_grok_agent::plugins::PluginRegistry>,
    compat: &xai_grok_tools::types::compat::CompatConfig,
) -> bool {
    let merged =
        merge_managed_mcp_servers(initial_client_mcp_servers, cwd, plugin_registry, compat);
    let (tx, _rx) = tokio::sync::oneshot::channel();
    cmd_tx
        .send(crate::session::SessionCommand::UpdateMcpServers {
            mcp_servers: merged,
            client_seed: None,
            respond_to: tx,
        })
        .is_ok()
}

pub(crate) fn merge_managed_mcp_servers_with_policy(
    client_mcp_servers: Vec<acp::McpServer>,
    cwd: &std::path::Path,
    plugin_registry: Option<&xai_grok_agent::plugins::PluginRegistry>,
    compat: &xai_grok_tools::types::compat::CompatConfig,
) -> Vec<McpServerWithPolicy> {
    merge_managed_mcp_servers_with_policy_from(
        client_mcp_servers,
        cwd,
        plugin_registry,
        compat,
        xai_grok_workspace::permission::resolution::managed_settings(),
    )
}

/// Test seam: `managed_settings()` is a `OnceLock`, so tests inject a constructed policy here.
fn merge_managed_mcp_servers_with_policy_from(
    client_mcp_servers: Vec<acp::McpServer>,
    cwd: &std::path::Path,
    plugin_registry: Option<&xai_grok_agent::plugins::PluginRegistry>,
    compat: &xai_grok_tools::types::compat::CompatConfig,
    ms: &xai_grok_workspace::permission::resolution::ManagedSettings,
) -> Vec<McpServerWithPolicy> {
    let project = crate::agent::folder_trust::project_scoped_mcp_names(cwd);
    let resolved = {
        let _mcp_merge_timer = crate::instrumentation::timer("mcp_merge_managed");
        xai_grok_config::mcp_servers::resolve_session_mcp_servers(
            &crate::util::config::mcp_server_sources(cwd, compat, "merge_managed_mcp_servers"),
            &plugin_mcp_servers(plugin_registry),
            client_mcp_servers,
        )
    };
    let merged: Vec<(acp::McpServer, McpSubject)> = resolved
        .into_iter()
        .map(|entry| {
            let subject =
                mcp_subject_for_tier(mcp_server_name(&entry.server), entry.tier, &project);
            (entry.server, subject)
        })
        .collect();

    let disabled = crate::util::config::disabled_mcp_server_names(cwd);
    let merged = crate::agent::folder_trust::filter_untrusted_project_mcp_with(
        cwd,
        merged,
        &project,
        |(server, _)| mcp_server_name(server),
    );
    apply_mcp_server_policy(merged, &disabled, ms)
}

pub(crate) fn mcp_subject(
    server: &acp::McpServer,
    origin: &McpServerOrigin,
    project_names: &std::collections::HashSet<String>,
) -> McpSubject {
    mcp_subject_for_tier(
        mcp_server_name(server),
        SessionMcpTier::from(origin),
        project_names,
    )
}

/// The only origin classifier for the merge, discovery, and the agent pool. A project-claimed
/// name classifies foreign even from a native tier (fail closed).
pub(crate) fn mcp_subject_for_tier(
    server_name: &str,
    tier: SessionMcpTier,
    project_names: &std::collections::HashSet<String>,
) -> McpSubject {
    let project_scoped = project_names.contains(server_name);
    let origin = match tier {
        SessionMcpTier::Native if !project_scoped => PolicySubjectOrigin::GrokNative,
        SessionMcpTier::Native | SessionMcpTier::Foreign => PolicySubjectOrigin::Foreign,
    };
    McpSubject {
        origin,
        project_scoped,
    }
}

fn dropped_by_project_pin(
    ms: &xai_grok_workspace::permission::resolution::ManagedSettings,
    server: &acp::McpServer,
    subject: McpSubject,
) -> bool {
    if ms.mcp_project_pin_block(server, subject).is_none() {
        return false;
    }
    tracing::warn!(
        server = %mcp_server_name(server),
        source = %ms
            .project_mcp
            .source()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
        "project-level MCP disabled by managed policy (enableAllProjectMcpServers = false)"
    );
    true
}

pub(crate) struct McpServerWithPolicy {
    pub server: acp::McpServer,
    pub disabled_reason: Option<McpBlockReason>,
}

/// The documented admin signal for a policy-dropped server. It also goes to the always-on
/// unified.jsonl because tracing alone reaches no file in a default run.
fn log_policy_block(name: &str, reason: &McpBlockReason) {
    tracing::warn!(
        name,
        reason = %reason,
        "MCP server blocked by managed settings policy"
    );
    xai_grok_telemetry::unified_log::warn(
        "MCP server blocked by managed settings policy",
        None,
        Some(serde_json::json!({ "server": name, "reason": reason.to_string() })),
    );
}

fn apply_mcp_server_policy(
    merged: Vec<(acp::McpServer, McpSubject)>,
    disabled: &std::collections::HashSet<String>,
    ms: &xai_grok_workspace::permission::resolution::ManagedSettings,
) -> Vec<McpServerWithPolicy> {
    merged
        .into_iter()
        .filter_map(|(server, subject)| {
            if disabled.contains(mcp_server_name(&server)) {
                return None;
            }
            if dropped_by_project_pin(ms, &server, subject) {
                return None;
            }
            match ms.mcp_verdict(&server, subject) {
                McpVerdict::Blocked(reason) => {
                    log_policy_block(mcp_server_name(&server), &reason);
                    Some(McpServerWithPolicy {
                        server,
                        disabled_reason: Some(reason),
                    })
                }
                McpVerdict::Allowed => Some(McpServerWithPolicy {
                    server,
                    disabled_reason: None,
                }),
            }
        })
        .collect()
}

/// Without this gate `x.ai/mcp/call` would spawn policy-blocked servers from the session-less pool.
pub(crate) fn filter_policy_blocked_agent_mcp(
    servers: Vec<acp::McpServer>,
    cwd: &std::path::Path,
) -> Vec<acp::McpServer> {
    let servers = drop_config_disabled_mcp(servers, cwd);
    if servers.is_empty() {
        return servers;
    }
    let toml_servers: HashMap<String, acp::McpServer> =
        crate::util::config::load_mcp_servers_toml_only(cwd)
            .into_iter()
            .map(|s| (mcp_server_name(&s).to_owned(), s))
            .collect();
    filter_policy_blocked_agent_mcp_with(
        servers,
        &toml_servers,
        &crate::agent::folder_trust::project_scoped_mcp_names(cwd),
        xai_grok_workspace::permission::resolution::managed_settings(),
    )
}

fn drop_config_disabled_mcp(
    servers: Vec<acp::McpServer>,
    cwd: &std::path::Path,
) -> Vec<acp::McpServer> {
    if servers.is_empty() {
        return servers;
    }
    let disabled = crate::util::config::disabled_mcp_server_names(cwd);
    servers
        .into_iter()
        .filter(|s| !disabled.contains(mcp_server_name(s)))
        .collect()
}

fn filter_policy_blocked_agent_mcp_with(
    servers: Vec<acp::McpServer>,
    toml_servers: &HashMap<String, acp::McpServer>,
    project_names: &std::collections::HashSet<String>,
    ms: &xai_grok_workspace::permission::resolution::ManagedSettings,
) -> Vec<acp::McpServer> {
    servers
        .into_iter()
        .filter(|server| {
            let name = mcp_server_name(server);
            // Name ownership alone is not enough: a squatted TOML name stays foreign (fail closed).
            let tier = if toml_servers.get(name).is_some_and(|disk| {
                xai_grok_config::mcp_servers::mcp_server_definitions_equal(disk, server)
            }) {
                SessionMcpTier::Native
            } else {
                SessionMcpTier::Foreign
            };
            let subject = mcp_subject_for_tier(name, tier, project_names);
            if dropped_by_project_pin(ms, server, subject) {
                return false;
            }
            match ms.mcp_verdict(server, subject) {
                McpVerdict::Allowed => true,
                McpVerdict::Blocked(reason) => {
                    log_policy_block(name, &reason);
                    false
                }
            }
        })
        .collect()
}

/// The only reasons-map assembly for doctor and `mcp/list`, so CLI verdicts cannot drift from the merge.
pub(crate) fn mcp_blocked_reasons<'a>(
    definitions: impl IntoIterator<Item = (&'a str, &'a acp::McpServer, McpSubject)>,
    ms: &xai_grok_workspace::permission::resolution::ManagedSettings,
) -> HashMap<String, McpBlockReason> {
    let mut out = HashMap::new();
    for (name, server, subject) in definitions {
        if let McpVerdict::Blocked(reason) = ms.mcp_verdict(server, subject) {
            out.entry(name.to_string()).or_insert(reason);
        }
    }
    out
}

pub(crate) fn merge_managed_mcp_servers_sourced(
    cwd: &std::path::Path,
    plugin_registry: Option<&xai_grok_agent::plugins::PluginRegistry>,
    compat: &xai_grok_tools::types::compat::CompatConfig,
) -> Vec<(acp::McpServer, McpServerOrigin)> {
    let _mcp_merge_timer = crate::instrumentation::timer("mcp_merge_managed");
    xai_grok_config::mcp_servers::load_mcp_servers_with_origin(
        &crate::util::config::mcp_server_sources(cwd, compat, "merge_managed_mcp_servers_sourced"),
        &plugin_mcp_servers(plugin_registry),
    )
}

#[derive(Clone, Copy)]
pub(crate) struct McpDiscoveryInputs<'a> {
    pub cwd: &'a std::path::Path,
    pub plugin_registry: Option<&'a xai_grok_agent::plugins::PluginRegistry>,
    pub compat: &'a xai_grok_tools::types::compat::CompatConfig,
}

pub(crate) fn discover_mcp_definitions_ignoring_disable(
    inputs: &McpDiscoveryInputs<'_>,
) -> HashMap<String, (acp::McpServer, McpSubject)> {
    use crate::util::config::{
        McpEnabledFilter, load_mcp_preferences, load_mcp_server_configs_with_project,
        materialize_mcp_config,
    };

    let cwd = inputs.cwd;
    let plugin_registry = inputs.plugin_registry;
    let compat = inputs.compat;

    let preferences = load_mcp_preferences().file();
    let sub = &crate::config::expand_env_vars_in_string;
    let toml_claimed = crate::util::config::all_toml_mcp_server_names(cwd);

    let project = crate::agent::folder_trust::project_scoped_mcp_names(cwd);
    let mut by_name: HashMap<String, (acp::McpServer, McpSubject)> = HashMap::new();
    for (name, (config, scope)) in load_mcp_server_configs_with_project(cwd) {
        let Some(transport) =
            materialize_mcp_config(&name, config, &preferences, sub, McpEnabledFilter::Ignore)
        else {
            continue;
        };
        let tier = if scope == crate::util::config::MCP_SCOPE_PROJECT {
            SessionMcpTier::Foreign
        } else {
            SessionMcpTier::Native
        };
        let subject = mcp_subject_for_tier(&name, tier, &project);
        by_name.insert(name, (transport, subject));
    }
    for (server, origin) in xai_grok_config::mcp_servers::non_toml_mcp_servers_with_origin(
        &crate::util::config::mcp_server_sources(
            cwd,
            compat,
            "discover_mcp_definitions_ignoring_disable",
        ),
        &plugin_mcp_servers(plugin_registry),
        &toml_claimed,
    ) {
        let subject = mcp_subject(&server, &origin, &project);
        by_name
            .entry(mcp_server_name(&server).to_owned())
            .or_insert((server, subject));
    }

    let entries: Vec<(acp::McpServer, McpSubject)> = by_name.into_values().collect();
    let entries = crate::agent::folder_trust::filter_untrusted_project_mcp_with(
        cwd,
        entries,
        &project,
        |(server, _)| mcp_server_name(server),
    );
    // Project-pinned definitions stay: consumers gate through `mcp_verdict`, whose ProjectPin leg
    // names the pinning source. Dropping them would report "not found in config".
    entries
        .into_iter()
        .map(|(server, subject)| (mcp_server_name(&server).to_owned(), (server, subject)))
        .collect()
}

fn plugin_mcp_servers(
    plugin_registry: Option<&xai_grok_agent::plugins::PluginRegistry>,
) -> Vec<xai_grok_config::mcp_servers::PluginMcpServers> {
    let Some(registry) = plugin_registry else {
        return Vec::new();
    };
    registry
        .active_plugins()
        .into_iter()
        .map(crate::util::config::plugin_mcp_servers)
        .collect()
}

pub(crate) fn collect_plugin_oauth_configs(
    plugin_registry: Option<&xai_grok_agent::plugins::PluginRegistry>,
) -> crate::util::config::McpOAuthConfigMap {
    xai_grok_config::mcp_servers::plugin_oauth_configs(&plugin_mcp_servers(plugin_registry))
}

pub(crate) fn merge_plugin_oauth_into(
    oauth_config_map: &mut crate::util::config::McpOAuthConfigMap,
    plugin_oauth: crate::util::config::McpOAuthConfigMap,
    toml_mcp_names: &std::collections::HashSet<String>,
) {
    for (name, cfg) in plugin_oauth {
        if toml_mcp_names.contains(&name) {
            continue;
        }
        oauth_config_map.insert(name, cfg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at<T>(xs: &[T], i: usize) -> &T {
        let Some(x) = xs.get(i) else {
            panic!("expected index {i}, len {}", xs.len());
        };
        x
    }

    fn empty_cwd() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn settings_with_policy(
        policy: xai_grok_workspace::permission::resolution::McpServerPolicy,
    ) -> xai_grok_workspace::permission::resolution::ManagedSettings {
        let mut ms = xai_grok_workspace::permission::resolution::ManagedSettings::default();
        ms.mcp_allowlist = policy;
        ms
    }

    fn foreign(server: acp::McpServer) -> (acp::McpServer, McpSubject) {
        (
            server,
            McpSubject {
                origin: PolicySubjectOrigin::Foreign,
                project_scoped: false,
            },
        )
    }

    #[test]
    fn client_provided_servers_survive_merge() {
        let client = vec![acp::McpServer::Http(
            acp::McpServerHttp::new(
                "demo-mcp".to_string(),
                "http://mcp.example.test/api/mcp".to_string(),
            )
            .headers(vec![]),
        )];
        let cwd = empty_cwd();
        let compat = xai_grok_tools::types::compat::CompatConfig::default();
        let merged = merge_managed_mcp_servers(client, cwd.path(), None, &compat);
        assert!(
            merged.iter().any(|s| matches!(
                s,
                acp::McpServer::Http(acp::McpServerHttp { name, .. }) if name == "demo-mcp"
            )),
            "client-provided server must survive a merge with no disk/managed sources"
        );
    }

    fn write_cursor_project_mcp(cwd: &std::path::Path, name: &str) {
        std::fs::create_dir_all(cwd.join(".cursor")).unwrap();
        std::fs::write(
            cwd.join(".cursor").join("mcp.json"),
            format!(r#"{{"mcpServers": {{"{name}": {{"command": "true"}}}}}}"#),
        )
        .unwrap();
    }

    #[test]
    fn sse_transport_matches_http_policy_verdicts() {
        use xai_grok_workspace::permission::resolution::{
            AllowedMcpServer, McpServerAllowlist, McpServerPolicy,
        };

        let denied = acp::McpServer::Sse(
            acp::McpServerSse::new("corp-sse", "https://denied.corp.com/mcp").headers(vec![]),
        );
        let allowed = acp::McpServer::Sse(
            acp::McpServerSse::new("ok-sse", "https://ok.example/mcp").headers(vec![]),
        );
        let ms = settings_with_policy(McpServerPolicy::single(McpServerAllowlist::new(
            vec![],
            vec![AllowedMcpServer::Http {
                url_pattern: "https://denied.corp.com/*".into(),
            }],
            Some(std::path::PathBuf::from("/test/managed_config.toml")),
        )));

        assert!(
            matches!(
                ms.mcp_verdict(&denied, foreign(denied.clone()).1),
                McpVerdict::Blocked(McpBlockReason::Deny { .. })
            ),
            "URL deny must bind the Sse transport"
        );

        let kept = apply_mcp_server_policy(
            vec![foreign(denied), foreign(allowed)],
            &std::collections::HashSet::new(),
            &ms,
        );
        let reason_of = |name: &str| {
            kept.iter()
                .find(|s| mcp_server_name(&s.server) == name)
                .expect("server present")
                .disabled_reason
                .clone()
        };
        assert!(
            matches!(reason_of("corp-sse"), Some(McpBlockReason::Deny { .. })),
            "merge must tag the denied Sse server"
        );
        assert!(
            reason_of("ok-sse").is_none(),
            "non-matching Sse server must stay allowed"
        );
    }

    #[test]
    fn blocked_map_covers_personally_disabled_servers() {
        use xai_grok_workspace::permission::resolution::{
            AllowedMcpServer, McpServerAllowlist, McpServerPolicy,
        };

        let cwd = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(cwd.path().join(".grok")).unwrap();
        std::fs::write(
            cwd.path().join(".grok").join("config.toml"),
            r#"
disabled_mcp_servers = ["corp"]

[mcp_servers.corp]
url = "https://denied.corp.com/mcp"
"#,
        )
        .unwrap();

        let compat = xai_grok_tools::types::compat::CompatConfig::default();
        let inputs = McpDiscoveryInputs {
            cwd: cwd.path(),
            plugin_registry: None,
            compat: &compat,
        };
        let discovered = discover_mcp_definitions_ignoring_disable(&inputs);
        assert!(
            discovered.contains_key("corp"),
            "disable-ignoring discovery must keep the disabled definition, got {:?}",
            discovered.keys().collect::<Vec<_>>()
        );

        let ms = settings_with_policy(McpServerPolicy::single(McpServerAllowlist::new(
            vec![],
            vec![AllowedMcpServer::Http {
                url_pattern: "https://denied.corp.com/*".into(),
            }],
            Some(std::path::PathBuf::from("/test/managed_config.toml")),
        )));
        let blocked = mcp_blocked_reasons(
            discovered
                .iter()
                .map(|(name, (server, subject))| (name.as_str(), server, *subject)),
            &ms,
        );
        assert!(
            blocked.contains_key("corp"),
            "denied+disabled server must be in the verdict map"
        );
    }

    #[test]
    fn discovery_drops_project_definitions_for_untrusted_folder() {
        let grok_home = tempfile::tempdir().unwrap();
        std::fs::write(
            grok_home.path().join("config.toml"),
            format!("[mcp_servers.usersrv]\nurl = \"{USER_MCP_URL}\"\n"),
        )
        .unwrap();
        let filter = module_path!()
            .split_once("::")
            .map(|(_, rest)| rest)
            .unwrap_or_default();
        let exe = std::env::current_exe().expect("current_exe");
        let mut cmd = std::process::Command::new(exe);
        cmd.env("GROK_HOME", grok_home.path())
            .env_remove("GROK_CONFIG")
            .env_remove("GROK_CONFIG_PATH")
            .env(UNTRUSTED_DISCOVERY_CHILD, "1")
            .arg("--ignored")
            .arg("--exact")
            .arg(format!("{filter}::untrusted_discovery_child"))
            .arg("--nocapture")
            .stdin(std::process::Stdio::null());
        xai_tty_utils::detach_std_command(&mut cmd);
        let output = cmd.output().expect("re-exec test binary");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "isolated discovery child failed:\n{stdout}\n{stderr}"
        );
        assert!(
            stdout.contains(UNTRUSTED_DISCOVERY_PASS_MARK),
            "child did not run (filter matched nothing?)\n{stdout}\n{stderr}"
        );
    }

    const UNTRUSTED_DISCOVERY_CHILD: &str = "GROK_TEST_UNTRUSTED_DISCOVERY_CHILD";
    const UNTRUSTED_DISCOVERY_PASS_MARK: &str = "untrusted-discovery-child-passed";
    const USER_MCP_URL: &str = "https://user.example.com/mcp";
    const PLUGIN_MCP_URL: &str = "https://plugin.example.com/mcp";

    fn http_url(server: &acp::McpServer) -> Option<&str> {
        match server {
            acp::McpServer::Http(acp::McpServerHttp { url, .. }) => Some(url),
            _ => None,
        }
    }

    #[test]
    #[ignore = "spawned as a subprocess by discovery_drops_project_definitions_for_untrusted_folder"]
    fn untrusted_discovery_child() {
        if std::env::var_os(UNTRUSTED_DISCOVERY_CHILD).is_none() {
            return;
        }
        let cwd = tempfile::tempdir().unwrap();
        git2::Repository::init(cwd.path()).unwrap();
        std::fs::create_dir_all(cwd.path().join(".grok")).unwrap();
        std::fs::write(
            cwd.path().join(".grok").join("config.toml"),
            r#"
[mcp_servers.projsrv]
command = "echo"
"#,
        )
        .unwrap();
        let plugin_root = tempfile::tempdir().unwrap();
        let registry =
            plugin_registry_with_inline_server(plugin_root.path(), "pluginsrv", PLUGIN_MCP_URL);

        let mut compat = xai_grok_tools::types::compat::CompatConfig::default();
        compat.claude.mcps = false;
        compat.cursor.mcps = false;
        let inputs = McpDiscoveryInputs {
            cwd: cwd.path(),
            plugin_registry: Some(&registry),
            compat: &compat,
        };

        crate::agent::folder_trust::record_for_test(cwd.path(), true);
        let trusted = discover_mcp_definitions_ignoring_disable(&inputs);
        let (_, subject) = trusted
            .get("projsrv")
            .expect("trusted repo-declared definition must be discoverable");
        assert!(
            subject.project_scoped,
            "repo-declared definition must be tagged project-scoped"
        );

        crate::agent::folder_trust::record_for_test(cwd.path(), false);
        let untrusted = discover_mcp_definitions_ignoring_disable(&inputs);
        assert!(
            !untrusted.contains_key("projsrv"),
            "untrusted repo-declared definition must not be discoverable, got {:?}",
            untrusted.keys().collect::<Vec<_>>()
        );
        let (user, _) = untrusted
            .get("usersrv")
            .expect("user-tier definition must survive the trust gate");
        assert_eq!(http_url(user), Some(USER_MCP_URL));
        let (plugin, _) = untrusted
            .get("pluginsrv")
            .expect("plugin-tier definition must survive the trust gate");
        assert_eq!(http_url(plugin), Some(PLUGIN_MCP_URL));

        println!("{UNTRUSTED_DISCOVERY_PASS_MARK}");
    }

    #[test]
    fn agent_pool_filter_drops_policy_blocked_servers() {
        use xai_grok_workspace::permission::resolution::{
            AllowedMcpServer, McpServerAllowlist, McpServerPolicy, PolicySourceAuthority,
        };

        let corp = || {
            acp::McpServer::Http(
                acp::McpServerHttp::new("corp", "https://denied.corp.com/mcp").headers(vec![]),
            )
        };
        let deny = |authority: PolicySourceAuthority| {
            settings_with_policy(McpServerPolicy::single(
                McpServerAllowlist::new(
                    vec![],
                    vec![AllowedMcpServer::Http {
                        url_pattern: "https://denied.corp.com/*".into(),
                    }],
                    Some(std::path::PathBuf::from("/test/managed_config.toml")),
                )
                .with_authority(authority),
            ))
        };
        let names = |servers: &[acp::McpServer]| -> Vec<String> {
            servers
                .iter()
                .map(|s| mcp_server_name(s).to_string())
                .collect()
        };
        let none = std::collections::HashSet::new;
        let no_toml = HashMap::new;

        let out = filter_policy_blocked_agent_mcp_with(
            vec![
                corp(),
                acp::McpServer::Http(
                    acp::McpServerHttp::new("ok", "https://ok.example/mcp").headers(vec![]),
                ),
            ],
            &no_toml(),
            &none(),
            &deny(PolicySourceAuthority::Native),
        );
        assert_eq!(names(&out), vec!["ok"]);

        let toml_servers = HashMap::from([("corp".to_string(), corp())]);
        let advisory = deny(PolicySourceAuthority::Advisory);
        let kept =
            filter_policy_blocked_agent_mcp_with(vec![corp()], &toml_servers, &none(), &advisory);
        assert_eq!(names(&kept), vec!["corp"]);
        assert!(
            filter_policy_blocked_agent_mcp_with(vec![corp()], &no_toml(), &none(), &advisory)
                .is_empty()
        );

        let project = std::collections::HashSet::from(["corp".to_string()]);
        assert!(
            filter_policy_blocked_agent_mcp_with(vec![corp()], &toml_servers, &project, &advisory)
                .is_empty()
        );

        let stdio = |cmd: &str| {
            acp::McpServer::Stdio(acp::McpServerStdio::new(
                "corp",
                std::path::PathBuf::from(cmd),
            ))
        };
        let deny_name = settings_with_policy(McpServerPolicy::single(
            McpServerAllowlist::new(
                vec![],
                vec![AllowedMcpServer::Name {
                    name: "corp".into(),
                }],
                Some(std::path::PathBuf::from("/test/managed_config.toml")),
            )
            .with_authority(PolicySourceAuthority::Advisory),
        ));
        let stdio_toml = HashMap::from([("corp".to_string(), stdio("good"))]);
        let http_squat = acp::McpServer::Http(
            acp::McpServerHttp::new("corp", "https://denied.corp.com/evil").headers(vec![]),
        );
        for (squatter, toml) in [
            (http_squat, &toml_servers),
            (stdio("evil"), &toml_servers),
            (stdio("evil"), &stdio_toml),
        ] {
            assert!(
                filter_policy_blocked_agent_mcp_with(vec![squatter], toml, &none(), &deny_name)
                    .is_empty()
            );
        }
        let kept = filter_policy_blocked_agent_mcp_with(
            vec![stdio("good")],
            &stdio_toml,
            &none(),
            &deny_name,
        );
        assert_eq!(names(&kept), vec!["corp"]);
    }

    #[test]
    fn filter_policy_blocked_agent_mcp_drops_enabled_false_name() {
        let cwd = tempfile::tempdir().unwrap();
        git2::Repository::init(cwd.path()).unwrap();
        std::fs::create_dir_all(cwd.path().join(".grok")).unwrap();
        std::fs::write(
            cwd.path().join(".grok").join("config.toml"),
            r#"
[mcp_servers.agent_md_ks_adder]
url = "https://toml.example/mcp"
enabled = false
"#,
        )
        .unwrap();
        let servers = vec![acp::McpServer::Http(
            acp::McpServerHttp::new("agent_md_ks_adder", "https://agent.example/mcp")
                .headers(vec![]),
        )];
        assert!(
            filter_policy_blocked_agent_mcp(servers, cwd.path()).is_empty(),
            "enabled = false must drop the name before overlay/pool spawn"
        );
    }

    #[test]
    fn toml_loaders_agree_on_env_and_header_bearing_definitions() {
        let tmp = tempfile::tempdir().unwrap();
        git2::Repository::init(tmp.path()).unwrap();
        std::fs::create_dir_all(tmp.path().join(".grok")).unwrap();
        std::fs::write(
            tmp.path().join(".grok").join("config.toml"),
            r#"
[mcp_servers.parity_stdio]
command = "echo"
args = ["a", "b"]
env = { A = "1", B = "2", C = "3" }

[mcp_servers.parity_http]
url = "https://parity.example/mcp"
headers = { "X-A" = "1", "X-B" = "2", "X-C" = "3" }
"#,
        )
        .unwrap();

        let compat = xai_grok_tools::types::compat::CompatConfig::default();
        let by_name = |servers: Vec<acp::McpServer>| -> HashMap<String, acp::McpServer> {
            servers
                .into_iter()
                .map(|s| (mcp_server_name(&s).to_owned(), s))
                .collect()
        };
        let full = by_name(crate::util::config::load_mcp_servers(tmp.path(), &compat));
        let toml = by_name(crate::util::config::load_mcp_servers_toml_only(tmp.path()));
        for name in ["parity_stdio", "parity_http"] {
            let (a, b) = (full.get(name).unwrap(), toml.get(name).unwrap());
            assert!(
                xai_grok_config::mcp_servers::mcp_server_definitions_equal(a, b),
                "{name}: {a:?} vs {b:?}"
            );
        }
    }

    #[test]
    fn advisory_policy_exempts_grok_native_servers() {
        use xai_grok_workspace::permission::resolution::{
            AllowedMcpServer, McpServerAllowlist, McpServerPolicy, PolicySourceAuthority,
        };

        let server = || {
            acp::McpServer::Http(
                acp::McpServerHttp::new("corp", "https://denied.corp.com/mcp").headers(vec![]),
            )
        };
        let deny = |authority: PolicySourceAuthority| {
            McpServerPolicy::single(
                McpServerAllowlist::new(
                    vec![],
                    vec![AllowedMcpServer::Http {
                        url_pattern: "https://denied.corp.com/*".into(),
                    }],
                    Some(std::path::PathBuf::from("/test/managed-settings.json")),
                )
                .with_authority(authority),
            )
        };
        let subject = |origin: PolicySubjectOrigin| McpSubject {
            origin,
            project_scoped: false,
        };

        let tagged = apply_mcp_server_policy(
            vec![(server(), subject(PolicySubjectOrigin::GrokNative))],
            &std::collections::HashSet::new(),
            &settings_with_policy(deny(PolicySourceAuthority::Advisory)),
        );
        assert!(
            at(&tagged, 0).disabled_reason.is_none(),
            "advisory deny must not bind a grok-native server"
        );

        let tagged = apply_mcp_server_policy(
            vec![(server(), subject(PolicySubjectOrigin::Foreign))],
            &std::collections::HashSet::new(),
            &settings_with_policy(deny(PolicySourceAuthority::Advisory)),
        );
        assert!(
            at(&tagged, 0).disabled_reason.is_some(),
            "advisory deny must bind a foreign server"
        );

        let tagged = apply_mcp_server_policy(
            vec![(server(), subject(PolicySubjectOrigin::GrokNative))],
            &std::collections::HashSet::new(),
            &settings_with_policy(deny(PolicySourceAuthority::Native)),
        );
        assert!(
            at(&tagged, 0).disabled_reason.is_some(),
            "a native policy source binds every origin"
        );
    }

    fn plugin_registry_with_inline_server(
        plugin_root: &std::path::Path,
        server_name: &str,
        url: &str,
    ) -> xai_grok_agent::plugins::PluginRegistry {
        use xai_grok_agent::plugins::discovery::{DiscoveredPlugin, PluginId};
        use xai_grok_agent::plugins::manifest::{PathOrInline, PluginManifest};
        use xai_grok_agent::plugins::{PluginRegistry, PluginScope};

        std::fs::create_dir_all(plugin_root).unwrap();
        let manifest = PluginManifest {
            name: "native-plugin".into(),
            version: None,
            description: None,
            author: None,
            homepage: None,
            repository: None,
            license: None,
            keywords: vec![],
            skills: None,
            commands: None,
            agents: None,
            hooks: None,
            mcp_servers: Some(PathOrInline::Inline(serde_json::json!({
                server_name: { "type": "http", "url": url }
            }))),
            lsp_servers: None,
        };
        let id = PluginId::new(PluginScope::User, plugin_root, "native-plugin");
        let dp = DiscoveredPlugin {
            manifest,
            id,
            root: plugin_root.to_path_buf(),
            canonical_root: plugin_root.to_path_buf(),
            scope: PluginScope::User,
            origin: xai_grok_agent::plugins::PluginOrigin::UserGrok,
            trusted: true,
            skill_dirs: vec![],
            command_dirs: vec![],
            agent_dirs: vec![],
            hooks_path: None,
            mcp_config_path: None,
            lsp_config_path: None,
            conflict: None,
        };
        PluginRegistry::from_discovered(vec![dp], &[], &["native-plugin".to_string()])
    }

    #[test]
    fn client_forwarded_native_name_keeps_native_subject() {
        use xai_grok_workspace::permission::resolution::{
            AllowedMcpServer, McpServerAllowlist, McpServerPolicy, PolicySourceAuthority,
        };

        let tmp = tempfile::tempdir().unwrap();
        let registry = plugin_registry_with_inline_server(
            &tmp.path().join("native-plugin"),
            "corp",
            "https://denied.corp.com/mcp",
        );
        let cwd = empty_cwd();
        let compat = xai_grok_tools::types::compat::CompatConfig::default();
        let ms = settings_with_policy(McpServerPolicy::single(
            McpServerAllowlist::new(
                vec![],
                vec![AllowedMcpServer::Http {
                    url_pattern: "https://denied.corp.com/*".into(),
                }],
                Some(std::path::PathBuf::from("/test/managed-settings.json")),
            )
            .with_authority(PolicySourceAuthority::Advisory),
        ));

        let client = |name: &str| {
            acp::McpServer::Http(
                acp::McpServerHttp::new(name, "https://denied.corp.com/mcp").headers(vec![]),
            )
        };
        let merged = merge_managed_mcp_servers_with_policy_from(
            vec![client("corp"), client("rogue")],
            cwd.path(),
            Some(&registry),
            &compat,
            &ms,
        );
        let reason_of = |name: &str| {
            merged
                .iter()
                .find(|s| mcp_server_name(&s.server) == name)
                .unwrap_or_else(|| panic!("{name} must be in the merge"))
                .disabled_reason
                .as_ref()
        };
        assert!(
            reason_of("corp").is_none(),
            "advisory deny must not bind the client-forwarded copy of a native server"
        );
        assert!(
            reason_of("rogue").is_some(),
            "client-only server must stay foreign and be tagged"
        );
    }

    #[test]
    fn client_redefinition_of_native_name_falls_back_to_foreign() {
        use xai_grok_workspace::permission::resolution::{
            AllowedMcpServer, McpServerAllowlist, McpServerPolicy, PolicySourceAuthority,
        };

        let tmp = tempfile::tempdir().unwrap();
        let registry = plugin_registry_with_inline_server(
            &tmp.path().join("native-plugin"),
            "corp",
            "https://ok.example.com/mcp",
        );
        let cwd = empty_cwd();
        let compat = xai_grok_tools::types::compat::CompatConfig::default();
        let ms = settings_with_policy(McpServerPolicy::single(
            McpServerAllowlist::new(
                vec![],
                vec![AllowedMcpServer::Http {
                    url_pattern: "https://evil.example.com/*".into(),
                }],
                Some(std::path::PathBuf::from("/test/managed-settings.json")),
            )
            .with_authority(PolicySourceAuthority::Advisory),
        ));

        let client = acp::McpServer::Http(
            acp::McpServerHttp::new("corp", "https://evil.example.com/mcp").headers(vec![]),
        );
        let merged = merge_managed_mcp_servers_with_policy_from(
            vec![client],
            cwd.path(),
            Some(&registry),
            &compat,
            &ms,
        );
        let corp = merged
            .iter()
            .find(|s| mcp_server_name(&s.server) == "corp")
            .expect("corp must be in the merge");
        assert!(
            corp.disabled_reason.is_some(),
            "a client redefinition of a native name must stay foreign and be tagged"
        );
    }

    #[test]
    fn mcp_subject_classifies_sources() {
        let server = acp::McpServer::Http(
            acp::McpServerHttp::new("srv", "https://s.example.com/mcp").headers(vec![]),
        );
        let empty = std::collections::HashSet::new();
        let native = [
            McpServerOrigin::ConfigToml {
                path: "/u/.grok/config.toml".into(),
            },
            McpServerOrigin::Plugin {
                plugin_name: "p".into(),
                path: "/p".into(),
            },
        ];
        for source in &native {
            assert_eq!(
                mcp_subject(&server, source, &empty).origin,
                PolicySubjectOrigin::GrokNative,
                "{source:?}"
            );
        }
        let foreign = [
            McpServerOrigin::ClaudeJson {
                path: "/u/.claude.json".into(),
            },
            McpServerOrigin::McpJson {
                path: "/repo/.mcp.json".into(),
            },
        ];
        for source in &foreign {
            assert_eq!(
                mcp_subject(&server, source, &empty).origin,
                PolicySubjectOrigin::Foreign,
                "{source:?}"
            );
        }
        let project: std::collections::HashSet<String> = ["srv".to_string()].into();
        assert_eq!(
            mcp_subject(&server, at(&native, 0), &project).origin,
            PolicySubjectOrigin::Foreign
        );
    }

    #[test]
    fn injected_settings_drive_lockdown_and_project_pin_through_merge() {
        use xai_grok_workspace::permission::resolution::{
            AllowedMcpServer, ManagedSettings, McpServerAllowlist, McpServerPolicy,
            PolicyLayerOwnership, PolicyPin,
        };

        let cwd = empty_cwd();
        std::fs::create_dir_all(cwd.path().join(".cursor")).unwrap();
        std::fs::write(
            cwd.path().join(".cursor").join("mcp.json"),
            r#"{"mcpServers": {
                "granted": {"url": "https://ok.example.com/mcp"},
                "ungranted": {"url": "https://other.example.com/mcp"}
            }}"#,
        )
        .unwrap();
        let compat = xai_grok_tools::types::compat::CompatConfig::default();
        let merge = |ms: &ManagedSettings| {
            let merged =
                merge_managed_mcp_servers_with_policy_from(vec![], cwd.path(), None, &compat, ms);
            merged
                .iter()
                .map(|s| {
                    (
                        mcp_server_name(&s.server).to_string(),
                        s.disabled_reason.is_some(),
                    )
                })
                .collect::<HashMap<String, bool>>()
        };

        let mut ms = ManagedSettings::default();
        let by_name = merge(&ms);
        assert_eq!(by_name.get("granted"), Some(&false));
        assert_eq!(by_name.get("ungranted"), Some(&false));

        ms.mcp_allowlist = McpServerPolicy::single(
            McpServerAllowlist::new(
                vec![AllowedMcpServer::Http {
                    url_pattern: "https://ok.example.com/*".into(),
                }],
                vec![],
                Some(std::path::PathBuf::from("/etc/grok/managed_config.toml")),
            )
            .with_managed_only()
            .with_ownership(PolicyLayerOwnership::Admin),
        );
        let by_name = merge(&ms);
        assert_eq!(by_name.get("granted"), Some(&false));
        assert_eq!(
            by_name.get("ungranted"),
            Some(&true),
            "unlisted server must be tagged blocked under managed-only"
        );

        ms.project_mcp = PolicyPin::Disabled {
            source: std::path::PathBuf::from("/etc/grok/managed_config.toml"),
            ownership: PolicyLayerOwnership::Admin,
        };
        let by_name = merge(&ms);
        assert_eq!(
            by_name.get("granted"),
            Some(&false),
            "allow-granted project server survives the pin"
        );
        assert!(
            !by_name.contains_key("ungranted"),
            "pinned-off project server must be dropped"
        );
    }

    #[test]
    fn merge_drops_denied_server_and_classifies_as_denylist() {
        use xai_grok_workspace::permission::resolution::{
            AllowedMcpServer, McpServerAllowlist, McpServerPolicy,
        };

        let allowlist = McpServerPolicy::single(McpServerAllowlist::new(
            vec![],
            vec![AllowedMcpServer::Http {
                url_pattern: "https://blocked.corp.com/*".into(),
            }],
            Some(std::path::PathBuf::from("/test/managed-settings.json")),
        ));

        let tagged = apply_mcp_server_policy(
            vec![
                foreign(acp::McpServer::Http(
                    acp::McpServerHttp::new("blocked", "https://blocked.corp.com/mcp")
                        .headers(vec![]),
                )),
                foreign(acp::McpServer::Http(
                    acp::McpServerHttp::new("ok", "https://ok.corp.com/mcp").headers(vec![]),
                )),
            ],
            &std::collections::HashSet::new(),
            &settings_with_policy(allowlist),
        );

        let blocked = tagged
            .iter()
            .find(|s| mcp_server_name(&s.server) == "blocked")
            .expect("denied server present in policy output");
        assert!(
            matches!(blocked.disabled_reason, Some(McpBlockReason::Deny { .. })),
            "expected Deny reason, got {:?}",
            blocked.disabled_reason
        );

        let ok = tagged
            .iter()
            .find(|s| mcp_server_name(&s.server) == "ok")
            .expect("allowed server present in policy output");
        assert!(ok.disabled_reason.is_none());

        let surviving: Vec<&str> = tagged
            .iter()
            .filter(|s| s.disabled_reason.is_none())
            .map(|s| mcp_server_name(&s.server))
            .collect();
        assert_eq!(
            surviving,
            ["ok"],
            "denied server must be dropped by the merge"
        );
    }

    #[test]
    fn policy_block_writes_unified_log_warn() {
        use xai_grok_workspace::permission::resolution::{
            AllowedMcpServer, McpServerAllowlist, McpServerPolicy,
        };

        let allowlist = McpServerPolicy::single(McpServerAllowlist::new(
            vec![],
            vec![AllowedMcpServer::Name {
                name: "corp-denied-unified-log".into(),
            }],
            Some(std::path::PathBuf::from("/test/managed-settings.json")),
        ));
        let tagged = apply_mcp_server_policy(
            vec![foreign(acp::McpServer::Http(
                acp::McpServerHttp::new("corp-denied-unified-log", "https://denied.example/mcp")
                    .headers(vec![]),
            ))],
            &std::collections::HashSet::new(),
            &settings_with_policy(allowlist),
        );
        assert!(
            at(&tagged, 0).disabled_reason.is_some(),
            "server must be tagged"
        );

        let log = xai_grok_telemetry::unified_log::snapshot_log().unwrap_or_default();
        let log = String::from_utf8_lossy(&log);
        assert!(
            log.lines().any(
                |l| l.contains("MCP server blocked by managed settings policy")
                    && l.contains("corp-denied-unified-log")
            ),
            "block signal missing from unified log"
        );
    }

    #[test]
    fn merge_drops_server_denied_by_name_including_managed_prefix() {
        use xai_grok_workspace::permission::resolution::{
            AllowedMcpServer, McpServerAllowlist, McpServerPolicy,
        };

        let allowlist = McpServerPolicy::single(McpServerAllowlist::new(
            vec![],
            vec![AllowedMcpServer::Name {
                name: "slack".into(),
            }],
            Some(std::path::PathBuf::from("/test/managed-settings.json")),
        ));

        let tagged = apply_mcp_server_policy(
            vec![
                foreign(acp::McpServer::Http(
                    acp::McpServerHttp::new("grok_com_slack", "https://mcp.slack.com/sse")
                        .headers(vec![]),
                )),
                foreign(acp::McpServer::Http(
                    acp::McpServerHttp::new("slackbot", "https://slackbot.example.com/mcp")
                        .headers(vec![]),
                )),
            ],
            &std::collections::HashSet::new(),
            &settings_with_policy(allowlist),
        );

        let slack = tagged
            .iter()
            .find(|s| mcp_server_name(&s.server) == "grok_com_slack")
            .expect("managed server present in policy output");
        assert!(
            matches!(slack.disabled_reason, Some(McpBlockReason::Deny { .. })),
            "name-denied managed server must classify as Deny, got {:?}",
            slack.disabled_reason
        );

        let bot = tagged
            .iter()
            .find(|s| mcp_server_name(&s.server) == "slackbot")
            .expect("unrelated server present in policy output");
        assert!(
            bot.disabled_reason.is_none(),
            "substring-only match must not be denied by name"
        );

        let surviving: Vec<&str> = tagged
            .iter()
            .filter(|s| s.disabled_reason.is_none())
            .map(|s| mcp_server_name(&s.server))
            .collect();
        assert_eq!(surviving, ["slackbot"]);
    }

    #[test]
    fn managed_only_lockdown_drops_non_allowlisted_keeps_allowlisted() {
        use xai_grok_workspace::permission::resolution::{
            AllowedMcpServer, McpServerAllowlist, McpServerPolicy,
        };

        let policy = McpServerPolicy::single(
            McpServerAllowlist::new(
                vec![
                    AllowedMcpServer::StdioArgv {
                        argv: vec![
                            "npx".into(),
                            "@example-corp/ui-kit-mcp".into(),
                            "enterprise-webc".into(),
                        ],
                    },
                    AllowedMcpServer::Http {
                        url_pattern: "https://mcp.figma.com/*".into(),
                    },
                ],
                vec![],
                Some(std::path::PathBuf::from("/etc/grok/requirements.toml")),
            )
            .with_managed_only(),
        );

        let ui_kit = acp::McpServer::Stdio(
            acp::McpServerStdio::new("ui-kit", std::path::PathBuf::from("npx")).args(vec![
                "@example-corp/ui-kit-mcp".into(),
                "enterprise-webc".into(),
            ]),
        );
        let rogue_stdio = acp::McpServer::Stdio(acp::McpServerStdio::new(
            "rogue",
            std::path::PathBuf::from("python3"),
        ));
        let figma = acp::McpServer::Http(
            acp::McpServerHttp::new("figma", "https://mcp.figma.com/mcp").headers(vec![]),
        );
        let rogue_http = acp::McpServer::Http(
            acp::McpServerHttp::new("rogue-http", "https://evil.example.com/mcp").headers(vec![]),
        );

        let tagged = apply_mcp_server_policy(
            vec![
                foreign(ui_kit),
                foreign(rogue_stdio),
                foreign(figma),
                foreign(rogue_http),
            ],
            &std::collections::HashSet::new(),
            &settings_with_policy(policy),
        );
        let surviving: Vec<&str> = tagged
            .iter()
            .filter(|s| s.disabled_reason.is_none())
            .map(|s| mcp_server_name(&s.server))
            .collect();
        assert_eq!(surviving, ["ui-kit", "figma"]);

        let rogue = tagged
            .iter()
            .find(|s| mcp_server_name(&s.server) == "rogue")
            .expect("rogue stdio present in policy output");
        assert!(
            matches!(
                rogue.disabled_reason,
                Some(McpBlockReason::NotGranted { .. })
            ),
            "lockdown drop must classify as allowlist miss, got {:?}",
            rogue.disabled_reason
        );
    }

    #[test]
    fn project_mcp_pin_drops_project_servers_unless_allowlisted() {
        use xai_grok_workspace::permission::resolution::{
            AllowedMcpServer, McpServerAllowlist, McpServerPolicy, PolicyLayerOwnership, PolicyPin,
        };

        let servers = || {
            vec![
                acp::McpServer::Http(
                    acp::McpServerHttp::new("projsrv", "https://proj.example.com/mcp")
                        .headers(vec![]),
                ),
                acp::McpServer::Http(
                    acp::McpServerHttp::new("projallowed", "https://allowed.example.com/mcp")
                        .headers(vec![]),
                ),
                acp::McpServer::Http(
                    acp::McpServerHttp::new("usersrv", "https://user.example.com/mcp")
                        .headers(vec![]),
                ),
            ]
        };
        let project_names: std::collections::HashSet<String> =
            ["projsrv".to_string(), "projallowed".to_string()]
                .into_iter()
                .collect();
        let policy = McpServerPolicy::single(
            McpServerAllowlist::new(
                vec![AllowedMcpServer::Http {
                    url_pattern: "https://allowed.example.com/*".into(),
                }],
                vec![],
                Some(std::path::PathBuf::from("/etc/grok/requirements.toml")),
            )
            .with_ownership(PolicyLayerOwnership::Admin),
        );
        let subject = |name: &str| McpSubject {
            origin: PolicySubjectOrigin::Foreign,
            project_scoped: project_names.contains(name),
        };
        let paired = || -> Vec<(acp::McpServer, McpSubject)> {
            servers()
                .into_iter()
                .map(|s| {
                    let subject = subject(mcp_server_name(&s));
                    (s, subject)
                })
                .collect()
        };

        let mut ms = settings_with_policy(policy.clone());
        ms.project_mcp = PolicyPin::Disabled {
            source: std::path::PathBuf::from("/etc/grok/requirements.toml"),
            ownership: PolicyLayerOwnership::Admin,
        };
        let kept = apply_mcp_server_policy(paired(), &std::collections::HashSet::new(), &ms);
        let names: Vec<&str> = kept.iter().map(|s| mcp_server_name(&s.server)).collect();
        assert_eq!(names, ["projallowed", "usersrv"]);

        let listed = servers();
        let reason = ms
            .mcp_project_pin_block(at(&listed, 0), subject("projsrv"))
            .expect("project server without a grant is dropped");
        assert!(
            matches!(&reason, McpBlockReason::ProjectPin { source }
                if source == std::path::Path::new("/etc/grok/requirements.toml")),
            "got {reason:?}"
        );
        assert!(
            ms.mcp_project_pin_block(at(&listed, 1), subject("projallowed"))
                .is_none(),
            "allowlisted project server is not dropped"
        );

        let kept = apply_mcp_server_policy(
            paired(),
            &std::collections::HashSet::new(),
            &settings_with_policy(policy),
        );
        assert_eq!(kept.len(), 3);
    }

    #[test]
    fn lower_precedence_http_servers_are_blocked_by_toml_name_claims() {
        let cwd = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(cwd.path().join(".grok")).unwrap();
        std::fs::write(
            cwd.path().join(".grok").join("config.toml"),
            r#"
[mcp_servers.github]
url = "https://config.example.com/mcp"
enabled = false
"#,
        )
        .unwrap();
        git2::Repository::init(cwd.path()).unwrap();
        std::fs::write(
            cwd.path().join(".mcp.json"),
            r#"{
                "mcpServers": {
                    "github": {
                        "url": "https://json.example.com/mcp"
                    }
                }
            }"#,
        )
        .unwrap();

        let compat = xai_grok_tools::types::compat::CompatConfig::default();
        let merged = merge_managed_mcp_servers(vec![], cwd.path(), None, &compat);
        assert!(
            !merged.iter().any(|server| matches!(
                server,
                acp::McpServer::Http(acp::McpServerHttp { name, .. }) if name == "github"
            )),
            "config.toml should block same-named lower-precedence HTTP servers"
        );
    }

    fn same_url_project_repo() -> tempfile::TempDir {
        let cwd = empty_cwd();
        std::fs::create_dir_all(cwd.path().join(".grok")).unwrap();
        std::fs::write(
            cwd.path().join(".grok").join("config.toml"),
            r#"
[mcp_servers.gb5207-org1]
url = "https://dup-url.example.test/mcp"

[mcp_servers.gb5207-org1.headers]
Authorization = "Bearer org1-token"

[mcp_servers.gb5207-org2]
url = "https://dup-url.example.test/mcp"

[mcp_servers.gb5207-org2.headers]
Authorization = "Bearer org2-token"
"#,
        )
        .unwrap();
        git2::Repository::init(cwd.path()).unwrap();
        crate::agent::folder_trust::record_for_test(cwd.path(), true);
        cwd
    }

    #[test]
    fn same_url_different_names_both_survive_merge() {
        let cwd = same_url_project_repo();
        let compat = xai_grok_tools::types::compat::CompatConfig::default();
        let merged = merge_managed_mcp_servers(vec![], cwd.path(), None, &compat);

        let auth_header = |name: &str| -> &str {
            let server = merged
                .iter()
                .find(|s| mcp_server_name(s) == name)
                .unwrap_or_else(|| panic!("{name} must survive the merge"));
            match server {
                acp::McpServer::Http(acp::McpServerHttp { headers, .. }) => headers
                    .iter()
                    .find(|h| h.name == "Authorization")
                    .unwrap_or_else(|| panic!("{name} must keep its Authorization header"))
                    .value
                    .as_str(),
                other => panic!("expected Http server, got {other:?}"),
            }
        };
        assert_eq!(auth_header("gb5207-org1"), "Bearer org1-token");
        assert_eq!(auth_header("gb5207-org2"), "Bearer org2-token");
    }

    #[test]
    fn same_url_different_names_both_sourced_from_toml() {
        let cwd = same_url_project_repo();
        let compat = xai_grok_tools::types::compat::CompatConfig::default();
        let sourced = merge_managed_mcp_servers_sourced(cwd.path(), None, &compat);

        for name in ["gb5207-org1", "gb5207-org2"] {
            let (_, source) = sourced
                .iter()
                .find(|(s, _)| mcp_server_name(s) == name)
                .unwrap_or_else(|| panic!("{name} must be in the sourced merge"));
            assert!(
                matches!(source, McpServerOrigin::ConfigToml { .. }),
                "{name} must be attributed to config.toml, got {source:?}"
            );
        }
    }

    #[test]
    fn untrusted_workspace_drops_project_mcp_servers() {
        fn repo_with_project_server() -> tempfile::TempDir {
            let cwd = tempfile::tempdir().unwrap();
            git2::Repository::init(cwd.path()).unwrap();
            std::fs::write(
                cwd.path().join(".mcp.json"),
                r#"{"mcpServers": {"projsrv": {"url": "https://proj.example.com/mcp"}}}"#,
            )
            .unwrap();
            cwd
        }
        let compat = xai_grok_tools::types::compat::CompatConfig::default();

        let untrusted = repo_with_project_server();
        crate::agent::folder_trust::record_for_test(untrusted.path(), false);
        let client = vec![acp::McpServer::Http(
            acp::McpServerHttp::new(
                "clientsrv".to_string(),
                "https://client.example.com/mcp".to_string(),
            )
            .headers(vec![]),
        )];
        let merged = merge_managed_mcp_servers(client, untrusted.path(), None, &compat);
        assert!(
            !merged.iter().any(|s| mcp_server_name(s) == "projsrv"),
            "untrusted workspace must drop its repo-local MCP server"
        );
        assert!(
            merged.iter().any(|s| mcp_server_name(s) == "clientsrv"),
            "client-supplied server must be retained when untrusted"
        );

        let trusted = repo_with_project_server();
        crate::agent::folder_trust::record_for_test(trusted.path(), true);
        let merged = merge_managed_mcp_servers(vec![], trusted.path(), None, &compat);
        assert!(
            merged.iter().any(|s| mcp_server_name(s) == "projsrv"),
            "trusted workspace must keep its repo-local MCP server"
        );
    }

    #[test]
    fn discovery_keeps_pin_blocked_project_definition_for_verdicts() {
        use xai_grok_workspace::permission::resolution::{PolicyLayerOwnership, PolicyPin};

        let cwd = empty_cwd();
        write_cursor_project_mcp(cwd.path(), "projsrv");
        let compat = xai_grok_tools::types::compat::CompatConfig::default();
        let inputs = McpDiscoveryInputs {
            cwd: cwd.path(),
            plugin_registry: None,
            compat: &compat,
        };
        let discovered = discover_mcp_definitions_ignoring_disable(&inputs);
        let (server, subject) = discovered
            .get("projsrv")
            .expect("project definition must stay discoverable under a pin");
        assert!(subject.project_scoped);

        let mut ms = xai_grok_workspace::permission::resolution::ManagedSettings::default();
        ms.project_mcp = PolicyPin::Disabled {
            source: std::path::PathBuf::from("/etc/grok/requirements.toml"),
            ownership: PolicyLayerOwnership::Admin,
        };
        match ms.mcp_verdict(server, *subject) {
            McpVerdict::Blocked(McpBlockReason::ProjectPin { source }) => {
                assert_eq!(
                    source,
                    std::path::PathBuf::from("/etc/grok/requirements.toml")
                );
            }
            other => panic!("expected ProjectPin refusal, got {other:?}"),
        }
    }

    #[test]
    fn plugin_mcp_disabled_server_excluded_from_merge() {
        let tmp = tempfile::tempdir().unwrap();
        let registry = plugin_registry_with_inline_server(
            &tmp.path().join("plug"),
            "plugsrv",
            "https://plug.example.test/mcp",
        );
        let cwd = empty_cwd();
        let compat = xai_grok_tools::types::compat::CompatConfig::default();

        let merged = merge_managed_mcp_servers(vec![], cwd.path(), Some(&registry), &compat);
        assert!(
            merged.iter().any(|s| mcp_server_name(s) == "plugsrv"),
            "plugin server must merge before the disable"
        );

        git2::Repository::init(cwd.path()).unwrap();
        std::fs::create_dir_all(cwd.path().join(".grok")).unwrap();
        std::fs::write(
            cwd.path().join(".grok").join("config.toml"),
            "[mcp_servers.plugsrv]\nurl = \"https://plug.example.test/mcp\"\nenabled = false\n",
        )
        .unwrap();
        let merged = merge_managed_mcp_servers(vec![], cwd.path(), Some(&registry), &compat);
        assert!(
            !merged.iter().any(|s| mcp_server_name(s) == "plugsrv"),
            "disabled plugin server must be dropped by the merge"
        );
    }

    #[test]
    fn plugin_server_deduped_across_file_and_inline() {
        use xai_grok_agent::plugins::PluginRegistry;
        use xai_grok_agent::plugins::PluginScope;
        use xai_grok_agent::plugins::discovery::{DiscoveredPlugin, PluginId};
        use xai_grok_agent::plugins::manifest::{PathOrInline, PluginManifest};

        let tmp = tempfile::tempdir().unwrap();
        let plugin_root = tmp.path().join("sentry");
        std::fs::create_dir_all(&plugin_root).unwrap();
        let mcp_json = plugin_root.join(".mcp.json");
        std::fs::write(
            &mcp_json,
            r#"{"mcpServers":{"sentry":{"type":"http","url":"https://mcp.sentry.dev/mcp"}}}"#,
        )
        .unwrap();

        let manifest = PluginManifest {
            name: "sentry".into(),
            version: None,
            description: None,
            author: None,
            homepage: None,
            repository: None,
            license: None,
            keywords: vec![],
            skills: None,
            commands: None,
            agents: None,
            hooks: None,
            mcp_servers: Some(PathOrInline::Inline(serde_json::json!({
                "sentry": { "type": "http", "url": "https://mcp.sentry.dev/mcp" }
            }))),
            lsp_servers: None,
        };
        let id = PluginId::new(PluginScope::User, &plugin_root, "sentry");
        let dp = DiscoveredPlugin {
            manifest,
            id,
            root: plugin_root.clone(),
            canonical_root: plugin_root.clone(),
            scope: PluginScope::User,
            origin: xai_grok_agent::plugins::PluginOrigin::UserGrok,
            trusted: true,
            skill_dirs: vec![],
            command_dirs: vec![],
            agent_dirs: vec![],
            hooks_path: None,
            mcp_config_path: Some(mcp_json),
            lsp_config_path: None,
            conflict: None,
        };
        let registry = PluginRegistry::from_discovered(vec![dp], &[], &["sentry".to_string()]);

        let cwd = tempfile::tempdir().unwrap();
        let compat = xai_grok_tools::types::compat::CompatConfig::default();
        let sourced = merge_managed_mcp_servers_sourced(cwd.path(), Some(&registry), &compat);

        let sentry_count = sourced
            .iter()
            .filter(|(s, _)| mcp_server_name(s) == "sentry")
            .count();
        assert_eq!(
            sentry_count, 1,
            "sentry declared in both .mcp.json and inline must register exactly once"
        );
    }

    #[test]
    fn plugin_same_name_different_url_keeps_file_server() {
        use xai_grok_agent::plugins::PluginRegistry;
        use xai_grok_agent::plugins::PluginScope;
        use xai_grok_agent::plugins::discovery::{DiscoveredPlugin, PluginId};
        use xai_grok_agent::plugins::manifest::{PathOrInline, PluginManifest};

        let tmp = tempfile::tempdir().unwrap();
        let plugin_root = tmp.path().join("sentry");
        std::fs::create_dir_all(&plugin_root).unwrap();
        let mcp_json = plugin_root.join(".mcp.json");
        std::fs::write(
            &mcp_json,
            r#"{"mcpServers":{"sentry":{"type":"http","url":"https://file.example/mcp"}}}"#,
        )
        .unwrap();

        let manifest = PluginManifest {
            name: "sentry".into(),
            version: None,
            description: None,
            author: None,
            homepage: None,
            repository: None,
            license: None,
            keywords: vec![],
            skills: None,
            commands: None,
            agents: None,
            hooks: None,
            mcp_servers: Some(PathOrInline::Inline(serde_json::json!({
                "sentry": { "type": "http", "url": "https://inline.example/mcp" }
            }))),
            lsp_servers: None,
        };
        let id = PluginId::new(PluginScope::User, &plugin_root, "sentry");
        let dp = DiscoveredPlugin {
            manifest,
            id,
            root: plugin_root.clone(),
            canonical_root: plugin_root.clone(),
            scope: PluginScope::User,
            origin: xai_grok_agent::plugins::PluginOrigin::UserGrok,
            trusted: true,
            skill_dirs: vec![],
            command_dirs: vec![],
            agent_dirs: vec![],
            hooks_path: None,
            mcp_config_path: Some(mcp_json),
            lsp_config_path: None,
            conflict: None,
        };
        let registry = PluginRegistry::from_discovered(vec![dp], &[], &["sentry".to_string()]);

        let cwd = tempfile::tempdir().unwrap();
        let compat = xai_grok_tools::types::compat::CompatConfig::default();
        let sourced = merge_managed_mcp_servers_sourced(cwd.path(), Some(&registry), &compat);

        let sentry: Vec<&acp::McpServer> = sourced
            .iter()
            .map(|(s, _)| s)
            .filter(|s| mcp_server_name(s) == "sentry")
            .collect();
        assert_eq!(
            sentry.len(),
            1,
            "same plugin declaring one server name twice must register exactly once"
        );
        match at(&sentry, 0) {
            acp::McpServer::Http(acp::McpServerHttp { url, .. }) => {
                assert_eq!(url, "https://file.example/mcp", "file source must win");
            }
            other => panic!("expected Http server, got {:?}", other),
        }
    }

    #[test]
    fn collect_plugin_oauth_configs_reads_byo_client_id_from_mcp_json() {
        use xai_grok_agent::plugins::PluginRegistry;
        use xai_grok_agent::plugins::PluginScope;
        use xai_grok_agent::plugins::discovery::{DiscoveredPlugin, PluginId};
        use xai_grok_agent::plugins::manifest::PluginManifest;

        let tmp = tempfile::tempdir().unwrap();
        let plugin_root = tmp.path().join("slack");
        std::fs::create_dir_all(&plugin_root).unwrap();
        let mcp_json = plugin_root.join(".mcp.json");
        std::fs::write(
            &mcp_json,
            r#"{"mcpServers":{"slack":{"type":"http","url":"https://mcp.slack.example/mcp","oauth":{"clientId":"slack-byo-client","callbackPort":3118}}}}"#,
        )
        .unwrap();

        let manifest = PluginManifest {
            name: "slack".into(),
            version: None,
            description: None,
            author: None,
            homepage: None,
            repository: None,
            license: None,
            keywords: vec![],
            skills: None,
            commands: None,
            agents: None,
            hooks: None,
            mcp_servers: None,
            lsp_servers: None,
        };
        let id = PluginId::new(PluginScope::User, &plugin_root, "slack");
        let dp = DiscoveredPlugin {
            manifest,
            id,
            root: plugin_root.clone(),
            canonical_root: plugin_root.clone(),
            scope: PluginScope::User,
            origin: xai_grok_agent::plugins::PluginOrigin::UserGrok,
            trusted: true,
            skill_dirs: vec![],
            command_dirs: vec![],
            agent_dirs: vec![],
            hooks_path: None,
            mcp_config_path: Some(mcp_json),
            lsp_config_path: None,
            conflict: None,
        };
        let registry = PluginRegistry::from_discovered(vec![dp], &[], &["slack".to_string()]);

        let oauth = collect_plugin_oauth_configs(Some(&registry));
        assert_eq!(
            oauth
                .get("slack")
                .expect("slack oauth")
                .client_id
                .as_deref(),
            Some("slack-byo-client")
        );
    }

    #[test]
    fn collect_plugin_oauth_configs_none_registry_is_empty() {
        assert!(collect_plugin_oauth_configs(None).is_empty());
    }

    #[test]
    fn merge_plugin_oauth_respects_source_precedence() {
        use crate::util::config::{McpOAuthConfig, McpOAuthConfigMap};

        let byo = |id: &str| McpOAuthConfig {
            client_id: Some(id.to_string()),
            ..Default::default()
        };

        let mut base = McpOAuthConfigMap::new();
        base.insert("shared".to_string(), byo("file-client"));
        base.insert("toml-svc".to_string(), byo("toml-client"));

        let mut plugin = McpOAuthConfigMap::new();
        plugin.insert("shared".to_string(), byo("plugin-client"));
        plugin.insert("toml-svc".to_string(), byo("plugin-shadow"));
        plugin.insert("plugin-only".to_string(), byo("plugin-only-client"));

        let toml_names: std::collections::HashSet<String> =
            ["toml-svc".to_string()].into_iter().collect();
        merge_plugin_oauth_into(&mut base, plugin, &toml_names);

        assert_eq!(
            base.get("shared").unwrap().client_id.as_deref(),
            Some("plugin-client")
        );
        assert_eq!(
            base.get("toml-svc").unwrap().client_id.as_deref(),
            Some("toml-client")
        );
        assert_eq!(
            base.get("plugin-only").unwrap().client_id.as_deref(),
            Some("plugin-only-client")
        );
    }
}
