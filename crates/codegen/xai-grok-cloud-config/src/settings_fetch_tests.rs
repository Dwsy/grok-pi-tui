use axum::Router;
use axum::http::StatusCode;
use axum::routing::get;

use super::*;

/// A mock cli-chat-proxy that serves `GET /settings` with a fixed status and body.
async fn start_settings_server(
    status: StatusCode,
    body: String,
) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock settings server");
    let base = format!(
        "http://127.0.0.1:{}",
        listener
            .local_addr()
            .expect("mock settings server address")
            .port()
    );

    let app = Router::new().route(
        "/settings",
        get(move || {
            let body = body.clone();
            async move { (status, body) }
        }),
    );

    let handle = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve mock settings")
    });
    (base, handle)
}

/// Only a 401 returns `Rejected`.
/// Every other failure returns `Retry`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settings_fetch_maps_status_to_outcome() {
    let auth = GrokAuth::test_default();
    let cases: [(StatusCode, &str, &str); 6] = [
        (StatusCode::OK, "{}", "Fetched"),
        (StatusCode::UNAUTHORIZED, "{}", "Rejected"),
        (StatusCode::FORBIDDEN, "{}", "Retry"), // 403 is ambiguous
        (StatusCode::TOO_MANY_REQUESTS, "{}", "Retry"),
        (StatusCode::INTERNAL_SERVER_ERROR, "{}", "Retry"),
        (StatusCode::OK, "not json", "Retry"),
    ];

    for (status, body, expected) in cases {
        let (base, server) = start_settings_server(status, body.to_string()).await;
        let auth = auth.clone();

        let outcome = tokio::task::spawn_blocking(move || {
            fetch_settings_blocking_with_attempts(&base, &auth, None, 1)
        })
        .await
        .expect("settings fetch task panicked");
        server.abort();

        let got = match outcome {
            SettingsFetch::Fetched(_) => "Fetched",
            SettingsFetch::Rejected => "Rejected",
            SettingsFetch::Retry => "Retry",
        };
        assert_eq!(got, expected, "status {status}, body {body:?}");
    }
}
