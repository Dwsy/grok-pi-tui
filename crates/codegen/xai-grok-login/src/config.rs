//! Auth configuration surface.
//!
//! The data types and env helpers live in `xai-grok-shell-base::auth_config`, below both this
//! crate and `xai-grok-shell`, so the Pi profile can build `AgentConfig` without linking the
//! login runtime. They are re-exported here so every `xai_grok_login::<item>` path keeps working.

pub use xai_grok_shell_base::auth_config::{
    ForceLoginTeam, GrokComConfig, LEGACY_AUTH_SCOPE, LEGACY_XAI_API_KEY_ENV_VAR,
    OAuth2ProviderConfig, OidcAuthConfig, PreferredAuthMethod, TEAM_PRINCIPAL_TYPE,
    XAI_API_KEY_ENV_VAR, XAI_OAUTH2_ISSUER, force_login_team_from_env,
    force_login_team_from_requirements_value, has_xai_api_key_env, is_xai_oauth2_issuer,
    read_xai_api_key_env, resolve_force_login_team, use_local_auth, xai_oauth2_issuer,
};

/// A separate const so the frozen contract test pins the production allowlist even when the non-production feature adds staging and local origins.
const PROD_ACCOUNTS_APP_ORIGINS: &[&str] = &["https://accounts.x.ai"];

/// Production build: accepts only the production accounts app.
pub fn allowed_accounts_app_origins() -> Vec<String> {
    PROD_ACCOUNTS_APP_ORIGINS
        .iter()
        .map(|o| o.to_string())
        .collect()
}

/// Builds a CORS layer accepting requests from the deployments in [`allowed_accounts_app_origins`] for the given HTTP method.
pub fn accounts_app_cors_layer(method: axum::http::Method) -> tower_http::cors::CorsLayer {
    tower_http::cors::CorsLayer::new()
        .allow_origin(tower_http::cors::AllowOrigin::list(
            allowed_accounts_app_origins()
                .iter()
                .filter_map(|origin| match origin.parse() {
                    Ok(value) => Some(value),
                    Err(_) => {
                        tracing::warn!(origin, "skipping malformed accounts-app CORS origin");
                        None
                    }
                }),
        ))
        .allow_methods([method])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FROZEN loopback contract: the accounts-app origins the CLI's loopback callback server accepts cross-origin requests from. The consent page (served from accounts.x.ai) delivers the code via `fetch(..., cors)`.
    /// Removing an origin therefore breaks loopback delivery for already-installed CLIs. Keep in sync with the oauth2-provider / accounts-app deployments. Non-production / local-dev origins are opt-in only.
    #[test]
    fn allowed_accounts_app_origins_are_frozen() {
        assert_eq!(PROD_ACCOUNTS_APP_ORIGINS, &["https://accounts.x.ai"]);
        assert_eq!(allowed_accounts_app_origins(), PROD_ACCOUNTS_APP_ORIGINS);
    }
}
