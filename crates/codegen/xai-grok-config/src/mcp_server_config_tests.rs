use super::*;

#[test]
fn transport_less_entry_fails_to_deserialize() {
    for value in [
        serde_json::json!({ "enabled": false }),
        serde_json::json!({ "enabled": true }),
        serde_json::json!({}),
    ] {
        assert!(
            serde_json::from_value::<McpServerConfig>(value.clone()).is_err(),
            "transport-less entry must not deserialize: {value}"
        );
    }
}

#[test]
fn blank_transport_field_detects_blank_url_and_blank_command() {
    let blank_url: McpServerConfig =
        serde_json::from_value(serde_json::json!({ "url": "  " })).expect("blank url parses");
    assert_eq!(Some("url"), blank_url.blank_transport_field());

    let blank_command: McpServerConfig =
        serde_json::from_value(serde_json::json!({ "command": "\t" }))
            .expect("blank command parses");
    assert_eq!(Some("command"), blank_command.blank_transport_field());

    let ok: McpServerConfig = serde_json::from_value(serde_json::json!({ "command": "npx" }))
        .expect("stdio server parses");
    assert_eq!(None, ok.blank_transport_field());
}

#[test]
fn known_mcp_server_fields_cover_serialized_keys() {
    let stdio = McpServerConfig {
        transport: McpServerTransportConfig::Stdio {
            command: "npx".into(),
            args: vec!["-y".into()],
            env: Some(HashMap::from([("A".into(), "b".into())])),
            cwd: Some("/tmp".into()),
        },
        enabled: true,
        oauth: Some(McpJsonOAuthBlock::default()),
        setup: None,
        startup_timeout_sec: Some(10),
        tool_timeout_sec: Some(20),
        tool_timeouts: Some(HashMap::from([("t".into(), 1)])),
        expose_image_base64: Some(true),
    };
    let http = McpServerConfig {
        transport: McpServerTransportConfig::StreamableHttp {
            url: "https://x/mcp".into(),
            transport_type: Some("http".into()),
            bearer_token_env_var: Some("TOK".into()),
            bearer_token_file: Some("/run/tok".into()),
            headers: Some(HashMap::from([("H".into(), "v".into())])),
            oauth_client_id: Some("id".into()),
            oauth_client_secret_env_var: Some("SEC".into()),
            oauth_scopes: Some(vec!["s".into()]),
        },
        enabled: true,
        oauth: None,
        setup: None,
        startup_timeout_sec: None,
        tool_timeout_sec: None,
        tool_timeouts: None,
        expose_image_base64: None,
    };

    for config in [stdio, http] {
        let value = serde_json::to_value(&config).expect("server config serializes");
        let object = value
            .as_object()
            .expect("server config serializes to an object");
        for key in object.keys() {
            assert!(
                KNOWN_MCP_SERVER_FIELDS.contains(&key.as_str()),
                "field `{key}` is serialized but missing from KNOWN_MCP_SERVER_FIELDS"
            );
        }
    }
}

#[test]
fn mcp_setup_schema_parses_and_missing_preference_requires_setup() {
    let config: McpConfig =
        serde_json::from_str(site_select_setup_json()).expect("setup fixture parses");
    let server = config
        .mcp_servers
        .get("acme")
        .expect("fixture defines acme");

    let setup = server.setup.as_ref().expect("acme declares a setup schema");
    let [field] = setup.fields.as_slice() else {
        panic!("expected exactly one setup field: {:?}", setup.fields);
    };
    assert_eq!("site", field.id);
    assert_eq!(Some("us1"), field.default.as_deref());
    assert!(setup.variables.contains_key("url"));

    assert!(matches!(
        server.resolve_setup(None),
        McpSetupResolution::Required(_)
    ));
    assert!(server.to_acp_mcp_server("acme").is_none());
}

#[test]
fn mcp_setup_valid_preference_resolves_mapped_url() {
    let config: McpConfig =
        serde_json::from_str(site_select_setup_json()).expect("setup fixture parses");
    let server = config
        .mcp_servers
        .get("acme")
        .expect("fixture defines acme");
    let prefs = site_preferences("us5");

    let resolved = match server.resolve_setup(Some(&prefs)) {
        McpSetupResolution::Resolved(config) => config,
        other => panic!("expected resolved config, got {other:?}"),
    };

    assert!(resolved.to_acp_mcp_server("acme").is_some());
    match &resolved.transport {
        McpServerTransportConfig::StreamableHttp { url, .. } => {
            assert_eq!("https://mcp.us5.example.com/v1/mcp", url);
        }
        _ => panic!("expected http config"),
    }
}

#[test]
fn mcp_setup_invalid_preference_value_requires_setup() {
    let setup = McpSetupConfig {
        fields: vec![McpSetupField {
            id: "site".into(),
            label: "Site".into(),
            field_type: McpSetupFieldType::Select,
            required: true,
            default: Some("us1".into()),
            options: vec![McpSetupOption {
                label: "US1".into(),
                value: "us1".into(),
            }],
        }],
        variables: HashMap::new(),
    };
    let config = http_server_with_setup("{{url}}", setup);
    let prefs = site_preferences("us5");

    assert!(matches!(
        config.resolve_setup(Some(&prefs)),
        McpSetupResolution::Required(_)
    ));
}

