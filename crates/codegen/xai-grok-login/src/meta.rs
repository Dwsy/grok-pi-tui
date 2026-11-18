//! Auth metadata wire types; canonical definitions live in
//! `xai_grok_shared::auth_types` so the pager can render auth state without
//! the login runtime in its dependency graph. Re-exported here for every
//! existing shell-side caller (`xai_grok_login::AuthMeta` / `GateInfo` keep
//! resolving unchanged).
pub use xai_grok_shared::auth_types::{AuthMeta, GateInfo};
