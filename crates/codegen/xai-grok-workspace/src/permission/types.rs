use agent_client_protocol as acp;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;
pub use xai_grok_permission_rules::types::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionEvent {
    pub tool_id: String,
    pub tool_name: String,
    pub access_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_detail: Option<String>,
    pub yolo_mode: bool,
    pub auto_approved: bool,
    pub user_prompted: bool,
    pub decision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_outcome: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reject_reason: Option<String>,
    pub timestamp: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classifier_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classifier_latency_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_denials_consecutive: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_denials_total: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_depth: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub security_findings: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classifier_verdict: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remember_tool_approvals: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct PermissionResolution {
    pub decision: Decision,
    pub event: Option<PermissionEvent>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum ClientType {
    #[default]
    #[serde(rename = "generic", alias = "grok-shell", alias = "grok_shell")]
    Generic,
    #[serde(rename = "grok-tui", alias = "grok_tui")]
    GrokTUI,
    #[serde(rename = "grok_web")]
    GrokWeb,
    #[serde(rename = "nebula")]
    Nebula,
    #[serde(rename = "extension")]
    Extension,
    #[serde(rename = "grok-pager", alias = "grok_pager")]
    GrokPager,
    #[serde(rename = "grok_desktop")]
    Desktop,
}

impl ClientType {
    pub fn user_agent_label(&self) -> &'static str {
        match self {
            Self::Generic => "grok-shell",
            Self::GrokTUI => "grok-tui",
            Self::GrokWeb => "grok-web",
            Self::Nebula => "nebula",
            Self::Extension => "grok-code-extension",
            Self::GrokPager => "grok-pager",
            Self::Desktop => "grok-desktop",
        }
    }

    pub fn from_client_identifier(id: Option<&str>) -> Self {
        match id {
            Some("grok-web") => Self::GrokWeb,
            Some("nebula") => Self::Nebula,
            Some("grok-code-extension") => Self::Extension,
            Some("grok-desktop") => Self::Desktop,
            Some("grok-pager") => Self::GrokPager,
            _ => Self::Generic,
        }
    }

    pub fn feedback_label(&self) -> &'static str {
        match self {
            Self::GrokTUI | Self::GrokPager => "tui",
            Self::GrokWeb => "web",
            Self::Nebula => "nebula",
            Self::Extension => "extension",
            Self::Generic => "agent",
            Self::Desktop => "desktop",
        }
    }

    pub const fn can_present_permission_prompt(self) -> bool {
        !matches!(self, Self::Generic)
    }
}

#[derive(Debug, Clone)]
pub struct RequestPathContext {
    pub real_cwd: std::path::PathBuf,
    pub display_cwd: Option<std::path::PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookAsk {
    pub hook_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

pub const HOOK_ASK_META_KEY: &str = "hookAsk";

const HOOK_ASK_SEPARATOR: &str = " — ";

impl HookAsk {
    pub fn ask_line(&self) -> String {
        let hook_name = &self.hook_name;
        let reason = self.reason.as_deref().unwrap_or_default();
        let reason = reason.split_whitespace().collect::<Vec<_>>().join(" ");
        if reason.is_empty() {
            format!("hook '{hook_name}' asks for confirmation")
        } else {
            format!("hook '{hook_name}' asks: {reason}")
        }
    }

    pub fn prompt_header(&self, action: &str) -> String {
        format!("{action}{HOOK_ASK_SEPARATOR}{}", self.ask_line())
    }

    pub fn strip_prompt_header<'a>(&self, title: &'a str) -> &'a str {
        title
            .strip_suffix(self.ask_line().as_str())
            .and_then(|action| action.strip_suffix(HOOK_ASK_SEPARATOR))
            .unwrap_or(title)
    }
}

#[derive(Debug, Clone)]
pub struct PermissionRequest {
    pub access: AccessKind,
    pub tool_call_update: acp::ToolCallUpdate,
    pub path_context: Option<RequestPathContext>,
    pub session_id: Option<String>,
    pub subagent_type: Option<String>,
    pub subagent_description: Option<String>,
    pub hook_ask: Option<HookAsk>,
}

impl PermissionRequest {
    pub fn new(access: AccessKind, tool_call_update: acp::ToolCallUpdate) -> Self {
        Self {
            access,
            tool_call_update,
            path_context: None,
            session_id: None,
            subagent_type: None,
            subagent_description: None,
            hook_ask: None,
        }
    }
}

