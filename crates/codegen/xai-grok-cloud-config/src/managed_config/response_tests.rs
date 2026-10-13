use super::*;

#[test]
fn pick_trusted_envelope_prefers_a_trusted_key_then_the_first() {
    use xai_grok_config::signed_policy::SignatureEnvelope;
    let envelope = |kid: &str| SignatureEnvelope {
        signed_payload: format!("payload-{kid}"),
        signature: format!("sig-{kid}"),
        key_id: kid.to_owned(),
    };
    let envelopes = vec![envelope("v1"), envelope("v2")];

    for (trusted, expected_key) in [("v2", "v2"), ("v1", "v1"), ("missing", "v1")] {
        let picked =
            pick_trusted_envelope(Some(&envelopes), |id| id == trusted).expect("envelopes present");
        assert_eq!(expected_key, picked.key_id);
    }

    assert!(pick_trusted_envelope(None, |_| true).is_none());
}
