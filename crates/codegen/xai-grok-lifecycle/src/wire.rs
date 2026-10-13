//! Wire shapes of the pre-stop hook contract v1 (frozen; the host is built against it).
//!
//! Every request type ignores unknown fields. Validation happens once, when a wire value is parsed into its domain
//! type; nothing downstream sees these structs.

use serde::{Deserialize, Serialize};

/// The only contract version this broker speaks.
pub(crate) const WIRE_VERSION: u64 = 1;

/// `POST /lifecycle/pre-stop` body.
#[derive(Debug, Deserialize)]
pub(crate) struct TriggerRequestWire {
    pub(crate) v: u64,
    pub(crate) reason: String,
    pub(crate) disk: String,
    pub(crate) deadline_ms: u64,
}

/// `PUT /lifecycle/handlers/{name}` body and the image manifest format.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct HandlerSpecWire {
    pub(crate) v: u64,
    pub(crate) argv: Vec<String>,
    /// `None` means "until the trigger deadline".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) timeout_ms: Option<u64>,
}

/// One persisted registration: the PUT body stamped with the kernel boot id it was registered under.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct PersistedHandlerWire {
    pub(crate) boot_id: String,
    #[serde(flatten)]
    pub(crate) spec: HandlerSpecWire,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Verdict {
    NoHandlers,
    Ok,
    Failed,
    TimedOut,
    Partial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, strum::IntoStaticStr)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub(crate) enum HandlerSource {
    Builtin,
    Image,
    Registered,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, strum::IntoStaticStr)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub(crate) enum OutcomeWire {
    Ok,
    Failed,
    TimedOut,
}

/// `200` body of the trigger.
#[derive(Debug, Serialize)]
pub(crate) struct TriggerResponseWire<'a> {
    pub(crate) v: u64,
    pub(crate) verdict: Verdict,
    pub(crate) elapsed_ms: u64,
    pub(crate) handlers: Vec<HandlerResultWire<'a>>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) truncated: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct HandlerResultWire<'a> {
    pub(crate) name: &'a str,
    pub(crate) source: HandlerSource,
    pub(crate) outcome: OutcomeWire,
    pub(crate) elapsed_ms: u64,
}

/// `200` body of `GET /lifecycle/handlers`.
#[derive(Debug, Serialize)]
pub(crate) struct HandlerListWire<'a> {
    pub(crate) v: u64,
    pub(crate) handlers: Vec<ListedHandlerWire<'a>>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ListedHandlerWire<'a> {
    pub(crate) name: &'a str,
    pub(crate) source: HandlerSource,
    /// Absent for built-in handlers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) argv: Option<&'a [String]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) timeout_ms: Option<u64>,
}

/// `200` body of a PUT.
#[derive(Debug, Serialize)]
pub(crate) struct RegisteredWire<'a> {
    pub(crate) v: u64,
    pub(crate) name: &'a str,
}

/// Body of every `4xx`/`5xx` the broker answers itself.
#[derive(Debug, Serialize)]
pub(crate) struct ErrorWire {
    pub(crate) v: u64,
    pub(crate) error: ErrorClass,
}

/// The `error` field of [`ErrorWire`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, strum::IntoStaticStr)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub(crate) enum ErrorClass {
    InvalidBody,
    BodyTooLarge,
    UnsupportedVersion,
    InvalidReason,
    InvalidDisk,
    InvalidDeadline,
    InvalidName,
    InvalidArgv,
    InvalidTimeout,
    Busy,
    ReservedName,
    TooManyHandlers,
    PersistFailed,
    Internal,
}