#[allow(clippy::large_enum_variant)]
pub enum PermissionCommand {
    Request {
        request: PermissionRequest,
        respond_to: oneshot::Sender<PermissionResolution>,
    },
    SetYoloMode(bool),
    SetAutoMode(bool),
    SetClassifier(Option<std::sync::Arc<dyn super::auto_mode::PermissionClassifier>>),
    SetClassifierTranscript(Vec<super::auto_mode::ClassifierTurn>),
    SetProjectInstructions(Option<String>),
    ResetState,
    Shutdown,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_ask_header_keeps_the_action_and_names_the_hook() {
        let with_reason = HookAsk {
            hook_name: "guard".to_owned(),
            reason: Some("confirm this".to_owned()),
        };
        let header = with_reason.prompt_header("Run `deploy`");
        assert_eq!(header, "Run `deploy` — hook 'guard' asks: confirm this");
        assert_eq!(with_reason.strip_prompt_header(&header), "Run `deploy`");
        let bare = HookAsk {
            hook_name: "guard".to_owned(),
            reason: None,
        };
        assert_eq!(
            bare.prompt_header("Run `deploy`"),
            "Run `deploy` — hook 'guard' asks for confirmation"
        );
        let blank = HookAsk {
            hook_name: "guard".to_owned(),
            reason: Some("  \n".to_owned()),
        };
        assert_eq!(blank.ask_line(), bare.ask_line());
        let multiline = HookAsk {
            hook_name: "guard".to_owned(),
            reason: Some("confirm\nthis".to_owned()),
        };
        assert_eq!(multiline.ask_line(), with_reason.ask_line());
    }

