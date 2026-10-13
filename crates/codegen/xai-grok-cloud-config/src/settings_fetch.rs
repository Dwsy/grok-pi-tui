//! The `/settings` request to the cli-chat-proxy and what its outcome means.
use std::ops::ControlFlow;
use xai_grok_config::RemoteSettings;
use xai_grok_login::{GrokAuth, GrokComConfig};
/// The outcome of a `/settings` fetch, as one of the three cases `OtelGate` handles.
#[derive(Debug, Clone)]
#[must_use]
pub enum SettingsFetch {
    /// The settings were fetched and parsed.
    /// `RemoteSettings` is boxed because it is large.
    Fetched(Box<RemoteSettings>),
    /// The server rejected the credential with a 401.
    /// No remote policy will ever reach this leader.
    Rejected,
    /// The fetch finished and failed with something other than a 401.
    /// External OTEL export may still start under the local policy.
    Retry,
}
impl SettingsFetch {
    pub fn into_option(self) -> Option<RemoteSettings> {
        match self {
            SettingsFetch::Fetched(s) => Some(*s),
            SettingsFetch::Rejected | SettingsFetch::Retry => None,
        }
    }
}
/// Makes up to [`xai_grok_http::SETTINGS_FETCH_MAX_ATTEMPTS`] attempts on transient failures.
pub fn fetch_settings_blocking(
    cli_chat_proxy_base_url: &str,
    auth: &GrokAuth,
    alpha_test_key: Option<&str>,
) -> SettingsFetch {
    fetch_settings_blocking_with_attempts(
        cli_chat_proxy_base_url,
        auth,
        alpha_test_key,
        xai_grok_http::SETTINGS_FETCH_MAX_ATTEMPTS,
    )
}
/// Tests call this with a small `max_attempts` to skip the retry backoff.
fn fetch_settings_blocking_with_attempts(
    cli_chat_proxy_base_url: &str,
    auth: &GrokAuth,
    alpha_test_key: Option<&str>,
    max_attempts: u32,
) -> SettingsFetch {
    let client = xai_grok_http::shared_startup_blocking_client();
    let url = format!("{cli_chat_proxy_base_url}/settings");
    let max_attempts = max_attempts.max(1);
    for attempt in 0u32..max_attempts {
        if attempt > 0 {
            std::thread::sleep(xai_grok_http::SETTINGS_RETRY_BACKOFF_STEP * attempt);
        }
        let request =
            add_cli_chat_proxy_headers_blocking(client.get(&url), auth, alpha_test_key, &url);
        if let ControlFlow::Break(outcome) = classify_settings_response(request.send(), attempt) {
            return outcome;
        }
    }
    tracing::error!(max_attempts, "Settings fetch failed");
    SettingsFetch::Retry
}
fn add_cli_chat_proxy_headers_blocking(
    builder: reqwest::blocking::RequestBuilder,
    auth: &GrokAuth,
    alpha_test_key: Option<&str>,
    url: &str,
) -> reqwest::blocking::RequestBuilder {
    let mut builder = builder
        .header("Authorization", format!("Bearer {}", &auth.key))
        .header("X-XAI-Token-Auth", GrokComConfig::default().token_header)
        .header("x-userid", &auth.user_id)
        .header("x-grok-client-version", xai_grok_version::VERSION);
    if let Some(email) = &auth.email {
        builder = builder.header("x-email", email);
    }
    let _ = (alpha_test_key, url);
    builder
        .header(
            "x-grok-client-identifier",
            xai_grok_http::process_client_identifier(),
        )
        .header(
            xai_grok_http::CLIENT_MODE_HEADER,
            xai_grok_http::process_client_mode(),
        )
}
/// Returns the final outcome of one `/settings` attempt, or `Continue` when another attempt may succeed.
fn classify_settings_response(
    response: reqwest::Result<reqwest::blocking::Response>,
    attempt: u32,
) -> ControlFlow<SettingsFetch> {
    match response {
        Ok(resp) if resp.status().is_success() => match resp.json() {
            Ok(settings) => {
                tracing::debug!("Fetched remote settings from cli-chat-proxy");
                ControlFlow::Break(SettingsFetch::Fetched(Box::new(settings)))
            }
            Err(e) => {
                tracing::warn!(attempt, "Failed to parse settings response: {e}");
                ControlFlow::Break(SettingsFetch::Retry)
            }
        },
        Ok(resp) if resp.status().is_server_error() => {
            tracing::warn!(
                attempt,
                status = resp.status().as_u16(),
                "Settings fetch server error, retrying"
            );
            ControlFlow::Continue(())
        }
        Ok(resp) if resp.status() == reqwest::StatusCode::UNAUTHORIZED => {
            tracing::warn!(
                status = resp.status().as_u16(),
                "Settings fetch rejected (401)"
            );
            ControlFlow::Break(SettingsFetch::Rejected)
        }
        Ok(resp) => {
            tracing::warn!(
                status = resp.status().as_u16(),
                "Settings fetch failed (non-2xx)"
            );
            ControlFlow::Break(SettingsFetch::Retry)
        }
        Err(e) => {
            tracing::warn!(attempt, "Settings fetch network error: {e}");
            ControlFlow::Continue(())
        }
    }
}
#[cfg(test)]
#[path = "settings_fetch_tests.rs"]
mod tests;
