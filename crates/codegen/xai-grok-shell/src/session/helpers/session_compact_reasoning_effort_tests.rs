use super::*;
use crate::sampling::{Client, ConversationItem, SamplerConfig};
use axum::Router;
use axum::body::Bytes;
use axum::response::IntoResponse;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::post;
use futures_util::stream;
use serde_json::json;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use xai_grok_sampling_types::ReasoningEffort;

const SUMMARY: &str = "<summary>ok</summary>";

fn chat_completions_stream() -> Vec<Event> {
    vec![
        Event::default().data(
            json!({
                "id": "chatcmpl-test", "object": "chat.completion.chunk",
                "created": 1234567890, "model": "test-model",
                "choices": [{
                    "index": 0,
                    "delta": { "role": "assistant", "content": SUMMARY },
                    "finish_reason": "stop"
                }]
            })
            .to_string(),
        ),
        Event::default().data("[DONE]"),
    ]
}

fn responses_stream() -> Vec<Event> {
    let response = |status: &str| {
        json!({
            "id": "resp_test", "object": "response", "created_at": 1234567890,
            "model": "test-model", "status": status, "output": []
        })
    };
    [
        json!({ "type": "response.created", "sequence_number": 0, "response": response("in_progress") }),
        json!({
            "type": "response.output_text.delta", "sequence_number": 1,
            "item_id": "msg_test", "output_index": 0, "content_index": 0, "delta": SUMMARY
        }),
        json!({ "type": "response.completed", "sequence_number": 2, "response": response("completed") }),
    ]
    .into_iter()
    .map(|event| Event::default().data(event.to_string()))
    .collect()
}

fn messages_stream() -> Vec<Event> {
    [
        json!({
            "type": "message_start",
            "message": {
                "id": "msg_test", "type": "message", "role": "assistant",
                "content": [], "model": "test-model", "stop_reason": null,
                "usage": {
                    "input_tokens": 10, "output_tokens": 0,
                    "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0
                }
            }
        }),
        json!({ "type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""} }),
        json!({ "type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": SUMMARY} }),
        json!({ "type": "content_block_stop", "index": 0 }),
        json!({
            "type": "message_delta",
            "delta": {"stop_reason": "end_turn"},
            "usage": {"output_tokens": 5, "input_tokens": 10}
        }),
        json!({ "type": "message_stop" }),
    ]
    .into_iter()
    .map(|event| Event::default().data(event.to_string()))
    .collect()
}

/// Runs one compaction against a mock server for `backend` and returns the parsed request body.
async fn compaction_request_body(
    backend: ApiBackend,
    reasoning_effort: Option<ReasoningEffort>,
) -> serde_json::Value {
    let (path, events): (&str, fn() -> Vec<Event>) = match backend {
        ApiBackend::ChatCompletions => ("/v1/chat/completions", chat_completions_stream),
        ApiBackend::Responses => ("/v1/responses", responses_stream),
        ApiBackend::Messages => ("/v1/messages", messages_stream),
    };
    let captured = Arc::new(Mutex::new(None::<serde_json::Value>));
    let cap = captured.clone();
    let app = Router::new().route(
        path,
        post(move |body: Bytes| {
            let cap = cap.clone();
            async move {
                *cap.lock().unwrap() = Some(serde_json::from_slice(&body).unwrap());
                let stream =
                    stream::iter(events().into_iter().map(Ok::<_, std::convert::Infallible>));
                Sse::new(stream)
                    .keep_alive(KeepAlive::default())
                    .into_response()
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
            .unwrap();
    });

    let config = SamplerConfig {
        api_key: Some("test-api-key".into()),
        base_url: format!("http://{addr}/v1"),
        model: "test-model".into(),
        max_completion_tokens: Some(1000),
        temperature: Some(0.7),
        api_backend: backend.clone(),
        context_window: 256_000,
        reasoning_effort,
        ..Default::default()
    };
    let client = Client::new(config.clone()).unwrap();
    let chat_history = vec![
        ConversationItem::system("You are a helpful assistant."),
        ConversationItem::user("<user_query>\nfix the bug\n</user_query>"),
        ConversationItem::assistant("I fixed it."),
        ConversationItem::user("Summarize the conversation so far."),
    ];
    let output = generate_session_compact(
        chat_history,
        0,
        vec![],
        vec![],
        client,
        acp::SessionId::new("reasoning-effort-test"),
        &config,
        std::time::Duration::from_secs(30),
        0,
        crate::util::config::CompactionToolChoice::Auto,
        &tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap_or_else(|_| panic!("{backend:?} compaction must succeed"));
    assert_eq!(output.content, SUMMARY);

    let body = captured
        .lock()
        .unwrap()
        .take()
        .expect("compaction request must reach the mock server");
    let _ = shutdown_tx.send(());
    body
}

fn absent(body: &serde_json::Value, pointer: &str) -> bool {
    body.pointer(pointer).is_none_or(serde_json::Value::is_null)
}

#[tokio::test]
async fn chat_completions_compaction_sends_session_reasoning_effort() {
    let body =
        compaction_request_body(ApiBackend::ChatCompletions, Some(ReasoningEffort::Xhigh)).await;
    assert_eq!(
        body.pointer("/reasoning_effort"),
        Some(&json!("xhigh")),
        "{body:#}"
    );
}

#[tokio::test]
async fn chat_completions_compaction_omits_unset_reasoning_effort() {
    let body = compaction_request_body(ApiBackend::ChatCompletions, None).await;
    assert!(absent(&body, "/reasoning_effort"), "{body:#}");
}

#[tokio::test]
async fn responses_compaction_sends_session_reasoning_effort() {
    let body = compaction_request_body(ApiBackend::Responses, Some(ReasoningEffort::Xhigh)).await;
    assert_eq!(
        body.pointer("/reasoning/effort"),
        Some(&json!("xhigh")),
        "{body:#}"
    );
}

#[tokio::test]
async fn responses_compaction_omits_unset_reasoning_effort() {
    let body = compaction_request_body(ApiBackend::Responses, None).await;
    assert!(absent(&body, "/reasoning/effort"), "{body:#}");
}

#[tokio::test]
async fn messages_compaction_sends_session_reasoning_effort() {
    let body = compaction_request_body(ApiBackend::Messages, Some(ReasoningEffort::Xhigh)).await;
    assert_eq!(
        body.pointer("/output_config/effort"),
        Some(&json!("xhigh")),
        "{body:#}"
    );
    assert_eq!(
        body.pointer("/thinking/type"),
        Some(&json!("adaptive")),
        "{body:#}"
    );
}

#[tokio::test]
async fn messages_compaction_omits_unset_reasoning_effort() {
    let body = compaction_request_body(ApiBackend::Messages, None).await;
    assert!(absent(&body, "/output_config/effort"), "{body:#}");
    assert!(absent(&body, "/thinking"), "{body:#}");
}
