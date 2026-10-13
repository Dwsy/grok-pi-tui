//! Model state — tracks available models and current selection.

use agent_client_protocol as acp;
use indexmap::IndexMap;
use xai_grok_shell::sampling::types::{
    ModelNotice, ReasoningEffort, ReasoningEffortOption, parse_canonical_effort_token,
    parse_context_window_meta, parse_context_windows_meta, parse_model_notice_meta,
    parse_reasoning_effort_meta, parse_reasoning_efforts_meta, supports_reasoning_effort_meta,
};

use crate::slash::commands::effort_levels::legacy_effort_options;

fn canonical_effort_if_offered(
    options: &[ReasoningEffortOption],
    token: &str,
) -> Option<ReasoningEffort> {
    parse_canonical_effort_token(token)
        .filter(|value| options.iter().any(|opt| opt.value == *value))
}

/// Why an effort token could not be applied to a model.
/// Shared by `/effort`, the CLI deferred switch, and headless so they classify the same input identically and differ only in how they report the error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EffortTokenError {
    /// The target model does not advertise `supportsReasoningEffort`.
    Unsupported,
    /// The token is neither a menu id nor a canonical value offered by this
    /// model's menu. `offered` is the model-specific list of option ids the
    /// user can type (never a hardcoded global set — so we do not advertise
    /// `none`/`minimal` when the model does not offer them).
    UnknownToken { token: String, offered: Vec<String> },
    /// No active model to resolve the effort against.
    NoActiveModel,
}

impl EffortTokenError {
    pub(crate) fn message(&self) -> String {
        match self {
            Self::Unsupported => "current model does not support reasoning effort".to_string(),
            Self::UnknownToken { token, offered } => {
                if offered.is_empty() {
                    format!(
                        "unknown effort level '{token}'; this model has no selectable effort levels"
                    )
                } else {
                    format!(
                        "unknown effort level '{token}'; use one of: {}",
                        offered.join(", ")
                    )
                }
            }
            Self::NoActiveModel => "no active model to apply effort to".to_string(),
        }
    }
}

/// One entry in the current session's Pi-style model scope.
///
/// Scope order defines Ctrl/action cycling order. An optional effort mirrors
/// Pi's `provider/model:effort` pattern and is applied when that row is selected.
#[derive(Debug, Clone, PartialEq)]
pub struct ScopedModel {
    pub model_id: acp::ModelId,
    pub effort: Option<ReasoningEffort>,
}

impl ScopedModel {
    pub fn new(model_id: acp::ModelId, effort: Option<ReasoningEffort>) -> Self {
        Self { model_id, effort }
    }
}

/// Per-agent model state.
#[derive(Debug, Clone, Default)]
pub struct ModelState {
    pub available: IndexMap<acp::ModelId, acp::ModelInfo>,
    pub current: Option<acp::ModelId>,
    pub reasoning_effort: Option<ReasoningEffort>,
    /// Session-only Pi scoped-model list. Empty means every available model.
    scoped_models: Vec<ScopedModel>,
    /// External override for the context window size (tokens).
    /// When set, `get_context_window()` returns this instead of
    /// reading from the current model's metadata. Used for subagent
    /// views where SubagentProgress reports the actual window size.
    context_window_override: Option<u64>,
    pub(crate) context_window_selection: Option<u64>,
    /// Set when a `ModelChanged` arrives while a local switch is in flight, so its completion keeps the broadcast selection.
    pub(crate) model_changed_during_switch: bool,
    /// The model the backend routed the current turn to, when it named one other than `current`.
    /// Footer only; `current` stays the user's selection.
    served_model_name: Option<String>,
}

impl ModelState {
    pub fn is_empty(&self) -> bool {
        self.available.is_empty()
    }

    /// Display name for the current model.
    pub fn current_model_name(&self) -> Option<String> {
        let current = self.current.as_ref()?;
        if let Some(model_info) = self.available.get(current) {
            Some(model_info.name.clone())
        } else {
            Some(current.0.to_string())
        }
    }

    /// The footer's model label: the served model for a routed turn, else the current model's name.
    pub fn footer_model_name(&self) -> Option<String> {
        self.served_model_name
            .clone()
            .or_else(|| self.current_model_name())
    }

    /// `footer_model_name` with the reasoning effort, which binds to the name rather than the flag row.
    pub fn footer_label(&self) -> Option<String> {
        let name = self.footer_model_name()?;
        Some(match self.reasoning_effort {
            Some(effort) => format!("{name} ({effort})"),
            None => name,
        })
    }

    pub fn set_served_model_name(&mut self, display_name: Option<String>) {
        self.served_model_name = display_name;
    }

    /// Machine-readable model ID string for the current model (e.g. "grok-4.5").
    pub fn current_model_id_str(&self) -> Option<&str> {
        Some(self.current.as_ref()?.0.as_ref())
    }

