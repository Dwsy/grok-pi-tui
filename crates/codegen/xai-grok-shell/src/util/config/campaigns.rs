//! The campaign-governed `Config` fields: resolving them from the campaign overlay and dismissing a campaign when the user sets one.
//! The overlay itself (remote cache, dismiss state, effective config) lives in [`xai_grok_config::effective_config`].
//!
//! Design, invariants, and the "adding a second governed field" recipe are documented alongside this module.

use xai_grok_config::ConfigLayers;
use xai_grok_config::campaigns::{CampaignEntry, ids_touching_paths};
use xai_grok_config::config_override::{PatchPath, patch_touches_any};
use xai_grok_config::effective_config::{
    CampaignOverlay, CampaignSources, cached_remote_campaigns, dismiss_campaign_ids,
    remote_campaigns_from_settings, resolve_dismissable_campaigns, set_remote_campaigns,
};

/// The effective `models.default` while an **active** campaign drives it, plus the pre-campaign base value it overrode.
pub struct CampaignModelsDefault {
    /// The campaign-nudged default model.
    pub value: String,
    /// The pre-campaign base `models.default` (`None` when the user had none).
    pub pre_campaign: Option<String>,
}

/// `None` unless an active (non-dismissed, kill-switch-respecting, requirements-losing) campaign changes the effective `models.default`.
/// Session creation uses this to apply a campaign to `/new` even when remote settings arrived only after boot. The `ModelsManager`'s `current_model_id` was resolved pre-campaign.
/// `ModelsManager::apply_config` deliberately never re-targets it on a campaign-only flip, so `/new` re-evaluates here. Reading the dismiss state fresh makes a `/model` pick win instantly. [`persist_user_choice`] records the dismissal before the config write, so the very next `/new` resolves campaign-free.
pub fn campaign_driven_models_default() -> Option<CampaignModelsDefault> {
    let layers = ConfigLayers::load().ok()?;
    campaign_driven_models_default_from(
        &layers,
        &CampaignSources::from_process(cached_remote_campaigns()),
    )
}

/// Env-free resolution core of [`campaign_driven_models_default`] (unit-testable without touching `GROK_HOME` or the process-global cache).
fn campaign_driven_models_default_from(
    layers: &ConfigLayers,
    sources: &CampaignSources,
) -> Option<CampaignModelsDefault> {
    let CampaignOverlay {
        base,
        active,
        effective,
    } = CampaignOverlay::new(layers, sources);
    if active.is_empty() {
        return None;
    }
    let base_value = read_path(&base, MODELS_DEFAULT_PATH);
    let value = read_path(&effective, MODELS_DEFAULT_PATH);
    if value == base_value {
        return None;
    }
    Some(CampaignModelsDefault {
        value: as_string(value)?,
        pre_campaign: as_string(base_value),
    })
}

fn read_path(tree: &toml::Value, path: PatchPath) -> Option<toml::Value> {
    let mut cur = tree;
    for key in path {
        cur = cur.get(*key)?;
    }
    Some(cur.clone())
}

fn as_string(v: Option<toml::Value>) -> Option<String> {
    v.and_then(|v| v.as_str().map(str::to_owned))
}

/// Resolved campaign state for one [`CampaignField`] after the overlay.
struct CampaignFieldValue {
    /// Effective value (campaign value if it won, else the merged base value).
    value: Option<toml::Value>,
    /// Whether an active campaign actually changed the effective value.
    driven: bool,
    /// Pre-campaign value to recover to; `Some` only when `driven` and the base had one.
    recovery: Option<toml::Value>,
}

