//! Pi session cache metrics — pure projection of `get_entries`.
//!
//! Mirrors npm `pi-cache-graph` formulas without UI or Pi TUI dependencies.
//! Hit % = cacheRead / (input + cacheRead + cacheWrite) * 100.
//!
//! When a provider writes `usage` with all zeros (common for some OpenAI-
//! compatible proxies on resume), fall back to `ceil(len/4)` content
//! estimates for input/output so the graph is not a flat zero timeline.

use serde_json::{Value, json};

use crate::model::string;

/// cacheRead / (input + cacheRead + cacheWrite) * 100.
pub(crate) fn compute_cache_hit_percent(input: u64, cache_read: u64, cache_write: u64) -> f64 {
    let denominator = input.saturating_add(cache_read).saturating_add(cache_write);
    if denominator == 0 {
        return 0.0;
    }
    (cache_read as f64 / denominator as f64) * 100.0
}

/// Collect pi-cache-graph-compatible metrics from a Pi `get_entries` payload.
///
/// Active branch = entry ids on the parent chain from `leafId` to root.
pub(crate) fn collect_cache_session_metrics(entries_payload: &Value) -> Value {
    let items = entries_payload
        .get("entries")
        .and_then(Value::as_array)
        .cloned()
        .or_else(|| entries_payload.as_array().cloned())
        .unwrap_or_default();

    let leaf_id = string(entries_payload, &["leafId", "leaf_id"]).map(str::to_owned);
    let active_branch_ids = active_branch_id_set(&items, leaf_id.as_deref());

    let mut tree_totals = empty_totals();
    let mut active_branch_totals = empty_totals();
    let mut all_messages = Vec::new();
    let mut active_branch_messages = Vec::new();
    let mut sequence: u32 = 0;
    let mut active_branch_sequence: u32 = 0;
    let mut estimated_count: u32 = 0;
    let mut rebilled_tokens: u64 = 0;
    let mut cache_miss_count: u32 = 0;
    let mut previous_prompt: Option<(u64, bool)> = None;

    for entry in &items {
        let kind = string(entry, &["type", "kind"])
            .unwrap_or_default()
            .to_ascii_lowercase();
        if kind == "compaction" || kind == "branch_summary" {
            previous_prompt = None;
            continue;
        }
        if kind != "message" {
            continue;
        }
        // Prefer nested message; also accept flat entry.message-less shape.
        let message = entry.get("message").unwrap_or(entry);
        let role = string(message, &["role"]).unwrap_or_default();
        if role != "assistant" {
            continue;
        }

        let (mut input, mut output, cache_read, cache_write, mut total_tokens, mut estimated) =
            extract_usage(message);

        // Provider wrote zeros / omitted usage — estimate from content so resume
        // graphs are not a flat zero timeline (context bar already estimates).
        if input == 0 && output == 0 && cache_read == 0 && cache_write == 0 && total_tokens == 0 {
            let est_out = estimate_assistant_output_tokens(message);
            let est_in = estimate_prompt_proxy_tokens(message, est_out);
            if est_out > 0 || est_in > 0 {
                output = est_out;
                input = est_in;
                total_tokens = input.saturating_add(output);
                estimated = true;
            }
        }

        // Still nothing to show — skip pure empty shells (e.g. aborted with no content).
        if input == 0 && output == 0 && cache_read == 0 && cache_write == 0 && total_tokens == 0 {
            continue;
        }

        sequence = sequence.saturating_add(1);
        if estimated {
            estimated_count = estimated_count.saturating_add(1);
        }

        let entry_id = string(entry, &["id"])
            .or_else(|| string(message, &["id"]))
            .unwrap_or_default()
            .to_string();
        let is_on_active_branch = if active_branch_ids.is_empty() {
            true
        } else {
            active_branch_ids.contains(&entry_id)
        };
        let hit = compute_cache_hit_percent(input, cache_read, cache_write);
        let prompt_tokens = input.saturating_add(cache_read).saturating_add(cache_write);
        if let Some((previous_tokens, reported_cache)) = previous_prompt {
            if prompt_tokens > 0 && (cache_read + cache_write > 0 || reported_cache) {
                let missed = previous_tokens
                    .min(prompt_tokens)
                    .saturating_sub(cache_read);
                if missed > 1024 {
                    rebilled_tokens = rebilled_tokens.saturating_add(missed);
                    cache_miss_count = cache_miss_count.saturating_add(1);
                }
            }
        }
        if prompt_tokens > 0 {
            previous_prompt = Some((
                prompt_tokens,
                previous_prompt.is_some_and(|(_, reported)| reported)
                    || cache_read.saturating_add(cache_write) > 0,
            ));
        }

        let mut metric = json!({
            "sequence": sequence,
            "entryId": entry_id,
            "timestamp": string(entry, &["timestamp"])
                .or_else(|| string(message, &["timestamp"]))
                .map(|s| s.to_string())
                .unwrap_or_else(|| {
                    message
                        .get("timestamp")
                        .and_then(|v| v.as_u64().or_else(|| v.as_i64().map(|n| n as u64)))
                        .map(|n| n.to_string())
                        .unwrap_or_default()
                }),
            "provider": string(message, &["provider"]).unwrap_or_default(),
            "model": string(message, &["model"]).unwrap_or_default(),
            "input": input,
            "output": output,
            "cacheRead": cache_read,
            "cacheWrite": cache_write,
            "totalTokens": total_tokens,
            "cacheHitPercent": hit,
            "isOnActiveBranch": is_on_active_branch,
            "usageEstimated": estimated,
        });

        add_to_totals(
            &mut tree_totals,
            input,
            output,
            cache_read,
            cache_write,
            total_tokens,
        );

        if is_on_active_branch {
            active_branch_sequence = active_branch_sequence.saturating_add(1);
            if let Some(obj) = metric.as_object_mut() {
                obj.insert(
                    "activeBranchSequence".into(),
                    Value::from(active_branch_sequence),
                );
            }
            add_to_totals(
                &mut active_branch_totals,
                input,
                output,
                cache_read,
                cache_write,
                total_tokens,
            );
            active_branch_messages.push(metric.clone());
        }

        all_messages.push(metric);
    }

    json!({
        "allMessages": all_messages,
        "activeBranchMessages": active_branch_messages,
        "treeTotals": totals_json(&tree_totals),
        "activeBranchTotals": totals_json(&active_branch_totals),
        "rebilledTokens": rebilled_tokens,
        "cacheMissCount": cache_miss_count,
        "estimatedCount": estimated_count,
    })
}

