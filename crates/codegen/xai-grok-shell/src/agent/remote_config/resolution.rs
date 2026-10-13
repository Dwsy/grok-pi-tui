//! Model-id resolution: catalog keys, routing slugs, and selection.

use std::collections::HashSet;

use indexmap::IndexMap;

use super::ModelGlobSet;
use crate::agent::config::{self, ModelEntry};
use agent_client_protocol as acp;
use xai_grok_sampling_types::ReasoningEffort;

/// Which sources feed the model catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CatalogSource {
    /// Built-in defaults until the first fetch, then the fetched list, plus every `[model.<id>]` table.
    Standard,
    /// External auth with a configured models endpoint. Built-in and bundled models never appear.
    /// See [`EndpointScope`] for which entries stay.
    ModelsEndpoint,
}

/// What the models endpoint has listed, as the external-auth catalog sees it.
enum EndpointListing {
    /// No list has arrived, so the `[model.<id>]` tables are the whole catalog.
    NotFetched,
    /// The ids the endpoint listed, possibly none. An entry stays only when its `model` is one of them.
    Listed(HashSet<String>),
}

/// Which entries the external-auth catalog keeps.
/// An entry must pass the [`EndpointListing`] check and send requests only to hosts the models endpoint serves.
struct EndpointScope {
    listing: EndpointListing,
    /// The models endpoint's inference host plus every host a listed row uses.
    hosts: HashSet<String>,
}

impl EndpointScope {
    fn new(cfg: &config::Config, prefetched: Option<&IndexMap<String, ModelEntry>>) -> Self {
        let mut hosts = HashSet::from([url_host(&cfg.endpoints.resolve_inference_base_url())]);
        let listing = match prefetched {
            None => EndpointListing::NotFetched,
            Some(rows) => {
                hosts.extend(rows.values().flat_map(entry_hosts));
                EndpointListing::Listed(
                    rows.iter()
                        .flat_map(|(key, entry)| [key.clone(), entry.info.model.clone()])
                        .collect(),
                )
            }
        };
        EndpointScope { listing, hosts }
    }

    fn keeps(&self, entry: &ModelEntry) -> bool {
        let listed = match &self.listing {
            EndpointListing::NotFetched => true,
            EndpointListing::Listed(ids) => ids.contains(&entry.info.model),
        };
        listed && entry_hosts(entry).all(|host| self.hosts.contains(&host))
    }
}

/// The hosts `entry` sends requests to: its `base_url`, and its `api_base_url` when set.
fn entry_hosts(entry: &ModelEntry) -> impl Iterator<Item = String> + '_ {
    std::iter::once(entry.info.base_url.as_str())
        .chain(entry.api_base_url.as_deref())
        .map(url_host)
}

/// The URL's host, or the raw string when it does not parse, so an unparseable URL matches only itself.
fn url_host(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_owned))
        .unwrap_or_else(|| url.to_owned())
}

/// The id used when the catalog has no usable model.
/// External auth with a models endpoint never names a bundled model. It keeps `preferred` or leaves the id empty.
pub(crate) fn fallback_model_id(cfg: &config::Config, preferred: Option<&str>) -> String {
    match CatalogSource::for_config(cfg) {
        CatalogSource::Standard => crate::models::default_model().to_owned(),
        CatalogSource::ModelsEndpoint => preferred.unwrap_or_default().to_owned(),
    }
}

impl CatalogSource {
    pub(crate) fn for_config(cfg: &config::Config) -> Self {
        if cfg.grok_com_config.auth_provider_command.is_some()
            && cfg.endpoints.has_custom_endpoint()
        {
            CatalogSource::ModelsEndpoint
        } else {
            CatalogSource::Standard
        }
    }
}

/// Map a model id (catalog key or routing slug) to its catalog key.
pub(crate) fn resolve_catalog_key(
    models: &IndexMap<String, ModelEntry>,
    id: &acp::ModelId,
) -> Option<acp::ModelId> {
    let id_str = id.0.as_ref();
    if models.contains_key(id_str) {
        return Some(id.clone());
    }
    models
        .iter()
        .rev()
        .find(|(_, entry)| entry.info.has_model_id(id_str))
        .map(|(key, _)| acp::ModelId::new(key.clone()))
}

/// Catalog key for a persisted session model id, restricted to **selectable** entries.
pub(crate) fn selectable_catalog_key_for_persisted(
    models: &IndexMap<String, ModelEntry>,
    available: &IndexMap<acp::ModelId, acp::ModelInfo>,
    id: &acp::ModelId,
) -> Option<acp::ModelId> {
    if available.contains_key(id) {
        return Some(id.clone());
    }
    let id_str = id.0.as_ref();
    if let Some((key, _)) = models.iter().rev().find(|(key, entry)| {
        available.contains_key(&acp::ModelId::new((*key).clone()))
            && entry.info.has_model_id(id_str)
    }) {
        return Some(acp::ModelId::new(key.clone()));
    }
    resolve_catalog_key(models, id).filter(|key| available.contains_key(key))
}

