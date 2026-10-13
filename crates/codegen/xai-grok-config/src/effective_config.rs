//! The effective config with its campaign overlay: the remote campaign cache, the dismiss state, and
//! the environment override.

use std::collections::HashSet;
use std::path::Path;
use std::sync::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::campaigns::{CampaignEntry, filter_active_campaigns};
use crate::config_layers::campaigns_disabled_in_config;
use crate::{
    CampaignOverride, CampaignsState, ConfigLayers, RemoteSettings, campaigns_state_path,
    load_dismissed_ids_from_home, user_grok_home,
};

/// FIFO cap on persisted dismissed ids.
/// Evicting the oldest can re-nudge for a still-live campaign after a user dismisses more than this over the CLI's life.
const MAX_DISMISSED_IDS: usize = 32;

static DISMISS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
static DISMISS_TMP_NONCE: AtomicU64 = AtomicU64::new(0);

static REMOTE_CAMPAIGN_CACHE: RwLock<Vec<CampaignEntry>> = RwLock::new(Vec::new());

/// Seed the process-global remote campaign cache.
/// A `None` settings value (e.g. a failed fetch) is a no-op so it can't clobber a previously-seeded cache.
/// `Some` with zero campaigns legitimately clears it (campaigns withdrawn).
pub fn set_remote_campaigns_from_settings(remote: Option<&RemoteSettings>) {
    let Some(remote) = remote else {
        return;
    };
    set_remote_campaigns(remote_campaigns_from_settings(Some(remote)));
}

pub fn set_remote_campaigns(entries: Vec<CampaignEntry>) {
    if let Ok(mut g) = REMOTE_CAMPAIGN_CACHE.write() {
        *g = entries;
    }
}

pub fn cached_remote_campaigns() -> Vec<CampaignEntry> {
    REMOTE_CAMPAIGN_CACHE
        .read()
        .map(|g| g.clone())
        .unwrap_or_default()
}

pub fn dismiss_campaign_ids(ids: impl IntoIterator<Item = String>) {
    let Some(home) = user_grok_home() else {
        return;
    };
    if let Err(e) = dismiss_campaign_ids_at(&home, ids) {
        tracing::warn!(error = %e, "campaigns: failed to persist dismiss state");
    }
}

