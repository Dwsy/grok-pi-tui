//! Grok's permission rule engine. The agent, the tools, and the session code stay out, so a
//! small consumer can link the rules alone.

#![deny(clippy::indexing_slicing)]

pub mod bash_command_splitting;
pub mod bash_permission_script;
pub mod claude_settings;
pub mod domain;
pub mod env_risk;
pub mod exec_risk;
pub mod folder_trust;
pub mod gate_preflight;
pub mod git_content_filters;
pub mod managed_policy;
pub mod policy;
pub mod project_config;
pub mod reasons;
pub mod repo;
pub mod resolution;
pub mod rules;
pub mod shell_access;
pub mod source_identity;
pub mod trust;
pub mod types;
pub mod util;
mod worktree;

#[cfg(test)]
mod test_support;
#[cfg(test)]
pub(crate) use test_support::{ENV_TEST_LOCK, TestEnvGuard, capturing_warn_logs};
