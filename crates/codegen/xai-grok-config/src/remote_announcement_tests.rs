use super::*;

/// The nested `cta` object is optional and per-field tolerant, matching the parent struct's style.
/// A partial cta parses instead of failing the whole announcement.
#[test]
fn cta_parses_nested_partial_and_absent() {
    let full: RemoteAnnouncement = serde_json::from_str(
        r#"{"id":"p","severity":"promo","cta":{"label":"Get SuperGrok","url":"https://x.ai/grok","caption":"or use Ctrl+O"}}"#,
    )
    .unwrap();
    let cta = full.cta.as_ref().expect("cta present");
    assert_eq!(cta.label.as_deref(), Some("Get SuperGrok"));
    assert_eq!(cta.url.as_deref(), Some("https://x.ai/grok"));
    assert_eq!(cta.caption.as_deref(), Some("or use Ctrl+O"));

    let partial: RemoteAnnouncement =
        serde_json::from_str(r#"{"cta":{"label":"only label"}}"#).unwrap();
    assert_eq!(
        partial.cta,
        Some(AnnouncementCta {
            label: Some("only label".into()),
            url: None,
            caption: None,
        })
    );

    let absent: RemoteAnnouncement = serde_json::from_str(r#"{"id":"a"}"#).unwrap();
    assert_eq!(absent.cta, None);
}