/// Parse usage from message.usage with tolerant number parsing.
fn extract_usage(message: &Value) -> (u64, u64, u64, u64, u64, bool) {
    let Some(usage) = message.get("usage").filter(|u| u.is_object()) else {
        return (0, 0, 0, 0, 0, false);
    };
    let input = usage_field(usage, &["input", "prompt_tokens", "promptTokens"]);
    let output = usage_field(usage, &["output", "completion_tokens", "completionTokens"]);
    let cache_read = usage_field(
        usage,
        &["cacheRead", "cache_read", "cached_tokens", "cachedTokens"],
    );
    let cache_write = usage_field(usage, &["cacheWrite", "cache_write"]);
    let total_tokens = usage_field(usage, &["totalTokens", "total_tokens"]).max(
        input
            .saturating_add(output)
            .saturating_add(cache_read)
            .saturating_add(cache_write),
    );
    (input, output, cache_read, cache_write, total_tokens, false)
}

fn usage_field(usage: &Value, names: &[&str]) -> u64 {
    names
        .iter()
        .find_map(|name| json_u64(usage.get(*name)))
        .unwrap_or(0)
}

/// Accept u64 / i64 / f64 / numeric string — RPC and providers are inconsistent.
fn json_u64(value: Option<&Value>) -> Option<u64> {
    let value = value?;
    if let Some(n) = value.as_u64() {
        return Some(n);
    }
    if let Some(n) = value.as_i64() {
        return Some(n.max(0) as u64);
    }
    if let Some(n) = value.as_f64() {
        if n.is_finite() && n >= 0.0 {
            return Some(n.round() as u64);
        }
    }
    if let Some(s) = value.as_str() {
        if let Ok(n) = s.trim().parse::<u64>() {
            return Some(n);
        }
        if let Ok(n) = s.trim().parse::<f64>() {
            if n.is_finite() && n >= 0.0 {
                return Some(n.round() as u64);
            }
        }
    }
    None
}

fn estimate_tokens_text(text: &str) -> u64 {
    if text.is_empty() {
        return 0;
    }
    ((text.len() as f64) / 4.0).ceil() as u64
}

