//! `/model` (alias `/m`) — switch model + (optionally) reasoning effort.
//! Chained autocomplete: pick a reasoning-supported model → trailing space
//! re-opens the dropdown into a `low|medium|high|xhigh` sub-menu.

use agent_client_protocol as acp;
use xai_grok_shell::sampling::types::supports_reasoning_effort_meta;

use crate::acp::model_state::ModelState;
use crate::app::actions::Action;
use crate::slash::command::{AppCtx, ArgItem, CommandExecCtx, CommandResult, SlashCommand};
use crate::slash::commands::effort_levels::build_effort_arg_items;

/// Switch the active model (and optionally its reasoning effort).
pub struct ModelCommand;

impl SlashCommand for ModelCommand {
    fn name(&self) -> &str {
        "model"
    }

    fn aliases(&self) -> &[&str] {
        &["m"]
    }

    fn description(&self) -> &str {
        "Switch the active model"
    }

    fn session_scoped(&self) -> bool {
        true
    }

    fn offered_when_session_less(&self) -> bool {
        // The dashboard offers `/model` to pick the model for the next
        // spawned agent (intercepted in `dispatch_dashboard_dispatch_slash`).
        true
    }

    fn usage(&self) -> &str {
        "/model <provider/model> [effort]"
    }

    fn takes_args(&self) -> bool {
        true
    }

    fn args_required(&self) -> bool {
        false
    }

    fn arg_placeholder(&self) -> Option<&str> {
        // Pi interactive: argumentHint "<provider/model>"; keep optional effort chain.
        Some("<provider/model> [effort]")
    }

    fn suggest_args(&self, ctx: &AppCtx, args_query: &str) -> Option<Vec<ArgItem>> {
        if ctx.models.is_empty() {
            return None;
        }

        // Effort phase if input is "<reasoning-model> ", else model phase.
        if let Some(model_id) = detect_effort_phase(ctx.models, args_query) {
            return Some(build_effort_items(ctx.models, &model_id));
        }
        Some(build_model_items(ctx.models))
    }

    fn run(&self, ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        let trimmed = args.trim();
        if trimmed.is_empty() {
            return CommandResult::Action(Action::OpenModelPicker);
        }
        if trimmed.eq_ignore_ascii_case("next") {
            return CommandResult::Action(Action::NextModel);
        }

        // Prefer an exact full-string catalog match first. Model display names
        // often contain spaces ("Grok 4.5"); if we split on the last token
        // first, a shorter catalog entry ("Grok") would steal the prefix and
        // treat "4.5" as an effort level.
        if let Some(id) = ctx.models.resolve_by_name_or_id(trimmed) {
            return CommandResult::Action(Action::SetDefaultModel(id));
        }

        // Trailing effort token + reasoning model → session-scoped switch
        // (not persisted as default). Resolve via the shared gate so a rejected
        // level (e.g. `none` on grok-4.5) surfaces the effort error with the
        // model's offered ids — not "Unknown model: … none".
        if let Some((prefix, token)) = split_trailing_token(trimmed)
            && let Some(id) = resolve_model(ctx.models, prefix)
            && ctx
                .models
                .available
                .get(&id)
                .map(supports_reasoning_effort)
                .unwrap_or(false)
        {
            return match ctx.models.resolve_effort_for_model(&id, token) {
                Ok(effort) => CommandResult::Action(Action::SwitchModel {
                    model_id: id,
                    effort: Some(effort),
                }),
                Err(err) => CommandResult::Error(err.message()),
            };
        }

        CommandResult::Error(format!("Unknown model: {trimmed}"))
    }
}

/// Look up a model by case-insensitive display name OR model id match.
fn resolve_model(models: &ModelState, name: &str) -> Option<acp::ModelId> {
    models.resolve_by_name_or_id(name)
}

fn supports_reasoning_effort(info: &acp::ModelInfo) -> bool {
    supports_reasoning_effort_meta(info.meta.as_ref())
}

/// Split `args` into `(prefix, last_token)` on the final whitespace run.
/// Returns `None` when there is no interior whitespace to split on. The token is
/// resolved to an effort against the picked model's options by the caller.
fn split_trailing_token(args: &str) -> Option<(&str, &str)> {
    let (prefix, last) = args.rsplit_once(char::is_whitespace)?;
    let prefix = prefix.trim_end();
    if prefix.is_empty() || last.is_empty() {
        return None;
    }
    Some((prefix, last))
}

