use std::time::Duration;

use axum::http::{Method, Request};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use tower::ServiceExt;

use super::*;
use crate::registry::MAX_REGISTERED_HANDLERS;
use crate::test_support::{BOOT_A, FixedHandler, GateHandler, TestBroker};
use crate::trigger::MAX_RESPONSE_BYTES;

/// The contract's bound on answering past the trigger deadline.
const DEADLINE_EPSILON: Duration = Duration::from_millis(250);

async fn send(
    router: &Router,
    method: Method,
    uri: &str,
    body: impl Into<Body>,
) -> (StatusCode, Bytes) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(body.into())
        .expect("request");
    let response = router.clone().oneshot(request).await.expect("infallible");
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    (status, body)
}

fn parse(body: &[u8]) -> Value {
    if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(body).expect("json body")
    }
}

async fn trigger(router: &Router, body: &str) -> (StatusCode, Value) {
    let (status, body) = send(router, Method::POST, "/lifecycle/pre-stop", body.to_owned()).await;
    (status, parse(&body))
}

async fn put(router: &Router, name: &str, body: &str) -> (StatusCode, Value) {
    let (status, body) = send(
        router,
        Method::PUT,
        &format!("/lifecycle/handlers/{name}"),
        body.to_owned(),
    )
    .await;
    (status, parse(&body))
}

async fn delete(router: &Router, name: &str) -> (StatusCode, Value) {
    let (status, body) = send(
        router,
        Method::DELETE,
        &format!("/lifecycle/handlers/{name}"),
        Body::empty(),
    )
    .await;
    (status, parse(&body))
}

async fn list(router: &Router) -> Value {
    let (status, body) = send(router, Method::GET, "/lifecycle/handlers", Body::empty()).await;
    assert_eq!(StatusCode::OK, status);
    parse(&body)
}

fn error(class: &str) -> Value {
    json!({"v": 1, "error": class})
}

const TRUE_HANDLER: &str = r#"{"v":1,"argv":["/bin/sh","-c","exit 0"]}"#;

