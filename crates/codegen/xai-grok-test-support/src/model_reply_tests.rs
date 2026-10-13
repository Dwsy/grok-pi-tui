use serde_json::{Value, json};

use super::ModelReply;
use crate::failure::{StatusFailure, TRUNCATED_REPLY};
use crate::inference_request::InferenceEndpoint;
use crate::scripted::ScriptedBody;
use crate::sse::UsageReport;
use crate::tools::PickedToolCall;

fn read_call() -> ModelReply {
    ModelReply::ToolCall {
        call_id: "call_mock_1_1".to_owned(),
        call: PickedToolCall {
            name: "read_file".to_owned(),
            arguments: json!({ "target_file": "a.rs" }),
        },
    }
}

fn sse_frames(reply: ModelReply, endpoint: InferenceEndpoint) -> Vec<Value> {
    let ScriptedBody::Sse(events) = reply.into_response(endpoint, "m").body else {
        panic!("reply renders as SSE");
    };
    events
        .iter()
        .filter(|event| event.data != "[DONE]")
        .map(|event| serde_json::from_str(&event.data).unwrap())
        .collect()
}

#[test]
fn each_endpoint_renders_the_reply_in_its_own_format() {
    let cases: [(InferenceEndpoint, ModelReply, &str, &str); 6] = [
        (
            InferenceEndpoint::ChatCompletions,
            read_call(),
            "/0/choices/0/delta/tool_calls/0/function/name",
            "read_file",
        ),
        (
            InferenceEndpoint::Responses,
            read_call(),
            "/5/response/output/0/name",
            "read_file",
        ),
        (
            InferenceEndpoint::Messages,
            read_call(),
            "/1/content_block/name",
            "read_file",
        ),
        (
            InferenceEndpoint::ChatCompletions,
            ModelReply::Text {
                text: "REPLY".to_owned(),
                usage: None,
            },
            "/0/choices/0/delta/content",
            "REPLY",
        ),
        (
            InferenceEndpoint::Responses,
            ModelReply::Text {
                text: "REPLY".to_owned(),
                usage: None,
            },
            "/1/delta",
            "REPLY",
        ),
        (
            InferenceEndpoint::Messages,
            ModelReply::Text {
                text: "REPLY".to_owned(),
                usage: None,
            },
            "/2/delta/text",
            "REPLY",
        ),
    ];
    for (endpoint, reply, pointer, expected) in cases {
        let found = Value::Array(sse_frames(reply, endpoint))
            .pointer(pointer)
            .and_then(Value::as_str)
            .map(str::to_owned);
        assert_eq!(Some(expected.to_owned()), found, "{endpoint:?} {pointer}");
    }
}

#[test]
fn parallel_calls_render_every_call_in_one_response_in_order_in_each_format() {
    let cases: [(InferenceEndpoint, [&str; 2]); 3] = [
        (
            InferenceEndpoint::ChatCompletions,
            [
                "/0/choices/0/delta/tool_calls/0/id",
                "/0/choices/0/delta/tool_calls/1/id",
            ],
        ),
        (
            InferenceEndpoint::Responses,
            [
                "/9/response/output/0/call_id",
                "/9/response/output/1/call_id",
            ],
        ),
        (
            InferenceEndpoint::Messages,
            ["/1/content_block/id", "/4/content_block/id"],
        ),
    ];
    for (endpoint, pointers) in cases {
        let reply = ModelReply::ToolCalls(vec![
            (
                "call_mock_1_1".to_owned(),
                PickedToolCall {
                    name: "read_file".to_owned(),
                    arguments: json!({ "target_file": "a.rs" }),
                },
            ),
            (
                "call_mock_1_2".to_owned(),
                PickedToolCall {
                    name: "run_terminal_command".to_owned(),
                    arguments: json!({ "command": "ls" }),
                },
            ),
        ]);

        let frames = Value::Array(sse_frames(reply, endpoint));
        let ids = pointers.map(|pointer| frames.pointer(pointer).and_then(Value::as_str));

        assert_eq!(
            [Some("call_mock_1_1"), Some("call_mock_1_2")],
            ids,
            "{endpoint:?}"
        );
    }
}