/// Whether `/model` is showing its model picker rather than the chained
/// reasoning-effort picker.
pub(crate) fn uses_pi_model_picker_search(models: &ModelState, args_query: &str) -> bool {
    detect_effort_phase(models, args_query).is_none()
}

/// Returns the matched model id when `args_query` is `"<reasoning-model> ..."`.
/// Longest-token-first so `provider/id` and multi-word names disambiguate.
fn detect_effort_phase(models: &ModelState, args_query: &str) -> Option<acp::ModelId> {
    let mut candidates: Vec<(acp::ModelId, String)> = models
        .available
        .iter()
        .filter(|(_, info)| supports_reasoning_effort(info))
        .map(|(id, info)| (id.clone(), model_completion_token(id, info)))
        .collect();
    candidates.sort_by_key(|(_, token)| std::cmp::Reverse(token.len()));

    for (id, token) in candidates {
        if args_query.len() > token.len()
            && args_query.is_char_boundary(token.len())
            && args_query[..token.len()].eq_ignore_ascii_case(&token)
            && args_query[token.len()..].starts_with(char::is_whitespace)
        {
            return Some(id);
        }
    }
    None
}

/// One row per logical model, with a human-readable model name and provider:
/// - label (`display`) = model name + provider
/// - value (`insert_text`) = `provider/id`
/// - search (`match_text`) = Pi `getModelSearchText`
///
/// Reasoning models still get a trailing space on `insert_text` so the prompt
/// widget chains into the effort sub-menu (Grok-only extension over Pi).
fn build_model_items(models: &ModelState) -> Vec<ArgItem> {
    let current_id = models.current.as_ref();
    let mut items: Vec<ArgItem> = Vec::with_capacity(models.available.len());
    for (id, info) in &models.available {
        let is_current = current_id == Some(id);
        let supports = supports_reasoning_effort(info);
        let (provider, model_id) = split_provider_model_id(id, info);
        let token = model_completion_token(id, info);

        let provider = provider
            .filter(|provider| !provider.is_empty())
            .unwrap_or("");
        let label = if info.name.trim().is_empty() {
            model_id
        } else {
            info.name.as_str()
        };
        let mut display = if provider.is_empty() {
            label.to_string()
        } else {
            format!("{label} · {provider}")
        };
        if models.is_model_scoped(id) {
            display.push_str(" (scoped)");
        }
        if is_current {
            display.push_str(" (current)");
        }

        let description = provider.to_string();

        // Trailing space on reasoning models: signals "more input
        // expected" to the prompt widget so Enter advances to effort
        // phase instead of submitting.
        let insert_text = if supports { format!("{token} ") } else { token };

        items.push(ArgItem {
            display,
            match_text: model_completion_search_text(id, info),
            insert_text,
            description,
        });
    }
    if let Some(current_id) = current_id {
        if let Some(current_index) = items
            .iter()
            .position(|item| resolve_model_for_arg_item(models, item).as_ref() == Some(current_id))
        {
            items.swap(0, current_index);
        }
    }
    items
}

/// Pi interactive completion value: always `provider/id` when provider is known.
fn model_completion_token(id: &acp::ModelId, info: &acp::ModelInfo) -> String {
    let (provider, model_id) = split_provider_model_id(id, info);
    match provider {
        Some(provider) if !provider.is_empty() => format!("{provider}/{model_id}"),
        _ => model_id.to_string(),
    }
}

/// Pi `getModelSearchText` — id-leading so bare model-id queries rank natural
/// id matches; still includes `provider/id` for exact-prefixed searches.
fn model_completion_search_text(id: &acp::ModelId, info: &acp::ModelInfo) -> String {
    let (provider, model_id) = split_provider_model_id(id, info);
    let name = if info.name.is_empty() {
        String::new()
    } else {
        format!(" {}", info.name)
    };
    match provider {
        Some(provider) if !provider.is_empty() => {
            format!("{model_id} {provider} {provider}/{model_id} {provider} {model_id}{name}")
        }
        _ => format!("{model_id}{name}"),
    }
}