    #[test]
    fn permission_event_subagent_fields_default_to_none() {
        let json = r#"{
            "tool_id": "tc1",
            "tool_name": "bash",
            "access_kind": "bash",
            "yolo_mode": false,
            "auto_approved": false,
            "user_prompted": true,
            "decision": "allow",
            "timestamp": "2026-03-24T00:00:00Z"
        }"#;
        let event: PermissionEvent = serde_json::from_str(json).unwrap();
        assert!(event.subagent_session_id.is_none());
        assert!(event.subagent_type.is_none());
        assert!(event.subagent_description.is_none());
        assert!(event.permission_mode.is_none());
        assert!(event.decision_reason.is_none());
        assert!(event.classifier_source.is_none());
        assert!(event.classifier_latency_ms.is_none());
        assert!(event.auto_denials_consecutive.is_none());
        assert!(event.auto_denials_total.is_none());
        assert!(event.wait_ms.is_none());
        assert!(event.queue_depth.is_none());
        assert!(event.security_findings.is_none());
        assert!(event.classifier_verdict.is_none());
    }

    #[test]
    fn permission_event_findings_none_vs_some_empty_are_distinct() {
        let base = r#"{
            "tool_id": "tc1",
            "tool_name": "bash",
            "access_kind": "bash",
            "yolo_mode": false,
            "auto_approved": false,
            "user_prompted": true,
            "decision": "allow",
            "timestamp": "2026-03-24T00:00:00Z",
            "security_findings": [],
            "classifier_verdict": "block"
        }"#;
        let event: PermissionEvent = serde_json::from_str(base).unwrap();
        assert_eq!(event.security_findings.as_deref(), Some([].as_slice()));
        assert_eq!(event.classifier_verdict.as_deref(), Some("block"));

        let with_tokens: PermissionEvent = serde_json::from_str(&base.replace(
            "\"security_findings\": []",
            "\"security_findings\": [\"opaque_shell\"]",
        ))
        .unwrap();
        assert_eq!(
            with_tokens.security_findings.as_deref(),
            Some(["opaque_shell".to_owned()].as_slice())
        );
    }

    #[test]
    fn permission_event_with_subagent_attribution() {
        let event = PermissionEvent {
            tool_id: "tc1".into(),
            tool_name: "bash".into(),
            access_kind: "bash".into(),
            access_detail: None,
            yolo_mode: false,
            auto_approved: false,
            user_prompted: true,
            decision: "allow".into(),
            prompt_outcome: None,
            reject_reason: None,
            timestamp: Utc::now(),
            subagent_session_id: Some("child-1".into()),
            subagent_type: Some("explore".into()),
            subagent_description: Some("Find endpoints".into()),
            permission_mode: Some("ask".into()),
            decision_reason: Some("needs_user".into()),
            classifier_source: Some("llm".into()),
            classifier_latency_ms: Some(42),
            auto_denials_consecutive: Some(2),
            auto_denials_total: Some(5),
            wait_ms: Some(1234),
            queue_depth: Some(3),
            security_findings: Some(vec!["opaque_shell".into()]),
            classifier_verdict: Some("block".into()),
            remember_tool_approvals: Some(true),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(
            json.get("subagent_session_id").and_then(|v| v.as_str()),
            Some("child-1")
        );
        assert_eq!(
            json.get("subagent_type").and_then(|v| v.as_str()),
            Some("explore")
        );
        assert_eq!(
            json.get("subagent_description").and_then(|v| v.as_str()),
            Some("Find endpoints")
        );
        assert_eq!(
            json.get("permission_mode").and_then(|v| v.as_str()),
            Some("ask")
        );
        assert_eq!(
            json.get("decision_reason").and_then(|v| v.as_str()),
            Some("needs_user")
        );
        assert_eq!(
            json.get("classifier_source").and_then(|v| v.as_str()),
            Some("llm")
        );
        assert_eq!(
            json.get("classifier_latency_ms"),
            Some(&serde_json::json!(42))
        );
        assert_eq!(
            json.get("auto_denials_consecutive"),
            Some(&serde_json::json!(2))
        );
        assert_eq!(json.get("auto_denials_total"), Some(&serde_json::json!(5)));
        assert_eq!(json.get("wait_ms"), Some(&serde_json::json!(1234)));
        assert_eq!(json.get("queue_depth"), Some(&serde_json::json!(3)));
        assert_eq!(
            json.get("security_findings")
                .and_then(|v| v.get(0))
                .and_then(|v| v.as_str()),
            Some("opaque_shell")
        );
        assert_eq!(
            json.get("classifier_verdict").and_then(|v| v.as_str()),
            Some("block")
        );
        assert_eq!(
            json.get("remember_tool_approvals"),
            Some(&serde_json::json!(true))
        );
    }

    #[test]
    fn permission_event_skips_none_optional_fields() {
        let event = PermissionEvent {
            tool_id: "tc1".into(),
            tool_name: "bash".into(),
            access_kind: "bash".into(),
            access_detail: None,
            yolo_mode: false,
            auto_approved: true,
            user_prompted: false,
            decision: "allow".into(),
            prompt_outcome: None,
            reject_reason: None,
            timestamp: Utc::now(),
            subagent_session_id: None,
            subagent_type: None,
            subagent_description: None,
            permission_mode: None,
            decision_reason: None,
            classifier_source: None,
            classifier_latency_ms: None,
            auto_denials_consecutive: None,
            auto_denials_total: None,
            wait_ms: None,
            queue_depth: None,
            security_findings: None,
            classifier_verdict: None,
            remember_tool_approvals: None,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(!json.contains("subagent_session_id"));
        assert!(!json.contains("subagent_type"));
        assert!(!json.contains("permission_mode"));
        assert!(!json.contains("decision_reason"));
        assert!(!json.contains("classifier_source"));
        assert!(!json.contains("classifier_latency_ms"));
        assert!(!json.contains("auto_denials_consecutive"));
        assert!(!json.contains("auto_denials_total"));
        assert!(!json.contains("wait_ms"));
        assert!(!json.contains("queue_depth"));
        assert!(!json.contains("security_findings"));
        assert!(!json.contains("classifier_verdict"));
        assert!(!json.contains("remember_tool_approvals"));
    }

    #[test]
    fn client_type_deserializes_grok_shell_as_generic() {
        assert_eq!(
            serde_json::from_value::<ClientType>("grok-shell".into()).unwrap(),
            ClientType::Generic,
        );
        assert_eq!(
            serde_json::from_value::<ClientType>("grok_shell".into()).unwrap(),
            ClientType::Generic,
        );
        assert_eq!(
            serde_json::from_value::<ClientType>("generic".into()).unwrap(),
            ClientType::Generic,
        );
    }
}