/// A config field a campaign may temporarily override until the user sets it. `apply_campaign_fields` drives every [`CAMPAIGN_FIELDS`] entry, so the resolve pass is one row here.
/// A field still needs its runtime state and a `persist_*` writer through [`persist_user_choice`]. It also needs any field-specific reaction (e.g. the model catalog-miss/live-session handling in `agent::remote_config`).
struct CampaignField {
    /// Path into the effective config; also the dismiss key shared with the writer.
    path: PatchPath,
    /// Store the resolved value, flag, and recovery onto the agent config.
    store: fn(&mut crate::agent::config::Config, CampaignFieldValue),
    /// Clear the campaign-driven flag and recovery (value untouched).
    /// Used when resolution fails so the runtime state is defined (fail closed, matching the apply path) instead of stale.
    reset: fn(&mut crate::agent::config::Config),
}

/// Path of the `models.default` campaign field, shared by the registry row and its dismiss writer so the two can't drift.
const MODELS_DEFAULT_PATH: PatchPath = &["models", "default"];

const CAMPAIGN_FIELDS: &[CampaignField] = &[CampaignField {
    path: MODELS_DEFAULT_PATH,
    store: |cfg, r| {
        cfg.models.default = as_string(r.value);
        cfg.models.default_is_campaign_driven = r.driven;
        cfg.models.pre_campaign_default = as_string(r.recovery);
    },
    reset: |cfg| {
        cfg.models.default_is_campaign_driven = false;
        cfg.models.pre_campaign_default = None;
    },
}];

/// Resolve each [`CAMPAIGN_FIELDS`] entry's value, campaign-driven flag, and recovery value from the campaign overlay and store them onto `cfg`.
/// Pure given the resolved `base`/`effective`/`active`; the I/O lives in [`sync_campaign_fields`].
fn apply_campaign_fields(
    cfg: &mut crate::agent::config::Config,
    base: &toml::Value,
    effective: &toml::Value,
    active: &[CampaignEntry],
) {
    for field in CAMPAIGN_FIELDS {
        let value = read_path(effective, field.path);
        let base_value = read_path(base, field.path);
        // A campaign only *drives* a field when it actually changed the effective value
        // Requirements are re-merged after campaigns, so an admin pin wins and the campaign patch is a no-op (don't flag it)
        let driven = value != base_value
            && active
                .iter()
                .any(|e| patch_touches_any(&e.patch, &[field.path]));
        let recovery = if driven { base_value } else { None };
        (field.store)(
            cfg,
            CampaignFieldValue {
                value,
                driven,
                recovery,
            },
        );
    }
}

/// Seed the remote cache, then set every [`CAMPAIGN_FIELDS`] entry (value, flag, and recovery) from the campaign overlay.
/// Finally re-apply requirements so admin pins win.
pub fn sync_campaign_fields(cfg: &mut crate::agent::config::Config) {
    let remote = remote_campaigns_from_settings(cfg.remote_settings.as_ref());
    // Seed the process-global cache from the parse we already did (skip on `None` so a failed fetch can't clobber a previously-seeded cache)
    if cfg.remote_settings.is_some() {
        set_remote_campaigns(remote.clone());
    }
    let Ok(layers) = ConfigLayers::load() else {
        // Fail closed like the apply path: leave the field values as loaded but clear the campaign-driven flags/recovery so they can't go stale
        // A stale flag would mislabel a user value as campaign-driven, or vice versa disable the live-session guard for a campaign value
        tracing::warn!("campaigns: config layer load failed; clearing campaign-driven field state");
        for field in CAMPAIGN_FIELDS {
            (field.reset)(cfg);
        }
        return;
    };
    let overlay = CampaignOverlay::new(&layers, &CampaignSources::from_process(remote));
    apply_campaign_fields(cfg, &overlay.base, &overlay.effective, &overlay.active);
    let _ = crate::config::apply_requirements(cfg);
}