/// Bottom detail pane for the selected `/model` row (pi-model-selector-x style).
/// Returns 1..=4 short lines: title, capabilities, cost, base URL.
pub(crate) fn model_picker_detail_lines(info: &acp::ModelInfo) -> Vec<String> {
    let meta = info.meta.as_ref();
    let (provider, model_id) = split_provider_model_id(&info.model_id, info);
    let mut lines = Vec::new();

    let mut title = info.name.clone();
    if let Some(provider) = provider.filter(|p| !p.is_empty()) {
        title.push_str(&format!("  [{provider}]"));
    }
    if model_id != info.name {
        title.push_str(&format!("  ·  {model_id}"));
    }
    lines.push(title);

    let mut caps = Vec::new();
    if let Some(tokens) = meta_u64(meta, "totalContextTokens") {
        caps.push(format!("Context {}", format_token_count(tokens)));
    }
    if let Some(tokens) = meta_u64(meta, "maxTokens") {
        caps.push(format!("MaxOut {}", format_token_count(tokens)));
    }
    if let Some(api) = meta_str(meta, "api").and_then(format_protocol_short) {
        caps.push(format!("API {api}"));
    }
    let input = format_input_short(meta);
    if !input.is_empty() {
        caps.push(format!("Input {input}"));
    }
    if meta_bool(meta, "reasoning").unwrap_or(false) || supports_reasoning_effort(info) {
        caps.push("⚡ reasoning".into());
    }
    if !caps.is_empty() {
        lines.push(caps.join("  ·  "));
    }

    if let Some(cost_line) = format_cost_line(meta) {
        lines.push(cost_line);
    }
    if let Some(base_url) = meta_str(meta, "baseUrl").filter(|s| !s.is_empty()) {
        lines.push(format!("BaseURL {base_url}"));
    }

    // Fall back to adapter description if meta was sparse.
    if lines.len() == 1
        && let Some(description) = info
            .description
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
    {
        lines.push(description.to_string());
    }
    lines
}

fn meta_str<'a>(
    meta: Option<&'a serde_json::Map<String, serde_json::Value>>,
    key: &str,
) -> Option<&'a str> {
    meta.and_then(|m| m.get(key)).and_then(|v| v.as_str())
}

fn meta_u64(meta: Option<&serde_json::Map<String, serde_json::Value>>, key: &str) -> Option<u64> {
    meta.and_then(|m| m.get(key)).and_then(|v| match v {
        serde_json::Value::Number(n) => n.as_u64(),
        _ => None,
    })
}

fn meta_bool(meta: Option<&serde_json::Map<String, serde_json::Value>>, key: &str) -> Option<bool> {
    meta.and_then(|m| m.get(key)).and_then(|v| v.as_bool())
}

fn format_token_count(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        let millions = tokens as f64 / 1_000_000.0;
        if millions.fract() == 0.0 {
            format!("{}M", millions as u64)
        } else {
            format!("{millions:.1}M")
        }
    } else if tokens >= 1_000 {
        let thousands = tokens as f64 / 1_000.0;
        if thousands.fract() == 0.0 {
            format!("{}k", thousands as u64)
        } else {
            format!("{thousands:.0}k")
        }
    } else {
        tokens.to_string()
    }
}

fn format_protocol_short(api: &str) -> Option<&'static str> {
    match api {
        "openai-responses" | "openai-codex-responses" => Some("resp"),
        "openai-completions" => Some("comp"),
        "anthropic-messages" => Some("anth"),
        "google-generative-ai" => Some("goog"),
        _ => None,
    }
}

fn format_input_short(meta: Option<&serde_json::Map<String, serde_json::Value>>) -> String {
    if let Some(modalities) = meta
        .and_then(|m| m.get("inputModalities"))
        .and_then(|v| v.as_array())
    {
        let mut parts = Vec::new();
        if modalities
            .iter()
            .any(|m| m.as_str().is_some_and(|s| s.eq_ignore_ascii_case("text")))
        {
            parts.push("txt");
        }
        if modalities
            .iter()
            .any(|m| m.as_str().is_some_and(|s| s.eq_ignore_ascii_case("image")))
        {
            parts.push("img");
        }
        if modalities
            .iter()
            .any(|m| m.as_str().is_some_and(|s| s.eq_ignore_ascii_case("audio")))
        {
            parts.push("aud");
        }
        if !parts.is_empty() {
            return parts.join("+");
        }
    }
    match meta_bool(meta, "acceptsImages") {
        Some(true) => "txt+img".into(),
        Some(false) => "txt".into(),
        None => String::new(),
    }
}

