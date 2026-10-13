//! The layer *files* are read by [`crate::loader`].
//! This module owns how those layers combine into the effective config: layer precedence, the `GROK_CONFIG` overlay, and campaign resolution.

use crate::loader::{
    deep_merge_toml, load_from_disk, load_managed_config, load_system_managed_config,
    normalize_config_layer,
};
use crate::validation::{load_requirements, load_system_requirements};

/// Whether a layer merge includes the `GROK_CONFIG` overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OverlayInclusion {
    Include,
    Exclude,
}

/// Layers from lowest to highest priority. `[[campaigns]]` is taken off each layer at load.
#[derive(Clone)]
pub struct ConfigLayers {
    pub system_managed: toml::Value,
    pub managed: toml::Value,
    pub user: toml::Value,
    /// `GROK_CONFIG` / `GROK_CONFIG_PATH` overlay, above user but below requirements.
    /// Soft settings only; this doc is the canonical source of truth for what the overlay can and cannot reach.
    /// This is fail-closed: every code-exec, auth, egress, trust, or discovery table is absent from the allowlist and dropped by default.
    pub env_overlay: Option<toml::Value>,
    pub user_requirements: Option<toml::Value>,
    pub system_requirements: Option<toml::Value>,
    /// macOS MDM requirements; highest requirements tier when present.
    pub mdm_requirements: Option<toml::Value>,
    pub campaigns: crate::campaigns::CampaignOverrides,
}

impl Default for ConfigLayers {
    fn default() -> Self {
        Self {
            system_managed: toml::Value::Table(Default::default()),
            managed: toml::Value::Table(Default::default()),
            user: toml::Value::Table(Default::default()),
            env_overlay: None,
            user_requirements: None,
            system_requirements: None,
            mdm_requirements: None,
            campaigns: crate::campaigns::CampaignOverrides::default(),
        }
    }
}

impl ConfigLayers {
    pub fn load() -> std::io::Result<Self> {
        use crate::campaigns::{CampaignOverrides, take_campaign_entries};

        let mut system_managed = load_system_managed_config()?;
        let system_managed_campaigns = take_campaign_entries(&mut system_managed, "system_managed");

        let mut managed = load_managed_config()?;
        let managed_campaigns = take_campaign_entries(&mut managed, "managed");

        let mut user = load_from_disk()?;
        let user_campaigns = take_campaign_entries(&mut user, "user");

        let env_overlay = crate::env_overlay::load_env_overlay();

        let mut user_requirements = load_requirements();
        let mut system_requirements = load_system_requirements();
        let mut mdm_requirements = crate::validation::mdm_requirements_value();

        // Highest-authority tier first: `merge_campaign_entries` is first-id-wins, so a duplicate campaign id must resolve mdm > system > user
        // That matches the layer precedence in `effective_config_base`, where mdm is merged last/highest
        let mut requirements_campaigns = Vec::new();
        if let Some(ref mut req) = mdm_requirements {
            requirements_campaigns.extend(take_campaign_entries(req, "requirements"));
        }
        if let Some(ref mut req) = system_requirements {
            requirements_campaigns.extend(take_campaign_entries(req, "requirements"));
        }
        if let Some(ref mut req) = user_requirements {
            requirements_campaigns.extend(take_campaign_entries(req, "requirements"));
        }

        // Normalize each layer before any merge, so `[toolset.web_search]`'s `allowed_domains` / `excluded_domains` travel together
        // A layer that sets one clears the other to `[]`
        // That makes `deep_merge_toml` replace the whole policy from the winning layer instead of mixing keys across layers
        normalize_config_layer(&mut system_managed);
        normalize_config_layer(&mut managed);
        normalize_config_layer(&mut user);
        for req in [
            &mut user_requirements,
            &mut system_requirements,
            &mut mdm_requirements,
        ]
        .into_iter()
        .flatten()
        {
            normalize_config_layer(req);
        }

        Ok(Self {
            system_managed,
            managed,
            user,
            env_overlay,
            user_requirements,
            system_requirements,
            mdm_requirements,
            campaigns: CampaignOverrides {
                requirements: requirements_campaigns,
                user: user_campaigns,
                managed: managed_campaigns,
                system_managed: system_managed_campaigns,
            },
        })
    }

