use super::*;

fn parse(text: &str) -> RequirementsToml {
    toml::from_str(text).expect("requirements text is valid TOML")
}

fn list(patterns: &[&str]) -> AllowlistPin {
    AllowlistPin::List(
        patterns
            .iter()
            .map(|pattern| (*pattern).to_owned())
            .collect(),
    )
}

#[test]
fn malformed_allowed_models_fail_closed() {
    let cases = [
        (
            "[models]\nallowed_models = \"grok-4\"\n",
            AllowlistPin::FailClosed,
        ),
        (
            "[models]\nallowed_models = [\"grok-4\", 4]\n",
            AllowlistPin::FailClosed,
        ),
    ];
    for (text, expected) in cases {
        assert_eq!(
            Some(expected),
            parse(text).models.allowed_models,
            "{text:?}"
        );
    }

    let text =
        "[models]\ndefault = 4\nallowed_models = [\"grok-4\"]\n[features]\nimage_gen = false\n";
    let parsed = parse(text);
    assert_eq!(None, parsed.models.default_model);
    assert_eq!(Some(list(&["grok-4"])), parsed.models.allowed_models);
    assert_eq!(Some(false), parsed.features.image_gen);
}

#[test]
fn tool_pins_keep_booleans_and_skip_other_types() {
    let parsed = parse(
        "\
[features]
image_gen = false
web_fetch = true
video_gen = \"off\"
voice_mode = false
",
    );
    assert_eq!(Some(false), parsed.features.image_gen);
    assert_eq!(Some(true), parsed.features.web_fetch);
    assert_eq!(None, parsed.features.video_gen);
    assert_eq!(None, parsed.features.ask_user_question);
    assert_eq!(
        vec![
            (ToolFeature::ImageGen, false),
            (ToolFeature::WebFetch, true)
        ],
        parsed.tool_pins().collect::<Vec<_>>()
    );
}

#[test]
fn a_bad_bool_or_string_is_skipped() {
    let cases = [
        (
            "[cli]\nauto_update = \"no\"\nchannel = \"stable\"\n",
            None,
            Some("stable"),
        ),
        (
            "[cli]\nauto_update = false\nchannel = 1\n",
            Some(false),
            None,
        ),
    ];

    for (text, auto_update, channel) in cases {
        let parsed = parse(text);
        assert_eq!(auto_update, parsed.cli.auto_update, "{text:?}");
        assert_eq!(channel, parsed.cli.channel.as_deref(), "{text:?}");
    }
}

