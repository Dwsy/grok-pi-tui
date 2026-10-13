//! The account that an entry in the signed remote settings cache is scoped to.

use xai_grok_config::EndpointsConfig;
use xai_grok_login::{AuthMode, GrokAuth};

use crate::settings_cache::{SettingsCacheAccount, SettingsCacheScope};

/// A hash of the account that stays the same when the bearer token refreshes.
/// The issuer and auth mode separate accounts that share ids across identity providers.
pub fn settings_cache_identity(auth: &GrokAuth, alpha_test_key: Option<&str>) -> String {
    let principal = if auth.user_id.is_empty() {
        auth.key.as_str()
    } else {
        auth.user_id.as_str()
    };
    let auth_mode = match auth.auth_mode {
        AuthMode::WebLogin => "web_login",
        AuthMode::Oidc => "oidc",
        AuthMode::External => "external",
        AuthMode::ApiKey => "api_key",
    };
    crate::remote_settings::scope_hash(&[
        principal,
        auth.team_id.as_deref().unwrap_or(""),
        auth.organization_id.as_deref().unwrap_or(""),
        auth.oidc_issuer.as_deref().unwrap_or(""),
        auth_mode,
        alpha_test_key.unwrap_or(""),
    ])
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsEndpoint {
    origin: String,
    alpha_test_key: Option<String>,
}

impl SettingsEndpoint {
    /// `origin` is the cli-chat-proxy base URL the fetch goes to.
    pub fn new(origin: String, alpha_test_key: Option<String>) -> SettingsEndpoint {
        SettingsEndpoint {
            origin,
            alpha_test_key,
        }
    }

    pub fn origin(&self) -> &str {
        &self.origin
    }

    pub fn alpha_test_key(&self) -> Option<&str> {
        self.alpha_test_key.as_deref()
    }

    /// The account that owns an entry fetched from this endpoint for `auth`.
    pub fn cache_account(&self, auth: &GrokAuth) -> SettingsCacheAccount {
        SettingsCacheAccount {
            identity: settings_cache_identity(auth, self.alpha_test_key()),
            origin: self.origin.clone(),
        }
    }

    /// The scope for an entry that `client` fetches from this endpoint for `auth`.
    pub fn cache_scope(&self, auth: &GrokAuth, client: String) -> SettingsCacheScope {
        SettingsCacheScope {
            account: self.cache_account(auth),
            client,
        }
    }
}

impl From<&EndpointsConfig> for SettingsEndpoint {
    fn from(endpoints: &EndpointsConfig) -> SettingsEndpoint {
        SettingsEndpoint::new(endpoints.proxy_url(), endpoints.alpha_test_key.clone())
    }
}

#[cfg(test)]
#[path = "settings_endpoint_tests.rs"]
mod tests;