    /// Layer merge (no campaigns), including the `GROK_CONFIG` overlay.
    /// Overlay-inclusive: security gates must not read this.
    /// Use [`Self::effective_config_base_without_overlay`] for any gate (the overlay-free set is enumerated on [`Self::env_overlay`]).
    pub fn effective_config_base(&self) -> toml::Value {
        self.merge(OverlayInclusion::Include)
    }

    /// Layer merge excluding the `GROK_CONFIG` overlay, for security gates.
    pub fn effective_config_base_without_overlay(&self) -> toml::Value {
        self.merge(OverlayInclusion::Exclude)
    }

    fn merge(&self, inclusion: OverlayInclusion) -> toml::Value {
        let Self {
            system_managed,
            managed,
            user,
            env_overlay,
            user_requirements: _,
            system_requirements: _,
            mdm_requirements: _,
            campaigns: _,
        } = self;
        let mut merged = system_managed.clone();
        deep_merge_toml(&mut merged, managed);
        deep_merge_toml(&mut merged, user);
        if let (OverlayInclusion::Include, Some(overlay)) = (inclusion, env_overlay) {
            deep_merge_toml(&mut merged, overlay);
        }
        for req in self.requirements_in_order() {
            deep_merge_toml(&mut merged, req);
        }
        merged
    }

    fn requirements_in_order(&self) -> impl Iterator<Item = &toml::Value> {
        [
            &self.user_requirements,
            &self.system_requirements,
            &self.mdm_requirements,
        ]
        .into_iter()
        .flatten()
    }

    /// Campaign source slices in priority order (first id wins): requirements > remote > user > managed > system_managed.
    /// This is the single source of truth for the precedence; both this crate and the shell resolver consume it.
    pub fn campaign_source_slices<'a>(
        &'a self,
        remote_campaigns: &'a [crate::campaigns::CampaignEntry],
    ) -> [&'a [crate::campaigns::CampaignEntry]; 5] {
        [
            &self.campaigns.requirements,
            remote_campaigns,
            &self.campaigns.user,
            &self.campaigns.managed,
            &self.campaigns.system_managed,
        ]
    }

    /// Active campaigns against `base`: the kill switch, then the priority merge (first-id-wins), then dropping dismissed ids.
    /// The environment-aware path is `effective_config::CampaignOverlay`, which also applies `GROK_CAMPAIGNS_OVERRIDE`.
    pub fn resolve_campaigns(
        &self,
        base: &toml::Value,
        remote_campaigns: &[crate::campaigns::CampaignEntry],
        dismissed_ids: &std::collections::HashSet<String>,
    ) -> Vec<crate::campaigns::CampaignEntry> {
        if campaigns_application_disabled(base) {
            return Vec::new();
        }
        self.merge_active_campaigns(remote_campaigns, dismissed_ids)
    }

    /// Every campaign source merged by priority (first id wins), less the dismissed ids.
    pub(crate) fn merge_active_campaigns(
        &self,
        remote_campaigns: &[crate::campaigns::CampaignEntry],
        dismissed_ids: &std::collections::HashSet<String>,
    ) -> Vec<crate::campaigns::CampaignEntry> {
        let merged = crate::campaigns::merge_campaign_entries(
            &self.campaign_source_slices(remote_campaigns),
        );
        crate::campaigns::filter_active_campaigns(merged, dismissed_ids)
    }

    /// Re-merge the requirements layers so an admin's `requirements.toml` always wins over a campaign overlay, whatever the campaign's source layer.
    /// Campaigns are full-power (any field), so this is the structural guarantee that a lower-trust campaign can't override an admin-set field.
    fn reapply_requirements(&self, merged: &mut toml::Value) {
        for req in self.requirements_in_order() {
            deep_merge_toml(merged, req);
        }
    }

    /// Apply campaign patches, re-apply the `GROK_CONFIG` overlay, then restore requirements.
    pub fn apply_campaign_overrides(
        &self,
        merged: &mut toml::Value,
        active: &[crate::campaigns::CampaignEntry],
    ) {
        crate::campaigns::apply_active_campaign_patches(merged, active);
        if let Some(overlay) = &self.env_overlay {
            deep_merge_toml(merged, overlay);
        }
        self.reapply_requirements(merged);
    }

    /// Layer merge and disk/remote campaign overlay, honoring the kill switch.
    /// The shell's `load_effective_config` is the remote/override-aware path; this is used by `effective_config_disk_only` and tests.
    pub fn effective_config_with_campaigns(
        &self,
        remote_campaigns: &[crate::campaigns::CampaignEntry],
        dismissed_ids: &std::collections::HashSet<String>,
    ) -> toml::Value {
        let mut merged = self.effective_config_base();
        let active = self.resolve_campaigns(&merged, remote_campaigns, dismissed_ids);
        self.apply_campaign_overrides(&mut merged, &active);
        merged
    }

    /// Disk campaigns and on-disk dismiss (`campaigns_state.json`); **no remote, no env override**.
    /// The name makes the divergence from the shell's remote-aware `load_effective_config` explicit at every call site.
    pub fn effective_config_disk_only(&self) -> toml::Value {
        self.effective_config_with_campaigns(&[], &load_dismissed_ids_from_home())
    }

    pub fn has_managed(&self) -> bool {
        self.managed.as_table().is_some_and(|t| !t.is_empty())
            || self
                .system_managed
                .as_table()
                .is_some_and(|t| !t.is_empty())
    }

    pub fn has_system_managed(&self) -> bool {
        self.system_managed
            .as_table()
            .is_some_and(|t| !t.is_empty())
    }
}

