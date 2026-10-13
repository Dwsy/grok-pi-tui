use crate::broker::MAX_BUILTIN_HANDLERS;
use crate::token::HandlerName;

/// Errors from the broker's public API.
#[derive(Debug, thiserror::Error)]
pub enum LifecycleError {
    #[error("handler name must match ^[a-z0-9][a-z0-9-]{{0,47}}$")]
    InvalidHandlerName,
    #[error("reason must match ^[a-z][a-z0-9_]{{0,31}}$")]
    InvalidReason,
    #[error("a built-in handler named {name} is already registered")]
    DuplicateBuiltin { name: HandlerName },
    #[error("at most {MAX_BUILTIN_HANDLERS} built-in handlers can be registered")]
    TooManyBuiltins,
}

pub type Result<T> = std::result::Result<T, LifecycleError>;