fn format_cost_num(value: f64) -> String {
    if value == 0.0 {
        "0".into()
    } else if value < 0.01 {
        format!("{value:.3}")
    } else if value < 1.0 {
        format!("{value:.2}")
    } else if (value - value.round()).abs() < f64::EPSILON {
        format!("{}", value.round() as i64)
    } else if value < 10.0 {
        format!("{value:.1}")
    } else {
        format!("{}", value.round() as i64)
    }
}

fn format_cost_line(meta: Option<&serde_json::Map<String, serde_json::Value>>) -> Option<String> {
    let cost = meta
        .and_then(|m| m.get("cost"))
        .and_then(|v| v.as_object())?;
    let input = cost.get("input").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let output = cost.get("output").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let mut line = if input == 0.0 && output == 0.0 {
        "Cost free".to_string()
    } else {
        format!(
            "Cost ${} / ${}",
            format_cost_num(input),
            format_cost_num(output)
        )
    };
    if let Some(cache_read) = cost
        .get("cacheRead")
        .and_then(|v| v.as_f64())
        .filter(|v| *v > 0.0)
    {
        line.push_str(&format!("  ·  cache read ${}", format_cost_num(cache_read)));
    }
    if let Some(cache_write) = cost
        .get("cacheWrite")
        .and_then(|v| v.as_f64())
        .filter(|v| *v > 0.0)
    {
        line.push_str(&format!(
            "  ·  cache write ${}",
            format_cost_num(cache_write)
        ));
    }
    Some(line)
}

/// Resolve the catalog model for an ArgPicker row.
pub(crate) fn resolve_model_for_arg_item(
    models: &ModelState,
    item: &crate::slash::command::ArgItem,
) -> Option<acp::ModelId> {
    let token = item.insert_text.trim_end();
    models
        .resolve_by_name_or_id(token)
        .or_else(|| models.resolve_by_name_or_id(item.display.trim_end_matches("(current)").trim()))
}

fn split_provider_model_id<'a>(
    id: &'a acp::ModelId,
    info: &'a acp::ModelInfo,
) -> (Option<&'a str>, &'a str) {
    let raw = id.0.as_ref();
    if let Some((provider, model_id)) = raw.split_once("::") {
        return (Some(provider), model_id);
    }
    let provider = info
        .meta
        .as_ref()
        .and_then(|meta| meta.get("provider"))
        .and_then(|v| v.as_str());
    let model_id = info
        .meta
        .as_ref()
        .and_then(|meta| meta.get("modelId"))
        .and_then(|v| v.as_str())
        .unwrap_or(raw);
    (provider, model_id)
}