/// `GROK_CAMPAIGNS=0` or `[features] campaigns = false` on pre-campaign base.
pub fn campaigns_application_disabled(base_effective: &toml::Value) -> bool {
    crate::env_bool("GROK_CAMPAIGNS") == Some(false) || campaigns_disabled_in_config(base_effective)
}

/// `[features] campaigns = false` on pre-campaign base.
pub(crate) fn campaigns_disabled_in_config(base_effective: &toml::Value) -> bool {
    base_effective
        .get("features")
        .and_then(|f| f.get("campaigns"))
        .and_then(|c| c.as_bool())
        == Some(false)
}

/// Process-global `GROK_CAMPAIGNS` lock. A mutex local to the setter is not
/// enough because `effective_config_with_campaigns` also reads the var.
#[cfg(test)]
pub(crate) fn lock_grok_campaigns_env() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

/// Disk layers only (no remote, no env override).
/// Prefer `xai_grok_shell::util::config::load_effective_config` when remote campaigns or `GROK_CAMPAIGNS_OVERRIDE` must be honored.
/// The name mirrors [`ConfigLayers::effective_config_disk_only`] so the divergence from the remote-aware loader is explicit at every call site.
pub fn load_effective_config_disk_only() -> std::io::Result<toml::Value> {
    Ok(ConfigLayers::load()?.effective_config_disk_only())
}

/// On-disk campaign dismiss state.
/// This is the single source of truth for the file's name, location, and JSON shape.
/// The shell's writer reuses these so the read and write sides can't drift.
pub const CAMPAIGNS_STATE_FILE: &str = "campaigns_state.json";

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct CampaignsState {
    #[serde(default)]
    pub dismissed_ids: Vec<String>,
}

/// Path to `$GROK_HOME/campaigns_state.json` under `home`.
pub fn campaigns_state_path(home: &std::path::Path) -> std::path::PathBuf {
    home.join(CAMPAIGNS_STATE_FILE)
}

/// Fail-open dismissed ids from `$GROK_HOME/campaigns_state.json`.
pub fn load_dismissed_ids_from_home() -> std::collections::HashSet<String> {
    crate::user_grok_home()
        .map(|grok_home| load_dismissed_ids(&grok_home))
        .unwrap_or_default()
}

