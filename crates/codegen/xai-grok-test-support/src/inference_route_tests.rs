use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::body::Bytes;
use axum::http::HeaderMap;
use serde_json::json;

use super::{InferenceRoute, OPEN_HOLD_GAP};
use crate::inference_override::InferenceOverrides;
use crate::inference_request::InferenceEndpoint;
use crate::request_log::RequestLog;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_hold_parks_exactly_one_of_two_auxiliary_requests() {
    let log = Arc::new(RequestLog::new());
    let overrides = InferenceOverrides::new(None);
    let route = InferenceRoute::new(Arc::clone(&log), overrides.clone());
    route.set_auxiliary_hold();
    OPEN_HOLD_GAP.store(true, Ordering::SeqCst);
    let _close_gap = CloseHoldGap;

    let started = Arc::new(std::sync::Barrier::new(2));
    let finished = Arc::new(AtomicUsize::new(0));
    let runtime = tokio::runtime::Handle::current();
    let mut joins = Vec::new();
    for index in 0..2 {
        let route = route.clone();
        let started = Arc::clone(&started);
        let finished = Arc::clone(&finished);
        let runtime = runtime.clone();
        joins.push(std::thread::spawn(move || {
            runtime.block_on(async move {
                started.wait();
                let body = json!({
                    "messages": [{ "role": "user", "content": format!("side {index}") }]
                });
                let raw = Bytes::from(serde_json::to_vec(&body).expect("auxiliary body"));
                route
                    .serve(
                        InferenceEndpoint::ChatCompletions,
                        None,
                        HeaderMap::new(),
                        raw,
                    )
                    .await;
                finished.fetch_add(1, Ordering::SeqCst);
            });
        }));
    }

    let parked = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::select! {
            _ = overrides.wait_until_parked_replies(1) => {
                match tokio::time::timeout(
                    Duration::from_millis(300),
                    overrides.wait_until_parked_replies(2),
                )
                .await
                {
                    Ok(()) => 2,
                    Err(_) => 1,
                }
            }
            _ = async {
                while finished.load(Ordering::SeqCst) < 2 {
                    tokio::task::yield_now().await;
                }
            } => 0,
        }
    })
    .await
    .expect("auxiliary requests did not finish deciding the hold");
    assert_eq!(1, parked, "one hold parked {parked} auxiliary requests");
    overrides.release_parked_replies();
    for join in joins {
        join.join().expect("auxiliary request");
    }
    assert_eq!(2, finished.load(Ordering::SeqCst));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prefix_hold_parks_the_matching_request_and_leaves_another_alone() {
    let log = Arc::new(RequestLog::new());
    let overrides = InferenceOverrides::new(None);
    let route = InferenceRoute::new(Arc::clone(&log), overrides.clone());
    route.set_auxiliary_hold_matching("xai-btw-");

    let mut other_headers = HeaderMap::new();
    other_headers.insert(
        axum::http::HeaderName::from_static("x-grok-req-id"),
        axum::http::HeaderValue::from_static("xai-title-refresh-1"),
    );
    let other = json!({ "messages": [{ "role": "user", "content": "title" }] });
    let other_raw = Bytes::from(serde_json::to_vec(&other).expect("other body"));
    tokio::time::timeout(
        Duration::from_secs(2),
        route.clone().serve(
            InferenceEndpoint::ChatCompletions,
            None,
            other_headers,
            other_raw,
        ),
    )
    .await
    .expect("a different auxiliary must not take the prefix hold");

    let mut side_headers = HeaderMap::new();
    side_headers.insert(
        axum::http::HeaderName::from_static("x-grok-req-id"),
        axum::http::HeaderValue::from_static("xai-btw-1"),
    );
    let side = json!({ "messages": [{ "role": "user", "content": "side" }] });
    let side_raw = Bytes::from(serde_json::to_vec(&side).expect("side body"));
    let route_for_side = route.clone();
    let side_task = tokio::spawn(async move {
        route_for_side
            .serve(
                InferenceEndpoint::ChatCompletions,
                None,
                side_headers,
                side_raw,
            )
            .await;
    });
    tokio::time::timeout(
        Duration::from_secs(2),
        overrides.wait_until_matching_parked(1),
    )
    .await
    .expect("matching auxiliary did not park");
    assert!(
        tokio::time::timeout(Duration::from_millis(200), async {
            while !side_task.is_finished() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .is_err(),
        "matching auxiliary must stay parked"
    );
    overrides.release_parked_replies();
    side_task.await.expect("matching auxiliary");
}

struct CloseHoldGap;

impl Drop for CloseHoldGap {
    fn drop(&mut self) {
        OPEN_HOLD_GAP.store(false, Ordering::SeqCst);
    }
}