    /// Whether the current model accepts image input, read from the model's `meta` (the ACP extension point, same
    /// source as `totalContextTokens`). Honors an explicit `acceptsImages` bool, else an `inputModalities` array
    /// containing `"image"`. Once the ACP server populates the key, non-vision models get suppressed.
    pub fn current_model_accepts_images(&self) -> bool {
        let Some(meta) = self
            .current
            .as_ref()
            .and_then(|id| self.available.get(id))
            .and_then(|info| info.meta.as_ref())
        else {
            return true;
        };
        if let Some(accepts) = meta.get("acceptsImages").and_then(|v| v.as_bool()) {
            return accepts;
        }
        if let Some(modalities) = meta.get("inputModalities").and_then(|v| v.as_array()) {
            return modalities
                .iter()
                .any(|m| m.as_str().is_some_and(|s| s.eq_ignore_ascii_case("image")));
        }
        true
    }

    /// The notice the current model carries in `meta.notice`, if any.
    pub fn current_notice(&self) -> Option<ModelNotice> {
        let info = self.available.get(self.current.as_ref()?)?;
        parse_model_notice_meta(info.meta.as_ref())
    }

    /// Returns the override, else the selection if the current model supports it, else the model default.
    pub fn get_context_window(&self) -> Option<u64> {
        if let Some(window) = self.context_window_override {
            return Some(window);
        }

        let id = self.current.as_ref()?;
        let default = self.model_default_window(id);
        match self.context_window_selection {
            Some(selection)
                if default == Some(selection)
                    || self.context_window_options_for(id).contains(&selection) =>
            {
                Some(selection)
            }
            _ => default,
        }
    }

    /// The window a switch to `id` uses without a pick: the selection if `id` lists it, else its default.
    pub(crate) fn window_after_switch_to(&self, id: &acp::ModelId) -> Option<u64> {
        self.context_window_selection
            .filter(|selection| self.context_window_options_for(id).contains(selection))
            .or_else(|| self.model_default_window(id))
    }

    /// The model's catalog default (`totalContextTokens`).
    pub(crate) fn model_default_window(&self, id: &acp::ModelId) -> Option<u64> {
        self.available
            .get(id)
            .and_then(|info| info.meta.as_ref())
            .and_then(|meta| meta.get("totalContextTokens"))
            .and_then(|value| value.as_u64())
    }

    /// The selectable windows for the current model, empty when the catalog lists none.
    pub fn context_window_options(&self) -> Vec<u64> {
        match self.current.as_ref() {
            Some(id) => self.context_window_options_for(id),
            None => Vec::new(),
        }
    }

    pub(crate) fn context_window_options_for(&self, id: &acp::ModelId) -> Vec<u64> {
        self.available
            .get(id)
            .and_then(|info| parse_context_windows_meta(info.meta.as_ref()))
            .map(|windows| windows.into_iter().map(|w| w.get()).collect())
            .unwrap_or_default()
    }

    /// Override the context window size.
    ///
    /// Used for subagent views where the actual context window is reported
    /// via SubagentProgress and may differ from the inherited model's metadata.
    pub fn override_context_window(&mut self, tokens: u64) {
        self.context_window_override = Some(tokens);
    }

    /// Replace the available-model list. Leaves `current` and
    /// `reasoning_effort` alone — those change only via `/model` / create / load.
    pub fn update_catalog(&mut self, new_available: IndexMap<acp::ModelId, acp::ModelInfo>) {
        self.available = new_available;
        // Pi owns the session scope, but it can only contain models this
        // session actually has.
        self.scoped_models
            .retain(|entry| self.available.contains_key(&entry.model_id));
    }

    /// Replace the session-only scope, preserving first occurrence order and
    /// ignoring catalog ids that are unavailable in this session.
    pub fn set_scoped_models(&mut self, entries: Vec<ScopedModel>) {
        let mut seen = std::collections::HashSet::new();
        self.scoped_models = entries
            .into_iter()
            .filter(|entry| self.available.contains_key(&entry.model_id))
            .filter(|entry| seen.insert(entry.model_id.clone()))
            .collect();
    }

    /// Clear the scope. Pi defines an empty scope as "all available models".
    pub fn clear_scoped_models(&mut self) {
        self.scoped_models.clear();
    }

    pub fn scoped_models(&self) -> &[ScopedModel] {
        &self.scoped_models
    }

    pub fn has_scoped_models(&self) -> bool {
        !self.scoped_models.is_empty()
    }

    pub fn is_model_scoped(&self, model_id: &acp::ModelId) -> bool {
        self.scoped_models
            .iter()
            .any(|entry| &entry.model_id == model_id)
    }

    /// Toggle one catalog model in the session scope. An empty scope means all
    /// models until the first row is selected, matching Pi's scoped selector.
    pub fn toggle_scoped_model(&mut self, model_id: acp::ModelId) {
        if let Some(index) = self
            .scoped_models
            .iter()
            .position(|entry| entry.model_id == model_id)
        {
            self.scoped_models.remove(index);
        } else if self.available.contains_key(&model_id) {
            self.scoped_models.push(ScopedModel::new(model_id, None));
        }
    }

