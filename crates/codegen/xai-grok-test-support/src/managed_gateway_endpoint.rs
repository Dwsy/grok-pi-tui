//! Mock managed MCP gateway (grok.com connectors): `GET /v1/mcp/tools/list` serves a scripted
//! catalog and `POST /v1/mcp/tools/call` records each call and answers a scripted result.
//!
//! Both routes answer 404 until a script is set, so a run that never opts in sees no connectors.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, RwLock};

use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::request_log::authorization_header;

/// One `POST /v1/mcp/tools/call` the mock answered.
#[derive(Debug, Clone, PartialEq)]
pub struct ManagedGatewayCall {
    /// The catalog row's `call_id` the client named.
    pub call_id: String,
    /// `Value::Null` when the body carried none.
    pub arguments: Value,
    pub authorization: Option<String>,
}

#[derive(Clone)]
struct GatewayScript {
    catalog: Value,
    call_result: Value,
}

#[derive(Default)]
pub(crate) struct ManagedGatewayEndpointState {
    script: RwLock<Option<GatewayScript>>,
    calls: Mutex<Vec<ManagedGatewayCall>>,
    observed: AtomicUsize,
}

impl ManagedGatewayEndpointState {
    pub(crate) fn set_script(&self, catalog: Value, call_result: Value) {
        *self.script.write().unwrap() = Some(GatewayScript {
            catalog,
            call_result,
        });
    }

    pub(crate) fn calls(&self) -> Vec<ManagedGatewayCall> {
        self.calls.lock().unwrap().clone()
    }

    /// Calls answered since the previous call. [`Self::calls`] stays the whole log.
    pub(crate) fn take_for_observation(&self) -> Vec<ManagedGatewayCall> {
        let calls = self.calls.lock().unwrap();
        let start = self.observed.load(Ordering::SeqCst);
        let fresh: Vec<ManagedGatewayCall> = calls.iter().skip(start).cloned().collect();
        self.observed.store(calls.len(), Ordering::SeqCst);
        fresh
    }

    pub(crate) fn list(&self) -> Response {
        match self.script.read().unwrap().as_ref() {
            Some(script) => axum::Json(script.catalog.clone()).into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        }
    }

    pub(crate) fn call(&self, headers: &HeaderMap, body: &Value) -> Response {
        let Some(script) = self.script.read().unwrap().clone() else {
            return StatusCode::NOT_FOUND.into_response();
        };
        self.calls.lock().unwrap().push(ManagedGatewayCall {
            call_id: body
                .get("call_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            arguments: body.get("arguments").cloned().unwrap_or(Value::Null),
            authorization: authorization_header(headers),
        });
        axum::Json(json!({
            "result": script.call_result,
            "connectors_needing_reauth": [],
        }))
        .into_response()
    }
}

#[allow(clippy::disallowed_methods)] // test clients hit localhost mocks
#[cfg(test)]
#[path = "managed_gateway_endpoint_tests.rs"]
mod tests;