/// Pick the default model: CLI > env > config > remote-settings hint, falling back to the first visible model, then [`fallback_model_id`].
pub(crate) fn resolve_default_model(
    cfg: &config::Config,
    catalog: &IndexMap<String, ModelEntry>,
    is_session_auth: bool,
) -> (String, ModelEntry, config::ConfigSource) {
    let visible: IndexMap<String, ModelEntry> = catalog
        .iter()
        .filter(|(_, e)| e.info.visible_for_auth(is_session_auth) && e.info.user_selectable)
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    let model_pref = config::resolve_string_flag(
        cfg.default_model_override.as_deref(),
        "GROK_DEFAULT_MODEL",
        cfg.models.default.as_deref(),
        cfg.remote_settings
            .as_ref()
            .and_then(|rs| rs.default_model.as_deref()),
    );

    let first_or_fallback = || -> (String, ModelEntry) {
        if let Some((key, first)) = visible.first() {
            return (key.clone(), first.clone());
        }
        if let Some((key, entry)) = catalog.iter().find(|(_, e)| e.info.user_selectable) {
            tracing::warn!("no auth-visible selectable model; using first selectable entry");
            return (key.clone(), entry.clone());
        }
        let local_pref = model_pref.as_ref().filter(|p| {
            matches!(
                p.source,
                config::ConfigSource::Cli
                    | config::ConfigSource::Env
                    | config::ConfigSource::Config
            )
        });
        let default_id = fallback_model_id(cfg, local_pref.map(|p| p.value.as_str()));
        tracing::warn!(model = %default_id, "no selectable models; using the fallback model id");
        let mut entry = ModelEntry::fallback(&default_id, &cfg.endpoints);
        entry.info.user_selectable = model_is_allowlisted(cfg, &default_id, &default_id);
        (default_id, entry)
    };

    match &model_pref {
        None => {
            let (key, first) = first_or_fallback();
            (key, first, config::ConfigSource::Default)
        }
        Some(pref) => {
            let found = visible
                .get_key_value(&pref.value)
                .or_else(|| visible.iter().find(|(_, m)| m.has_model_id(&pref.value)));

            if let Some((key, entry)) = found {
                (key.clone(), entry.clone(), pref.source)
            } else {
                let is_explicit = matches!(
                    pref.source,
                    config::ConfigSource::Cli
                        | config::ConfigSource::Env
                        | config::ConfigSource::Config
                );
                if is_explicit {
                    tracing::warn!(
                        model_id = %pref.value, source = %pref.source,
                        "preferred model not in available models, falling back"
                    );
                } else {
                    tracing::debug!(
                        model_id = %pref.value, source = %pref.source,
                        "remote default_model not in available models, skipping"
                    );
                }
                let campaign_pref_missing = cfg.models.default_is_campaign_driven
                    && matches!(pref.source, config::ConfigSource::Config);
                if campaign_pref_missing
                    && let Some(prev) = cfg
                        .models
                        .pre_campaign_default
                        .as_deref()
                        .filter(|s| !s.is_empty())
                    && let Some((key, entry)) = visible
                        .get_key_value(prev)
                        .or_else(|| visible.iter().find(|(_, m)| m.has_model_id(prev)))
                {
                    tracing::info!(
                        unavailable = %pref.value, fallback = %prev,
                        "campaign-driven default unavailable in catalog; recovering the pre-campaign default"
                    );
                    return (key.clone(), entry.clone(), config::ConfigSource::Config);
                }
                let (key, first) = first_or_fallback();
                (key, first, config::ConfigSource::Default)
            }
        }
    }
}

/// Keep the picker projection of `catalog` (`ModelInfo::is_picker_eligible`) in ACP wire format.
pub(crate) fn available_models(
    catalog: &IndexMap<String, ModelEntry>,
    is_session_auth: bool,
) -> IndexMap<acp::ModelId, acp::ModelInfo> {
    let visible: IndexMap<String, ModelEntry> = catalog
        .iter()
        .filter(|(_, e)| e.info.is_picker_eligible(is_session_auth))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    config::to_acp_model_info(&visible)
}