/// One row per effort level for the `/model` chained effort phase.
/// `insert_text` is `"ModelToken high"` so selecting a row completes both tokens.
fn build_effort_items(models: &ModelState, model_id: &acp::ModelId) -> Vec<ArgItem> {
    let info = match models.available.get(model_id) {
        Some(info) => info,
        None => return Vec::new(),
    };
    let model_token = model_completion_token(model_id, info);
    let is_current_model = models.current.as_ref() == Some(model_id);
    let options = models.reasoning_effort_options_for(model_id);
    build_effort_arg_items(
        &options,
        models.reasoning_effort,
        is_current_model,
        |option| format!("{model_token} {}", option.id),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use xai_grok_shell::sampling::types::ReasoningEffort;

    fn model_with_reasoning(id: &str, name: &str) -> (acp::ModelId, acp::ModelInfo) {
        let catalog_id = if id.contains("::") {
            id.to_string()
        } else {
            format!("test::{id}")
        };
        let bare = catalog_id.split_once("::").map(|(_, m)| m).unwrap_or(id);
        let provider = catalog_id
            .split_once("::")
            .map(|(p, _)| p)
            .unwrap_or("test");
        let model_id = acp::ModelId::new(Arc::from(catalog_id.as_str()));
        let mut meta = serde_json::Map::new();
        meta.insert(
            "supportsReasoningEffort".into(),
            serde_json::Value::Bool(true),
        );
        meta.insert(
            "provider".into(),
            serde_json::Value::String(provider.into()),
        );
        meta.insert("modelId".into(), serde_json::Value::String(bare.into()));
        let info = acp::ModelInfo::new(model_id.clone(), name.to_string())
            .meta(serde_json::Value::Object(meta).as_object().cloned());
        (model_id, info)
    }

    fn plain_model(id: &str, name: &str) -> (acp::ModelId, acp::ModelInfo) {
        let catalog_id = if id.contains("::") {
            id.to_string()
        } else {
            format!("test::{id}")
        };
        let bare = catalog_id.split_once("::").map(|(_, m)| m).unwrap_or(id);
        let provider = catalog_id
            .split_once("::")
            .map(|(p, _)| p)
            .unwrap_or("test");
        let model_id = acp::ModelId::new(Arc::from(catalog_id.as_str()));
        let mut meta = serde_json::Map::new();
        meta.insert(
            "provider".into(),
            serde_json::Value::String(provider.into()),
        );
        meta.insert("modelId".into(), serde_json::Value::String(bare.into()));
        let info = acp::ModelInfo::new(model_id.clone(), name.to_string())
            .meta(serde_json::Value::Object(meta).as_object().cloned());
        (model_id, info)
    }

    static EMPTY_BUNDLE: crate::app::bundle::BundleState = crate::app::bundle::BundleState {
        has_cache: false,
        version: String::new(),
        personas: Vec::new(),
        roles: Vec::new(),
        agents: Vec::new(),
        skills: Vec::new(),
        persona_details: Vec::new(),
        role_details: Vec::new(),
    };

    fn dummy_exec_ctx(models: &ModelState) -> CommandExecCtx<'_> {
        CommandExecCtx {
            models,
            session_id: None,
            bundle_state: &EMPTY_BUNDLE,
            screen_mode: crate::app::ScreenMode::Inline,
            billing_surface_visible: true,
            usage_command_visible: true,
            pager_state: crate::settings::PagerLocalSnapshot {
                multiline_mode: false,
                yolo_mode: false,
                ..crate::settings::PagerLocalSnapshot::default()
            },
        }
    }

    #[test]
    fn split_trailing_token_splits_on_final_whitespace() {
        assert_eq!(
            split_trailing_token("Reasoning X high"),
            Some(("Reasoning X", "high"))
        );
        assert_eq!(
            split_trailing_token("reasoning-x  xhigh"),
            Some(("reasoning-x", "xhigh"))
        );
        // No interior whitespace → nothing to split off.
        assert!(split_trailing_token("reasoning-x-pro").is_none());
    }

    #[test]
    fn model_next_uses_scoped_cycle_action() {
        let models = ModelState::default();
        let mut ctx = dummy_exec_ctx(&models);
        assert!(matches!(
            ModelCommand.run(&mut ctx, "next"),
            CommandResult::Action(Action::NextModel)
        ));
    }

    #[test]
    fn bare_model_opens_picker() {
        let models = ModelState::default();
        let mut ctx = dummy_exec_ctx(&models);
        assert!(matches!(
            ModelCommand.run(&mut ctx, ""),
            CommandResult::Action(Action::OpenModelPicker)
        ));
    }

    #[test]
    fn empty_query_returns_one_row_per_logical_model() {
        let mut state = ModelState::default();
        let (rid, rinfo) = model_with_reasoning("reasoning-x", "Reasoning X");
        let (pid, pinfo) = plain_model("grok-4.5", "Grok 4.5");
        state.available.insert(rid, rinfo);
        state.available.insert(pid.clone(), pinfo);
        state.current = Some(pid);

        let cmd = ModelCommand;
        let ctx = AppCtx {
            models: &state,
            cwd: std::path::Path::new("."),
            has_session_announcements: false,
            billing_surface_visible: true,
            usage_command_visible: true,
            workflows_available: true,
            screen_mode: crate::app::ScreenMode::Fullscreen,
            current_title: None,
        };
        let items = cmd.suggest_args(&ctx, "").unwrap();
        assert_eq!(items.len(), 2, "model phase: one row per logical model");
        assert!(items[0].display.contains("(current)"));

        // Pi interactive: insert_text is provider/id; trailing space on reasoning
        // keeps the dropdown open so the effort sub-menu can render.
        let reasoning = items
            .iter()
            .find(|i| i.insert_text.starts_with("test/reasoning-x"))
            .unwrap();
        assert_eq!(reasoning.insert_text, "test/reasoning-x ");
        assert_eq!(reasoning.display, "Reasoning X · test");
        assert_eq!(reasoning.description, "test");

        // Plain model has no trailing space -- Enter commits immediately.
        let plain = items
            .iter()
            .find(|i| i.insert_text == "test/grok-4.5")
            .unwrap();
        assert_eq!(plain.insert_text, "test/grok-4.5");
        assert!(plain.display.contains("grok-4.5"));
        assert!(plain.display.contains("(current)"));
        assert_eq!(plain.description, "test");
    }

    #[test]
    fn model_rows_show_provider_and_disambiguate_duplicate_names() {
        let mut state = ModelState::default();
        let a = acp::ModelId::new(Arc::from("anthropic::claude-haiku-4-5"));
        let b = acp::ModelId::new(Arc::from("openrouter::claude-haiku-4-5"));
        let a_info = acp::ModelInfo::new(a.clone(), "Claude Haiku 4.5".to_string()).meta(
            serde_json::json!({
                "provider": "anthropic",
                "modelId": "claude-haiku-4-5",
                "totalContextTokens": 200000,
                "maxTokens": 64000,
                "api": "anthropic-messages",
                "acceptsImages": true,
                "inputModalities": ["text", "image"],
                "reasoning": true,
                "cost": { "input": 1.0, "output": 5.0, "cacheRead": 0.1 }
            })
            .as_object()
            .cloned(),
        );
        let b_info = acp::ModelInfo::new(b.clone(), "Claude Haiku 4.5".to_string()).meta(
            serde_json::json!({
                "provider": "openrouter",
                "modelId": "claude-haiku-4-5",
                "totalContextTokens": 200000,
                "maxTokens": 64000,
                "api": "openai-completions",
                "acceptsImages": false,
                "cost": { "input": 0.25, "output": 1.25 }
            })
            .as_object()
            .cloned(),
        );
        state.available.insert(a, a_info.clone());
        state.available.insert(b, b_info);

        let cmd = ModelCommand;
        let ctx = AppCtx {
            models: &state,
            cwd: std::path::Path::new("."),
            has_session_announcements: false,
            screen_mode: crate::app::ScreenMode::Fullscreen,
            billing_surface_visible: false,
            usage_command_visible: true,
            workflows_available: true,
            current_title: None,
        };
        let items = cmd.suggest_args(&ctx, "").unwrap();
        assert_eq!(items.len(), 2);

        // The picker label combines the model name and provider; value stays provider/id.
        let anthropic = items.iter().find(|i| i.description == "anthropic").unwrap();
        assert_eq!(anthropic.display, "Claude Haiku 4.5 · anthropic");
        assert_eq!(anthropic.description, "anthropic");
        assert_eq!(anthropic.insert_text, "anthropic/claude-haiku-4-5");

        let openrouter = items
            .iter()
            .find(|i| i.description == "openrouter")
            .unwrap();
        assert_eq!(openrouter.display, "Claude Haiku 4.5 · openrouter");
        assert_eq!(openrouter.insert_text, "openrouter/claude-haiku-4-5");

        let detail = model_picker_detail_lines(&a_info);
        assert!(detail[0].contains("Claude Haiku 4.5"));
        assert!(detail[0].contains("[anthropic]"));
        assert!(
            detail.iter().any(|l| l.contains("Context 200k")),
            "{detail:?}"
        );
        assert!(detail.iter().any(|l| l.contains("API anth")), "{detail:?}");
        assert!(
            detail.iter().any(|l| l.contains("Cost $1 / $5")),
            "{detail:?}"
        );
        assert!(
            detail.iter().any(|l| l.contains("cache read")),
            "{detail:?}"
        );
    }

    #[test]
    fn trailing_space_after_reasoning_model_enters_effort_phase() {
        let mut state = ModelState::default();
        let (id, info) = model_with_reasoning("reasoning-x", "Reasoning X");
        state.available.insert(id, info);

        let cmd = ModelCommand;
        let ctx = AppCtx {
            models: &state,
            cwd: std::path::Path::new("."),
            has_session_announcements: false,
            billing_surface_visible: true,
            usage_command_visible: true,
            workflows_available: true,
            screen_mode: crate::app::ScreenMode::Fullscreen,
            current_title: None,
        };
        // Args query has a trailing space after provider/id -> effort phase.
        // Items come out ordered xhigh -> low (strongest first) per EFFORT_LEVELS.
        let items = cmd.suggest_args(&ctx, "test/reasoning-x ").unwrap();
        assert_eq!(items.len(), 4);
        assert_eq!(items[0].insert_text, "test/reasoning-x xhigh");
        assert_eq!(items[1].insert_text, "test/reasoning-x high");
        assert_eq!(items[2].insert_text, "test/reasoning-x medium");
        assert_eq!(items[3].insert_text, "test/reasoning-x low");
        // Display is just the level so the user sees a clean column.
        assert_eq!(items[0].display, "xhigh");
        // match_text carries the sort-key prefix that forces the matcher's
        // alphabetical tiebreak to render rows in EFFORT_LEVELS order.
        assert!(items[0].match_text.starts_with("a "));
        assert!(items[3].match_text.starts_with("d "));
    }

    #[test]
    fn partial_effort_query_still_in_effort_phase() {
        let mut state = ModelState::default();
        let (id, info) = model_with_reasoning("reasoning-x", "Reasoning X");
        state.available.insert(id, info);

        let cmd = ModelCommand;
        let ctx = AppCtx {
            models: &state,
            cwd: std::path::Path::new("."),
            has_session_announcements: false,
            billing_surface_visible: true,
            usage_command_visible: true,
            workflows_available: true,
            screen_mode: crate::app::ScreenMode::Fullscreen,
            current_title: None,
        };
        // Still in effort phase; matcher upstream narrows to high / xhigh.
        let items = cmd.suggest_args(&ctx, "test/reasoning-x h").unwrap();
        assert_eq!(items.len(), 4);
    }

    #[test]
    fn partial_model_query_stays_in_model_phase() {
        let mut state = ModelState::default();
        let (id, info) = model_with_reasoning("reasoning-x", "Reasoning X");
        state.available.insert(id, info);

        let cmd = ModelCommand;
        let ctx = AppCtx {
            models: &state,
            cwd: std::path::Path::new("."),
            has_session_announcements: false,
            billing_surface_visible: true,
            usage_command_visible: true,
            workflows_available: true,
            screen_mode: crate::app::ScreenMode::Fullscreen,
            current_title: None,
        };
        // No trailing space, user is still typing the model id / provider prefix.
        let items = cmd.suggest_args(&ctx, "reason").unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].insert_text, "test/reasoning-x ");
        assert!(uses_pi_model_picker_search(&state, "reason"));
        assert!(!uses_pi_model_picker_search(&state, "test/reasoning-x h"));
    }

    #[test]
    fn model_completion_search_text_matches_pi_get_model_search_text() {
        let id = acp::ModelId::new(Arc::from("openai-codex::gpt-5.5"));
        let info = acp::ModelInfo::new(id.clone(), "GPT 5.5".to_string());
        // Pi getModelSearchText: `${id} ${provider} ${provider}/${id} ${provider} ${id}${name}`
        assert_eq!(
            model_completion_search_text(&id, &info),
            "gpt-5.5 openai-codex openai-codex/gpt-5.5 openai-codex gpt-5.5 GPT 5.5"
        );
        assert_eq!(model_completion_token(&id, &info), "openai-codex/gpt-5.5");
    }

    #[test]
    fn run_parses_model_plus_effort_when_supported() {
        let mut state = ModelState::default();
        let (id, info) = model_with_reasoning("reasoning-x", "Reasoning X");
        state.available.insert(id, info);
        let mut ctx = dummy_exec_ctx(&state);
        let result = ModelCommand.run(&mut ctx, "test/reasoning-x xhigh");
        match result {
            CommandResult::Action(Action::SwitchModel { model_id, effort }) => {
                assert_eq!(model_id.0.as_ref(), "test::reasoning-x");
                assert_eq!(effort, Some(ReasoningEffort::Xhigh));
            }
            other => panic!("expected SwitchModel with effort, got {other:?}"),
        }
    }

    #[test]
    fn run_rejects_unoffered_effort_with_effort_error_not_unknown_model() {
        // Regression: previously `resolve_effort_token_for` returned None and
        // the handler fell through to `Unknown model: Reasoning X none`.
        let mut state = ModelState::default();
        let (id, info) = model_with_reasoning("reasoning-x", "Reasoning X");
        state.available.insert(id, info);
        let mut ctx = dummy_exec_ctx(&state);
        let result = ModelCommand.run(&mut ctx, "test/reasoning-x none");
        match result {
            CommandResult::Error(msg) => {
                assert!(
                    msg.contains("unknown effort level 'none'"),
                    "expected effort error, got {msg}"
                );
                assert!(
                    msg.contains("use one of:"),
                    "expected offered levels in message, got {msg}"
                );
                assert!(
                    !msg.to_lowercase().contains("unknown model"),
                    "must not misreport as unknown model: {msg}"
                );
                let offered = msg.split_once("; ").map(|(_, r)| r).unwrap_or("");
                assert!(
                    !offered.contains("none"),
                    "must not list none as offered: {msg}"
                );
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn run_prefers_full_multi_word_model_name_over_prefix_plus_effort() {
        // Catalog has both "Grok" (reasoning) and "Grok 4.5". `/model Grok 4.5`
        // must select the full name, not treat "4.5" as an effort on "Grok".
        let mut state = ModelState::default();
        let (short_id, short_info) = model_with_reasoning("grok", "Grok");
        let (long_id, long_info) = model_with_reasoning("grok-4.5", "Grok 4.5");
        state.available.insert(short_id, short_info);
        state.available.insert(long_id.clone(), long_info);
        let mut ctx = dummy_exec_ctx(&state);
        let result = ModelCommand.run(&mut ctx, "Grok 4.5");
        match result {
            CommandResult::Action(Action::SetDefaultModel(resolved_id)) => {
                assert_eq!(resolved_id, long_id);
            }
            other => panic!("expected SetDefaultModel(Grok 4.5), got {other:?}"),
        }
    }

    #[test]
    fn run_rejects_effort_for_non_reasoning_model() {
        let mut state = ModelState::default();
        let (id, info) = plain_model("grok-4.5", "Grok 4.5");
        state.available.insert(id, info);
        let mut ctx = dummy_exec_ctx(&state);
        let result = ModelCommand.run(&mut ctx, "Grok 4.5 high");
        // Falls through to "is the whole string a model name?" — which
        // it isn't, so we get an Unknown error.
        assert!(matches!(result, CommandResult::Error(_)));
    }

    /// The bare `/model <name>` form dispatches
    /// `Action::SetDefaultModel(<ModelId>)` instead of the legacy
    /// `Action::SwitchModel { effort: None }`. The dispatcher routes
    /// the typed setter through both `Effect::SwitchModel`
    /// (session-level mutation) AND `Effect::PersistSetting`
    /// (next-session default).
    ///
    /// The payload is the typed `acp::ModelId` (resolved at the slash
    /// boundary), not a String.
    #[test]
    fn run_bare_model_name_dispatches_set_default_model() {
        let mut state = ModelState::default();
        let (id, info) = plain_model("grok-4.5", "Grok 4.5");
        state.available.insert(id.clone(), info);
        let mut ctx = dummy_exec_ctx(&state);
        let result = ModelCommand.run(&mut ctx, "Grok 4.5");
        match result {
            CommandResult::Action(Action::SetDefaultModel(resolved_id)) => {
                assert_eq!(resolved_id, id);
            }
            other => panic!("expected Action::SetDefaultModel(<id>), got {other:?}"),
        }
    }

    /// Case-insensitive matching against the catalog: `/model grok 4.5`
    /// resolves to the same `ModelId` as `/model Grok 4.5`.
    #[test]
    fn run_set_default_model_resolves_case_insensitively() {
        let mut state = ModelState::default();
        let (id, info) = plain_model("grok-4.5", "Grok 4.5");
        state.available.insert(id.clone(), info);
        let mut ctx = dummy_exec_ctx(&state);
        let result = ModelCommand.run(&mut ctx, "grok 4.5");
        match result {
            CommandResult::Action(Action::SetDefaultModel(resolved_id)) => {
                assert_eq!(resolved_id, id);
            }
            other => panic!("expected Action::SetDefaultModel(<id>), got {other:?}"),
        }
    }
}
