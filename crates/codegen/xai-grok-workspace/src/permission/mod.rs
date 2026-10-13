pub mod auto_mode;
pub use xai_grok_permission_rules::bash_command_splitting;
use xai_grok_permission_rules::bash_permission_script;
pub use xai_grok_permission_rules::claude_settings;
use xai_grok_permission_rules::exec_risk;
use xai_grok_permission_rules::gate_preflight;
mod grants;
mod hub_gate;
mod hub_permission;
mod managed_hooks;
pub mod managed_policy {
    pub use crate::permission::managed_hooks::disabled_hooks_snapshot;
    pub use xai_grok_permission_rules::managed_policy::*;
}
mod manager;
use xai_grok_permission_rules::policy;
mod prompter;
pub use xai_grok_permission_rules::reasons;
pub mod resolution {
    pub use crate::permission::managed_hooks::disabled_hooks_snapshot;
    pub use xai_grok_permission_rules::resolution::*;
}
pub use xai_grok_permission_rules::rules;
mod sandbox_gate;
mod sandbox_network;
pub mod sandbox_wire;
use xai_grok_permission_rules::shell_access;
mod state;
pub mod types;

macro_rules! wire_enum {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $($variant:ident => $wire:literal),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        $vis enum $name {
            $($variant),+
        }

        impl $name {
            $vis const ALL: &'static [Self] = &[$(Self::$variant),+];

            $vis const fn wire_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $wire),+
                }
            }
        }
    };
}
pub(crate) use wire_enum;

pub use auto_mode::{
    AUTO_MODE_CLASSIFIER_SYSTEM_PROMPT, AutoFastPath, BashSecurityAssessment,
    CLASSIFIER_TURN_MAX_LEN, ClassifierContext, ClassifierFailure, ClassifierMessage,
    ClassifierMessageRole, ClassifierOutcome, ClassifierPromptType, ClassifierSecurityFinding,
    ClassifierSource, ClassifierSourceKind, ClassifierTurn, ClassifierVerdict, ClassifyTextChannel,
    ClassifyTextFn, FixedClassifier, HeuristicPermissionClassifier, LlmPermissionClassifier,
    PermissionClassifier, SharedClassifier, access_requires_user_interaction, auto_mode_fast_path,
    build_classifier_messages, classifier_output_json_schema, default_auto_mode_classifier,
    is_auto_mode_allowlisted_access, is_auto_mode_allowlisted_tool_name,
    parse_classifier_model_output, parse_classifier_model_text, permission_decision_args,
};
pub use gate_preflight::GatePreflight;

#[cfg(test)]
pub(crate) use hub_gate::grant_store_access;
pub(crate) use hub_gate::{SessionApproval, approve_hub_call};
pub use hub_gate::{ToolApprovalGate, approval_gate_for};
pub use hub_permission::{
    PermissionHookTransport, ToolServerPermissionTransport, hitl_permission_live_enabled,
    prompt_outcome_allows, request_permission_via_hub,
};
pub(crate) use sandbox_gate::{
    SandboxPath, SettleContext, ViolationSettlement, refuse_mode_layer_write, settle_violation,
};

pub(crate) fn init_metrics() {
    hub_permission::init_metrics();
    hub_gate::init_metrics();
}

pub use grants::{
    always_allow_scope_persists, default_always_allow_scope, default_always_deny_scope,
    minimum_always_allow_scope,
};
pub use manager::{
    AUTO_DENY_CONSECUTIVE_LIMIT, AUTO_DENY_TOTAL_LIMIT, PROMPT_POLICY_DENY_REASON,
    PermissionHandle, broad_allow_floor_requires_prompt, spawn_permission_manager,
    spawn_permission_manager_with_hub, spawn_permission_manager_with_pin,
};
pub use policy::{
    CompiledPolicy, bash_glob_is_catchall, bash_pattern_is_broad, bash_pattern_matches_command,
};
pub use prompter::{
    ALLOW_EDITS_SESSION_OPTION_ID, AcpPrompter, BashCommandPermission, BashCommandSelectedTerms,
    ENABLE_ALWAYS_APPROVE_OPTION_ID, MCP_TOOL_NAME_DELIMITER, McpScopeSelection, McpToolPermission,
    PromptOutcome, PromptOutcomeKind, is_enable_always_approve_option,
    mcp_pretty_name_if_qualified, mcp_titleize_segment, mcp_tool_action, mcp_tool_display_name,
    tool_name_for_access as prompter_tool_name_for_access,
};
pub use sandbox_network::{
    GrantView, HoldAnswer, SandboxNetworkDecider, SandboxNetworkDeciderConfig, ViolationSink,
    WebFetchDomainFile, WebFetchDomains,
};
pub use shell_access::{ProtectedEditPermission, ProtectedEditReason};
pub use state::PermissionState;
pub use state::cleanup_stale_permission_state;
/// The folder's grant store, for tests of the hub path that seed a `permission.toml` row.
#[cfg(test)]
pub(crate) use state::{StateFileAccess, load_state_from_disk, persist_state};
pub use types::{
    AccessKind, ClientType, Decision, HOOK_ASK_META_KEY, HookAsk, PermissionCommand,
    PermissionEvent, PermissionRequest, PermissionResolution,
};