#[test]
fn mcp_setup_multi_field_schema_is_invalid() {
    let setup = McpSetupConfig {
        fields: vec![
            McpSetupField {
                id: "a".into(),
                label: "A".into(),
                field_type: McpSetupFieldType::Select,
                required: true,
                default: None,
                options: vec![McpSetupOption {
                    label: "1".into(),
                    value: "1".into(),
                }],
            },
            McpSetupField {
                id: "b".into(),
                label: "B".into(),
                field_type: McpSetupFieldType::Select,
                required: true,
                default: None,
                options: vec![McpSetupOption {
                    label: "2".into(),
                    value: "2".into(),
                }],
            },
        ],
        variables: HashMap::new(),
    };
    let config = http_server_with_setup("https://example.com", setup);

    assert!(matches!(
        config.resolve_setup(None),
        McpSetupResolution::Invalid(_)
    ));
    assert!(config.to_acp_mcp_server("x").is_none());
}

#[test]
fn oauth_settings_take_the_flat_fields_before_the_oauth_block() {
    const URL: &str = "https://docs.example.test/mcp";
    let flat = McpServerOAuth {
        client_id: "flat".to_owned(),
        client_secret_env_var: Some("FLAT_SECRET".to_owned()),
        scopes: Some(vec!["read".to_owned()]),
        callback_port: None,
    };
    let block = McpServerOAuth {
        client_id: "block".to_owned(),
        client_secret_env_var: Some("BLOCK_SECRET".to_owned()),
        scopes: None,
        callback_port: Some(3118),
    };
    let oauth_block = serde_json::json!({
        "clientId": "block",
        "clientSecretEnvVar": "BLOCK_SECRET",
        "callbackPort": 3118,
    });
    let cases = [
        (serde_json::json!({ "url": URL }), None),
        (
            serde_json::json!({
                "url": URL,
                "oauth_client_id": "flat",
                "oauth_client_secret_env_var": "FLAT_SECRET",
                "oauth_scopes": ["read"],
            }),
            Some(flat.clone()),
        ),
        (
            serde_json::json!({ "url": URL, "oauth": oauth_block.clone() }),
            Some(block),
        ),
        (
            serde_json::json!({ "url": URL, "oauth": { "callbackPort": 3118 } }),
            None,
        ),
        (
            serde_json::json!({
                "url": URL,
                "oauth_client_id": "flat",
                "oauth_client_secret_env_var": "FLAT_SECRET",
                "oauth_scopes": ["read"],
                "oauth": oauth_block,
            }),
            Some(flat),
        ),
    ];

    for (entry, expected) in cases {
        let config: McpServerConfig =
            serde_json::from_value(entry.clone()).expect("http server parses");

        assert_eq!(expected, config.oauth_settings(), "{entry}");
    }
}

fn site_select_setup_json() -> &'static str {
    r#"{
        "mcpServers": {
            "acme": {
                "type": "http",
                "urlTemplate": "{{url}}",
                "setup": {
                    "fields": [{
                        "id": "site",
                        "label": "Site",
                        "type": "select",
                        "required": true,
                        "default": "us1",
                        "options": [
                            {"label": "US1", "value": "us1"},
                            {"label": "US5", "value": "us5"}
                        ]
                    }],
                    "values": {
                        "url": {
                            "from": "site",
                            "map": {
                                "us1": "https://mcp.example.com/v1/mcp",
                                "us5": "https://mcp.us5.example.com/v1/mcp"
                            }
                        }
                    }
                }
            }
        }
    }"#
}

fn site_preferences(site: &str) -> McpServerPreferences {
    McpServerPreferences {
        values: HashMap::from([("site".to_string(), site.to_string())]),
        source: None,
        updated_at: None,
    }
}

fn http_server_with_setup(url: &str, setup: McpSetupConfig) -> McpServerConfig {
    McpServerConfig {
        transport: McpServerTransportConfig::StreamableHttp {
            url: url.into(),
            transport_type: None,
            bearer_token_env_var: None,
            bearer_token_file: None,
            headers: None,
            oauth_client_id: None,
            oauth_client_secret_env_var: None,
            oauth_scopes: None,
        },
        enabled: true,
        oauth: None,
        setup: Some(setup),
        startup_timeout_sec: None,
        tool_timeout_sec: None,
        tool_timeouts: None,
        expose_image_base64: None,
    }
}

#[test]
fn setup_templates_render_bearer_token_file() {
    let config: McpServerConfig = serde_json::from_value(serde_json::json!({
        "url": "https://x/mcp",
        "bearer_token_file": "{{dir}}/token",
        "setup": {
            "fields": [{
                "id": "site",
                "label": "Site",
                "type": "select",
                "options": [{"label": "US1", "value": "us1"}]
            }],
            "values": {"dir": {"from": "site", "map": {"us1": "/run/us1"}}}
        }
    }))
    .expect("setup server parses");

    let McpSetupResolution::Resolved(resolved) =
        config.resolve_setup(Some(&site_preferences("us1")))
    else {
        panic!("expected resolved config");
    };
    let McpServerTransportConfig::StreamableHttp {
        bearer_token_file, ..
    } = resolved.transport
    else {
        panic!("expected http config");
    };
    assert_eq!(Some("/run/us1/token".to_owned()), bearer_token_file);
}
