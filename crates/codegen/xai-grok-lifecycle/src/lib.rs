//! Pre-stop lifecycle broker for the workspace server.
//!
//! The host calls `POST /lifecycle/pre-stop` on the diagnostics socket right before it stops the VM. The broker fans
//! that event out, concurrently and under one absolute deadline, to every handler it knows: built-in handlers that
//! components register in-process with [`LifecycleBroker::register_builtin`], image handlers declared by JSON
//! manifests under [`DEFAULT_IMAGE_HANDLER_DIR`], and handlers any process in the VM registers at runtime with
//! `PUT /lifecycle/handlers/{name}`. The broker holds no consumer logic of its own.
//!
//! Invariants:
//! - One trigger runs at a time. A concurrent trigger gets `409`; nothing queues.
//! - A trigger answers by its absolute deadline plus at most 250 ms. Handlers still running are cut (exec handlers
//!   get SIGTERM, then SIGKILL, on their whole process group) and reported `timed_out`.
//! - Every externally influenced input has a named cap: request bodies, argv bytes, the registered and image handler
//!   counts, the exec log, and the trigger response.
//! - Exec handler output goes to a capped log file, never into a response.
//! - Registrations survive a workspace-server restart within one kernel boot and are dropped across boots.

#![deny(clippy::indexing_slicing)]
#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)
)]

mod broker;
mod error;
mod event;
mod exec;
mod exec_log;
mod exec_spec;
mod image;
mod persist;
mod registry;
mod routes;
mod token;
mod trigger;
mod wire;

#[cfg(test)]
mod test_support;

pub use broker::{DEFAULT_IMAGE_HANDLER_DIR, LifecycleBroker, LifecycleConfig};
pub use error::{LifecycleError, Result};
pub use event::{Disk, HandlerFuture, HandlerOutcome, PreStopEvent, PreStopHandler};
pub use token::{HandlerName, ReasonToken};
