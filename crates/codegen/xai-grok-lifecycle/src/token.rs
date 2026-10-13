//! Validated wire tokens: handler names and trigger reasons.

use std::fmt;
use std::sync::Arc;

use crate::error::LifecycleError;

const MAX_HANDLER_NAME_BYTES: usize = 48;
const MAX_REASON_BYTES: usize = 32;

/// A handler name matching `^[a-z0-9][a-z0-9-]{0,47}$`. It is also the file stem of image manifests and persisted
/// registrations, so it can never contain a path separator or a dot.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HandlerName(Arc<str>);

impl TryFrom<&str> for HandlerName {
    type Error = LifecycleError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let bytes = value.as_bytes();
        let is_valid = bytes.len() <= MAX_HANDLER_NAME_BYTES
            && bytes
                .first()
                .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
            && bytes
                .iter()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-');
        if is_valid {
            Ok(HandlerName(Arc::from(value)))
        } else {
            Err(LifecycleError::InvalidHandlerName)
        }
    }
}

impl AsRef<str> for HandlerName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for HandlerName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A trigger reason matching `^[a-z][a-z0-9_]{0,31}$`. The broker never interprets it; handlers get it verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ReasonToken(Arc<str>);

impl TryFrom<&str> for ReasonToken {
    type Error = LifecycleError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let bytes = value.as_bytes();
        let is_valid = bytes.len() <= MAX_REASON_BYTES
            && bytes.first().is_some_and(u8::is_ascii_lowercase)
            && bytes
                .iter()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'_');
        if is_valid {
            Ok(ReasonToken(Arc::from(value)))
        } else {
            Err(LifecycleError::InvalidReason)
        }
    }
}

impl AsRef<str> for ReasonToken {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ReasonToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
#[path = "token_tests.rs"]
mod tests;
