use super::*;

fn endpoints(
    proxy: &str,
    models_base_url: Option<&str>,
    models_list_url: Option<&str>,
) -> EndpointsConfig {
    EndpointsConfig {
        cli_chat_proxy_base_url: Some(proxy.to_owned()),
        models_base_url: models_base_url.map(|s| s.to_owned()),
        models_list_url: models_list_url.map(|s| s.to_owned()),
        ..Default::default()
    }
}

#[test]
fn inference_url_defaults_to_proxy() {
    let ep = endpoints("https://proxy.grok.com/v1", None, None);
    assert_eq!(ep.resolve_inference_base_url(), "https://proxy.grok.com/v1");
}

#[test]
fn inference_url_uses_models_base_url() {
    let ep = endpoints(
        "https://proxy.grok.com/v1",
        Some("https://enterprise.acme.com/v1"),
        None,
    );
    assert_eq!(
        ep.resolve_inference_base_url(),
        "https://enterprise.acme.com/v1"
    );
}

#[test]
fn inference_url_ignores_models_list_url() {
    let ep = endpoints(
        "https://proxy.grok.com/v1",
        Some("https://inference.acme.com/v1"),
        Some("https://registry.acme.com/api/models"),
    );
    assert_eq!(
        ep.resolve_inference_base_url(),
        "https://inference.acme.com/v1"
    );
}

#[test]
fn list_url_defaults_to_proxy_models() {
    let ep = endpoints("https://proxy.grok.com/v1", None, None);
    assert_eq!(
        ep.resolve_models_list_url(),
        "https://proxy.grok.com/v1/models"
    );
}

#[test]
fn list_url_derived_from_base_url() {
    let ep = endpoints(
        "https://proxy.grok.com/v1",
        Some("https://api.x.ai/v1"),
        None,
    );
    assert_eq!(ep.resolve_models_list_url(), "https://api.x.ai/v1/models");
}

#[test]
fn list_url_explicit_overrides_derivation() {
    let ep = endpoints(
        "https://proxy.grok.com/v1",
        Some("https://inference.acme.com/v1"),
        Some("https://registry.acme.com/api/list-models"),
    );
    assert_eq!(
        ep.resolve_models_list_url(),
        "https://registry.acme.com/api/list-models"
    );
}

#[test]
fn otlp_traces_endpoint_precedence() {
    let proxy = "https://inference.acme.com/v1".to_string();

    let derived = EndpointsConfig {
        cli_chat_proxy_base_url: Some(proxy.clone()),
        ..Default::default()
    };
    assert_eq!(
        derived.resolve_otlp_traces_endpoint(),
        "https://inference.acme.com/v1/traces"
    );

    let base = EndpointsConfig {
        cli_chat_proxy_base_url: Some(proxy.clone()),
        otel_exporter_otlp_endpoint: Some("https://otel.acme.com".to_string()),
        ..Default::default()
    };
    assert_eq!(
        base.resolve_otlp_traces_endpoint(),
        "https://otel.acme.com/v1/traces"
    );

    let full = EndpointsConfig {
        cli_chat_proxy_base_url: Some(proxy),
        otel_exporter_otlp_endpoint: Some("https://ignored.example".to_string()),
        otel_exporter_otlp_traces_endpoint: Some("https://otel.acme.com/v1/traces".to_string()),
        ..Default::default()
    };
    assert_eq!(
        full.resolve_otlp_traces_endpoint(),
        "https://otel.acme.com/v1/traces"
    );
}

#[test]
fn otlp_headers_trim_whitespace_and_skip_blank_keys() {
    let cfg = EndpointsConfig {
        otel_exporter_otlp_headers: Some("a=1, b = 2 ,=skip,c=".to_string()),
        ..Default::default()
    };

    assert_eq!(
        cfg.resolve_otlp_headers(),
        vec![
            ("a".to_string(), "1".to_string()),
            ("b".to_string(), "2".to_string()),
            ("c".to_string(), String::new()),
        ]
    );
}

/// A config with a fixed proxy URL, every OTLP field unset, and the master switch off.
/// `EndpointsConfig::default()` would fill each of these from env vars and config files.
fn internal_otlp_test_config() -> EndpointsConfig {
    EndpointsConfig {
        cli_chat_proxy_base_url: Some("https://proxy.example/v1".to_string()),
        otel_exporter_otlp_endpoint: None,
        otel_exporter_otlp_traces_endpoint: None,
        otel_exporter_otlp_headers: None,
        grok_internal_otlp_traces_endpoint: None,
        grok_internal_otlp_headers: None,
        external_otel_master_switch: false,
        ..Default::default()
    }
}

#[test]
fn internal_otlp_endpoint_grok_internal_wins_regardless_of_switch() {
    for switch in [false, true] {
        let cfg = EndpointsConfig {
            grok_internal_otlp_traces_endpoint: Some(
                "https://internal.example/traces/".to_string(),
            ),
            otel_exporter_otlp_traces_endpoint: Some(
                "https://legacy.example/v1/traces".to_string(),
            ),
            otel_exporter_otlp_endpoint: Some("https://legacy-base.example".to_string()),
            external_otel_master_switch: switch,
            ..internal_otlp_test_config()
        };
        assert_eq!(
            cfg.resolve_otlp_traces_endpoint(),
            "https://internal.example/traces",
            "switch={switch}: GROK_INTERNAL_OTLP_TRACES_ENDPOINT must win verbatim (trailing / trimmed)"
        );
    }
}