/// Dismiss any active campaign whose patch touches `path`, then persist the setting via `update_config`.
/// This is the single field-keyed chokepoint: a new campaign-governable field is one call here with no per-field dismiss wiring.
/// The dismiss is recorded **before** the config write so a crash between the two can't leave the campaign active over the user's just-saved value. If the dismiss lands but the write fails, the dismiss stands: failure leans toward not nudging.
pub(super) async fn persist_user_choice(
    path: PatchPath,
    write: impl FnOnce(&mut super::mcp::Config),
) -> anyhow::Result<()> {
    // Config-layer reads and the flock'd read-modify-write are blocking I/O; keep them off the async worker The task is awaited before the config write so the dismiss-before-write ordering above holds
    // A panicked/cancelled dismiss task must NOT abort the user's write Bookkeeping failure is logged and the write proceeds (the campaign may re-nudge; the pick is never lost)
    let dismissed = tokio::task::spawn_blocking(move || {
        let ids = ids_touching_paths(&resolve_dismissable_campaigns(), &[path]);
        if !ids.is_empty() {
            tracing::info!(
                ?ids,
                ?path,
                "campaigns: dismissed after the user set the field"
            );
            dismiss_campaign_ids(ids);
        }
    })
    .await;
    if let Err(e) = dismissed {
        tracing::warn!(error = %e, "campaigns: dismiss bookkeeping task failed; persisting the choice anyway");
    }
    super::persist::update_config(write).await
}

