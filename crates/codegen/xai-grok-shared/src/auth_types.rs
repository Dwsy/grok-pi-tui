//! Auth metadata wire types shared between the shell and the pager.
//!
//! `xai_grok_login` remains the owner of auth *flows* (AuthManager, refresh,
//! device-code, token storage); these two structs are the pure metadata the
//! pager renders. They live here so the Pager can display auth state without
//! pulling the login runtime (jsonwebtoken, xai-grok-memory, xai-grok-sampler)
//! into its dependency graph. `xai_grok_login::meta` re-exports them for
//! compatibility with existing callers.

use serde::{Deserialize, Serialize};

/// Access gate from `grok_build_access_gate`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateInfo {
    pub message: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
}

/// Typed auth metadata passed from the shell to the pager via ACP.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthMeta {
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub auth_mode: Option<String>,
    /// Team principal UUID when the session is a team login (`None` for personal).
    #[serde(default)]
    pub team_id: Option<String>,
    #[serde(default)]
    pub team_name: Option<String>,
    #[serde(default)]
    pub is_zdr: bool,
    #[serde(default)]
    pub team_role: Option<String>,
    /// Defaults to opted-out (safer) until auth meta is populated.
    #[serde(default = "default_coding_data_retention_opt_out")]
    pub coding_data_retention_opt_out: bool,
    #[serde(default)]
    pub show_resolved_model: Option<bool>,
    /// `Some` means the user is blocked; `None` means the user has access.
    #[serde(default)]
    pub gate: Option<GateInfo>,
    /// Display name for the current subscription tier (e.g. "SuperGrok Heavy", "X Premium", "Free"), from CCP `/settings`.
    #[serde(default)]
    pub subscription_tier: Option<String>,
    /// Whether `/feedback` may offer a one-shot trace upload; it lives on auth meta so it refreshes with auth changes.
    #[serde(default)]
    pub feedback_trace_offer: bool,
}

/// Same value as `xai_grok_login`'s retained helper; duplicated locally so
/// this module stays dependency-free of the login crate.
fn default_coding_data_retention_opt_out() -> bool {
    true
}

impl Default for AuthMeta {
    fn default() -> Self {
        Self {
            email: None,
            auth_mode: None,
            team_id: None,
            team_name: None,
            is_zdr: false,
            team_role: None,
            coding_data_retention_opt_out: default_coding_data_retention_opt_out(),
            show_resolved_model: None,
            gate: None,
            subscription_tier: None,
            feedback_trace_offer: false,
        }
    }
}