#[test]
fn reasoning_reply_streams_the_thought_in_each_format() {
    let cases: [(InferenceEndpoint, &str, &str); 3] = [
        (
            InferenceEndpoint::ChatCompletions,
            "/0/choices/0/delta/reasoning_content",
            "THINK",
        ),
        (
            InferenceEndpoint::Responses,
            "/3/response/output/0/summary/0/text",
            "THINK",
        ),
        (InferenceEndpoint::Messages, "/2/delta/thinking", "THINK"),
    ];
    for (endpoint, pointer, expected) in cases {
        let reply = ModelReply::ReasoningReply {
            reasoning: "THINK".to_owned(),
            text: "REPLY".to_owned(),
            usage: None,
        };
        let found = Value::Array(sse_frames(reply, endpoint))
            .pointer(pointer)
            .and_then(Value::as_str)
            .map(str::to_owned);
        assert_eq!(Some(expected.to_owned()), found, "{endpoint:?} {pointer}");
    }
}

#[test]
fn truncated_reply_ends_on_its_text_delta_in_each_format() {
    let cases: [(InferenceEndpoint, &str, Value); 4] = [
        (
            InferenceEndpoint::ChatCompletions,
            "/choices/0/delta/content",
            json!(TRUNCATED_REPLY),
        ),
        (
            InferenceEndpoint::ChatCompletions,
            "/choices/0/finish_reason",
            Value::Null,
        ),
        (
            InferenceEndpoint::Responses,
            "/delta",
            json!(TRUNCATED_REPLY),
        ),
        (
            InferenceEndpoint::Messages,
            "/delta/text",
            json!(TRUNCATED_REPLY),
        ),
    ];
    for (endpoint, pointer, expected) in cases {
        let ScriptedBody::Sse(events) = ModelReply::Truncated.into_response(endpoint, "m").body
        else {
            panic!("a truncated reply renders as SSE");
        };
        let last = events.last().expect("a truncated reply has frames");
        let found = serde_json::from_str::<Value>(&last.data)
            .ok()
            .and_then(|frame| frame.pointer(pointer).cloned());
        assert_eq!(Some(expected), found, "{endpoint:?} last frame {pointer}");
    }
}

#[test]
fn empty_reply_completes_with_no_content_in_each_format() {
    let cases: [(InferenceEndpoint, &str, Value); 3] = [
        (
            InferenceEndpoint::ChatCompletions,
            "/0/choices/0/delta",
            json!({ "role": "assistant" }),
        ),
        (
            InferenceEndpoint::Responses,
            "/1/response/output",
            json!([]),
        ),
        (
            InferenceEndpoint::Messages,
            "/1/type",
            json!("message_delta"),
        ),
    ];
    for (endpoint, pointer, expected) in cases {
        let found = Value::Array(sse_frames(ModelReply::Empty, endpoint))
            .pointer(pointer)
            .cloned();
        assert_eq!(Some(expected), found, "{endpoint:?} {pointer}");
    }
}

#[test]
fn refusal_without_a_body_serves_the_default_error_json() {
    let response = ModelReply::Refusal(StatusFailure::new(500))
        .into_response(InferenceEndpoint::ChatCompletions, "m");
    let ScriptedBody::Json(body) = &response.body else {
        panic!("a refusal without a body is served as JSON");
    };
    assert_eq!(
        (
            500,
            &json!({ "error": { "message": "mock upstream failure 500", "type": "server_error" } })
        ),
        (response.status, body)
    );
}

#[test]
fn messages_text_reply_carries_the_scripted_token_counts() {
    let frames = Value::Array(sse_frames(
        ModelReply::Text {
            text: "OK".to_owned(),
            usage: Some(UsageReport {
                prompt_tokens: 1234,
                completion_tokens: 56,
                cost_usd_ticks: Some(250_000_000),
            }),
        },
        InferenceEndpoint::Messages,
    ));
    assert_eq!(
        Some(json!(1234)),
        frames.pointer("/0/message/usage/input_tokens").cloned()
    );
    assert_eq!(
        Some(json!({"output_tokens": 56, "input_tokens": 1234})),
        frames.pointer("/4/usage").cloned()
    );
}

fn rendered(endpoint: InferenceEndpoint) -> Vec<Value> {
    let events = vec![
        super::ev_reasoning_item("reason-1", &["think"], &["raw"]),
        super::ev_assistant_message("msg-1", "hello-body"),
        super::ev_function_call("call-9", "shell_tool", "{\"cmd\":\"ls\"}"),
        super::ev_completed_with_tokens("done-1", 42),
    ];
    super::render_model_events(&events, endpoint, "m")
        .into_iter()
        .filter(|event| event.data != "[DONE]")
        .map(|event| serde_json::from_str(&event.data).unwrap())
        .collect()
}