/// Persist the default model (and optional reasoning effort) through [`persist_user_choice`].
/// Picking a model thus dismisses a campaign nudging `models.default`.
/// `None` clears the field.
pub async fn persist_models_default(
    value: Option<String>,
    reasoning_effort: Option<xai_grok_sampling_types::ReasoningEffort>,
) -> anyhow::Result<()> {
    let s = value.unwrap_or_default();
    if s.len() > super::settings_writes::MAX_DEFAULT_MODEL_LEN {
        anyhow::bail!(
            "model name too long ({} > {} bytes)",
            s.len(),
            super::settings_writes::MAX_DEFAULT_MODEL_LEN
        );
    }
    persist_user_choice(MODELS_DEFAULT_PATH, move |cfg| {
        cfg.models.default = if s.is_empty() { None } else { Some(s) };
        if let Some(effort) = reasoning_effort {
            cfg.models.default_reasoning_effort = Some(effort);
        }
    })
    .await
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    fn models_default_patch(default: &str) -> toml::Table {
        let mut models = toml::map::Map::new();
        models.insert("default".into(), toml::Value::String(default.into()));
        let mut t = toml::map::Map::new();
        t.insert("models".into(), toml::Value::Table(models));
        t
    }

    /// `campaign_driven_models_default_from` tracks remote entries and dismissals.
    /// It is `Some` while the campaign is active and `None` the instant its dismissal lands, so a `/new` right after a `/model` pick never re-nudges.
    #[test]
    fn campaign_driven_models_default_tracks_remote_and_dismissals() {
        let layers = ConfigLayers {
            user: toml::from_str("[models]\ndefault = \"config-model\"\n").unwrap(),
            ..Default::default()
        };
        let remote = vec![CampaignEntry {
            id: "t-models-nudge".into(),
            patch: models_default_patch("campaign-model"),
        }];
        let sources = |dismissed: HashSet<String>| CampaignSources {
            remote: remote.clone(),
            dismissed,
            ..CampaignSources::default()
        };

        let nudge = campaign_driven_models_default_from(&layers, &sources(HashSet::new()))
            .expect("active campaign drives the default");
        assert_eq!(nudge.value, "campaign-model");
        assert_eq!(nudge.pre_campaign.as_deref(), Some("config-model"));

        // A dismissal (what a `/model` pick records first) deactivates the nudge for the very next resolution
        let dismissed: HashSet<String> = ["t-models-nudge".to_string()].into_iter().collect();
        assert!(
            campaign_driven_models_default_from(&layers, &sources(dismissed)).is_none(),
            "a dismissed campaign must not nudge"
        );

        // A campaign that loses to a requirements pin never reports campaign-driven
        let mut pinned = layers.clone();
        pinned.user_requirements =
            Some(toml::from_str("[models]\ndefault = \"config-model\"\n").unwrap());
        assert!(
            campaign_driven_models_default_from(&pinned, &sources(HashSet::new())).is_none(),
            "a requirements-pinned default must not report campaign-driven"
        );
    }

    /// Contract: `persist_user_choice(["models","default"], ..)` dismisses only campaigns that touch that path, never a sibling-field campaign.
    /// The full wiring (set_default_model, then persist, then dismiss) is covered end to end by the pager `pty_e2e` campaign test.
    #[test]
    fn models_default_persist_targets_only_model_campaigns() {
        let model_campaign = CampaignEntry {
            id: "release".into(),
            patch: models_default_patch("new-model"),
        };
        let other_campaign = CampaignEntry {
            id: "other".into(),
            patch: toml::from_str::<toml::Table>("[features]\nweb_fetch = true\n").unwrap(),
        };
        let path: &[PatchPath] = &[&["models", "default"]];
        let ids = ids_touching_paths(&[model_campaign, other_campaign], path);
        assert_eq!(ids, vec!["release".to_string()]);
    }

    /// `apply_campaign_fields` flags a field campaign-driven only when the campaign actually changed the effective value.
    /// A campaign win sets the flag and recovery, but a requirements win (effective == base) does not (and stores no recovery).
    #[test]
    fn campaign_field_flags_campaign_win_not_requirements_win() {
        let active = vec![CampaignEntry {
            id: "release".into(),
            patch: models_default_patch("campaign-model"),
        }];
        let base: toml::Value = toml::from_str("[models]\ndefault = \"base-model\"\n").unwrap();

        // Campaign won the effective default.
        let mut cfg = crate::agent::config::Config::default();
        let won: toml::Value = toml::from_str("[models]\ndefault = \"campaign-model\"\n").unwrap();
        apply_campaign_fields(&mut cfg, &base, &won, &active);
        assert_eq!(cfg.models.default.as_deref(), Some("campaign-model"));
        assert!(cfg.models.default_is_campaign_driven);
        assert_eq!(
            cfg.models.pre_campaign_default.as_deref(),
            Some("base-model")
        );

        // Requirements re-merge clobbered the campaign back to the base value.
        let mut cfg = crate::agent::config::Config::default();
        apply_campaign_fields(&mut cfg, &base, &base, &active);
        assert_eq!(cfg.models.default.as_deref(), Some("base-model"));
        assert!(!cfg.models.default_is_campaign_driven);
        assert_eq!(cfg.models.pre_campaign_default, None);

        // No active campaign touching the field: never driven.
        let mut cfg = crate::agent::config::Config::default();
        apply_campaign_fields(&mut cfg, &base, &won, &[]);
        assert!(!cfg.models.default_is_campaign_driven);
        assert_eq!(cfg.models.pre_campaign_default, None);
    }

    /// Every registry row's `reset` clears the campaign-driven runtime state a prior `store` set.
    /// It runs on the resolution-failure path so flags can't go stale.
    #[test]
    fn campaign_field_reset_clears_driven_state() {
        let mut cfg = crate::agent::config::Config::default();
        for field in CAMPAIGN_FIELDS {
            (field.store)(
                &mut cfg,
                CampaignFieldValue {
                    value: Some(toml::Value::String("campaign-model".into())),
                    driven: true,
                    recovery: Some(toml::Value::String("base-model".into())),
                },
            );
        }
        assert!(cfg.models.default_is_campaign_driven);
        assert!(cfg.models.pre_campaign_default.is_some());

        for field in CAMPAIGN_FIELDS {
            (field.reset)(&mut cfg);
        }
        assert!(!cfg.models.default_is_campaign_driven);
        assert_eq!(cfg.models.pre_campaign_default, None);
        // The field *value* is left as loaded; reset only clears the metadata.
        assert_eq!(cfg.models.default.as_deref(), Some("campaign-model"));
    }
}
