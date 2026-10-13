//! HTTP surface of the broker; the diagnostics server serves it next to its own routes.
//!
//! Bodies are read by hand with a hard cap so an oversized body is a `400` with a JSON error, as the contract asks,
//! rather than axum's default `413`.
//!
//! A `500 persist_failed` from PUT or DELETE can mean the record write timed out and still lands later, in the store and
//! the table together. Both are idempotent by name, so a client retries them safely.

use std::time::Duration;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use serde::Serialize;
use tokio::time::Instant;

use crate::broker::{IMAGE_SCAN_TIMEOUT, LifecycleBroker, TriggerBusy};
use crate::exec_spec::{ExecSpec, MIN_TIMEOUT_MS};
use crate::image::is_image_name;
use crate::registry::RegistryError;
use crate::token::HandlerName;
use crate::trigger::{HandlerKind, TriggerRequest, encode_trigger_response};
use crate::wire::{
    ErrorClass, ErrorWire, HandlerListWire, HandlerSpecWire, ListedHandlerWire, RegisteredWire,
    WIRE_VERSION,
};

/// Cap on the trigger body.
pub(crate) const MAX_TRIGGER_BODY_BYTES: usize = 1024;

/// Cap on a PUT body.
pub(crate) const MAX_REGISTRATION_BODY_BYTES: usize = 4096;

/// Bound on reading a registration body.
const BODY_READ_TIMEOUT: Duration = Duration::from_secs(1);

/// Bound on reading the trigger body. The trigger deadline starts at receipt, so the read gets no more than the
/// smallest budget a trigger can carry; a slow body then never pushes the answer past the deadline epsilon.
const TRIGGER_BODY_READ_TIMEOUT: Duration = Duration::from_millis(MIN_TIMEOUT_MS);

pub(crate) fn router(broker: LifecycleBroker) -> Router {
    Router::new()
        .route("/lifecycle/pre-stop", post(trigger))
        .route("/lifecycle/handlers", get(list))
        .route(
            "/lifecycle/handlers/{name}",
            put(register).delete(unregister),
        )
        .with_state(broker)
}

async fn trigger(State(broker): State<LifecycleBroker>, body: Body) -> Response {
    let received = Instant::now();
    let parsed = read_body(body, MAX_TRIGGER_BODY_BYTES, TRIGGER_BODY_READ_TIMEOUT)
        .await
        .and_then(|bytes| TriggerRequest::parse(&bytes));
    let request = match parsed {
        Ok(request) => request,
        Err(class) => return error_response(StatusCode::BAD_REQUEST, class),
    };
    match broker.trigger(request, received).await {
        Ok(report) => match encode_trigger_response(&report) {
            Ok(body) => json_bytes(StatusCode::OK, body),
            Err(e) => {
                tracing::warn!(error = %e, "encoding the pre-stop response failed");
                error_response(StatusCode::INTERNAL_SERVER_ERROR, ErrorClass::Internal)
            }
        },
        Err(TriggerBusy) => error_response(StatusCode::CONFLICT, ErrorClass::Busy),
    }
}

async fn list(State(broker): State<LifecycleBroker>) -> Response {
    let table = broker
        .handler_table(Instant::now() + IMAGE_SCAN_TIMEOUT)
        .await;
    let handlers = table
        .iter()
        .map(|entry| {
            let spec = match &entry.kind {
                HandlerKind::Exec(spec) => Some(spec),
                HandlerKind::Builtin(_) => None,
            };
            ListedHandlerWire {
                name: entry.name.as_ref(),
                source: entry.source,
                argv: spec.map(ExecSpec::argv),
                timeout_ms: spec.and_then(ExecSpec::timeout_ms),
            }
        })
        .collect();
    json_response(
        StatusCode::OK,
        &HandlerListWire {
            v: WIRE_VERSION,
            handlers,
        },
    )
}