fn estimate_tokens_value(value: &Value) -> u64 {
    match value {
        Value::Null => 0,
        Value::String(s) => estimate_tokens_text(s),
        Value::Array(items) => items.iter().map(estimate_tokens_value).sum(),
        Value::Object(map) => {
            // Prefer text-like fields in content parts.
            if let Some(t) = map.get("text").and_then(Value::as_str) {
                return estimate_tokens_text(t);
            }
            if let Some(t) = map.get("thinking").and_then(Value::as_str) {
                return estimate_tokens_text(t);
            }
            map.values().map(estimate_tokens_value).sum()
        }
        other => estimate_tokens_text(&other.to_string()),
    }
}

/// Estimate assistant **output** tokens from content (+ thinking if present).
fn estimate_assistant_output_tokens(message: &Value) -> u64 {
    let mut total = 0u64;
    if let Some(content) = message.get("content") {
        total = total.saturating_add(estimate_tokens_value(content));
    }
    // Some shapes keep thinking separately.
    if let Some(thinking) = message.get("thinking") {
        total = total.saturating_add(estimate_tokens_value(thinking));
    }
    total
}

/// Rough prompt-size proxy when providers omit usage.
///
/// We cannot recover cacheRead. Use max(output*4, output+256) as a stand-in
/// "prompt mass" so cumulative-total and per-turn height are non-zero for
/// resume of zero-usage sessions (e.g. qwen3.8max via openai-responses).
fn estimate_prompt_proxy_tokens(message: &Value, output_tokens: u64) -> u64 {
    // If the message has toolCalls in content, prompt tends to be larger.
    let tool_calls = message
        .get("content")
        .and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .filter(|p| {
                    string(p, &["type", "kind"])
                        .unwrap_or_default()
                        .eq_ignore_ascii_case("toolCall")
                        || string(p, &["type", "kind"])
                            .unwrap_or_default()
                            .eq_ignore_ascii_case("tool_call")
                })
                .count()
        })
        .unwrap_or(0);
    let base = output_tokens
        .saturating_mul(4)
        .max(output_tokens.saturating_add(256));
    if tool_calls > 0 {
        base.saturating_add((tool_calls as u64).saturating_mul(128))
    } else {
        base
    }
}

fn empty_totals() -> (u64, u64, u64, u64, u64, u64) {
    (0, 0, 0, 0, 0, 0)
}

fn add_to_totals(
    totals: &mut (u64, u64, u64, u64, u64, u64),
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
    total_tokens: u64,
) {
    totals.0 = totals.0.saturating_add(input);
    totals.1 = totals.1.saturating_add(output);
    totals.2 = totals.2.saturating_add(cache_read);
    totals.3 = totals.3.saturating_add(cache_write);
    totals.4 = totals.4.saturating_add(total_tokens);
    totals.5 = totals.5.saturating_add(1);
}

fn totals_json(totals: &(u64, u64, u64, u64, u64, u64)) -> Value {
    json!({
        "input": totals.0,
        "output": totals.1,
        "cacheRead": totals.2,
        "cacheWrite": totals.3,
        "totalTokens": totals.4,
        "assistantMessages": totals.5,
    })
}