#[test]
fn events_render_reasoning_text_call_and_tokens() {
    assert_eq!(
        serde_json::from_str::<Vec<Value>>(r#"[{"choices":[{"delta":{"reasoning_content":"thinkraw","role":"assistant"},"finish_reason":null,"index":0}],"created":1234567890,"id":"chatcmpl-test","model":"m","object":"chat.completion.chunk"},{"choices":[{"delta":{"content":"hello-body"},"finish_reason":"stop","index":0}],"created":1234567890,"id":"chatcmpl-test","model":"m","object":"chat.completion.chunk"},{"choices":[{"delta":{"content":null,"role":"assistant","tool_calls":[{"function":{"arguments":"{\"cmd\":\"ls\"}","name":"shell_tool"},"id":"call-9","index":0,"type":"function"}]},"finish_reason":null,"index":0}],"created":1234567890,"id":"chatcmpl-test","model":"m","object":"chat.completion.chunk"},{"choices":[],"created":1234567890,"id":"chatcmpl-test","model":"m","object":"chat.completion.chunk","usage":{"completion_tokens":0,"prompt_tokens":42,"total_tokens":42}}]"#).unwrap(),
        rendered(InferenceEndpoint::ChatCompletions)
    );
    assert_eq!(
        serde_json::from_str::<Vec<Value>>(r#"[{"response":{"created_at":1234567890,"id":"resp_test","model":"m","object":"response","output":[],"status":"in_progress"},"sequence_number":0,"type":"response.created"},{"delta":"thinkraw ","item_id":"reasoning_item_1","output_index":0,"sequence_number":1,"summary_index":0,"type":"response.reasoning_summary_text.delta"},{"content_index":0,"delta":"hello-body ","item_id":"item_test","output_index":1,"sequence_number":2,"type":"response.output_text.delta"},{"item":{"arguments":"","call_id":"call-9","id":"fc_call-9","name":"shell_tool","status":"in_progress","type":"function_call"},"output_index":0,"sequence_number":3,"type":"response.output_item.added"},{"delta":"{\"cmd\":\"ls\"}","item_id":"fc_call-9","output_index":0,"sequence_number":4,"type":"response.function_call_arguments.delta"},{"arguments":"{\"cmd\":\"ls\"}","item_id":"fc_call-9","name":"shell_tool","output_index":0,"sequence_number":5,"type":"response.function_call_arguments.done"},{"item":{"arguments":"{\"cmd\":\"ls\"}","call_id":"call-9","id":"fc_call-9","name":"shell_tool","status":"completed","type":"function_call"},"output_index":0,"sequence_number":6,"type":"response.output_item.done"},{"response":{"created_at":1234567890,"id":"resp_test","model":"m","object":"response","output":[{"id":"reasoning_item_1","status":"completed","summary":[{"text":"thinkraw","type":"summary_text"}],"type":"reasoning"},{"content":[{"annotations":[],"text":"hello-body","type":"output_text"}],"id":"msg_test","role":"assistant","status":"completed","type":"message"}],"status":"completed","usage":{"input_tokens":42,"input_tokens_details":{"cached_tokens":0},"output_tokens":0,"output_tokens_details":{"reasoning_tokens":0},"total_tokens":42}},"sequence_number":7,"type":"response.completed"}]"#).unwrap(),
        rendered(InferenceEndpoint::Responses)
    );
    assert_eq!(
        serde_json::from_str::<Vec<Value>>(r#"[{"message":{"content":[],"id":"msg_test","model":"m","role":"assistant","stop_reason":null,"type":"message","usage":{"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"input_tokens":42,"output_tokens":0}},"type":"message_start"},{"content_block":{"signature":"","thinking":"","type":"thinking"},"index":0,"type":"content_block_start"},{"delta":{"thinking":"thinkraw","type":"thinking_delta"},"index":0,"type":"content_block_delta"},{"index":0,"type":"content_block_stop"},{"content_block":{"text":"","type":"text"},"index":1,"type":"content_block_start"},{"delta":{"text":"hello-body","type":"text_delta"},"index":1,"type":"content_block_delta"},{"index":1,"type":"content_block_stop"},{"content_block":{"id":"call-9","input":{},"name":"shell_tool","type":"tool_use"},"index":0,"type":"content_block_start"},{"delta":{"partial_json":"{\"cmd\":\"ls\"}","type":"input_json_delta"},"index":0,"type":"content_block_delta"},{"index":0,"type":"content_block_stop"},{"delta":{"stop_reason":"end_turn"},"type":"message_delta","usage":{"input_tokens":42,"output_tokens":0}},{"type":"message_stop"}]"#).unwrap(),
        rendered(InferenceEndpoint::Messages)
    );
}