async fn register(
    State(broker): State<LifecycleBroker>,
    Path(name): Path<String>,
    body: Body,
) -> Response {
    let Ok(name) = HandlerName::try_from(name.as_str()) else {
        return error_response(StatusCode::BAD_REQUEST, ErrorClass::InvalidName);
    };
    let parsed = read_body(body, MAX_REGISTRATION_BODY_BYTES, BODY_READ_TIMEOUT)
        .await
        .and_then(|bytes| {
            serde_json::from_slice::<HandlerSpecWire>(&bytes).map_err(|e| {
                tracing::debug!(error = %e, "invalid lifecycle registration body");
                ErrorClass::InvalidBody
            })
        })
        .and_then(ExecSpec::from_wire);
    let spec = match parsed {
        Ok(spec) => spec,
        Err(class) => return error_response(StatusCode::BAD_REQUEST, class),
    };
    if is_reserved(&broker, &name).await {
        return error_response(StatusCode::CONFLICT, ErrorClass::ReservedName);
    }
    match broker.registry().put(name.clone(), spec).await {
        Ok(()) => {
            tracing::info!(handler = %name, "lifecycle handler registered");
            json_response(
                StatusCode::OK,
                &RegisteredWire {
                    v: WIRE_VERSION,
                    name: name.as_ref(),
                },
            )
        }
        Err(RegistryError::Full) => {
            error_response(StatusCode::CONFLICT, ErrorClass::TooManyHandlers)
        }
        Err(RegistryError::Persist(e)) => {
            tracing::warn!(handler = %name, error = %e, "persisting a lifecycle registration failed");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, ErrorClass::PersistFailed)
        }
    }
}

async fn unregister(State(broker): State<LifecycleBroker>, Path(name): Path<String>) -> Response {
    let Ok(name) = HandlerName::try_from(name.as_str()) else {
        return error_response(StatusCode::BAD_REQUEST, ErrorClass::InvalidName);
    };
    if is_reserved(&broker, &name).await {
        return error_response(StatusCode::CONFLICT, ErrorClass::ReservedName);
    }
    match broker.registry().delete(&name).await {
        Ok(()) => {
            tracing::info!(handler = %name, "lifecycle handler unregistered");
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => {
            tracing::warn!(handler = %name, error = %e, "deleting a lifecycle registration failed");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, ErrorClass::PersistFailed)
        }
    }
}

/// A built-in name, or a name with an image manifest file (valid or not).
async fn is_reserved(broker: &LifecycleBroker, name: &HandlerName) -> bool {
    broker.is_builtin(name)
        || is_image_name(
            broker.image_dir(),
            name,
            Instant::now() + IMAGE_SCAN_TIMEOUT,
        )
        .await
}

async fn read_body(body: Body, limit: usize, timeout: Duration) -> Result<Bytes, ErrorClass> {
    match tokio::time::timeout(timeout, axum::body::to_bytes(body, limit)).await {
        Ok(Ok(bytes)) => Ok(bytes),
        // The length limit is the only read error a client can still observe; a broken connection reads nothing.
        Ok(Err(e)) => {
            tracing::debug!(error = %e, limit, "lifecycle request body rejected");
            Err(ErrorClass::BodyTooLarge)
        }
        Err(_elapsed) => Err(ErrorClass::InvalidBody),
    }
}

fn error_response(status: StatusCode, error: ErrorClass) -> Response {
    json_response(
        status,
        &ErrorWire {
            v: WIRE_VERSION,
            error,
        },
    )
}

fn json_response(status: StatusCode, body: &impl Serialize) -> Response {
    match serde_json::to_vec(body) {
        Ok(bytes) => json_bytes(status, bytes),
        Err(e) => {
            tracing::warn!(error = %e, "encoding a lifecycle response failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

fn json_bytes(status: StatusCode, body: Vec<u8>) -> Response {
    (status, [(header::CONTENT_TYPE, "application/json")], body).into_response()
}

#[cfg(test)]
#[path = "routes_tests.rs"]
mod tests;