/// Resolved allowlist: fleet pin, user/project list, or unrestricted.
enum EffectiveAllowlist<'a> {
    Unrestricted,
    Invalid,
    User(&'a [String]),
    Fleet(&'a [String]),
}

fn effective_allowlist(cfg: &config::Config) -> EffectiveAllowlist<'_> {
    use crate::agent::config::AllowlistPin;
    match cfg.requirements.allowed_models.pin_ref() {
        Some(AllowlistPin::FailClosed) => EffectiveAllowlist::Invalid,
        Some(AllowlistPin::List(patterns)) if patterns.is_empty() => {
            EffectiveAllowlist::Unrestricted
        }
        Some(AllowlistPin::List(patterns)) => EffectiveAllowlist::Fleet(patterns),
        None => match cfg.models.allowed_models.as_deref() {
            Some(patterns) if !patterns.is_empty() => EffectiveAllowlist::User(patterns),
            _ => EffectiveAllowlist::Unrestricted,
        },
    }
}

impl EffectiveAllowlist<'_> {
    fn is_unrestricted(&self) -> bool {
        matches!(self, Self::Unrestricted)
    }

    fn is_fleet(&self) -> bool {
        matches!(self, Self::Fleet(_) | Self::Invalid)
    }

    fn is_selected(&self, key: &str, model: &str) -> bool {
        match self {
            Self::Unrestricted => true,
            Self::Invalid => false,
            Self::Fleet(patterns) | Self::User(patterns) => {
                match ModelGlobSet::compile(Some(patterns)) {
                    Ok(None) => true,
                    Ok(Some(set)) => {
                        if matches!(self, Self::Fleet(_)) {
                            set.matches_model(model)
                        } else {
                            set.matches(key, model)
                        }
                    }
                    Err(_) => false,
                }
            }
        }
    }

    fn apply_selectability(&self, catalog: &mut IndexMap<String, ModelEntry>) {
        match self {
            Self::Unrestricted => {
                for entry in catalog.values_mut() {
                    entry.info.user_selectable = true;
                }
            }
            Self::Invalid => {
                for entry in catalog.values_mut() {
                    entry.info.user_selectable = false;
                }
            }
            Self::Fleet(patterns) | Self::User(patterns) => {
                match ModelGlobSet::compile(Some(patterns)) {
                    Ok(None) => {
                        for entry in catalog.values_mut() {
                            entry.info.user_selectable = true;
                        }
                    }
                    Ok(Some(set)) => {
                        let fleet = matches!(self, Self::Fleet(_));
                        for (key, entry) in catalog.iter_mut() {
                            entry.info.user_selectable = if fleet {
                                set.matches_model(&entry.model)
                            } else {
                                set.matches(key, &entry.model)
                            };
                        }
                    }
                    Err(bad) => {
                        tracing::error!(
                            patterns = ?bad,
                            "allowed_models: invalid glob(s); marking nothing selectable"
                        );
                        for entry in catalog.values_mut() {
                            entry.info.user_selectable = false;
                        }
                    }
                }
            }
        }
    }
}

/// Catalog-key match is user-config only. A fleet pin matches the routing
/// slug so a user `[model.grok-4-anything]` cannot satisfy `grok-4*`.
fn model_is_allowlisted(cfg: &config::Config, key: &str, model: &str) -> bool {
    effective_allowlist(cfg).is_selected(key, model)
}

pub(crate) fn allowlist_denied_message(cfg: &config::Config) -> &'static str {
    if effective_allowlist(cfg).is_fleet() {
        "This model isn't allowed by your organization's policy. Contact your administrator."
    } else {
        "This model isn't allowed by your allowed_models setting."
    }
}

pub(crate) fn allowlist_excludes_all_message(cfg: &config::Config) -> String {
    match effective_allowlist(cfg) {
        EffectiveAllowlist::Invalid => {
            "The organization model policy is invalid. Contact your administrator.".to_owned()
        }
        EffectiveAllowlist::Fleet(_) => {
            "None of your models are allowed by your organization's policy. Contact your administrator."
                .to_owned()
        }
        _ => "None of your models are allowed by allowed_models. \
             Broaden it or remove it from your config, then restart."
            .to_owned(),
    }
}

