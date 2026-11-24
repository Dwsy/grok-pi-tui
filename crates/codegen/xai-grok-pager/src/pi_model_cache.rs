//! Persistent model-catalog cache for external Pi sessions.
//!
//! Pi's startup handshake (`SessionCreated`) can take several seconds to fetch
//! the live model list. This module hydrates `AppView.models` immediately at
//! startup so `/model`, `Ctrl+M`, `NextModel`, and `CycleThinkingLevel` work
//! before Pi finishes booting:
//!
//! 1. Warm launch: read `${GROK_HOME:-~/.grok-pi}/pi-models-cache.json`.
//! 2. Cold launch (no cache yet): synthesize an initial catalog from Pi's
//!    already-existing local files (`~/.pi/agent/models.json` and
//!    `~/.pi/agent/settings.json`).
//! 3. Live refresh: every time Pi returns `new_models` on `SessionCreated`,
//!    `WorktreeSessionCreated`, or `SessionLoaded`, overwrite the cache
//!    atomically in the background with the latest authoritative catalog.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use agent_client_protocol as acp;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::pi_model_config::PiModelConfigSnapshot;
use crate::pi_resource_config::resolve_pi_agent_dir;

const CACHE_VERSION: u32 = 1;
const CACHE_FILENAME: &str = "pi-models-cache.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PiModelsCacheFile {
    version: u32,
    models: acp::SessionModelState,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PiSettingsModelHints {
    #[serde(default)]
    default_provider: Option<String>,
    #[serde(default)]
    default_model: Option<String>,
    #[serde(default)]
    default_thinking_level: Option<String>,
    #[serde(default)]
    enabled_models: Vec<String>,
}

/// Resolve `${GROK_HOME:-~/.grok-pi}/pi-models-cache.json`.
pub fn pi_models_cache_path() -> Option<PathBuf> {
    if let Ok(raw) = std::env::var("GROK_HOME") {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed).join(CACHE_FILENAME));
        }
    }
    Some(xai_dirs::home_dir()?.join(".grok-pi").join(CACHE_FILENAME))
}

/// Load the best available initial model catalog before Pi finishes starting:
/// cache first, falling back to Pi's existing `~/.pi/agent/{models,settings}.json`.
pub fn load_initial_pi_models() -> Option<acp::SessionModelState> {
    if let Some(path) = pi_models_cache_path()
        && let Some(cached) = load_cache_from_path(&path)
    {
        return Some(cached);
    }
    load_cold_start_models_from_pi_config()
}

fn load_cache_from_path(path: &Path) -> Option<acp::SessionModelState> {
    let raw = fs::read_to_string(path).ok()?;
    let parsed: PiModelsCacheFile = serde_json::from_str(&raw).ok()?;
    if parsed.version != CACHE_VERSION || parsed.models.available_models.is_empty() {
        return None;
    }
    Some(parsed.models)
}

/// Persist the latest authoritative Pi model catalog atomically off the UI thread.
pub fn save_pi_models_async(models: acp::SessionModelState) {
    if models.available_models.is_empty() {
        return;
    }
    let Some(path) = pi_models_cache_path() else {
        return;
    };
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn_blocking(move || {
            if let Err(err) = save_cache_to_path(&path, &models) {
                tracing::debug!(%err, path = %path.display(), "failed to write Pi model cache");
            }
        });
    } else if let Err(err) = save_cache_to_path(&path, &models) {
        tracing::debug!(%err, path = %path.display(), "failed to write Pi model cache");
    }
}

fn save_cache_to_path(path: &Path, models: &acp::SessionModelState) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let payload = PiModelsCacheFile {
        version: CACHE_VERSION,
        models: models.clone(),
    };
    let bytes = serde_json::to_vec(&payload)?;
    let tmp_path = path.with_extension(format!("json.tmp-{}", std::process::id()));
    let mut file = fs::File::create(&tmp_path)?;
    file.write_all(&bytes)?;
    file.flush()?;
    drop(file);
    if let Err(err) = fs::rename(&tmp_path, path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(err.into());
    }
    Ok(())
}

fn model_key(provider: &str, model_id: &str) -> String {
    let provider = provider.trim();
    let model_id = model_id.trim();
    if provider.is_empty() {
        model_id.to_string()
    } else {
        format!("{provider}::{model_id}")
    }
}

fn model_display_label(name: &str, provider: &str) -> String {
    let label = name.trim();
    let provider = provider.trim();
    if provider.is_empty() || label.is_empty() {
        return label.to_string();
    }
    if label.ends_with(&format!("· {provider}")) || label.starts_with(&format!("{provider}/")) {
        return label.to_string();
    }
    format!("{label} · {provider}")
}

fn acp_effort_for_pi_level(level: &str) -> Option<&'static str> {
    match level.trim().to_ascii_lowercase().as_str() {
        "off" | "none" => Some("none"),
        "minimal" => Some("minimal"),
        "low" => Some("low"),
        "medium" => Some("medium"),
        "high" => Some("high"),
        "xhigh" | "max" => Some("max"),
        _ => None,
    }
}

fn default_reasoning_efforts() -> Vec<Value> {
    [
        ("none", "Off"),
        ("minimal", "Minimal"),
        ("low", "Low"),
        ("medium", "Medium"),
        ("high", "High"),
        ("max", "Max"),
    ]
    .into_iter()
    .map(|(value, label)| json!({ "id": value, "value": value, "label": label }))
    .collect()
}