fn active_branch_id_set(
    entries: &[Value],
    leaf_id: Option<&str>,
) -> std::collections::HashSet<String> {
    let mut by_id: std::collections::HashMap<String, Option<String>> =
        std::collections::HashMap::with_capacity(entries.len());
    for entry in entries {
        let Some(id) = string(entry, &["id"]).map(str::to_owned) else {
            continue;
        };
        let parent = string(entry, &["parentId", "parent_id"]).map(str::to_owned);
        by_id.insert(id, parent);
    }

    let mut set = std::collections::HashSet::new();
    let Some(mut current) = leaf_id.map(str::to_owned) else {
        // No leaf → treat every entry as active (linear session).
        for id in by_id.keys() {
            set.insert(id.clone());
        }
        return set;
    };
    while set.insert(current.clone()) {
        match by_id.get(&current).and_then(|p| p.clone()) {
            Some(parent) => current = parent,
            None => break,
        }
    }
    set
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn hit_percent_matches_pi_cache_graph_formula() {
        assert!((compute_cache_hit_percent(50, 50, 0) - 50.0).abs() < f64::EPSILON);
        assert!((compute_cache_hit_percent(10, 80, 10) - 80.0).abs() < f64::EPSILON);
        assert_eq!(compute_cache_hit_percent(0, 0, 0), 0.0);
    }

    #[test]
    fn collect_marks_active_branch_via_parent_chain() {
        let payload = json!({
            "leafId": "a2",
            "entries": [
                {
                    "id": "u1", "parentId": null, "type": "message", "timestamp": "2026-01-01T00:00:00Z",
                    "message": { "role": "user", "content": "hi" }
                },
                {
                    "id": "a1", "parentId": "u1", "type": "message", "timestamp": "2026-01-01T00:00:01Z",
                    "message": {
                        "role": "assistant", "provider": "xai", "model": "grok",
                        "usage": { "input": 100, "output": 10, "cacheRead": 0, "cacheWrite": 50, "totalTokens": 160 }
                    }
                },
                {
                    "id": "u2", "parentId": "a1", "type": "message", "timestamp": "2026-01-01T00:00:02Z",
                    "message": { "role": "user", "content": "again" }
                },
                {
                    "id": "a2", "parentId": "u2", "type": "message", "timestamp": "2026-01-01T00:00:03Z",
                    "message": {
                        "role": "assistant", "provider": "xai", "model": "grok",
                        "usage": { "input": 20, "output": 5, "cacheRead": 80, "cacheWrite": 0, "totalTokens": 105 }
                    }
                },
                {
                    "id": "a_side", "parentId": "u1", "type": "message", "timestamp": "2026-01-01T00:00:04Z",
                    "message": {
                        "role": "assistant", "provider": "xai", "model": "grok",
                        "usage": { "input": 5, "output": 1, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 6 }
                    }
                }
            ]
        });
        let metrics = collect_cache_session_metrics(&payload);
        let all = metrics["allMessages"].as_array().unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(metrics["treeTotals"]["assistantMessages"], 3);
        assert_eq!(metrics["activeBranchTotals"]["assistantMessages"], 2);
        let side = all.iter().find(|m| m["entryId"] == "a_side").unwrap();
        assert_eq!(side["isOnActiveBranch"], false);
        let a2 = all.iter().find(|m| m["entryId"] == "a2").unwrap();
        assert_eq!(a2["isOnActiveBranch"], true);
        assert!((a2["cacheHitPercent"].as_f64().unwrap() - 80.0).abs() < 0.01);
        assert_eq!(metrics["estimatedCount"], 0);
    }

    #[test]
    fn zero_usage_falls_back_to_content_estimate() {
        // Mirrors 3838/qwen3.8max resume sessions: usage present but all zeros.
        let long_text = "hello world ".repeat(40); // ~480 chars → ~120 tokens
        let payload = json!({
            "leafId": "a1",
            "entries": [
                {
                    "id": "a1", "parentId": null, "type": "message", "timestamp": "2026-01-01T00:00:00Z",
                    "message": {
                        "role": "assistant",
                        "provider": "3838",
                        "model": "qwen3.8max",
                        "content": [{ "type": "text", "text": long_text }],
                        "usage": {
                            "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 0
                        }
                    }
                }
            ]
        });
        let metrics = collect_cache_session_metrics(&payload);
        let all = metrics["allMessages"].as_array().unwrap();
        assert_eq!(all.len(), 1);
        assert!(
            all[0]["output"].as_u64().unwrap() > 0,
            "output estimated from content"
        );
        assert!(
            all[0]["input"].as_u64().unwrap() > 0,
            "input proxy estimated"
        );
        assert_eq!(all[0]["usageEstimated"], true);
        assert_eq!(metrics["estimatedCount"], 1);
        // No real cache data — hit stays 0
        assert_eq!(all[0]["cacheHitPercent"].as_f64().unwrap(), 0.0);
    }

    #[test]
    fn empty_entries_yield_empty_metrics() {
        let metrics = collect_cache_session_metrics(&json!({ "entries": [], "leafId": null }));
        assert!(metrics["allMessages"].as_array().unwrap().is_empty());
        assert_eq!(metrics["treeTotals"]["assistantMessages"], 0);
    }

    #[test]
    fn json_u64_accepts_float_and_string() {
        assert_eq!(json_u64(Some(&json!(12.7))), Some(13));
        assert_eq!(json_u64(Some(&json!("42"))), Some(42));
        assert_eq!(json_u64(Some(&json!(-3))), Some(0));
    }
}