/// Single source of truth for the catalog.
/// It keeps the sources [`CatalogSource`] allows, then applies `disabled_models`, `allowed_models`, and `hidden_models`.
pub(crate) fn resolve_model_catalog(
    cfg: &config::Config,
    prefetched: Option<IndexMap<String, ModelEntry>>,
) -> IndexMap<String, ModelEntry> {
    let scope = (CatalogSource::for_config(cfg) == CatalogSource::ModelsEndpoint)
        .then(|| EndpointScope::new(cfg, prefetched.as_ref()));
    let mut catalog: IndexMap<String, ModelEntry> = config::resolve_model_list(cfg, prefetched);

    if let Some(scope) = scope {
        let unlisted: Vec<String> = catalog
            .iter()
            .filter(|(_, entry)| !scope.keeps(entry))
            .map(|(key, _)| key.clone())
            .collect();
        if !unlisted.is_empty() {
            tracing::warn!(
                models = ?unlisted,
                "external auth: dropping models the endpoint did not list or that use another host"
            );
            catalog.retain(|key, _| !unlisted.contains(key));
        }
    }

    if let Ok(Some(disabled)) = ModelGlobSet::compile(cfg.models.disabled_models.as_deref()) {
        let before = catalog.len();
        catalog.retain(|key, entry| !disabled.matches(key, &entry.model));
        let removed = before - catalog.len();
        if removed > 0 {
            tracing::info!(count = removed, "disabled_models: removed from catalog");
        }
    }

    effective_allowlist(cfg).apply_selectability(&mut catalog);

    if let Ok(Some(hidden)) = ModelGlobSet::compile(cfg.models.hidden_models.as_deref()) {
        for (key, entry) in catalog.iter_mut() {
            if hidden.matches(key, &entry.model) {
                entry.info.hidden = true;
            }
        }
    }

    if let Some(effort) = cfg.models.default_reasoning_effort
        && let Some(default_id) = cfg.models.default.as_deref()
        && let Some(entry) = catalog.get_mut(default_id)
        && entry.info.supports_reasoning_effort
    {
        stamp_effort(&mut entry.info, effort);
    }

    if let Some(effort) = cfg.reasoning_effort_override {
        for entry in catalog.values_mut() {
            if model_offers_reasoning_effort(&entry.info, effort) {
                stamp_effort(&mut entry.info, effort);
            }
        }
    }

    catalog
}

/// The entry keeps its own model id, and `model_at` picks the id for this effort when a request is prepared.
fn stamp_effort(info: &mut config::ModelInfo, effort: ReasoningEffort) {
    info.reasoning_effort = Some(effort);
}

/// Whether `effort` is a value this model will accept on the wire.
fn model_offers_reasoning_effort(info: &config::ModelInfo, effort: ReasoningEffort) -> bool {
    if !info.supports_reasoning_effort {
        return false;
    }
    if info.reasoning_efforts.is_empty() {
        matches!(
            effort,
            ReasoningEffort::Low
                | ReasoningEffort::Medium
                | ReasoningEffort::High
                | ReasoningEffort::Xhigh
        )
    } else {
        info.reasoning_efforts.iter().any(|opt| opt.value == effort)
    }
}

/// True when an active `allowed_models` allowlist leaves no selectable model.
pub(crate) fn allowlist_matches_nothing(
    cfg: &config::Config,
    catalog: &IndexMap<String, ModelEntry>,
) -> bool {
    !effective_allowlist(cfg).is_unrestricted() && !catalog.values().any(|e| e.info.user_selectable)
}

/// The message that blocks prompts when [`CatalogSource::ModelsEndpoint`] leaves the catalog empty.
pub(crate) fn models_endpoint_empty_message(
    cfg: &config::Config,
    remote_fetch_enabled: bool,
) -> String {
    let url = cfg.endpoints.resolve_models_list_url();
    if remote_fetch_enabled {
        format!(
            "No models are available: {url} returned none or could not be reached. \
             Check the endpoint and your login, then try again."
        )
    } else {
        format!(
            "No models are available: `[features] remote_fetch = false` stops Grok from reading {url}. \
             Add a `[model.<id>]` table that names an endpoint id, or turn remote_fetch on."
        )
    }
}

/// Reject an `allowed_models` allowlist that leaves no selectable model, or excludes an explicitly configured default.
/// Run only against a real catalog.
pub(crate) fn validate_selectable(
    cfg: &config::Config,
    catalog: &IndexMap<String, ModelEntry>,
) -> Result<(), String> {
    let allowlist = effective_allowlist(cfg);
    match allowlist {
        EffectiveAllowlist::Unrestricted => return Ok(()),
        EffectiveAllowlist::Invalid => {
            return Err(
                "The organization model policy is invalid. Contact your administrator.".to_owned(),
            );
        }
        EffectiveAllowlist::Fleet(_) | EffectiveAllowlist::User(_) => {}
    }
    if !catalog.values().any(|e| e.info.user_selectable) {
        return Err(allowlist_excludes_all_message(cfg));
    }
    for (src, id) in [
        ("default", cfg.models.default.as_deref()),
        ("-m flag", cfg.default_model_override.as_deref()),
    ] {
        if let Some(id) = id
            && let Some(entry) = catalog
                .get(id)
                .or_else(|| catalog.values().find(|e| e.has_model_id(id)))
            && !entry.info.user_selectable
        {
            return Err(if allowlist.is_fleet() {
                format!(
                    "\"{id}\" (your {src}) isn't allowed by your organization's policy. \
                     Contact your administrator."
                )
            } else {
                format!(
                    "\"{id}\" (your {src}) isn't allowed by allowed_models. \
                     Broaden the patterns or remove allowed_models, then try again."
                )
            });
        }
    }
    Ok(())
}