fn parse_enabled_model_spec(spec: &str) -> Option<(String, String)> {
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Strip optional trailing `:effort` (e.g., `anthropic/claude-sonnet-4:high`).
    let without_effort = if let Some((head, tail)) = trimmed.rsplit_once(':') {
        if !head.is_empty() && acp_effort_for_pi_level(tail).is_some() {
            head
        } else {
            trimmed
        }
    } else {
        trimmed
    };
    if let Some((provider, model_id)) = without_effort.split_once("::") {
        let p = provider.trim();
        let m = model_id.trim();
        if !p.is_empty() && !m.is_empty() {
            return Some((p.to_string(), m.to_string()));
        }
    }
    if let Some((provider, model_id)) = without_effort.split_once('/') {
        let p = provider.trim();
        let m = model_id.trim();
        if !p.is_empty() && !m.is_empty() {
            return Some((p.to_string(), m.to_string()));
        }
    }
    None
}

fn build_fallback_model_info(
    provider: &str,
    model_id: &str,
    display_name: &str,
    thinking_level: Option<&str>,
) -> (acp::ModelId, acp::ModelInfo) {
    let key = model_key(provider, model_id);
    let id = acp::ModelId::new(key);
    let mut meta = Map::new();
    if !provider.is_empty() {
        meta.insert("provider".into(), json!(provider));
    }
    meta.insert("modelId".into(), json!(model_id));
    meta.insert("acceptsImages".into(), json!(true));
    if let Some(level) = thinking_level
        && let Some(effort) = acp_effort_for_pi_level(level)
    {
        meta.insert("supportsReasoningEffort".into(), json!(true));
        meta.insert("reasoningEffort".into(), json!(effort));
        meta.insert(
            "reasoningEfforts".into(),
            Value::Array(default_reasoning_efforts()),
        );
    }
    let label = model_display_label(display_name, provider);
    let info = acp::ModelInfo::new(id.clone(), label).meta(Some(meta));
    (id, info)
}

fn load_cold_start_models_from_pi_config() -> Option<acp::SessionModelState> {
    let agent_dir = resolve_pi_agent_dir().ok()?;
    let settings: PiSettingsModelHints = fs::read_to_string(agent_dir.join("settings.json"))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();
    let thinking_level = settings.default_thinking_level.as_deref();

    let mut available: IndexMap<acp::ModelId, acp::ModelInfo> = IndexMap::new();

    if let Ok(snapshot) = PiModelConfigSnapshot::load_from_path(agent_dir.join("models.json")) {
        for (provider, provider_cfg) in &snapshot.document.providers {
            for model in &provider_cfg.models {
                if model.id.trim().is_empty() {
                    continue;
                }
                let key = model_key(provider, &model.id);
                let id = acp::ModelId::new(key);
                let mut meta = Map::new();
                if !provider.is_empty() {
                    meta.insert("provider".into(), json!(provider));
                }
                meta.insert("modelId".into(), json!(model.id));
                if let Some(tokens) = model.context_window {
                    meta.insert("totalContextTokens".into(), json!(tokens));
                }
                if let Some(tokens) = model.max_tokens {
                    meta.insert("maxTokens".into(), json!(tokens));
                }
                if let Some(api) = model.api.as_ref().or(provider_cfg.api.as_ref()) {
                    meta.insert("api".into(), json!(api));
                }
                if let Some(base_url) = provider_cfg.base_url.as_ref() {
                    meta.insert("baseUrl".into(), json!(base_url));
                }
                let accepts_images = if model.input.is_empty() {
                    true
                } else {
                    model
                        .input
                        .iter()
                        .any(|modality| modality.eq_ignore_ascii_case("image"))
                };
                meta.insert("acceptsImages".into(), json!(accepts_images));
                let reasoning = model.reasoning.unwrap_or(false);
                meta.insert("reasoning".into(), json!(reasoning));
                if !model.input.is_empty() {
                    meta.insert(
                        "inputModalities".into(),
                        Value::Array(model.input.iter().cloned().map(Value::String).collect()),
                    );
                }
                if reasoning {
                    meta.insert("supportsReasoningEffort".into(), json!(true));
                    if let Some(level) = thinking_level
                        && let Some(effort) = acp_effort_for_pi_level(level)
                    {
                        meta.insert("reasoningEffort".into(), json!(effort));
                    }
                    meta.insert(
                        "reasoningEfforts".into(),
                        Value::Array(default_reasoning_efforts()),
                    );
                }
                let raw_name = if model.name.trim().is_empty() {
                    model.id.as_str()
                } else {
                    model.name.as_str()
                };
                let label = model_display_label(raw_name, provider);
                let info = acp::ModelInfo::new(id.clone(), label).meta(Some(meta));
                available.insert(id, info);
            }
        }
    }

    for spec in &settings.enabled_models {
        if let Some((provider, model_id)) = parse_enabled_model_spec(spec) {
            let key = acp::ModelId::new(model_key(&provider, &model_id));
            if !available.contains_key(&key) {
                let (id, info) =
                    build_fallback_model_info(&provider, &model_id, &model_id, thinking_level);
                available.insert(id, info);
            }
        }
    }

    let default_id = match (
        settings.default_provider.as_deref().map(str::trim),
        settings.default_model.as_deref().map(str::trim),
    ) {
        (Some(provider), Some(model_id)) if !provider.is_empty() && !model_id.is_empty() => {
            let key = acp::ModelId::new(model_key(provider, model_id));
            if !available.contains_key(&key) {
                let (id, info) =
                    build_fallback_model_info(provider, model_id, model_id, thinking_level);
                available.insert(id, info);
            }
            Some(key)
        }
        _ => None,
    };

    let current = default_id.or_else(|| available.keys().next().cloned())?;
    Some(acp::SessionModelState::new(
        current,
        available.into_values().collect(),
    ))
}
