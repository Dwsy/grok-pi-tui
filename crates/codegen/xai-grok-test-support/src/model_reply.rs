use crate::failure::{
    CONTENT_FILTER_REPLY, CUT_REPLY, MALFORMED_SSE_BODY, StatusFailure, StreamError,
    TRUNCATED_REPLY,
};
use crate::inference_request::InferenceEndpoint;
use crate::scripted::{ScriptedBody, ScriptedResponse};
use crate::sse::{
    UsageReport, chat_completion_deltas, chat_completion_script_from_deltas_with_usage,
    chat_completion_script_with_reasoning, chat_completions_no_content_events,
    chat_completions_reasoning_then_tool_call_events, messages_api_no_content_events,
    messages_api_script_chunks, messages_api_script_with_reasoning, renumber_sequence_numbers,
    responses_api_completed_only_events, responses_api_deltas,
    responses_api_reasoning_and_text_events_with_usage, responses_api_reasoning_only_events,
    responses_api_reasoning_then_tool_call_events, responses_api_script_exact,
    responses_api_script_from_deltas_with_usage,
};
use crate::tool_call_turn::{
    ToolCallTurn, TurnCall, chat_completion_tool_calls_events, content_filter_events,
    cut_reply_events, looping_reply_events, messages_api_tool_uses_events,
    responses_api_tool_call_events, responses_api_tool_calls_events, stream_error_events,
    truncated_reply_events,
};
use crate::tools::PickedToolCall;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ModelReply {
    ToolCall {
        call_id: String,
        call: PickedToolCall,
    },
    ToolCalls(Vec<(String, PickedToolCall)>),
    Text {
        text: String,
        usage: Option<UsageReport>,
    },
    ReasoningReply {
        reasoning: String,
        text: String,
        usage: Option<UsageReport>,
    },
    LoopingReply {
        reported: bool,
    },
    Refusal(StatusFailure),
    StreamError(StreamError),
    CutReply,
    ContentFilter,
    Dropped,
    Malformed,
    Hang,
    Empty,
    /// No finish reason, no terminal event, and no `[DONE]`.
    Truncated,
    Events(Vec<ModelEvent>),
    Scripted(EventParts),
}

impl ModelReply {
    pub(crate) fn into_response(
        self,
        endpoint: InferenceEndpoint,
        model: &str,
    ) -> ScriptedResponse {
        match self {
            ModelReply::ToolCall { call_id, call } => finish_parts(
                endpoint,
                model,
                EventParts {
                    reasoning: None,
                    text: None,
                    calls: vec![ScriptedCall {
                        call_id,
                        name: call.name,
                        arguments: call.arguments.to_string(),
                    }],
                    usage: None,
                },
                false,
            ),
            ModelReply::ToolCalls(calls) => finish_parts(
                endpoint,
                model,
                EventParts {
                    reasoning: None,
                    text: None,
                    calls: calls
                        .into_iter()
                        .map(|(call_id, call)| ScriptedCall {
                            call_id,
                            name: call.name,
                            arguments: call.arguments.to_string(),
                        })
                        .collect(),
                    usage: None,
                },
                false,
            ),
            ModelReply::Text { text, usage } => finish_parts(
                endpoint,
                model,
                EventParts {
                    reasoning: None,
                    text: Some(text),
                    calls: Vec::new(),
                    usage,
                },
                false,
            ),
            ModelReply::ReasoningReply {
                reasoning,
                text,
                usage,
            } => finish_parts(
                endpoint,
                model,
                EventParts {
                    reasoning: Some(reasoning),
                    // An empty visible answer is still a text item, not a reasoning-only turn.
                    text: Some(text),
                    calls: Vec::new(),
                    usage,
                },
                false,
            ),
            ModelReply::Empty => finish_parts(
                endpoint,
                model,
                EventParts {
                    reasoning: None,
                    text: None,
                    calls: Vec::new(),
                    usage: None,
                },
                false,
            ),
            ModelReply::Events(events) => {
                ScriptedResponse::sse(render_model_events(&events, endpoint, model))
            }
            ModelReply::Scripted(parts) => finish_parts(endpoint, model, parts, true),
            ModelReply::LoopingReply { reported } => {
                ScriptedResponse::sse(looping_reply_events(endpoint, model, reported))
            }
            ModelReply::StreamError(stream_error) => {
                ScriptedResponse::sse(stream_error_events(endpoint, &stream_error, model))
            }
            ModelReply::CutReply => {
                ScriptedResponse::sse(cut_reply_events(endpoint, CUT_REPLY, model))
            }
            ModelReply::ContentFilter => {
                ScriptedResponse::sse(content_filter_events(endpoint, CONTENT_FILTER_REPLY, model))
            }
            ModelReply::Truncated => {
                ScriptedResponse::sse(truncated_reply_events(endpoint, TRUNCATED_REPLY, model))
            }
            ModelReply::Refusal(failure) => failure.into_scripted_response(),
            ModelReply::Dropped => ScriptedResponse::dropped(),
            ModelReply::Malformed => ScriptedResponse::text(200, MALFORMED_SSE_BODY),
            ModelReply::Hang => ScriptedResponse::hang(),
        }
    }
}