/// Append `ids` to the dismissed set and write `campaigns_state.json` atomically (write to a temp file, then rename).
/// Corrupt prior state is renamed aside, not discarded.
fn dismiss_campaign_ids_at(
    home: &Path,
    ids: impl IntoIterator<Item = String>,
) -> std::io::Result<()> {
    use fs2::FileExt as _;
    let _guard = DISMISS_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let path = campaigns_state_path(home);
    // Cross-process advisory lock over the read-modify-write: in leader mode several grok processes share `$GROK_HOME`
    // The in-process mutex alone would let one process overwrite another's update
    // The lock is best-effort; a lock failure still proceeds
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path.with_extension("json.lock"));
    if let Ok(ref f) = lock {
        let _ = f.lock_exclusive();
    }
    let mut ordered = match std::fs::read_to_string(&path) {
        Ok(contents) => match serde_json::from_str::<CampaignsState>(&contents) {
            Ok(s) => s.dismissed_ids,
            Err(e) => {
                let _ = std::fs::rename(&path, path.with_extension("json.corrupt"));
                tracing::warn!(error = %e, "campaigns: corrupt dismiss state; renamed aside");
                Vec::new()
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e),
    };
    let mut seen: HashSet<String> = ordered.iter().cloned().collect();
    for id in ids {
        if id.is_empty() || !seen.insert(id.clone()) {
            continue;
        }
        ordered.push(id);
    }
    if ordered.len() > MAX_DISMISSED_IDS {
        let drop_n = ordered.len() - MAX_DISMISSED_IDS;
        ordered.drain(..drop_n);
    }
    let json = serde_json::to_string(&CampaignsState {
        dismissed_ids: ordered,
    })
    .map_err(std::io::Error::other)?;
    let nonce = DISMISS_TMP_NONCE.fetch_add(1, Ordering::Relaxed);
    let tmp = path.with_extension(format!("json.{}.{}.tmp", std::process::id(), nonce));
    std::fs::write(&tmp, &json)?;
    std::fs::rename(&tmp, &path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// The campaign switches a process reads from its environment.
#[derive(Debug, Clone, Default)]
pub struct CampaignEnv {
    /// `GROK_CAMPAIGNS_OVERRIDE`: a JSON array of campaigns that replaces every other source and
    /// beats the kill switch. Invalid JSON replaces them with none: the variable means "exactly
    /// these campaigns", so a typo must not fall back to the sources it was meant to replace.
    pub override_json: Option<String>,
    /// `GROK_CAMPAIGNS=0` stops campaigns from applying.
    pub is_kill_switch_set: bool,
}

impl CampaignEnv {
    pub fn from_process() -> CampaignEnv {
        CampaignEnv {
            override_json: std::env::var("GROK_CAMPAIGNS_OVERRIDE").ok(),
            is_kill_switch_set: crate::env_bool("GROK_CAMPAIGNS") == Some(false),
        }
    }

    fn replacement(&self) -> Option<Vec<CampaignEntry>> {
        let json = self.override_json.as_deref()?;
        match serde_json::from_str::<Vec<CampaignOverride>>(json) {
            Ok(list) => Some(
                list.into_iter()
                    .filter_map(remote_campaign_to_entry)
                    .collect(),
            ),
            Err(e) => {
                tracing::warn!(error = %e, "invalid GROK_CAMPAIGNS_OVERRIDE JSON; suppressing all campaigns");
                Some(Vec::new())
            }
        }
    }
}

/// What decides which campaigns apply over the config layers, besides the layers themselves.
#[derive(Debug, Clone, Default)]
pub struct CampaignSources {
    pub remote: Vec<CampaignEntry>,
    pub dismissed: HashSet<String>,
    pub env: CampaignEnv,
}

impl CampaignSources {
    /// `remote` with the dismiss state under the user's grok home and this process's environment.
    pub fn from_process(remote: Vec<CampaignEntry>) -> CampaignSources {
        CampaignSources {
            remote,
            dismissed: load_dismissed_ids_from_home(),
            env: CampaignEnv::from_process(),
        }
    }
}

/// The effective config over some config layers, and the campaigns that shaped it.
pub struct CampaignOverlay {
    /// The merged layers before any campaign applied.
    pub base: toml::Value,
    /// The campaigns applied, highest priority first.
    pub active: Vec<CampaignEntry>,
    pub effective: toml::Value,
}

impl CampaignOverlay {
    /// The single campaign-resolution path: the environment override replaces every source and
    /// beats the kill switch; otherwise the kill switch, then the layer and remote merge, then the
    /// dismiss filter.
    pub fn new(layers: &ConfigLayers, sources: &CampaignSources) -> CampaignOverlay {
        let base = layers.effective_config_base();
        let active = match sources.env.replacement() {
            Some(replacement) => filter_active_campaigns(replacement, &sources.dismissed),
            None if sources.env.is_kill_switch_set || campaigns_disabled_in_config(&base) => {
                Vec::new()
            }
            None => layers.merge_active_campaigns(&sources.remote, &sources.dismissed),
        };
        let mut effective = base.clone();
        layers.apply_campaign_overrides(&mut effective, &active);
        CampaignOverlay {
            base,
            active,
            effective,
        }
    }
}

fn remote_campaign_to_entry(c: CampaignOverride) -> Option<CampaignEntry> {
    let id = c.id.as_deref()?.trim();
    if id.is_empty() {
        return None;
    }
    let id = id.to_owned();
    // The patch may set any field with no allowlist filtering; `ConfigLayers::apply_campaign_overrides` restores requirements precedence
    let patch = match toml::Value::try_from(serde_json::Value::Object(c.patch)) {
        Ok(toml::Value::Table(t)) => t,
        Ok(_) => return None,
        Err(e) => {
            tracing::warn!(error = %e, %id, "campaigns: invalid remote patch; ignoring");
            return None;
        }
    };
    if patch.is_empty() {
        return None;
    }
    Some(CampaignEntry { id, patch })
}

pub fn remote_campaigns_from_settings(remote: Option<&RemoteSettings>) -> Vec<CampaignEntry> {
    remote
        .map(|settings| {
            settings
                .campaigns
                .iter()
                .cloned()
                .filter_map(remote_campaign_to_entry)
                .collect()
        })
        .unwrap_or_default()
}

/// Campaigns eligible for dismissal when the user persists a choice (loads the layers, the remote cache, and the dismiss state).
/// Unlike the apply path this deliberately **ignores the kill switch**: dismissing a suppressed campaign is harmless.
/// Skipping the dismissal lets a later re-enabled campaign override a choice the user already made ("user pick wins, forever"). A layer-load failure likewise falls back to the remote cache instead of failing closed: remote campaigns still get dismissed on that path. Disk-layer campaigns can be missed until the transient failure clears (they re-dismiss on the next pick).
pub fn resolve_dismissable_campaigns() -> Vec<CampaignEntry> {
    dismissable_campaigns(
        ConfigLayers::load(),
        &CampaignSources::from_process(cached_remote_campaigns()),
    )
}

fn dismissable_campaigns(
    layers: std::io::Result<ConfigLayers>,
    sources: &CampaignSources,
) -> Vec<CampaignEntry> {
    if let Some(replacement) = sources.env.replacement() {
        return filter_active_campaigns(replacement, &sources.dismissed);
    }
    match layers {
        Ok(layers) => layers.merge_active_campaigns(&sources.remote, &sources.dismissed),
        Err(e) => {
            tracing::warn!(error = %e, "campaigns: layer load failed; dismiss bookkeeping using remote cache only");
            filter_active_campaigns(sources.remote.clone(), &sources.dismissed)
        }
    }
}

/// Effective config with the remote/override-aware campaign overlay, from one `ConfigLayers::load`.
pub fn load_effective_config() -> std::io::Result<toml::Value> {
    load_effective_config_with_layers().map(|loaded| loaded.effective)
}

/// [`load_effective_config`] and what it merged: the layers and the campaign patches it applied,
/// highest priority first.
pub struct EffectiveConfigLayers {
    pub layers: ConfigLayers,
    pub active_campaigns: Vec<CampaignEntry>,
    pub effective: toml::Value,
}

/// [`load_effective_config`] plus its inputs, for a caller that also reads the layers one by one.
pub fn load_effective_config_with_layers() -> std::io::Result<EffectiveConfigLayers> {
    let layers = ConfigLayers::load()?;
    let overlay = CampaignOverlay::new(
        &layers,
        &CampaignSources::from_process(cached_remote_campaigns()),
    );
    Ok(EffectiveConfigLayers {
        layers,
        active_campaigns: overlay.active,
        effective: overlay.effective,
    })
}

/// An effective config and the remote settings whose campaigns it applied.
pub struct ConfigWithRemote {
    pub effective: toml::Value,
    pub remote: CachedRemote,
}

/// The remote settings read from the settings cache by a process that never fetches them.
pub enum CachedRemote {
    /// The config turns remote settings off, or no account is signed in.
    NotApplicable,
    Entry(Box<RemoteSettings>),
    /// No entry could be read for the signed-in account.
    Unavailable,
}

#[cfg(test)]
#[path = "effective_config_tests.rs"]
mod tests;