    /// Next model and optional per-scope effort. Empty scope cycles the full
    /// catalog; a current model outside a non-empty scope jumps to its first row.
    pub fn next_model_selection(&self) -> Option<(acp::ModelId, Option<ReasoningEffort>)> {
        if self.scoped_models.is_empty() {
            return self.next_model().map(|model_id| (model_id, None));
        }
        let next_index = self
            .current
            .as_ref()
            .and_then(|current| {
                self.scoped_models
                    .iter()
                    .position(|entry| &entry.model_id == current)
            })
            .map_or(0, |index| (index + 1) % self.scoped_models.len());
        self.scoped_models
            .get(next_index)
            .map(|entry| (entry.model_id.clone(), entry.effort.clone()))
    }

    /// Set the current model and resolve reasoning effort from catalog meta.
    pub fn set_current(
        &mut self,
        model_id: acp::ModelId,
        effort_override: Option<ReasoningEffort>,
    ) {
        self.current = Some(model_id.clone());
        self.served_model_name = None;
        self.reasoning_effort = effort_override.or_else(|| {
            self.available
                .get(&model_id)
                .and_then(|info| parse_reasoning_effort_meta(info.meta.as_ref()))
        });
    }

    /// The reasoning-effort menu for the current model. Gate-first: an unset or
    /// unsupported model yields no menu; a supported model uses the server list
    /// when present, else the built-in fallback.
    pub fn reasoning_effort_options(&self) -> Vec<ReasoningEffortOption> {
        match self.current.as_ref() {
            Some(id) => self.reasoning_effort_options_for(id),
            None => Vec::new(),
        }
    }

    /// Cycle through the current model's advertised thinking levels.
    pub fn next_reasoning_effort(&self) -> Option<ReasoningEffort> {
        let options = self.reasoning_effort_options();
        let next_index = self
            .reasoning_effort
            .and_then(|current| options.iter().position(|option| option.value == current))
            .map_or(0, |index| (index + 1) % options.len());
        options.get(next_index).map(|option| option.value)
    }

    /// Menu for a specific catalog model id (used by `/model`'s effort phase).
    /// `parse_reasoning_efforts_meta` returns `None` for absent, non-array, or
    /// present-but-unusable lists, so all of those fall back to the built-in menu
    /// exactly as the shell's session picker does.
    pub(crate) fn reasoning_effort_options_for(
        &self,
        id: &acp::ModelId,
    ) -> Vec<ReasoningEffortOption> {
        let Some(info) = self.available.get(id) else {
            return Vec::new();
        };
        if !supports_reasoning_effort_meta(info.meta.as_ref()) {
            return Vec::new();
        }
        parse_reasoning_efforts_meta(info.meta.as_ref()).unwrap_or_else(legacy_effort_options)
    }

    /// The effort row the pickers highlight when the menu for `id` opens. `None` leaves the cursor on the first row.
    /// Precedence: the session's live effort when `id` is the current model, else the model's `meta.reasoningEffort`,
    /// else the option flagged `default`. A candidate counts only if the menu offers its value.
    pub(crate) fn preselected_effort_option_for(
        &self,
        id: &acp::ModelId,
    ) -> Option<ReasoningEffortOption> {
        let options = self.reasoning_effort_options_for(id);
        let offered = |effort: ReasoningEffort| options.iter().find(|opt| opt.value == effort);
        let live = self
            .reasoning_effort
            .filter(|_| self.current.as_ref() == Some(id));
        let meta_default = self
            .available
            .get(id)
            .and_then(|info| parse_reasoning_effort_meta(info.meta.as_ref()));
        live.and_then(offered)
            .or_else(|| meta_default.and_then(offered))
            .or_else(|| options.iter().find(|opt| opt.default))
            .cloned()
    }

    /// Menu id or label for the current model. No current model: a canonical level only.
    pub fn resolve_effort_token(&self, token: &str) -> Option<ReasoningEffort> {
        match self.current.as_ref() {
            Some(id) => self.resolve_effort_token_for(id, token),
            None => parse_canonical_effort_token(token),
        }
    }

    /// Menu id, menu label, or a canonical level that menu offers.
    pub(crate) fn resolve_effort_token_for(
        &self,
        id: &acp::ModelId,
        token: &str,
    ) -> Option<ReasoningEffort> {
        let options = self.reasoning_effort_options_for(id);
        options
            .iter()
            .find(|opt| opt.id.eq_ignore_ascii_case(token) || opt.label.eq_ignore_ascii_case(token))
            .map(|opt| opt.value)
            .or_else(|| canonical_effort_if_offered(&options, token))
    }

    /// Menu id or a canonical level that menu offers.
    fn resolve_cli_effort_token_for(
        &self,
        id: &acp::ModelId,
        token: &str,
    ) -> Option<ReasoningEffort> {
        let options = self.reasoning_effort_options_for(id);
        options
            .iter()
            .find(|opt| opt.id.eq_ignore_ascii_case(token))
            .map(|opt| opt.value)
            .or_else(|| canonical_effort_if_offered(&options, token))
    }

    fn reject_unknown_effort(
        &self,
        id: &acp::ModelId,
        token: &str,
        resolved: Option<ReasoningEffort>,
    ) -> Result<ReasoningEffort, EffortTokenError> {
        let supports = self
            .available
            .get(id)
            .map(|info| supports_reasoning_effort_meta(info.meta.as_ref()))
            .unwrap_or(false);
        if !supports {
            return Err(EffortTokenError::Unsupported);
        }
        resolved.ok_or_else(|| EffortTokenError::UnknownToken {
            token: token.to_string(),
            offered: self
                .reasoning_effort_options_for(id)
                .into_iter()
                .map(|opt| opt.id)
                .collect(),
        })
    }

