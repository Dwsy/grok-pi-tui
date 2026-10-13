use toml::Value as TomlValue;
use xai_grok_sampling_types::ReasoningEffort;

const SESSION_RECAP_MODEL_DEFAULT: &str = "grok-4.5";
const SESSION_RECAP_REASONING_EFFORT_DEFAULT: ReasoningEffort = ReasoningEffort::Low;
/// Pasted logs or files can make a user message huge; its opening carries the ask.
const SESSION_RECAP_USER_MESSAGE_MAX_CHARS_DEFAULT: usize = 2_000;
/// Conclusions come at the end of a reply, so a long reply keeps its tail.
const SESSION_RECAP_AGENT_REPLY_MAX_CHARS_DEFAULT: usize = 4_000;
const SESSION_RECAP_MESSAGE_MAX_CHARS_MIN: usize = 200;
const SESSION_RECAP_MESSAGE_MAX_CHARS_MAX: usize = 200_000;
/// About 12k tokens at the bytes/4 estimate; the most recent messages win.
const SESSION_RECAP_TRANSCRIPT_MAX_CHARS_DEFAULT: usize = 48_000;
const SESSION_RECAP_TRANSCRIPT_MAX_CHARS_MIN: usize = 1_000;
const SESSION_RECAP_TRANSCRIPT_MAX_CHARS_MAX: usize = 400_000;
const SESSION_RECAP_TIMEOUT_MS_MIN: u64 = 1_000;
const SESSION_RECAP_TIMEOUT_MS_DEFAULT: u64 = 45_000;
const SESSION_RECAP_TIMEOUT_MS_MAX: u64 = 120_000;

/// Sampling settings for the session recap.
/// Local `[session_recap]` wins per field over remote `session_recap_config`; unset fields take the defaults above.
/// The on/off gate stays the `session_recap` feature.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct SessionRecapConfig {
    /// Used when the catalog serves it on the session's endpoint and auth; otherwise the session model writes the recap.
    pub model: Option<String>,
    /// Applied when the chosen model supports it; otherwise the session's effort stays.
    pub reasoning_effort: Option<ReasoningEffort>,
    pub user_message_max_chars: Option<usize>,
    pub agent_reply_max_chars: Option<usize>,
    pub transcript_max_chars: Option<usize>,
    pub timeout_ms: Option<u64>,
}

/// [`SessionRecapConfig`] with defaults applied and limits clamped.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SessionRecapSettings {
    pub(crate) model: String,
    pub(crate) reasoning_effort: ReasoningEffort,
    pub(crate) user_message_max_chars: usize,
    pub(crate) agent_reply_max_chars: usize,
    pub(crate) transcript_max_chars: usize,
    pub(crate) timeout: std::time::Duration,
}

impl SessionRecapConfig {
    pub(crate) fn settings(&self) -> SessionRecapSettings {
        let chars = |value: Option<usize>, default| {
            value.unwrap_or(default).clamp(
                SESSION_RECAP_MESSAGE_MAX_CHARS_MIN,
                SESSION_RECAP_MESSAGE_MAX_CHARS_MAX,
            )
        };
        SessionRecapSettings {
            model: self
                .model
                .clone()
                .filter(|m| !m.trim().is_empty())
                .unwrap_or_else(|| SESSION_RECAP_MODEL_DEFAULT.to_owned()),
            reasoning_effort: self
                .reasoning_effort
                .unwrap_or(SESSION_RECAP_REASONING_EFFORT_DEFAULT),
            user_message_max_chars: chars(
                self.user_message_max_chars,
                SESSION_RECAP_USER_MESSAGE_MAX_CHARS_DEFAULT,
            ),
            agent_reply_max_chars: chars(
                self.agent_reply_max_chars,
                SESSION_RECAP_AGENT_REPLY_MAX_CHARS_DEFAULT,
            ),
            transcript_max_chars: self
                .transcript_max_chars
                .unwrap_or(SESSION_RECAP_TRANSCRIPT_MAX_CHARS_DEFAULT)
                .clamp(
                    SESSION_RECAP_TRANSCRIPT_MAX_CHARS_MIN,
                    SESSION_RECAP_TRANSCRIPT_MAX_CHARS_MAX,
                ),
            timeout: std::time::Duration::from_millis(
                self.timeout_ms
                    .unwrap_or(SESSION_RECAP_TIMEOUT_MS_DEFAULT)
                    .clamp(SESSION_RECAP_TIMEOUT_MS_MIN, SESSION_RECAP_TIMEOUT_MS_MAX),
            ),
        }
    }
}