#[test]
fn each_requirement_key_writes_its_field_path_and_redaction() {
    let parsed = parse(
        "\
[cli]
auto_update = false
use_leader = true
show_tips = false
channel = \"stable\"
minimum_version = \"1.0.0\"
maximum_version = \"2.0.0\"
required_minimum_version = \"1.2.0\"
required_maximum_version = \"1.9.0\"

[memory]
enabled = false

[subagents]
enabled = true

[managed_mcps]
enabled = false

[models]
web_search = \"https://search.example\"

[endpoints]
models_base_url = \"https://models.example\"
models_list_url = \"https://models.example/list\"
trace_upload_url = \"https://traces.example\"
feedback_base_url = \"https://feedback.example\"
deployment_key = \"deploy-secret\"
trace_upload_bucket = \"trace-bucket\"
trace_upload_region = \"us-east-1\"
trace_upload_credentials_file = \"/var/creds\"
trace_upload_endpoint_url = \"https://upload.example\"
trace_upload_credentials = \"upload-secret\"

[telemetry]
events_url = \"https://events.example\"
events_api_key = \"events-secret\"
mixpanel_enabled = true
mixpanel_token = \"mix-secret\"
",
    );

    let mut auto_update = Some(true);
    let mut use_leader = Some(false);
    let mut show_tips = Some(true);
    let mut memory_enabled = Some(true);
    let mut subagents_enabled = false;
    let mut managed_mcps_enabled = true;
    let mut mixpanel_enabled = false;
    let mut channel = Some("other".to_owned());
    let mut minimum_version = Some("other".to_owned());
    let mut maximum_version = Some("other".to_owned());
    let mut required_minimum_version = Some("other".to_owned());
    let mut required_maximum_version = Some("other".to_owned());
    let mut web_search = Some("other".to_owned());
    let mut models_base_url = Some("other".to_owned());
    let mut models_list_url = Some("other".to_owned());
    let mut trace_upload_url = Some("other".to_owned());
    let mut feedback_base_url = Some("other".to_owned());
    let mut deployment_key = Some("other".to_owned());
    let mut events_url = Some("other".to_owned());
    let mut events_api_key = Some("other".to_owned());
    let mut mixpanel_token = Some("other".to_owned());
    let mut trace_upload_bucket = Some("other".to_owned());
    let mut trace_upload_region = Some("other".to_owned());
    let mut trace_upload_credentials_file = Some("other".to_owned());
    let mut trace_upload_endpoint_url = Some("other".to_owned());
    let mut trace_upload_credentials = Some("other".to_owned());

    let mut pushes = Vec::new();
    let mut push = |path: &'static str, shown: String| pushes.push((path, shown));

    parsed.enforce_cli_toggles(&mut auto_update, &mut use_leader, &mut show_tips, &mut push);
    parsed.enforce_service_toggles(
        &mut ServiceTogglePins {
            memory_enabled: &mut memory_enabled,
            subagents_enabled: &mut subagents_enabled,
            managed_mcps_enabled: &mut managed_mcps_enabled,
        },
        &mut push,
    );
    parsed.enforce_web_search(&mut web_search, &mut push);
    parsed.enforce_cli_strings(
        &mut CliStringPins {
            channel: &mut channel,
            minimum_version: &mut minimum_version,
            maximum_version: &mut maximum_version,
            required_minimum_version: &mut required_minimum_version,
            required_maximum_version: &mut required_maximum_version,
        },
        &mut push,
    );
    parsed.enforce_model_urls(&mut models_base_url, &mut models_list_url, &mut push);
    parsed.enforce_upload_and_telemetry(
        &mut UploadTelemetryPins {
            trace_upload_url: &mut trace_upload_url,
            feedback_base_url: &mut feedback_base_url,
            deployment_key: &mut deployment_key,
            events_url: &mut events_url,
            events_api_key: &mut events_api_key,
            mixpanel_enabled: &mut mixpanel_enabled,
            mixpanel_token: &mut mixpanel_token,
            trace_upload_bucket: &mut trace_upload_bucket,
            trace_upload_region: &mut trace_upload_region,
            trace_upload_credentials_file: &mut trace_upload_credentials_file,
            trace_upload_endpoint_url: &mut trace_upload_endpoint_url,
            trace_upload_credentials: &mut trace_upload_credentials,
        },
        &mut push,
    );

    let expected_pushes = [
        ("cli.auto_update", "false"),
        ("cli.use_leader", "true"),
        ("cli.show_tips", "false"),
        ("memory.enabled", "false"),
        ("subagents.enabled", "true"),
        ("managed_mcps.enabled", "false"),
        ("models.web_search", "https://search.example"),
        ("cli.channel", "stable"),
        ("cli.minimum_version", "1.0.0"),
        ("cli.maximum_version", "2.0.0"),
        ("cli.required_minimum_version", "1.2.0"),
        ("cli.required_maximum_version", "1.9.0"),
        ("endpoints.models_base_url", "https://models.example"),
        ("endpoints.models_list_url", "https://models.example/list"),
        ("endpoints.trace_upload_url", "https://traces.example"),
        ("endpoints.feedback_base_url", "https://feedback.example"),
        ("endpoints.deployment_key", "[redacted]"),
        ("telemetry.events_url", "https://events.example"),
        ("telemetry.events_api_key", "[redacted]"),
        ("telemetry.mixpanel_enabled", "true"),
        ("telemetry.mixpanel_token", "[redacted]"),
        ("endpoints.trace_upload_bucket", "trace-bucket"),
        ("endpoints.trace_upload_region", "us-east-1"),
        ("endpoints.trace_upload_credentials_file", "/var/creds"),
        (
            "endpoints.trace_upload_endpoint_url",
            "https://upload.example",
        ),
        ("endpoints.trace_upload_credentials", "[redacted]"),
    ]
    .map(|(path, shown)| (path, shown.to_owned()));
    assert_eq!(expected_pushes.as_slice(), pushes.as_slice());

    assert_eq!(
        [
            ("cli.auto_update", Some(false)),
            ("cli.use_leader", Some(true)),
            ("cli.show_tips", Some(false)),
            ("memory.enabled", Some(false)),
            ("subagents.enabled", Some(true)),
            ("managed_mcps.enabled", Some(false)),
            ("telemetry.mixpanel_enabled", Some(true)),
        ],
        [
            ("cli.auto_update", auto_update),
            ("cli.use_leader", use_leader),
            ("cli.show_tips", show_tips),
            ("memory.enabled", memory_enabled),
            ("subagents.enabled", Some(subagents_enabled)),
            ("managed_mcps.enabled", Some(managed_mcps_enabled)),
            ("telemetry.mixpanel_enabled", Some(mixpanel_enabled)),
        ]
    );

    assert_eq!(
        [
            ("cli.channel", Some("stable")),
            ("cli.minimum_version", Some("1.0.0")),
            ("cli.maximum_version", Some("2.0.0")),
            ("cli.required_minimum_version", Some("1.2.0")),
            ("cli.required_maximum_version", Some("1.9.0")),
            ("models.web_search", Some("https://search.example")),
            ("endpoints.models_base_url", Some("https://models.example")),
            (
                "endpoints.models_list_url",
                Some("https://models.example/list")
            ),
            ("endpoints.trace_upload_url", Some("https://traces.example")),
            (
                "endpoints.feedback_base_url",
                Some("https://feedback.example")
            ),
            ("endpoints.deployment_key", Some("deploy-secret")),
            ("telemetry.events_url", Some("https://events.example")),
            ("telemetry.events_api_key", Some("events-secret")),
            ("telemetry.mixpanel_token", Some("mix-secret")),
            ("endpoints.trace_upload_bucket", Some("trace-bucket")),
            ("endpoints.trace_upload_region", Some("us-east-1")),
            (
                "endpoints.trace_upload_credentials_file",
                Some("/var/creds"),
            ),
            (
                "endpoints.trace_upload_endpoint_url",
                Some("https://upload.example"),
            ),
            ("endpoints.trace_upload_credentials", Some("upload-secret"),),
        ],
        [
            ("cli.channel", channel.as_deref()),
            ("cli.minimum_version", minimum_version.as_deref()),
            ("cli.maximum_version", maximum_version.as_deref()),
            (
                "cli.required_minimum_version",
                required_minimum_version.as_deref(),
            ),
            (
                "cli.required_maximum_version",
                required_maximum_version.as_deref(),
            ),
            ("models.web_search", web_search.as_deref()),
            ("endpoints.models_base_url", models_base_url.as_deref()),
            ("endpoints.models_list_url", models_list_url.as_deref()),
            ("endpoints.trace_upload_url", trace_upload_url.as_deref()),
            ("endpoints.feedback_base_url", feedback_base_url.as_deref()),
            ("endpoints.deployment_key", deployment_key.as_deref()),
            ("telemetry.events_url", events_url.as_deref()),
            ("telemetry.events_api_key", events_api_key.as_deref()),
            ("telemetry.mixpanel_token", mixpanel_token.as_deref()),
            (
                "endpoints.trace_upload_bucket",
                trace_upload_bucket.as_deref(),
            ),
            (
                "endpoints.trace_upload_region",
                trace_upload_region.as_deref(),
            ),
            (
                "endpoints.trace_upload_credentials_file",
                trace_upload_credentials_file.as_deref(),
            ),
            (
                "endpoints.trace_upload_endpoint_url",
                trace_upload_endpoint_url.as_deref(),
            ),
            (
                "endpoints.trace_upload_credentials",
                trace_upload_credentials.as_deref(),
            ),
        ]
    );
}

#[test]
fn non_table_section_pins_nothing() {
    let cases = [
        "cli = [false]\n",
        "endpoints = [\"https://x\"]\n",
        "models = [\"a\"]\n",
    ];
    for text in cases {
        assert_eq!(RequirementsToml::default(), parse(text), "{text:?}");
    }
}