    pub(crate) fn resolve_effort_for_model(
        &self,
        id: &acp::ModelId,
        token: &str,
    ) -> Result<ReasoningEffort, EffortTokenError> {
        self.reject_unknown_effort(id, token, self.resolve_effort_token_for(id, token))
    }

    pub(crate) fn resolve_cli_effort_for_model(
        &self,
        id: &acp::ModelId,
        token: &str,
    ) -> Result<ReasoningEffort, EffortTokenError> {
        self.reject_unknown_effort(id, token, self.resolve_cli_effort_token_for(id, token))
    }

    /// Resolve a user-supplied name to a `ModelId` via case-insensitive
    /// ASCII match against the catalog.
    ///
    /// Accepts display name, full `provider::id`, `provider/id`, bare model id,
    /// or the Pi-style `id [provider]` row text (without a trailing `(current)`).
    pub fn resolve_by_name_or_id(&self, query: &str) -> Option<acp::ModelId> {
        let query = query
            .trim()
            .trim_end_matches("(current)")
            .trim()
            .trim_end_matches('✓')
            .trim();
        if query.is_empty() {
            return None;
        }

        // Exact full-key hit first (unique by construction).
        if let Some((id, _)) = self
            .available
            .iter()
            .find(|(id, _)| id.0.as_ref().eq_ignore_ascii_case(query))
        {
            return Some(id.clone());
        }

        // Display name: only when unique across the catalog.
        let mut name_match: Option<acp::ModelId> = None;
        for (id, info) in &self.available {
            if info.name.eq_ignore_ascii_case(query) {
                if name_match.is_some() {
                    name_match = None;
                    break;
                }
                name_match = Some(id.clone());
            }
        }
        if let Some(id) = name_match {
            return Some(id);
        }

        // Pi-style `model-id [provider]` from the picker row.
        if let Some((model_part, provider_part)) = query.rsplit_once('[')
            && let Some(provider) = provider_part.strip_suffix(']')
        {
            let model_part = model_part.trim();
            let provider = provider.trim();
            if !model_part.is_empty() && !provider.is_empty() {
                let key = format!("{provider}::{model_part}");
                if let Some((id, _)) = self
                    .available
                    .iter()
                    .find(|(id, _)| id.0.as_ref().eq_ignore_ascii_case(&key))
                {
                    return Some(id.clone());
                }
            }
        }

        // `provider/id` (slash form used in search / CLI). The model id may
        // itself contain slashes (e.g. Pi `openrouter/stealth/ox-alpha high`),
        // so only the first `/` separates provider from id.
        if let Some((provider, model_id)) = query.split_once('/')
            && !provider.is_empty()
            && !model_id.is_empty()
        {
            let key = format!("{provider}::{model_id}");
            if let Some((id, _)) = self
                .available
                .iter()
                .find(|(id, _)| id.0.as_ref().eq_ignore_ascii_case(&key))
            {
                return Some(id.clone());
            }
        }

        // Bare model id after `::` — only when unique.
        let mut bare_match: Option<acp::ModelId> = None;
        for (id, _) in &self.available {
            let bare =
                id.0.as_ref()
                    .split_once("::")
                    .map(|(_, m)| m)
                    .unwrap_or(id.0.as_ref());
            if bare.eq_ignore_ascii_case(query) {
                if bare_match.is_some() {
                    return None;
                }
                bare_match = Some(id.clone());
            }
        }
        bare_match
    }

    /// Look up the display name for a `ModelId` in the catalog.
    pub fn display_name_for(&self, id: &acp::ModelId) -> String {
        self.available
            .get(id)
            .map(|info| info.name.clone())
            .unwrap_or_else(|| id.0.to_string())
    }

    /// Cycle to the next model.
    pub fn next_model(&self) -> Option<acp::ModelId> {
        if self.available.is_empty() {
            None
        } else if let Some(ref current) = self.current {
            let idx = self.available.get_index_of(current)?;
            let idx = (idx + 1) % self.available.len();
            Some(self.available.get_index(idx)?.0.clone())
        } else {
            Some(self.available.first()?.0.clone())
        }
    }
}