/// Malformed payloads are ignored so remote settings are applied atomically.
fn coerce_session_recap_json(value: serde_json::Value) -> Option<SessionRecapConfig> {
    match serde_json::from_value(value) {
        Ok(cfg) => Some(cfg),
        Err(e) => {
            tracing::warn!(error = %e, "[session_recap]: dropped malformed remote payload");
            None
        }
    }
}

static REMOTE_SESSION_RECAP_CONFIG: std::sync::RwLock<Option<SessionRecapConfig>> =
    std::sync::RwLock::new(None);

/// Call wherever `RemoteSettings` is applied.
pub fn cache_remote_session_recap_config(value: Option<serde_json::Value>) {
    let coerced = value.and_then(coerce_session_recap_json);
    if let Ok(mut guard) = REMOTE_SESSION_RECAP_CONFIG.write() {
        *guard = coerced;
    }
}

fn session_recap_config_from_toml(v: Option<&TomlValue>) -> Option<SessionRecapConfig> {
    let table = v?.get("session_recap")?.clone();
    table
        .try_into()
        .map_err(|e| tracing::warn!(error = %e, "[session_recap]: dropped malformed local table"))
        .ok()
}

fn merge_session_recap_config(
    config: SessionRecapConfig,
    remote: SessionRecapConfig,
) -> SessionRecapConfig {
    SessionRecapConfig {
        model: config.model.or(remote.model),
        reasoning_effort: config.reasoning_effort.or(remote.reasoning_effort),
        user_message_max_chars: config
            .user_message_max_chars
            .or(remote.user_message_max_chars),
        agent_reply_max_chars: config
            .agent_reply_max_chars
            .or(remote.agent_reply_max_chars),
        transcript_max_chars: config.transcript_max_chars.or(remote.transcript_max_chars),
        timeout_ms: config.timeout_ms.or(remote.timeout_ms),
    }
}

/// Overlay-free local config merged over the cached remote config, read at call time.
pub(crate) fn resolve_session_recap_settings_from_disk() -> SessionRecapSettings {
    let config = match crate::config::ConfigLayers::load() {
        Ok(layers) => {
            session_recap_config_from_toml(Some(&layers.effective_config_base_without_overlay()))
                .unwrap_or_default()
        }
        Err(_) => SessionRecapConfig::default(),
    };
    let remote = REMOTE_SESSION_RECAP_CONFIG
        .read()
        .ok()
        .and_then(|g| g.clone())
        .unwrap_or_default();
    merge_session_recap_config(config, remote).settings()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_fields_win_over_remote_and_defaults_fill_the_rest() {
        let local: SessionRecapConfig = toml::from_str::<TomlValue>(
            "[session_recap]\nmodel = \"grok-4.20\"\ntimeout_ms = 999999\ntranscript_max_chars = 10",
        )
        .ok()
        .and_then(|v| session_recap_config_from_toml(Some(&v)))
        .unwrap();
        let remote = coerce_session_recap_json(serde_json::json!({
            "model": "remote-model",
            "reasoning_effort": "medium",
            "user_message_max_chars": 3_000,
        }))
        .unwrap();
        let settings = merge_session_recap_config(local, remote).settings();
        assert_eq!(settings.model, "grok-4.20");
        assert_eq!(settings.reasoning_effort, ReasoningEffort::Medium);
        assert_eq!(settings.user_message_max_chars, 3_000);
        assert_eq!(
            settings.agent_reply_max_chars,
            SESSION_RECAP_AGENT_REPLY_MAX_CHARS_DEFAULT
        );
        assert_eq!(
            settings.transcript_max_chars,
            SESSION_RECAP_TRANSCRIPT_MAX_CHARS_MIN
        );
        assert_eq!(
            settings.timeout,
            std::time::Duration::from_millis(SESSION_RECAP_TIMEOUT_MS_MAX)
        );
        assert_eq!(
            coerce_session_recap_json(serde_json::json!({"timeout_ms": "x"})),
            None
        );
    }
}
