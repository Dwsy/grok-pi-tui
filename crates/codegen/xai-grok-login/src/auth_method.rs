//! First-party API-key environment primitives.
//!
//! They live in `xai-grok-shell-base::auth_config` because the Pi profile reads them while
//! building the ACP `auth_methods` list without linking the login runtime. The ACP
//! `auth_methods` surface (`build_auth_methods`, `AuthMethodsBuildInputs`, ...) stays in
//! `xai_grok_shell::agent::auth_method`, which depends on shell's `ModelEntry`.

pub use xai_grok_shell_base::auth_config::{
    LEGACY_XAI_API_KEY_ENV_VAR, XAI_API_KEY_ENV_VAR, has_xai_api_key_env, read_xai_api_key_env,
};