impl From<Option<acp::SessionModelState>> for ModelState {
    fn from(state: Option<acp::SessionModelState>) -> Self {
        state
            .map(|state| {
                let mut models = IndexMap::new();
                for model in state.available_models {
                    models.insert(model.model_id.clone(), model);
                }
                let current_model = models
                    .contains_key(&state.current_model_id)
                    .then_some(state.current_model_id);
                let reasoning_effort = current_model
                    .as_ref()
                    .and_then(|id| models.get(id))
                    .and_then(|info| parse_reasoning_effort_meta(info.meta.as_ref()));
                let context_window_selection = current_model
                    .as_ref()
                    .and_then(|id| models.get(id))
                    .and_then(|info| parse_context_window_meta(info.meta.as_ref()))
                    .map(std::num::NonZeroU64::get);
                Self {
                    available: models,
                    current: current_model,
                    reasoning_effort,
                    scoped_models: Vec::new(),
                    context_window_override: None,
                    context_window_selection,
                    model_changed_during_switch: false,
                    served_model_name: None,
                }
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn sample_models() -> ModelState {
        let mut state = ModelState::default();
        let id_a = acp::ModelId::new(Arc::from("model-a"));
        let id_b = acp::ModelId::new(Arc::from("model-b"));
        state.available.insert(
            id_a.clone(),
            acp::ModelInfo::new(id_a.clone(), "Model A".to_string()),
        );
        state.available.insert(
            id_b.clone(),
            acp::ModelInfo::new(id_b.clone(), "Model B".to_string()),
        );
        state.current = Some(id_a);
        state
    }

    #[test]
    fn scoped_cycle_uses_scope_order_and_starts_first_when_current_is_outside() {
        let mut state = sample_models();
        let id_a = acp::ModelId::new(Arc::from("model-a"));
        let id_b = acp::ModelId::new(Arc::from("model-b"));
        state.current = Some(acp::ModelId::new(Arc::from("outside")));
        state.set_scoped_models(vec![
            ScopedModel::new(id_b.clone(), None),
            ScopedModel::new(id_a.clone(), None),
        ]);
        assert_eq!(state.next_model_selection(), Some((id_b.clone(), None)));
        state.current = Some(id_b);
        assert_eq!(state.next_model_selection(), Some((id_a, None)));
    }

    #[test]
    fn scoped_toggle_adds_and_removes_catalog_model() {
        let mut state = sample_models();
        let id_a = acp::ModelId::new(Arc::from("model-a"));
        state.toggle_scoped_model(id_a.clone());
        assert!(state.is_model_scoped(&id_a));
        state.toggle_scoped_model(id_a.clone());
        assert!(!state.is_model_scoped(&id_a));
        state.toggle_scoped_model(acp::ModelId::new(Arc::from("missing")));
        assert!(!state.has_scoped_models());
    }

    #[test]
    fn scoped_cycle_carries_per_model_effort() {
        let mut state = sample_models();
        let id_b = acp::ModelId::new(Arc::from("model-b"));
        let effort: ReasoningEffort = "high".parse().unwrap();
        state.set_scoped_models(vec![ScopedModel::new(id_b.clone(), Some(effort.clone()))]);
        assert_eq!(state.next_model_selection(), Some((id_b, Some(effort))));
    }

    #[test]
    fn scoped_models_dedupe_unknown_ids_and_clear_to_all() {
        let mut state = sample_models();
        let id_a = acp::ModelId::new(Arc::from("model-a"));
        state.set_scoped_models(vec![
            ScopedModel::new(id_a.clone(), None),
            ScopedModel::new(acp::ModelId::new(Arc::from("missing")), None),
            ScopedModel::new(id_a.clone(), Some("low".parse().unwrap())),
        ]);
        assert_eq!(state.scoped_models().len(), 1);
        assert_eq!(state.scoped_models()[0].model_id, id_a.clone());
        state.clear_scoped_models();
        assert!(!state.has_scoped_models());
        state.current = None;
        assert_eq!(state.next_model_selection(), Some((id_a, None)));
    }

    #[test]
    fn catalog_update_prunes_missing_scoped_models() {
        let mut state = sample_models();
        let id_a = acp::ModelId::new(Arc::from("model-a"));
        let id_b = acp::ModelId::new(Arc::from("model-b"));
        state.set_scoped_models(vec![
            ScopedModel::new(id_a, None),
            ScopedModel::new(id_b.clone(), None),
        ]);
        let mut refreshed = IndexMap::new();
        refreshed.insert(
            id_b.clone(),
            acp::ModelInfo::new(id_b.clone(), "Model B".to_string()),
        );
        state.update_catalog(refreshed);
        assert_eq!(state.scoped_models().len(), 1);
        assert_eq!(state.scoped_models()[0].model_id, id_b);
    }

    #[test]
    fn test_current_model_name() {
        let state = sample_models();
        assert_eq!(state.current_model_name(), Some("Model A".to_string()));
    }

    #[test]
    fn test_next_model_cycles() {
        let state = sample_models();
        let next = state.next_model().unwrap();
        assert_eq!(next.0.as_ref(), "model-b");
    }

    #[test]
    fn test_next_model_wraps() {
        let mut state = sample_models();
        state.current = Some(acp::ModelId::new(Arc::from("model-b")));
        let next = state.next_model().unwrap();
        assert_eq!(next.0.as_ref(), "model-a");
    }

    #[test]
    fn test_empty_state() {
        let state = ModelState::default();
        assert!(state.is_empty());
        assert!(state.current_model_name().is_none());
        assert!(state.next_model().is_none());
    }

    #[test]
    fn footer_shows_the_served_model_until_the_selection_changes() {
        let mut state = sample_models();
        assert_eq!(Some("Model A".to_string()), state.footer_model_name());

        state.set_served_model_name(Some("Model Z".to_string()));
        assert_eq!(
            Some("Model Z".to_string()),
            state.footer_model_name(),
            "the footer names the served model"
        );
        assert_eq!(
            Some("Model A".to_string()),
            state.current_model_name(),
            "the selection is unchanged"
        );

        state.set_current(acp::ModelId::new(Arc::from("model-b")), None);
        assert_eq!(
            Some("Model B".to_string()),
            state.footer_model_name(),
            "a model switch drops the served label"
        );
    }

    #[test]
    fn footer_label_attaches_the_effort_only_to_a_name() {
        let mut state = sample_models();
        state.reasoning_effort = Some(ReasoningEffort::High);
        assert_eq!(Some("Model A (high)".to_owned()), state.footer_label());

        state.current = None;
        assert_eq!(None, state.footer_label(), "no name, no effort");
    }

    #[test]
    fn accepts_images_defaults_true_when_meta_absent() {
        // No current model, empty meta, and a meta without the key all default
        // permissive — correct today and a no-op until the server populates it.
        assert!(ModelState::default().current_model_accepts_images());
        assert!(state_with_meta(None).current_model_accepts_images());
        assert!(
            state_with_meta(Some(serde_json::json!({ "totalContextTokens": 256000 })))
                .current_model_accepts_images()
        );
    }

    #[test]
    fn reasoning_effort_options_renders_server_list() {
        let state = state_with_meta(Some(serde_json::json!({
            "supportsReasoningEffort": true,
            "reasoningEfforts": [
                { "id": "balanced", "value": "medium", "label": "Balanced" },
                { "id": "deep", "value": "xhigh", "label": "Deep", "description": "Max" },
            ],
        })));
        let opts = state.reasoning_effort_options();
        let [first, second] = opts.as_slice() else {
            panic!("expected 2 options, got {}", opts.len());
        };
        assert_eq!(first.label, "Balanced");
        assert_eq!(first.value, ReasoningEffort::Medium);
        assert_eq!(second.id, "deep");
        assert_eq!(second.description.as_deref(), Some("Max"));
    }

    #[test]
    fn next_reasoning_effort_cycles_current_model_options() {
        let mut state = state_with_meta(Some(serde_json::json!({
            "supportsReasoningEffort": true,
            "reasoningEfforts": [
                { "id": "off", "value": "none", "label": "Off" },
                { "id": "low", "value": "low", "label": "Low" },
                { "id": "high", "value": "high", "label": "High" },
            ],
        })));

        assert_eq!(state.next_reasoning_effort(), Some(ReasoningEffort::None));
        state.reasoning_effort = Some(ReasoningEffort::Low);
        assert_eq!(state.next_reasoning_effort(), Some(ReasoningEffort::High));
        state.reasoning_effort = Some(ReasoningEffort::High);
        assert_eq!(state.next_reasoning_effort(), Some(ReasoningEffort::None));
    }

    #[test]
    fn reasoning_effort_options_gate_first_empty_when_unsupported() {
        // No current model → empty.
        assert!(ModelState::default().reasoning_effort_options().is_empty());
        // Current model that does not support effort → empty (even with a list).
        let state = state_with_meta(Some(serde_json::json!({
            "reasoningEfforts": [{ "value": "high" }],
        })));
        assert!(state.reasoning_effort_options().is_empty());
    }

    #[test]
    fn reasoning_effort_options_falls_back_to_builtin_menu() {
        // Supported but no server list → today's four-row built-in menu.
        let state = state_with_meta(Some(serde_json::json!({
            "supportsReasoningEffort": true,
        })));
        let ids: Vec<_> = state
            .reasoning_effort_options()
            .into_iter()
            .map(|o| o.id)
            .collect();
        assert_eq!(ids, ["xhigh", "high", "medium", "low"]);
    }

    #[test]
    fn reasoning_effort_options_falls_back_when_list_present_but_unusable() {
        // Matches the shell picker: an explicit empty list, and a list where every
        // entry skip-invalidated under version skew, both fall back to the built-in
        // menu rather than silently vanishing.
        for meta in [
            serde_json::json!({ "supportsReasoningEffort": true, "reasoningEfforts": [] }),
            serde_json::json!({
                "supportsReasoningEffort": true,
                "reasoningEfforts": [{ "value": "quantum" }],
            }),
        ] {
            let ids: Vec<_> = state_with_meta(Some(meta.clone()))
                .reasoning_effort_options()
                .into_iter()
                .map(|o| o.id)
                .collect();
            assert_eq!(ids, ["xhigh", "high", "medium", "low"], "for meta {meta}");
        }
    }

    #[test]
    fn resolve_effort_token_maps_remap_id_to_canonical_value() {
        let state = state_with_meta(Some(serde_json::json!({
            "supportsReasoningEffort": true,
            "reasoningEfforts": [
                { "id": "deep", "value": "xhigh", "label": "Deep" },
                { "id": "high", "value": "high", "label": "High" },
            ],
        })));
        // Design-2 remap: the typed id resolves to its canonical wire value.
        assert_eq!(
            state.resolve_effort_token("deep"),
            Some(ReasoningEffort::Xhigh)
        );
        assert_eq!(
            state.resolve_effort_token("DEEP"),
            Some(ReasoningEffort::Xhigh)
        );
        // Canonical level offered by the menu is accepted by value.
        assert_eq!(
            state.resolve_effort_token("high"),
            Some(ReasoningEffort::High)
        );
        // Levels the model does not offer (none/minimal on 4.5-style menus)
        // are rejected — better than a server-side 400.
        assert!(state.resolve_effort_token("minimal").is_none());
        assert!(state.resolve_effort_token("none").is_none());
        assert!(state.resolve_effort_token("bogus").is_none());
    }

    #[test]
    fn resolve_effort_token_accepts_menu_label() {
        let state = state_with_meta(Some(serde_json::json!({
            "supportsReasoningEffort": true,
            "reasoningEfforts": [{ "value": "xhigh", "label": "Extra High" }],
        })));
        assert_eq!(
            state.resolve_effort_token("Extra High"),
            Some(ReasoningEffort::Xhigh)
        );
        let id = state.current.as_ref().unwrap();
        assert!(
            state
                .resolve_cli_effort_for_model(id, "Extra High")
                .is_err()
        );
        assert_eq!(
            state.resolve_cli_effort_for_model(id, "xhigh").unwrap(),
            ReasoningEffort::Xhigh
        );
    }

    #[test]
    fn resolve_effort_token_accepts_none_only_when_menu_offers_it() {
        let with_none = state_with_meta(Some(serde_json::json!({
            "supportsReasoningEffort": true,
            "reasoningEfforts": [
                { "value": "none", "label": "None", "default": true },
                { "value": "high", "label": "High" },
            ],
        })));
        assert_eq!(
            with_none.resolve_effort_token("none"),
            Some(ReasoningEffort::None)
        );

        let without_none = state_with_meta(Some(serde_json::json!({
            "supportsReasoningEffort": true,
            "reasoningEfforts": [
                { "value": "high", "label": "High", "default": true },
                { "value": "low", "label": "Low" },
            ],
        })));
        assert!(without_none.resolve_effort_token("none").is_none());
        let err = without_none
            .resolve_effort_for_model(without_none.current.as_ref().unwrap(), "none")
            .unwrap_err();
        assert_eq!(
            err,
            EffortTokenError::UnknownToken {
                token: "none".to_string(),
                offered: vec!["high".to_string(), "low".to_string()],
            }
        );
        // Error copy must list only this model's options — never hardcode
        // none/minimal/… as offered values (the rejected token may still appear
        // quoted in "unknown effort level '…'").
        let msg = err.message();
        assert!(msg.contains("use one of: high, low"), "msg={msg}");
        let offered_half = msg
            .split_once("; ")
            .map(|(_, rest)| rest)
            .expect("message should have '; ' separator");
        assert!(
            !offered_half.contains("none"),
            "must not advertise blocked level: {msg}"
        );
        assert!(
            !offered_half.contains("minimal"),
            "must not advertise blocked level: {msg}"
        );
        assert!(
            !msg.contains("unset"),
            "unset is log-only, not a user token: {msg}"
        );
    }

    #[test]
    fn preselected_effort_option_follows_live_then_meta_then_default_flag() {
        let menu = serde_json::json!([
            { "value": "xhigh" },
            { "value": "high", "default": true },
            { "value": "medium" },
            { "value": "low" },
        ]);
        let preselected_id = |state: &ModelState| {
            state
                .preselected_effort_option_for(&acp::ModelId::new(Arc::from("m")))
                .map(|option| option.id)
        };

        let mut state = state_with_meta(Some(serde_json::json!({
            "supportsReasoningEffort": true,
            "reasoningEffort": "medium",
            "reasoningEfforts": menu.clone(),
        })));
        state.reasoning_effort = Some(ReasoningEffort::Low);
        assert_eq!(Some("low".to_owned()), preselected_id(&state));

        // Not the current model: the live effort no longer applies, so meta wins
        state.current = None;
        assert_eq!(Some("medium".to_owned()), preselected_id(&state));

        let state = state_with_meta(Some(serde_json::json!({
            "supportsReasoningEffort": true,
            "reasoningEfforts": menu,
        })));
        assert_eq!(Some("high".to_owned()), preselected_id(&state));

        // A meta level the menu does not offer is skipped, and without a default flag nothing is preselected
        let state = state_with_meta(Some(serde_json::json!({
            "supportsReasoningEffort": true,
            "reasoningEffort": "none",
            "reasoningEfforts": [{ "value": "xhigh" }, { "value": "high" }],
        })));
        assert_eq!(None, preselected_id(&state));

        // Built-in fallback menu: its rows never carry `default`, so meta is the only source
        let state = state_with_meta(Some(serde_json::json!({
            "supportsReasoningEffort": true,
            "reasoningEffort": "high",
        })));
        assert_eq!(Some("high".to_owned()), preselected_id(&state));
    }

    #[test]
    fn resolve_effort_token_legacy_menu_rejects_none() {
        // supportsReasoningEffort without a server list → built-in low..xhigh.
        let state = state_with_meta(Some(serde_json::json!({
            "supportsReasoningEffort": true,
        })));
        assert!(state.resolve_effort_token("none").is_none());
        assert!(state.resolve_effort_token("minimal").is_none());
        assert_eq!(
            state.resolve_effort_token("low"),
            Some(ReasoningEffort::Low)
        );
    }

    #[test]
    fn selected_context_window_applies_only_while_supported() {
        let mut state = state_with_meta(Some(serde_json::json!({
            "totalContextTokens": 256_000,
            "contextWindows": [256_000, 500_000],
        })));

        assert_eq!(state.context_window_options(), vec![256_000, 500_000]);
        assert_eq!(state.get_context_window(), Some(256_000));

        state.context_window_selection = Some(500_000);
        assert_eq!(state.get_context_window(), Some(500_000));

        state.context_window_selection = Some(1_000_000);
        assert_eq!(state.get_context_window(), Some(256_000));

        state.override_context_window(64_000);
        assert_eq!(state.get_context_window(), Some(64_000));
    }

    #[test]
    fn accepts_images_honors_explicit_meta() {
        assert!(
            !state_with_meta(Some(serde_json::json!({ "acceptsImages": false })))
                .current_model_accepts_images()
        );
        assert!(
            state_with_meta(Some(serde_json::json!({ "acceptsImages": true })))
                .current_model_accepts_images()
        );
        // inputModalities array form.
        assert!(
            state_with_meta(Some(
                serde_json::json!({ "inputModalities": ["text", "image"] })
            ))
            .current_model_accepts_images()
        );
        assert!(
            !state_with_meta(Some(serde_json::json!({ "inputModalities": ["text"] })))
                .current_model_accepts_images()
        );
    }

    #[test]
    fn resolve_by_name_or_id_accepts_provider_forms() {
        let mut state = ModelState::default();
        let id = acp::ModelId::new(Arc::from("anthropic::claude-haiku-4-5"));
        state.available.insert(
            id.clone(),
            acp::ModelInfo::new(id.clone(), "Claude Haiku 4.5".to_string()),
        );

        assert_eq!(
            state.resolve_by_name_or_id("Claude Haiku 4.5").as_ref(),
            Some(&id)
        );
        assert_eq!(
            state
                .resolve_by_name_or_id("anthropic::claude-haiku-4-5")
                .as_ref(),
            Some(&id)
        );
        assert_eq!(
            state
                .resolve_by_name_or_id("anthropic/claude-haiku-4-5")
                .as_ref(),
            Some(&id)
        );
        assert_eq!(
            state
                .resolve_by_name_or_id("claude-haiku-4-5 [anthropic]")
                .as_ref(),
            Some(&id)
        );
        assert_eq!(
            state
                .resolve_by_name_or_id("claude-haiku-4-5 [anthropic] (current)")
                .as_ref(),
            Some(&id)
        );
        assert_eq!(
            state.resolve_by_name_or_id("claude-haiku-4-5").as_ref(),
            Some(&id)
        );
    }

    #[test]
    fn resolve_by_name_or_id_accepts_multi_slash_provider_id() {
        // Pi model ids may themselves contain slashes
        // (e.g. `openrouter/stealth/ox-alpha high` → catalog key
        // `openrouter::stealth/ox-alpha high`). Only the first `/` splits.
        let mut state = ModelState::default();
        let id = acp::ModelId::new(Arc::from("openrouter::stealth/ox-alpha high"));
        state.available.insert(
            id.clone(),
            acp::ModelInfo::new(id.clone(), "OX Alpha High".to_string()),
        );

        assert_eq!(
            state
                .resolve_by_name_or_id("openrouter/stealth/ox-alpha high")
                .as_ref(),
            Some(&id)
        );
        assert_eq!(
            state
                .resolve_by_name_or_id("openrouter::stealth/ox-alpha high")
                .as_ref(),
            Some(&id)
        );
    }

    #[test]
    fn resolve_by_name_or_id_rejects_ambiguous_bare_id() {
        let mut state = ModelState::default();
        let a = acp::ModelId::new(Arc::from("anthropic::claude-haiku-4-5"));
        let b = acp::ModelId::new(Arc::from("openrouter::claude-haiku-4-5"));
        state.available.insert(
            a.clone(),
            acp::ModelInfo::new(a.clone(), "Claude Haiku 4.5".to_string()),
        );
        state.available.insert(
            b.clone(),
            acp::ModelInfo::new(b.clone(), "Claude Haiku 4.5".to_string()),
        );

        assert!(state.resolve_by_name_or_id("claude-haiku-4-5").is_none());
        assert!(
            state.resolve_by_name_or_id("Claude Haiku 4.5").is_none(),
            "duplicate display names must not resolve ambiguously"
        );
        assert_eq!(
            state
                .resolve_by_name_or_id("openrouter/claude-haiku-4-5")
                .map(|id| id.0.to_string())
                .as_deref(),
            Some("openrouter::claude-haiku-4-5")
        );
    }

    fn state_with_meta(meta: Option<serde_json::Value>) -> ModelState {
        let id = acp::ModelId::new(Arc::from("m"));
        let mut state = ModelState::default();
        state.available.insert(
            id.clone(),
            acp::ModelInfo::new(id.clone(), "M".to_string())
                .meta(meta.and_then(|v| v.as_object().cloned())),
        );
        state.current = Some(id);
        state
    }
}