#[tokio::test]
async fn trigger_rejects_each_invalid_body_with_400() {
    let test = TestBroker::new();
    let router = test.broker.router();
    let oversized = format!(
        r#"{{"v":1,"reason":"manual","disk":"kept","deadline_ms":1000,"pad":"{}"}}"#,
        "x".repeat(MAX_TRIGGER_BODY_BYTES)
    );
    let cases = [
        ("not json", "invalid_body"),
        (r#"{"v":1,"reason":"manual","disk":"kept"}"#, "invalid_body"),
        (
            r#"{"v":2,"reason":"manual","disk":"kept","deadline_ms":1000}"#,
            "unsupported_version",
        ),
        (
            r#"{"v":1,"reason":"Manual","disk":"kept","deadline_ms":1000}"#,
            "invalid_reason",
        ),
        (
            r#"{"v":1,"reason":"idle-grace","disk":"kept","deadline_ms":1000}"#,
            "invalid_reason",
        ),
        (
            r#"{"v":1,"reason":"manual","disk":"gone","deadline_ms":1000}"#,
            "invalid_disk",
        ),
        (
            r#"{"v":1,"reason":"manual","disk":"kept","deadline_ms":99}"#,
            "invalid_deadline",
        ),
        (
            r#"{"v":1,"reason":"manual","disk":"kept","deadline_ms":60001}"#,
            "invalid_deadline",
        ),
        (
            r#"{"v":1,"reason":"manual","disk":"kept","deadline_ms":-5}"#,
            "invalid_body",
        ),
        (oversized.as_str(), "body_too_large"),
    ];
    for (body, class) in cases {
        assert_eq!(
            (StatusCode::BAD_REQUEST, error(class)),
            trigger(&router, body).await,
            "{body}"
        );
    }
}

#[tokio::test]
async fn trigger_without_handlers_answers_no_handlers() {
    let test = TestBroker::new();
    let (status, body) = trigger(
        &test.broker.router(),
        r#"{"v":1,"reason":"idle_grace","disk":"discarded","deadline_ms":10000}"#,
    )
    .await;
    assert_eq!(StatusCode::OK, status);
    assert_eq!(
        Some("no_handlers"),
        body.get("verdict").and_then(Value::as_str)
    );
    assert_eq!(Some(&json!([])), body.get("handlers"));
}

#[tokio::test]
async fn concurrent_trigger_gets_409_and_the_running_one_completes() {
    let test = TestBroker::new();
    let (started_tx, mut started_rx) = tokio::sync::mpsc::channel(1);
    let release = std::sync::Arc::new(tokio::sync::Notify::new());
    test.add_builtin(
        "gate",
        GateHandler {
            started: started_tx,
            release: std::sync::Arc::clone(&release),
        },
    );
    let router = test.broker.router();
    let body = r#"{"v":1,"reason":"manual","disk":"kept","deadline_ms":30000}"#;

    let first = tokio::spawn({
        let router = router.clone();
        async move { trigger(&router, body).await }
    });
    tokio::time::timeout(Duration::from_secs(10), started_rx.recv())
        .await
        .expect("first trigger started")
        .expect("gate alive");

    assert_eq!(
        (StatusCode::CONFLICT, error("busy")),
        trigger(&router, body).await
    );

    release.notify_one();
    let (status, first) = tokio::time::timeout(Duration::from_secs(10), first)
        .await
        .expect("first trigger finished")
        .expect("task");
    assert_eq!(StatusCode::OK, status);
    assert_eq!(Some("ok"), first.get("verdict").and_then(Value::as_str));
}

#[cfg(unix)]
#[tokio::test]
async fn slow_exec_handler_is_cut_and_the_answer_arrives_within_the_epsilon() {
    let test = TestBroker::new();
    test.register("stubborn", &["/bin/sh", "-c", "trap '' TERM; sleep 30"])
        .await;
    test.add_builtin("quick", FixedHandler::ok());
    let router = test.broker.router();

    let started = Instant::now();
    let (status, body) = trigger(
        &router,
        r#"{"v":1,"reason":"terminate","disk":"discarded","deadline_ms":300}"#,
    )
    .await;
    let elapsed = started.elapsed();

    assert_eq!(StatusCode::OK, status);
    assert!(
        elapsed < Duration::from_millis(300) + DEADLINE_EPSILON,
        "answered after {elapsed:?}"
    );
    assert_eq!(
        Some("timed_out"),
        body.get("verdict").and_then(Value::as_str)
    );
    let outcomes: Vec<(&str, &str)> = body
        .get("handlers")
        .and_then(Value::as_array)
        .expect("handlers")
        .iter()
        .filter_map(|entry| {
            Some((
                entry.get("name")?.as_str()?,
                entry.get("outcome")?.as_str()?,
            ))
        })
        .collect();
    assert_eq!(vec![("quick", "ok"), ("stubborn", "timed_out")], outcomes);
}

#[cfg(unix)]
#[tokio::test]
async fn response_stays_within_the_cap_with_many_handlers() {
    let test = TestBroker::new();
    for index in 0..16 {
        test.add_builtin(
            &format!("builtin-{index:02}-{}", "b".repeat(36)),
            FixedHandler::ok(),
        );
    }
    for index in 0..MAX_REGISTERED_HANDLERS {
        test.register(
            &format!("registered-{index:02}-{}", "r".repeat(33)),
            &["/bin/sh", "-c", "exit 0"],
        )
        .await;
    }

    let (status, body) = send(
        &test.broker.router(),
        Method::POST,
        "/lifecycle/pre-stop",
        r#"{"v":1,"reason":"manual","disk":"kept","deadline_ms":10000}"#,
    )
    .await;

    assert_eq!(StatusCode::OK, status);
    assert!(body.len() <= MAX_RESPONSE_BYTES, "{} bytes", body.len());
    let body = parse(&body);
    assert_eq!(Some("ok"), body.get("verdict").and_then(Value::as_str));
    assert_eq!(Some(true), body.get("truncated").and_then(Value::as_bool));
}

#[cfg(unix)]
#[tokio::test]
async fn put_is_idempotent_persists_and_survives_a_restart_in_the_same_boot() {
    let test = TestBroker::new();
    let router = test.broker.router();
    let first = r#"{"v":1,"argv":["/usr/local/bin/myapp","flush"],"timeout_ms":5000}"#;
    let second = r#"{"v":1,"argv":["/usr/local/bin/myapp","flush","--all"]}"#;

    assert_eq!(
        (StatusCode::OK, json!({"v": 1, "name": "myapp-flush"})),
        put(&router, "myapp-flush", first).await
    );
    assert_eq!(
        (StatusCode::OK, json!({"v": 1, "name": "myapp-flush"})),
        put(&router, "myapp-flush", second).await
    );
    let expected = json!({"v": 1, "handlers": [
        {"name": "myapp-flush", "source": "registered", "argv": ["/usr/local/bin/myapp", "flush", "--all"]},
    ]});
    assert_eq!(expected, list(&router).await);

    let record: Value = serde_json::from_slice(
        &std::fs::read(test.state_dir().join("myapp-flush.json")).expect("record"),
    )
    .expect("record json");
    assert_eq!(
        json!({"v": 1, "boot_id": BOOT_A, "argv": ["/usr/local/bin/myapp", "flush", "--all"]}),
        record
    );

    let restarted = test.restart(BOOT_A).router();
    assert_eq!(expected, list(&restarted).await);
}

#[cfg(unix)]
#[tokio::test]
async fn registrations_from_another_boot_are_dropped() {
    let test = TestBroker::new();
    assert_eq!(
        StatusCode::OK,
        put(&test.broker.router(), "myapp-flush", TRUE_HANDLER)
            .await
            .0
    );

    let restored = test.restart("boot-b").router();

    assert_eq!(json!({"v": 1, "handlers": []}), list(&restored).await);
    assert!(!test.state_dir().join("myapp-flush.json").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn delete_removes_the_registration_and_is_idempotent() {
    let test = TestBroker::new();
    let router = test.broker.router();
    assert_eq!(
        StatusCode::OK,
        put(&router, "myapp-flush", TRUE_HANDLER).await.0
    );

    assert_eq!(
        (StatusCode::NO_CONTENT, Value::Null),
        delete(&router, "myapp-flush").await
    );
    assert_eq!(
        (StatusCode::NO_CONTENT, Value::Null),
        delete(&router, "myapp-flush").await
    );
    assert_eq!(json!({"v": 1, "handlers": []}), list(&router).await);
    assert!(!test.state_dir().join("myapp-flush.json").exists());
    assert_eq!(
        json!({"v": 1, "handlers": []}),
        list(&test.restart(BOOT_A).router()).await
    );
}

#[cfg(unix)]
#[tokio::test]
async fn builtin_and_image_names_are_read_only() {
    let test = TestBroker::new();
    test.add_builtin("chrome-cookies", FixedHandler::ok());
    test.write_image_manifest(
        "baked.json",
        r#"{"v":1,"argv":["/bin/true"],"timeout_ms":1000}"#,
    );
    test.write_image_manifest("broken.json", "{not json");
    let router = test.broker.router();

    for name in ["chrome-cookies", "baked", "broken"] {
        assert_eq!(
            (StatusCode::CONFLICT, error("reserved_name")),
            put(&router, name, TRUE_HANDLER).await,
            "PUT {name}"
        );
        assert_eq!(
            (StatusCode::CONFLICT, error("reserved_name")),
            delete(&router, name).await,
            "DELETE {name}"
        );
    }
    assert_eq!(
        json!({"v": 1, "handlers": [
            {"name": "chrome-cookies", "source": "builtin"},
            {"name": "baked", "source": "image", "argv": ["/bin/true"], "timeout_ms": 1000},
        ]}),
        list(&router).await
    );
}

#[cfg(unix)]
#[tokio::test]
async fn seventeenth_new_registration_is_refused_but_replacement_is_not() {
    let test = TestBroker::new();
    let router = test.broker.router();
    for index in 0..MAX_REGISTERED_HANDLERS {
        assert_eq!(
            StatusCode::OK,
            put(&router, &format!("h{index:02}"), TRUE_HANDLER).await.0
        );
    }

    assert_eq!(
        (StatusCode::CONFLICT, error("too_many_handlers")),
        put(&router, "one-too-many", TRUE_HANDLER).await
    );
    assert_eq!(StatusCode::OK, put(&router, "h00", TRUE_HANDLER).await.0);
    assert_eq!(StatusCode::NO_CONTENT, delete(&router, "h00").await.0);
    assert_eq!(
        StatusCode::OK,
        put(&router, "one-too-many", TRUE_HANDLER).await.0
    );
}

#[tokio::test]
async fn put_rejects_invalid_names_and_bodies_with_400() {
    let test = TestBroker::new();
    let router = test.broker.router();
    let oversized = format!(
        r#"{{"v":1,"argv":["/bin/sh","{}"]}}"#,
        "x".repeat(MAX_REGISTRATION_BODY_BYTES)
    );
    let cases = [
        ("Bad_Name", TRUE_HANDLER, "invalid_name"),
        ("ok-name", "{", "invalid_body"),
        (
            "ok-name",
            r#"{"v":2,"argv":["/bin/true"]}"#,
            "unsupported_version",
        ),
        ("ok-name", r#"{"v":1,"argv":[]}"#, "invalid_argv"),
        ("ok-name", r#"{"v":1,"argv":["relative"]}"#, "invalid_argv"),
        (
            "ok-name",
            r#"{"v":1,"argv":["/bin/true"],"timeout_ms":50}"#,
            "invalid_timeout",
        ),
        ("ok-name", oversized.as_str(), "body_too_large"),
    ];
    for (name, body, class) in cases {
        assert_eq!(
            (StatusCode::BAD_REQUEST, error(class)),
            put(&router, name, body).await,
            "{name} {body}"
        );
    }
    assert_eq!(
        (StatusCode::BAD_REQUEST, error("invalid_name")),
        delete(&router, "Bad_Name").await
    );
}

#[cfg(unix)]
#[tokio::test]
async fn routes_work_over_a_unix_socket_through_the_diag_server() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use xai_grok_diag_server::{DiagHandle, DiagListener, DiagServeOptions};

    async fn request(sock: &std::path::Path, method: &str, path: &str, body: &str) -> (u16, Value) {
        let mut stream = tokio::net::UnixStream::connect(sock)
            .await
            .expect("connect");
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: ws\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(head.as_bytes()).await.expect("write head");
        stream.write_all(body.as_bytes()).await.expect("write body");
        let mut response = Vec::new();
        tokio::time::timeout(Duration::from_secs(20), stream.read_to_end(&mut response))
            .await
            .expect("response in time")
            .expect("read response");
        let response = String::from_utf8(response).expect("utf8 response");
        let (head, body) = response.split_once("\r\n\r\n").expect("header end");
        let status = head
            .split(' ')
            .nth(1)
            .and_then(|code| code.parse().ok())
            .expect("status code");
        (status, parse(body.as_bytes()))
    }

    let test = TestBroker::new();
    let sock = test.path("ws.sock");
    let _bound = xai_grok_diag_server::serve_with(
        DiagListener::Unix(sock.clone()),
        DiagHandle::new(Some("nonce-1".to_owned())),
        DiagServeOptions {
            log_file: None,
            extra_routes: test.broker.router(),
        },
    )
    .await
    .expect("bind");

    assert_eq!(
        (200, json!({"v": 1, "name": "myapp-flush"})),
        request(
            &sock,
            "PUT",
            "/lifecycle/handlers/myapp-flush",
            TRUE_HANDLER
        )
        .await
    );
    let (status, listed) = request(&sock, "GET", "/lifecycle/handlers", "").await;
    assert_eq!(200, status);
    assert_eq!(
        Some("myapp-flush"),
        listed.pointer("/handlers/0/name").and_then(Value::as_str)
    );

    let (status, body) = request(
        &sock,
        "POST",
        "/lifecycle/pre-stop",
        r#"{"v":1,"reason":"idle_grace","disk":"kept","deadline_ms":10000}"#,
    )
    .await;
    assert_eq!(200, status);
    assert_eq!(Some("ok"), body.get("verdict").and_then(Value::as_str));
    assert_eq!(
        Some("registered"),
        body.pointer("/handlers/0/source").and_then(Value::as_str)
    );

    assert_eq!(
        (204, Value::Null),
        request(&sock, "DELETE", "/lifecycle/handlers/myapp-flush", "").await
    );
    let (status, ready) = request(&sock, "GET", "/ready", "").await;
    assert_eq!(503, status);
    assert_eq!(
        Some("nonce-1"),
        ready.get("launch_id").and_then(Value::as_str)
    );
}

#[cfg(unix)]
#[tokio::test]
async fn dribbled_trigger_body_is_answered_within_the_smallest_budget_plus_the_epsilon() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use xai_grok_diag_server::{DiagHandle, DiagListener, DiagServeOptions};

    let test = TestBroker::new();
    let sock = test.path("ws.sock");
    let _bound = xai_grok_diag_server::serve_with(
        DiagListener::Unix(sock.clone()),
        DiagHandle::new(None),
        DiagServeOptions {
            log_file: None,
            extra_routes: test.broker.router(),
        },
    )
    .await
    .expect("bind");
    let body = r#"{"v":1,"reason":"manual","disk":"kept","deadline_ms":100}"#;
    let head = format!(
        "POST /lifecycle/pre-stop HTTP/1.1\r\nHost: ws\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    let mut stream = tokio::net::UnixStream::connect(&sock)
        .await
        .expect("connect");

    let started = Instant::now();
    stream.write_all(head.as_bytes()).await.expect("write head");
    stream
        .write_all(body.split_at(10).0.as_bytes())
        .await
        .expect("write the start of the body");
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(20), stream.read_to_end(&mut response))
        .await
        .expect("response in time")
        .expect("read response");
    let elapsed = started.elapsed();

    let response = String::from_utf8(response).expect("utf8 response");
    let (head, body) = response.split_once("\r\n\r\n").expect("header end");
    assert!(head.starts_with("HTTP/1.1 400 "), "{head}");
    assert_eq!(error("invalid_body"), parse(body.as_bytes()));
    assert!(
        elapsed < Duration::from_millis(MIN_TIMEOUT_MS) + DEADLINE_EPSILON,
        "answered after {elapsed:?}"
    );
}
