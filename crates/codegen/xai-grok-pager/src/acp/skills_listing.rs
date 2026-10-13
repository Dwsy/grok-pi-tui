//! Reads the `x.ai/skills/list` and `x.ai/skills/toggle` answers the Skills tab shows.

use xai_grok_shell::extensions::skills::SkillsListResponse;

/// A skills reply, bare or wrapped in `result`.
pub fn parse_reply(raw: &str) -> serde_json::Result<SkillsListResponse> {
    let mut reply: serde_json::Value = serde_json::from_str(raw)?;
    let listing = match reply.get_mut("result") {
        Some(result) => result.take(),
        None => reply,
    };
    serde_json::from_value(listing)
}

#[cfg(test)]
mod tests {
    use super::*;

    use pretty_assertions::assert_eq;
    use xai_grok_shell::extensions::skills::SkillScanError;

    #[test]
    fn a_reply_parses_bare_or_wrapped_in_result() {
        let bare = r#"{"skills":[{"name":"lint","description":"Lint","path":"/skills/lint/SKILL.md","scope":"user"}],"scanErrors":[{"path":"/work/locked","message":"permission denied"}]}"#;
        let wrapped = format!(r#"{{"result":{bare}}}"#);

        for raw in [bare, wrapped.as_str()] {
            let listing = parse_reply(raw).expect("the reply parses");
            assert_eq!(
                vec!["lint"],
                listing
                    .skills
                    .iter()
                    .map(|skill| skill.name.as_str())
                    .collect::<Vec<_>>(),
                "{raw}"
            );
            assert_eq!(
                vec![SkillScanError {
                    path: "/work/locked".to_owned(),
                    message: "permission denied".to_owned(),
                }],
                listing.scan_errors,
                "{raw}"
            );
        }
    }

    #[test]
    fn a_malformed_reply_is_an_error() {
        assert!(parse_reply(r#"{"skills":[{"name":7}]}"#).is_err());
    }
}
