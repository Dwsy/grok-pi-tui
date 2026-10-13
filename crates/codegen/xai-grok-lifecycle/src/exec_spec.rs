//! A validated exec handler (argv plus optional timeout), shared by image manifests and runtime registrations.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::wire::{ErrorClass, HandlerSpecWire, WIRE_VERSION};

/// Cap on the summed byte length of all argv elements. The contract caps a PUT body and a manifest at the same 4 KiB,
/// so through those paths the body cap answers first (`body_too_large`) and usable argv is 4 KiB minus the JSON
/// framing. This cap binds for persisted records, whose file cap is larger.
pub(crate) const MAX_ARGV_BYTES: usize = 4096;

/// Shortest `timeout_ms` (and trigger `deadline_ms`) the contract accepts.
pub(crate) const MIN_TIMEOUT_MS: u64 = 100;

/// Longest `timeout_ms` (and trigger `deadline_ms`) the contract accepts.
pub(crate) const MAX_TIMEOUT_MS: u64 = 60_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExecSpec {
    /// Non-empty; element 0 is an absolute path. No element contains NUL.
    argv: Arc<[String]>,
    /// `None` runs until the trigger deadline.
    timeout_ms: Option<u64>,
}

impl ExecSpec {
    pub(crate) fn from_wire(wire: HandlerSpecWire) -> Result<Self, ErrorClass> {
        if wire.v != WIRE_VERSION {
            return Err(ErrorClass::UnsupportedVersion);
        }
        let argv_bytes: usize = wire.argv.iter().map(String::len).sum();
        let is_valid_argv = wire
            .argv
            .first()
            .is_some_and(|program| Path::new(program).is_absolute())
            && argv_bytes <= MAX_ARGV_BYTES
            && !wire.argv.iter().any(|arg| arg.contains('\0'));
        if !is_valid_argv {
            return Err(ErrorClass::InvalidArgv);
        }
        if wire
            .timeout_ms
            .is_some_and(|ms| !(MIN_TIMEOUT_MS..=MAX_TIMEOUT_MS).contains(&ms))
        {
            return Err(ErrorClass::InvalidTimeout);
        }
        Ok(ExecSpec {
            argv: Arc::from(wire.argv),
            timeout_ms: wire.timeout_ms,
        })
    }

    pub(crate) fn to_wire(&self) -> HandlerSpecWire {
        HandlerSpecWire {
            v: WIRE_VERSION,
            argv: self.argv.to_vec(),
            timeout_ms: self.timeout_ms,
        }
    }

    pub(crate) fn argv(&self) -> &[String] {
        &self.argv
    }

    pub(crate) fn timeout_ms(&self) -> Option<u64> {
        self.timeout_ms
    }

    pub(crate) fn timeout(&self) -> Option<Duration> {
        self.timeout_ms.map(Duration::from_millis)
    }
}

#[cfg(test)]
#[path = "exec_spec_tests.rs"]
mod tests;
