use super::*;

#[test]
fn identity_survives_token_rotation_and_changes_with_every_other_input() {
    let mut base = GrokAuth::test_default();
    base.user_id = "user-1".into();
    base.key = "token-A".into();

    let mut rotated = base.clone();
    rotated.key = "token-B".into();
    assert_eq!(
        settings_cache_identity(&base, None),
        settings_cache_identity(&rotated, None),
        "identity must survive bearer-token rotation",
    );

    let mut other_team = base.clone();
    other_team.team_id = Some("team-2".into());
    assert_ne!(
        settings_cache_identity(&base, None),
        settings_cache_identity(&other_team, None),
        "a different team must yield a different identity",
    );

    let mut other_org = base.clone();
    other_org.organization_id = Some("org-2".into());
    assert_ne!(
        settings_cache_identity(&base, None),
        settings_cache_identity(&other_org, None),
        "a different organization must yield a different identity",
    );

    let mut no_user_id_a = GrokAuth::test_default();
    no_user_id_a.user_id = String::new();
    no_user_id_a.key = "api-key-A".into();
    let mut no_user_id_b = no_user_id_a.clone();
    no_user_id_b.key = "api-key-B".into();
    assert_ne!(
        settings_cache_identity(&no_user_id_a, None),
        settings_cache_identity(&no_user_id_b, None),
        "with no user_id the key must discriminate identity",
    );

    let mut other_issuer = base.clone();
    other_issuer.oidc_issuer = Some("https://idp-b.example".into());
    assert_ne!(
        settings_cache_identity(&base, None),
        settings_cache_identity(&other_issuer, None),
        "a different OIDC issuer must yield a different identity",
    );

    let mut other_mode = base.clone();
    other_mode.auth_mode = AuthMode::ApiKey;
    assert_ne!(
        settings_cache_identity(&base, None),
        settings_cache_identity(&other_mode, None),
        "a different auth mode must yield a different identity",
    );

    assert_ne!(
        settings_cache_identity(&base, Some("alpha-a")),
        settings_cache_identity(&base, Some("alpha-b")),
        "a different alpha test key must yield a different identity",
    );
}