fn finish_parts(
    endpoint: InferenceEndpoint,
    model: &str,
    parts: EventParts,
    scripted: bool,
) -> ScriptedResponse {
    let mut rendered = render_reply(endpoint, model, &parts);
    if scripted {
        // Spliced Responses frames reuse the tool-call builder's sequence numbers.
        if matches!(endpoint, InferenceEndpoint::Responses) {
            rendered = renumber_sequence_numbers(rendered);
        }
        // Reasoning-then-tool and reasoning-only builders bake in their own usage.
        if let Some(report) = &parts.usage {
            patch_usage(&mut rendered, report);
        }
    }
    ScriptedResponse::sse(rendered)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelEvent {
    kind: EventKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum EventKind {
    AssistantMessage {
        id: String,
        text: String,
    },
    FunctionCall {
        call_id: String,
        name: String,
        arguments: String,
    },
    Reasoning {
        id: String,
        summary: Vec<String>,
        content: Vec<String>,
    },
    Completed {
        id: String,
        total_tokens: Option<i64>,
    },
}

#[must_use]
pub fn ev_assistant_message(id: &str, text: &str) -> ModelEvent {
    ModelEvent {
        kind: EventKind::AssistantMessage {
            id: id.to_owned(),
            text: text.to_owned(),
        },
    }
}

#[must_use]
pub fn ev_function_call(call_id: &str, name: &str, arguments: &str) -> ModelEvent {
    ModelEvent {
        kind: EventKind::FunctionCall {
            call_id: call_id.to_owned(),
            name: name.to_owned(),
            arguments: arguments.to_owned(),
        },
    }
}

/// `summary` is the visible thought; `content` is the raw reasoning text.
#[must_use]
pub fn ev_reasoning_item(id: &str, summary: &[&str], content: &[&str]) -> ModelEvent {
    ModelEvent {
        kind: EventKind::Reasoning {
            id: id.to_owned(),
            summary: summary.iter().map(|text| (*text).to_owned()).collect(),
            content: content.iter().map(|text| (*text).to_owned()).collect(),
        },
    }
}

#[must_use]
pub fn ev_completed(id: &str) -> ModelEvent {
    ModelEvent {
        kind: EventKind::Completed {
            id: id.to_owned(),
            total_tokens: None,
        },
    }
}

/// `total_tokens` is reported as the input total.
#[must_use]
pub fn ev_completed_with_tokens(id: &str, total_tokens: i64) -> ModelEvent {
    ModelEvent {
        kind: EventKind::Completed {
            id: id.to_owned(),
            total_tokens: Some(total_tokens),
        },
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ScriptedCall {
    call_id: String,
    name: String,
    arguments: String,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct EventParts {
    reasoning: Option<String>,
    text: Option<String>,
    calls: Vec<ScriptedCall>,
    usage: Option<UsageReport>,
}

impl EventParts {
    fn from_events(events: &[ModelEvent]) -> Self {
        let mut reasoning = Vec::new();
        let mut texts = Vec::new();
        let mut calls = Vec::new();
        let mut usage = None;
        for event in events {
            match &event.kind {
                EventKind::AssistantMessage { id: _id, text } => texts.push(text.clone()),
                EventKind::FunctionCall {
                    call_id,
                    name,
                    arguments,
                } => calls.push(ScriptedCall {
                    call_id: call_id.clone(),
                    name: name.clone(),
                    arguments: arguments.clone(),
                }),
                EventKind::Reasoning {
                    id: _id,
                    summary,
                    content,
                } => {
                    let mut pieces = summary.clone();
                    pieces.extend(content.iter().cloned());
                    if !pieces.is_empty() {
                        reasoning.push(pieces.join(""));
                    }
                }
                EventKind::Completed {
                    id: _id,
                    total_tokens,
                } => {
                    if let Some(total) = total_tokens {
                        let prompt_tokens = u64::try_from((*total).max(0)).unwrap_or(0);
                        usage = Some(UsageReport {
                            prompt_tokens,
                            completion_tokens: 0,
                            cost_usd_ticks: None,
                        });
                    }
                }
            }
        }
        EventParts {
            reasoning: (!reasoning.is_empty()).then(|| reasoning.join("")),
            text: (!texts.is_empty()).then(|| texts.concat()),
            calls,
            usage,
        }
    }
}

fn render_reply(
    endpoint: InferenceEndpoint,
    model: &str,
    parts: &EventParts,
) -> Vec<crate::scripted::SseEvent> {
    match endpoint {
        InferenceEndpoint::ChatCompletions => render_chat(parts, model),
        InferenceEndpoint::Responses => render_responses(parts, model),
        InferenceEndpoint::Messages => render_messages(parts, model),
    }
}

pub(crate) fn render_model_events(
    events: &[ModelEvent],
    endpoint: InferenceEndpoint,
    model: &str,
) -> Vec<crate::scripted::SseEvent> {
    let ScriptedBody::Sse(rendered) = ModelReply::Scripted(EventParts::from_events(events))
        .into_response(endpoint, model)
        .body
    else {
        panic!("scripted model events render as SSE");
    };
    rendered
}

fn turn_calls(calls: &[ScriptedCall]) -> Vec<TurnCall<'_>> {
    calls
        .iter()
        .map(|call| TurnCall {
            call_id: &call.call_id,
            name: &call.name,
            arguments: &call.arguments,
        })
        .collect()
}

fn render_chat(parts: &EventParts, model: &str) -> Vec<crate::scripted::SseEvent> {
    let usage = parts.usage.as_ref();
    if let Some(reasoning) = &parts.reasoning {
        if parts.calls.is_empty() {
            return chat_completion_script_with_reasoning(
                reasoning,
                parts.text.as_deref().unwrap_or(""),
                model,
                usage,
            );
        }
        if parts.text.is_none()
            && parts.calls.len() == 1
            && let Some(call) = parts.calls.first()
        {
            return chat_completions_reasoning_then_tool_call_events(
                reasoning,
                &call.call_id,
                &call.name,
                &call.arguments,
                model,
            );
        }
    }
    if parts.reasoning.is_none() && parts.text.is_none() && !parts.calls.is_empty() {
        return chat_completion_tool_calls_events(&turn_calls(&parts.calls), model);
    }
    if parts.reasoning.is_none() && parts.calls.is_empty() {
        if let Some(text) = &parts.text {
            return chat_completion_script_from_deltas_with_usage(
                &chat_completion_deltas(text),
                model,
                "stop",
                usage,
            );
        }
        return chat_completions_no_content_events(model);
    }
    let base = if let Some(reasoning) = &parts.reasoning {
        chat_completion_script_with_reasoning(
            reasoning,
            parts.text.as_deref().unwrap_or(""),
            model,
            usage,
        )
    } else if let Some(text) = &parts.text {
        chat_completion_script_from_deltas_with_usage(
            &chat_completion_deltas(text),
            model,
            "stop",
            usage,
        )
    } else {
        chat_completions_no_content_events(model)
    };
    splice_payload(
        base,
        chat_completion_tool_calls_events(&turn_calls(&parts.calls), model),
    )
}

fn render_responses(parts: &EventParts, model: &str) -> Vec<crate::scripted::SseEvent> {
    let usage = parts.usage.as_ref();
    if let Some(reasoning) = &parts.reasoning {
        if parts.calls.is_empty() {
            if let Some(text) = &parts.text {
                return responses_api_reasoning_and_text_events_with_usage(
                    reasoning, text, model, usage,
                );
            }
            return responses_api_reasoning_only_events(reasoning, model);
        }
        if parts.text.is_none()
            && parts.calls.len() == 1
            && let Some(call) = parts.calls.first()
        {
            return responses_api_reasoning_then_tool_call_events(
                reasoning,
                &call.call_id,
                &call.name,
                &call.arguments,
                model,
            );
        }
    }
    if parts.reasoning.is_none() && parts.text.is_none() && !parts.calls.is_empty() {
        if let [call] = parts.calls.as_slice() {
            return responses_api_tool_call_events(ToolCallTurn {
                call_id: &call.call_id,
                name: &call.name,
                arguments: &call.arguments,
                model,
            });
        }
        return responses_api_tool_calls_events(&turn_calls(&parts.calls), model);
    }
    if parts.reasoning.is_none() && parts.calls.is_empty() {
        if let Some(text) = &parts.text {
            return match usage {
                Some(report) => responses_api_script_from_deltas_with_usage(
                    &responses_api_deltas(text),
                    text,
                    model,
                    Some(report),
                ),
                None => responses_api_script_exact(text, model),
            };
        }
        return responses_api_completed_only_events(model);
    }
    let base = if let (Some(reasoning), Some(text)) = (&parts.reasoning, &parts.text) {
        responses_api_reasoning_and_text_events_with_usage(reasoning, text, model, usage)
    } else if let Some(reasoning) = &parts.reasoning {
        responses_api_reasoning_only_events(reasoning, model)
    } else if let Some(text) = &parts.text {
        responses_api_script_exact(text, model)
    } else {
        responses_api_completed_only_events(model)
    };
    splice_payload(
        base,
        responses_api_tool_calls_events(&turn_calls(&parts.calls), model),
    )
}

fn render_messages(parts: &EventParts, model: &str) -> Vec<crate::scripted::SseEvent> {
    let usage = parts.usage.as_ref();
    if parts.calls.is_empty() {
        if let Some(reasoning) = &parts.reasoning {
            return messages_api_script_with_reasoning(
                reasoning,
                parts.text.as_deref().unwrap_or(""),
                model,
                "end_turn",
                usage,
            );
        }
        if let Some(text) = &parts.text {
            return messages_api_script_chunks(&[text.as_str()], model, "end_turn", usage);
        }
        return messages_api_no_content_events(model);
    }
    if parts.reasoning.is_none() && parts.text.is_none() {
        return messages_api_tool_uses_events(&turn_calls(&parts.calls), model);
    }
    let base = if let Some(reasoning) = &parts.reasoning {
        messages_api_script_with_reasoning(
            reasoning,
            parts.text.as_deref().unwrap_or(""),
            model,
            "end_turn",
            usage,
        )
    } else if let Some(text) = &parts.text {
        messages_api_script_chunks(&[text.as_str()], model, "end_turn", usage)
    } else {
        messages_api_no_content_events(model)
    };
    splice_payload(
        base,
        messages_api_tool_uses_events(&turn_calls(&parts.calls), model),
    )
}

fn splice_payload(
    base: Vec<crate::scripted::SseEvent>,
    extra: Vec<crate::scripted::SseEvent>,
) -> Vec<crate::scripted::SseEvent> {
    let inserted: Vec<_> = extra
        .into_iter()
        .filter(|event| !is_done(event) && !is_start(event) && !is_terminal(event))
        .collect();
    let mut out = Vec::new();
    let mut placed = false;
    for event in base {
        if !placed && (is_terminal(&event) || is_done(&event)) {
            out.extend(inserted.iter().cloned());
            placed = true;
        }
        out.push(event);
    }
    if !placed {
        out.extend(inserted);
    }
    out
}

fn is_done(event: &crate::scripted::SseEvent) -> bool {
    event.data == "[DONE]"
}

fn is_start(event: &crate::scripted::SseEvent) -> bool {
    event_type(event)
        .is_some_and(|kind| matches!(kind.as_str(), "response.created" | "message_start"))
}

fn is_terminal(event: &crate::scripted::SseEvent) -> bool {
    if is_done(event) {
        return true;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&event.data) else {
        return false;
    };
    if event_kind(&value).is_some_and(|kind| {
        matches!(
            kind,
            "response.completed" | "message_delta" | "message_stop"
        )
    }) {
        return true;
    }
    let delta = value.pointer("/choices/0/delta");
    let has_payload = delta.is_some_and(|delta| {
        delta.get("content").is_some()
            || delta.get("tool_calls").is_some()
            || delta.get("reasoning_content").is_some()
    });
    if value.get("usage").is_some() && !has_payload {
        return true;
    }
    let finish = value
        .pointer("/choices/0/finish_reason")
        .and_then(serde_json::Value::as_str);
    let delta_empty = delta
        .map(|delta| delta.as_object().is_some_and(serde_json::Map::is_empty) || delta.is_null())
        .unwrap_or(true);
    finish.is_some() && delta_empty
}

fn event_type(event: &crate::scripted::SseEvent) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(&event.data)
        .ok()
        .as_ref()
        .and_then(event_kind)
        .map(str::to_owned)
}

fn event_kind(value: &serde_json::Value) -> Option<&str> {
    value.get("type").and_then(serde_json::Value::as_str)
}

fn patch_usage(events: &mut [crate::scripted::SseEvent], report: &UsageReport) {
    for event in events {
        if event.data == "[DONE]" {
            continue;
        }
        let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&event.data) else {
            continue;
        };
        if patch_usage_value(&mut value, report) {
            event.data = value.to_string();
        }
    }
}

fn patch_usage_value(value: &mut serde_json::Value, report: &UsageReport) -> bool {
    match value {
        serde_json::Value::Array(items) => {
            let mut changed = false;
            for item in items {
                changed |= patch_usage_value(item, report);
            }
            changed
        }
        serde_json::Value::Object(map) => {
            let mut changed = false;
            if let Some(usage) = map.get_mut("usage")
                && usage.is_object()
            {
                write_usage(usage, report);
                changed = true;
            }
            let keys: Vec<String> = map
                .keys()
                .filter(|key| key.as_str() != "usage")
                .cloned()
                .collect();
            for key in keys {
                if let Some(child) = map.get_mut(&key) {
                    changed |= patch_usage_value(child, report);
                }
            }
            changed
        }
        serde_json::Value::Null
        | serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::String(_) => false,
    }
}

fn write_usage(usage: &mut serde_json::Value, report: &UsageReport) {
    let Some(map) = usage.as_object_mut() else {
        return;
    };
    let total = report.prompt_tokens + report.completion_tokens;
    if map.contains_key("prompt_tokens") {
        map.insert(
            "prompt_tokens".to_owned(),
            serde_json::json!(report.prompt_tokens),
        );
        map.insert(
            "completion_tokens".to_owned(),
            serde_json::json!(report.completion_tokens),
        );
        map.insert("total_tokens".to_owned(), serde_json::json!(total));
    }
    if map.contains_key("input_tokens") {
        map.insert(
            "input_tokens".to_owned(),
            serde_json::json!(report.prompt_tokens),
        );
        if map.contains_key("output_tokens") {
            map.insert(
                "output_tokens".to_owned(),
                serde_json::json!(report.completion_tokens),
            );
        }
        if map.contains_key("total_tokens") {
            map.insert("total_tokens".to_owned(), serde_json::json!(total));
        }
    }
}

#[cfg(test)]
#[path = "model_reply_tests.rs"]
mod tests;
