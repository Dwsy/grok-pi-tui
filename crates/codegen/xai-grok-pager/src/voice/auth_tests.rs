use super::{build_stt_routes, build_voice_auth};
use chrono::{Duration, Utc};
use pretty_assertions::assert_eq;
use std::sync::Arc;
use xai_grok_login::{AuthManager, AuthMode, GrokAuth, GrokComConfig};
use xai_grok_test_support::EnvGuard;
use xai_grok_voice::VoiceAuthError;
fn session(issuer: &str) -> GrokAuth {
    GrokAuth {
        key: "session-token".to_owned(),
        auth_mode: AuthMode::Oidc,
        oidc_issuer: Some(issuer.to_owned()),
        refresh_token: Some("rt".to_owned()),
        expires_at: Some(Utc::now() + Duration::hours(1)),
        ..GrokAuth::test_default()
    }
}
/// The positive case is a static key, which every build of the manager serves; the xAI-session case is pinned in
/// `xai-grok-login`.
#[tokio::test]
#[serial_test::serial]
async fn foreign_session_is_refused_and_xai_credential_is_served() {
    let _xai = EnvGuard::unset("XAI_API_KEY");
    let _legacy = EnvGuard::unset("GROK_CODE_XAI_API_KEY");
    let _auth_path = EnvGuard::unset("GROK_AUTH_PATH");
    let dir = tempfile::tempdir().unwrap();
    let mgr = Arc::new(AuthManager::new(dir.path(), GrokComConfig::default()));
    let auth = build_voice_auth(mgr.clone());
    mgr.hot_swap(session("https://cursor.com"));
    assert_eq!(Err(VoiceAuthError::ForeignSession), auth.bearer().await);
    mgr.set_process_static_api_key(Some("xai-static-key".to_owned()));
    assert_eq!(Ok("xai-static-key".to_owned()), auth.bearer().await);
    mgr.set_process_static_api_key(None);
    mgr.clear().unwrap();
    assert_eq!(Err(VoiceAuthError::NotSignedIn), auth.bearer().await);
}
/// A foreign login still reports `ForeignSession` on the streaming bearer (the pipeline's cue for the clip route); an
/// xAI credential is untouched.
#[tokio::test]
#[serial_test::serial]
async fn stt_routes_offer_the_clip_route_only_beside_a_foreign_session() {
    let _xai = EnvGuard::unset("XAI_API_KEY");
    let _legacy = EnvGuard::unset("GROK_CODE_XAI_API_KEY");
    let _auth_path = EnvGuard::unset("GROK_AUTH_PATH");
    let dir = tempfile::tempdir().unwrap();
    let mgr = Arc::new(AuthManager::new(dir.path(), GrokComConfig::default()));
    let routes = build_stt_routes(mgr.clone());
    let ships_clip_transcriber = false;
    assert_eq!(
        ships_clip_transcriber,
        routes.clip_transcriber.is_some(),
        "the clip transcriber ships only on a build that can reach its backend"
    );
    mgr.hot_swap(session("https://cursor.com"));
    assert_eq!(
        Err(VoiceAuthError::ForeignSession),
        routes.auth.bearer().await
    );
    mgr.set_process_static_api_key(Some("xai-static-key".to_owned()));
    assert_eq!(Ok("xai-static-key".to_owned()), routes.auth.bearer().await);
}
