use super::*;
use serde_json::json;

#[test]
fn notice_round_trips_through_meta_json() {
    let notice = ModelNotice {
        severity: ModelNoticeSeverity::Warning,
        text: "Deprecated Oct 15. Switch to Grok 4.6".to_owned(),
        label: Some("deprecated".to_owned()),
    };

    let value = notice.to_meta_value();

    assert_eq!(
        json!({"severity": "warning", "text": "Deprecated Oct 15. Switch to Grok 4.6", "label": "deprecated"}),
        value
    );
    assert_eq!(Some(notice), ModelNotice::from_value(&value));
}

#[test]
fn notice_without_label_omits_the_key() {
    let notice = ModelNotice {
        severity: ModelNoticeSeverity::Critical,
        text: "Retired".to_owned(),
        label: None,
    };

    assert_eq!(
        json!({"severity": "critical", "text": "Retired"}),
        notice.to_meta_value()
    );
}

#[test]
fn missing_or_unknown_severity_reads_as_info() {
    let expected = Some(ModelNotice {
        severity: ModelNoticeSeverity::Info,
        text: "Preview".to_owned(),
        label: None,
    });

    assert_eq!(
        expected,
        ModelNotice::from_value(&json!({"text": "Preview"}))
    );
    assert_eq!(
        expected,
        ModelNotice::from_value(&json!({"severity": "catastrophic", "text": "Preview"}))
    );
}

#[test]
fn blank_text_drops_the_notice() {
    assert_eq!(
        None,
        ModelNotice::from_value(&json!({"severity": "warning", "text": "  "}))
    );
}

#[test]
fn malformed_notice_is_dropped() {
    assert_eq!(None, ModelNotice::from_value(&json!("deprecated")));
    assert_eq!(
        None,
        ModelNotice::from_value(&json!({"severity": "warning"}))
    );
}

#[test]
fn normalized_trims_and_caps_text_and_label() {
    let long_text = "x".repeat(MODEL_NOTICE_MAX_TEXT_CHARS + 10);
    let notice = ModelNotice {
        severity: ModelNoticeSeverity::Warning,
        text: format!("  {long_text}  "),
        label: Some(format!(
            " {} ",
            "y".repeat(MODEL_NOTICE_MAX_LABEL_CHARS + 1)
        )),
    }
    .normalized()
    .expect("non-blank text keeps the notice");

    let expected_text = format!("{}…", "x".repeat(MODEL_NOTICE_MAX_TEXT_CHARS - 1));
    let expected_label = format!("{}…", "y".repeat(MODEL_NOTICE_MAX_LABEL_CHARS - 1));
    assert_eq!(expected_text, notice.text);
    assert_eq!(Some(expected_label), notice.label);
}

#[test]
fn blank_label_is_dropped() {
    let notice = ModelNotice::from_value(&json!({"text": "Preview", "label": " "}));

    assert_eq!(None, notice.and_then(|n| n.label));
}

#[test]
fn parse_model_notice_meta_reads_the_notice_key() {
    let meta =
        json!({"totalContextTokens": 1000, "notice": {"severity": "critical", "text": "Retired"}});

    assert_eq!(
        Some(ModelNotice {
            severity: ModelNoticeSeverity::Critical,
            text: "Retired".to_owned(),
            label: None,
        }),
        parse_model_notice_meta(meta.as_object())
    );
    assert_eq!(
        None,
        parse_model_notice_meta(json!({"totalContextTokens": 1000}).as_object())
    );
    assert_eq!(
        None,
        parse_model_notice_meta(json!({"notice": "deprecated"}).as_object())
    );
    assert_eq!(None, parse_model_notice_meta(None));
}