#[test]
fn internal_otlp_endpoint_legacy_fallback_when_switch_unset() {
    let traces = EndpointsConfig {
        otel_exporter_otlp_traces_endpoint: Some("https://legacy.example/v1/traces".to_string()),
        ..internal_otlp_test_config()
    };
    assert_eq!(
        traces.resolve_otlp_traces_endpoint(),
        "https://legacy.example/v1/traces"
    );

    let base = EndpointsConfig {
        otel_exporter_otlp_endpoint: Some("https://legacy-base.example/".to_string()),
        ..internal_otlp_test_config()
    };
    assert_eq!(
        base.resolve_otlp_traces_endpoint(),
        "https://legacy-base.example/v1/traces"
    );
}

#[test]
fn internal_otlp_ignores_legacy_vars_when_switch_set() {
    let cfg = EndpointsConfig {
        otel_exporter_otlp_traces_endpoint: Some(
            "https://admin-collector.example/v1/traces".to_string(),
        ),
        otel_exporter_otlp_endpoint: Some("https://admin-collector.example".to_string()),
        otel_exporter_otlp_headers: Some("authorization=Bearer admin".to_string()),
        external_otel_master_switch: true,
        ..internal_otlp_test_config()
    };

    assert_eq!(
        cfg.resolve_otlp_traces_endpoint(),
        "https://proxy.example/v1/traces",
        "internal firehose must never follow OTEL_* to the external collector"
    );
    assert_eq!(cfg.resolve_otlp_headers(), Vec::<(String, String)>::new());
}

#[test]
fn internal_otlp_consumed_standard_vars_cases() {
    struct Case {
        switch: bool,
        legacy_traces_ep: bool,
        legacy_base_ep: bool,
        legacy_headers: bool,
        internal_ep: bool,
        internal_headers: bool,
        expected: bool,
        why: &'static str,
    }

    let unset = Case {
        switch: false,
        legacy_traces_ep: false,
        legacy_base_ep: false,
        legacy_headers: false,
        internal_ep: false,
        internal_headers: false,
        expected: false,
        why: "nothing set",
    };
    let cases = [
        Case { ..unset },
        Case {
            legacy_traces_ep: true,
            expected: true,
            why: "legacy traces endpoint consumed",
            ..unset
        },
        Case {
            legacy_base_ep: true,
            expected: true,
            why: "legacy base endpoint consumed",
            ..unset
        },
        Case {
            legacy_headers: true,
            expected: true,
            why: "legacy headers consumed",
            ..unset
        },
        Case {
            legacy_traces_ep: true,
            internal_ep: true,
            expected: false,
            why: "internal endpoint shadows legacy",
            ..unset
        },
        Case {
            legacy_headers: true,
            internal_headers: true,
            expected: false,
            why: "internal headers shadow legacy",
            ..unset
        },
        Case {
            legacy_traces_ep: true,
            legacy_headers: true,
            internal_ep: true,
            expected: true,
            why: "endpoint shadowed but legacy headers still consumed (headers half)",
            ..unset
        },
        Case {
            switch: true,
            legacy_traces_ep: true,
            legacy_base_ep: true,
            legacy_headers: true,
            expected: false,
            why: "switch set: legacy vars ignored",
            ..unset
        },
    ];

    for case in cases {
        let cfg = EndpointsConfig {
            external_otel_master_switch: case.switch,
            otel_exporter_otlp_traces_endpoint: case
                .legacy_traces_ep
                .then(|| "https://legacy.example/v1/traces".to_string()),
            otel_exporter_otlp_endpoint: case
                .legacy_base_ep
                .then(|| "https://legacy-base.example".to_string()),
            otel_exporter_otlp_headers: case.legacy_headers.then(|| "k=v".to_string()),
            grok_internal_otlp_traces_endpoint: case
                .internal_ep
                .then(|| "https://internal.example/traces".to_string()),
            grok_internal_otlp_headers: case.internal_headers.then(|| "ik=iv".to_string()),
            ..internal_otlp_test_config()
        };
        assert_eq!(
            cfg.internal_otlp_consumed_standard_vars(),
            case.expected,
            "case: {}",
            case.why
        );
    }
}

#[test]
fn internal_otlp_headers_precedence() {
    for switch in [false, true] {
        let cfg = EndpointsConfig {
            grok_internal_otlp_headers: Some("x-debug=1".to_string()),
            otel_exporter_otlp_headers: Some("legacy=1".to_string()),
            external_otel_master_switch: switch,
            ..internal_otlp_test_config()
        };
        assert_eq!(
            cfg.resolve_otlp_headers(),
            vec![("x-debug".to_string(), "1".to_string())],
            "switch={switch}"
        );
    }

    let legacy = EndpointsConfig {
        otel_exporter_otlp_headers: Some("legacy=1".to_string()),
        ..internal_otlp_test_config()
    };
    assert_eq!(
        legacy.resolve_otlp_headers(),
        vec![("legacy".to_string(), "1".to_string())]
    );
}

#[test]
fn external_otel_master_switch_resolves_from_all_layers() {
    let enabled_table: toml::Value =
        toml::from_str("[telemetry]\notel_enabled = true").expect("enabled table parses");
    let disabled_table: toml::Value =
        toml::from_str("[telemetry]\notel_enabled = false").expect("disabled table parses");

    assert!(external_otel_master_switch_from(
        None,
        None,
        Some(&enabled_table)
    ));

    assert!(!external_otel_master_switch_from(None, None, None));

    assert!(!external_otel_master_switch_from(
        None,
        Some(false),
        Some(&enabled_table)
    ));
    assert!(external_otel_master_switch_from(
        None,
        Some(true),
        Some(&disabled_table)
    ));

    assert!(!external_otel_master_switch_from(
        Some(&disabled_table),
        Some(true),
        Some(&enabled_table)
    ));
    assert!(external_otel_master_switch_from(
        Some(&enabled_table),
        Some(false),
        None
    ));
}