/// Fail-open dismissed ids from `campaigns_state.json` under `grok_home`.
pub fn load_dismissed_ids(grok_home: &std::path::Path) -> std::collections::HashSet<String> {
    let Ok(contents) = std::fs::read_to_string(campaigns_state_path(grok_home)) else {
        return std::collections::HashSet::new();
    };
    serde_json::from_str::<CampaignsState>(&contents)
        .map(|s| s.dismissed_ids.into_iter().collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requirements_pin_subagents_enabled_over_a_user_true() {
        let layers = ConfigLayers {
            user: toml::from_str("[subagents]\nenabled = true\n").unwrap(),
            user_requirements: Some(toml::from_str("[subagents]\nenabled = false\n").unwrap()),
            ..Default::default()
        };
        assert_eq!(
            layers
                .effective_config_base()
                .get("subagents")
                .and_then(|section| section.get("enabled"))
                .and_then(toml::Value::as_bool),
            Some(false),
        );
    }

    #[test]
    fn effective_config_mdm_requirements_win_over_system_and_user() {
        // MDM is merged last, so an admin-forced value clamps the effective config over both the user config and the system requirements layer
        let layers = ConfigLayers {
            user: toml::from_str("[features]\nweb_fetch = true\n").unwrap(),
            system_requirements: Some(toml::from_str("[features]\nweb_fetch = true\n").unwrap()),
            mdm_requirements: Some(toml::from_str("[features]\nweb_fetch = false\n").unwrap()),
            ..Default::default()
        };
        assert_eq!(
            layers
                .effective_config_disk_only()
                .get("features")
                .and_then(|f| f.get("web_fetch"))
                .and_then(toml::Value::as_bool),
            Some(false),
        );
    }

    /// `GROK_CAMPAIGNS=0` disables campaign application regardless of config.
    #[test]
    fn kill_switch_env_var_disables() {
        let _g = lock_grok_campaigns_env();
        let prior = std::env::var_os("GROK_CAMPAIGNS");
        let empty = toml::Value::Table(Default::default());

        // SAFETY: `lock_grok_campaigns_env` serializes this against every test that
        // mutates or reads GROK_CAMPAIGNS.
        unsafe { std::env::set_var("GROK_CAMPAIGNS", "0") };
        assert!(campaigns_application_disabled(&empty));

        unsafe { std::env::remove_var("GROK_CAMPAIGNS") };
        assert!(!campaigns_application_disabled(&empty));

        match prior {
            Some(v) => unsafe { std::env::set_var("GROK_CAMPAIGNS", v) },
            None => unsafe { std::env::remove_var("GROK_CAMPAIGNS") },
        }
    }

    #[test]
    fn env_overlay_precedence_and_overlay_free_merge() {
        let _env = lock_grok_campaigns_env();
        let mut layers = ConfigLayers {
            user: toml::from_str("[models]\ndefault = \"user\"\n[telemetry]\nmode = \"on\"\n")
                .unwrap(),
            env_overlay: Some(
                toml::from_str(
                    "[models]\ndefault = \"overlay\"\ndefault_reasoning_effort = \"high\"\n",
                )
                .unwrap(),
            ),
            ..Default::default()
        };
        layers.campaigns.managed = vec![crate::campaigns::CampaignEntry {
            id: "c1".into(),
            patch: toml::from_str("[models]\ndefault = \"campaign\"\n").unwrap(),
        }];
        let none = std::collections::HashSet::new();

        let with_overlay: toml::Value = toml::from_str(
            "[models]\ndefault = \"overlay\"\ndefault_reasoning_effort = \"high\"\n\
             [telemetry]\nmode = \"on\"\n",
        )
        .unwrap();
        assert_eq!(
            layers.effective_config_with_campaigns(&[], &none),
            with_overlay
        );

        let overlay_free: toml::Value =
            toml::from_str("[models]\ndefault = \"user\"\n[telemetry]\nmode = \"on\"\n").unwrap();
        assert_eq!(layers.effective_config_base_without_overlay(), overlay_free);

        layers.user_requirements =
            Some(toml::from_str("[models]\ndefault = \"pinned\"\n").unwrap());
        let clamped: toml::Value = toml::from_str(
            "[models]\ndefault = \"pinned\"\ndefault_reasoning_effort = \"high\"\n\
             [telemetry]\nmode = \"on\"\n",
        )
        .unwrap();
        assert_eq!(layers.effective_config_with_campaigns(&[], &none), clamped);
    }

    fn toml_bool(value: &toml::Value, table: &str, key: &str) -> Option<bool> {
        value
            .get(table)
            .and_then(|section| section.get(key))
            .and_then(toml::Value::as_bool)
    }

    fn toml_str(value: &toml::Value, table: &str, key: &str) -> Option<String> {
        value
            .get(table)
            .and_then(|section| section.get(key))
            .and_then(toml::Value::as_str)
            .map(str::to_owned)
    }

    /// User config outranks managed config. The daemon ignores this merge, so the e2e suite keeps
    /// one row per route only where a resolved value is used.
    #[test]
    fn user_config_wins_over_managed_layer() {
        let layers = ConfigLayers {
            managed: toml::from_str("[ui]\nyolo = false\n").unwrap(),
            user: toml::from_str("[ui]\nyolo = true\n").unwrap(),
            ..Default::default()
        };
        let none = std::collections::HashSet::new();
        assert_eq!(
            toml_bool(
                &layers.effective_config_with_campaigns(&[], &none),
                "ui",
                "yolo"
            ),
            Some(true)
        );
    }

    #[test]
    fn requirement_wins_over_user_config() {
        let layers = ConfigLayers {
            user: toml::from_str("[ui]\nyolo = false\n").unwrap(),
            user_requirements: Some(toml::from_str("[ui]\nyolo = true\n").unwrap()),
            ..Default::default()
        };
        let none = std::collections::HashSet::new();
        assert_eq!(
            toml_bool(
                &layers.effective_config_with_campaigns(&[], &none),
                "ui",
                "yolo"
            ),
            Some(true)
        );
    }

    #[test]
    fn requirement_timeout_wins_over_user_timeout() {
        let layers = ConfigLayers {
            user: toml::from_str("[toolset.bash]\ntimeout_secs = 30\n").unwrap(),
            user_requirements: Some(toml::from_str("[toolset.bash]\ntimeout_secs = 1\n").unwrap()),
            ..Default::default()
        };
        let none = std::collections::HashSet::new();
        let secs = layers
            .effective_config_with_campaigns(&[], &none)
            .get("toolset")
            .and_then(|toolset| toolset.get("bash"))
            .and_then(|bash| bash.get("timeout_secs"))
            .and_then(toml::Value::as_integer);
        assert_eq!(secs, Some(1));
    }

    #[test]
    fn in_range_version_override_applies_and_out_of_range_is_ignored() {
        let version = semver::Version::new(1, 0, 0);
        let mut in_range: toml::Value = toml::from_str(
            "[ui]\nyolo = false\n\n[[version_overrides]]\nmaximum_version = \"9999.0.0\"\n[version_overrides.ui]\nyolo = true\n",
        )
        .unwrap();
        crate::version_overrides::apply_version_overrides(&mut in_range, &version).unwrap();
        assert_eq!(toml_bool(&in_range, "ui", "yolo"), Some(true));

        let mut out_of_range: toml::Value = toml::from_str(
            "[ui]\nyolo = true\n\n[[version_overrides]]\nminimum_version = \"9999.0.0\"\n[version_overrides.ui]\nyolo = false\n",
        )
        .unwrap();
        crate::version_overrides::apply_version_overrides(&mut out_of_range, &version).unwrap();
        assert_eq!(toml_bool(&out_of_range, "ui", "yolo"), Some(true));
    }

    #[test]
    fn version_override_applies_on_the_requirements_layer_before_merge() {
        let version = semver::Version::new(1, 0, 0);
        let mut requirements: toml::Value = toml::from_str(
            "[[version_overrides]]\nmaximum_version = \"9999.0.0\"\n[version_overrides.ui]\nyolo = true\n",
        )
        .unwrap();
        crate::version_overrides::apply_version_overrides(&mut requirements, &version).unwrap();
        let layers = ConfigLayers {
            user: toml::from_str("[ui]\nyolo = false\n").unwrap(),
            user_requirements: Some(requirements),
            ..Default::default()
        };
        let none = std::collections::HashSet::new();
        assert_eq!(
            toml_bool(
                &layers.effective_config_with_campaigns(&[], &none),
                "ui",
                "yolo"
            ),
            Some(true)
        );
    }

    #[test]
    fn bad_version_override_soft_fails_and_keeps_the_layer() {
        let mut layer: toml::Value = toml::from_str(
            "[ui]\nyolo = true\n\n[[version_overrides]]\nminimum_version = \"not-a-version\"\n",
        )
        .unwrap();
        let error = crate::version_overrides::apply_version_overrides(
            &mut layer,
            &semver::Version::new(1, 0, 0),
        );
        assert!(error.is_err());
        assert_eq!(toml_bool(&layer, "ui", "yolo"), Some(true));

        // A requirements layer that would turn yolo off is dropped whole when its
        // version override cannot parse, so the user's yolo stays on.
        let rejected = crate::validation::normalize_requirements_value(
            toml::from_str(
                "[ui]\nyolo = false\n\n[[version_overrides]]\nminimum_version = \"not-a-version\"\n",
            )
            .unwrap(),
            "requirements.toml",
        );
        assert!(rejected.is_none());
        let layers = ConfigLayers {
            user: toml::from_str("[ui]\nyolo = true\n").unwrap(),
            user_requirements: rejected,
            ..Default::default()
        };
        let none = std::collections::HashSet::new();
        assert_eq!(
            toml_bool(
                &layers.effective_config_with_campaigns(&[], &none),
                "ui",
                "yolo"
            ),
            Some(true)
        );
    }

    #[test]
    fn campaign_overlays_user_model_and_loses_to_requirements() {
        let none = std::collections::HashSet::new();
        let mut layers = ConfigLayers {
            user: toml::from_str("[models]\ndefault = \"user-model\"\n").unwrap(),
            ..Default::default()
        };
        layers.campaigns.managed = vec![crate::campaigns::CampaignEntry {
            id: "conformance-campaign-1".into(),
            patch: toml::from_str("[models]\ndefault = \"campaign-model\"\n").unwrap(),
        }];
        assert_eq!(
            toml_str(
                &layers.effective_config_with_campaigns(&[], &none),
                "models",
                "default"
            )
            .as_deref(),
            Some("campaign-model")
        );

        layers.user_requirements =
            Some(toml::from_str("[models]\ndefault = \"pinned-model\"\n").unwrap());
        assert_eq!(
            toml_str(
                &layers.effective_config_with_campaigns(&[], &none),
                "models",
                "default"
            )
            .as_deref(),
            Some("pinned-model")
        );
    }

    fn layer(text: &str) -> toml::Value {
        toml::from_str(text).unwrap()
    }

    fn overlay(text: &str) -> toml::Value {
        let mut value = layer(text);
        crate::config_override::retain_overlay_allowed(value.as_table_mut().unwrap());
        value
    }

    fn web_fetch(config: &toml::Value) -> Option<bool> {
        config
            .get("features")
            .and_then(|features| features.get("web_fetch"))
            .and_then(toml::Value::as_bool)
    }

    fn exclude_len(config: &toml::Value) -> Option<usize> {
        config
            .get("shell_environment_policy")
            .and_then(|policy| policy.get("exclude"))
            .and_then(toml::Value::as_array)
            .map(Vec::len)
    }

    fn empty() -> toml::Value {
        toml::Value::Table(toml::map::Map::new())
    }

    #[test]
    fn layer_precedence_managed_user_overlay_requirements() {
        type Row<'a> = (
            Option<&'a str>,
            Option<&'a str>,
            Option<&'a str>,
            Option<&'a str>,
            Option<bool>,
        );
        let rows: &[Row] = &[
            (
                Some("[features]\nweb_fetch = true\n"),
                None,
                None,
                None,
                Some(true),
            ),
            (
                Some("[features]\nweb_fetch = true\n"),
                Some("[features]\nweb_fetch = false\n"),
                None,
                None,
                Some(false),
            ),
            (
                Some("[features]\nweb_fetch = true\n"),
                Some("[features]\nweb_fetch = false\n"),
                Some("[features]\nweb_fetch = true\n"),
                None,
                Some(true),
            ),
            (
                Some("[features]\nweb_fetch = true\n"),
                Some("[features]\nweb_fetch = false\n"),
                Some("[features]\nweb_fetch = true\n"),
                Some("[features]\nweb_fetch = false\n"),
                Some(false),
            ),
            (
                None,
                Some("[features]\nweb_fetch = true\n"),
                Some("[features]\nweb_fetch = false\n"),
                None,
                Some(false),
            ),
            (
                None,
                None,
                Some("[features]\nweb_fetch = true\n"),
                Some("[features]\nweb_fetch = false\n"),
                Some(false),
            ),
            (None, None, None, None, None),
        ];
        for (managed, user, env_overlay, requirements, expected) in rows {
            let layers = ConfigLayers {
                managed: managed.map(layer).unwrap_or_else(empty),
                user: user.map(layer).unwrap_or_else(empty),
                env_overlay: env_overlay.map(overlay),
                user_requirements: requirements.map(layer),
                ..ConfigLayers::default()
            };
            assert_eq!(*expected, web_fetch(&layers.effective_config_base()));
        }
    }

    #[test]
    fn valid_overlay_clears_a_lower_exclude_and_a_missing_overlay_leaves_it() {
        let user = layer("[shell_environment_policy]\nexclude = [\"CONFORMANCE_PROBED\"]\n");
        let without = ConfigLayers {
            user: user.clone(),
            env_overlay: None,
            ..ConfigLayers::default()
        };
        assert_eq!(Some(1), exclude_len(&without.effective_config_base()));
        let with = ConfigLayers {
            user,
            env_overlay: Some(overlay("[shell_environment_policy]\nexclude = []\n")),
            ..ConfigLayers::default()
        };
        assert_eq!(Some(0), exclude_len(&with.effective_config_base()));
    }
}
