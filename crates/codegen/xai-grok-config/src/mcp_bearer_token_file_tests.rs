use super::*;
use crate::{McpServerConfig, McpServerTransportConfig};

fn meta_for(configured: &str) -> Option<acp::Meta> {
    let config = McpServerConfig {
        transport: McpServerTransportConfig::StreamableHttp {
            url: "https://x/mcp".into(),
            transport_type: None,
            bearer_token_env_var: None,
            bearer_token_file: Some(configured.into()),
            headers: None,
            oauth_client_id: None,
            oauth_client_secret_env_var: None,
            oauth_scopes: None,
        },
        enabled: true,
        oauth: None,
        setup: None,
        startup_timeout_sec: None,
        tool_timeout_sec: None,
        tool_timeouts: None,
        expose_image_base64: None,
    };
    let Some(acp::McpServer::Http(server)) = config.to_acp_mcp_server("x") else {
        panic!("expected an http server");
    };
    server.meta
}

#[test]
fn absolute_path_round_trips_through_server_meta() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("token");
    let configured = path.to_str().expect("utf-8 tempdir");

    let decoded = BearerTokenPath::from_meta(meta_for(configured).as_ref());

    assert_eq!(Ok(Some(BearerTokenPath(path))), decoded);
}

#[test]
fn server_without_token_file_has_none() {
    assert_eq!(Ok(None), BearerTokenPath::from_meta(None));
}

#[test]
fn home_prefix_expands_against_home() {
    let home = tempfile::tempdir().expect("tempdir");
    assert_eq!(
        Ok(BearerTokenPath(home.path().join(".tok"))),
        BearerTokenPath::parse("~/.tok", Some(home.path()))
    );
}

#[test]
fn home_prefix_without_home_is_rejected() {
    assert_eq!(
        Err(BearerTokenPathError::NoHomeDir("~/.tok".to_owned())),
        BearerTokenPath::parse("~/.tok", None)
    );
}

#[test]
fn relative_path_fails_the_server() {
    assert_eq!(
        Err(BearerTokenPathError::NotAbsolute("tok".to_owned())),
        BearerTokenPath::from_meta(meta_for("tok").as_ref())
    );
}

#[test]
fn non_string_meta_value_is_rejected() {
    let meta = acp::Meta::from_iter([(META_KEY.to_owned(), serde_json::json!(7))]);
    assert_eq!(
        Err(BearerTokenPathError::NotAString),
        BearerTokenPath::from_meta(Some(&meta))
    );
}

#[test]
fn client_listing_keeps_the_configured_string() {
    assert_eq!(
        Some("~/.tok".to_owned()),
        configured_bearer_token_file(meta_for("~/.tok").as_ref())
    );
}
