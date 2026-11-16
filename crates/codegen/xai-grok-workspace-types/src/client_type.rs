//! Shared client identity and user-agent classification.
//!
//! Kept below Workspace so HTTP, Login, Memory and permission handling do
//! not need the full tool and permission runtime just to name a client.

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_names_and_http_labels_remain_identical() {
        assert_eq!(
            serde_json::from_str::<ClientType>("\"grok_shell\"").unwrap(),
            ClientType::Generic
        );
        assert_eq!(
            serde_json::from_str::<ClientType>("\"grok_pager\"").unwrap(),
            ClientType::GrokPager
        );
        assert_eq!(
            serde_json::to_string(&ClientType::GrokPager).unwrap(),
            "\"grok-pager\""
        );
        assert_eq!(ClientType::GrokPager.user_agent_label(), "grok-pager");
        assert_eq!(
            ClientType::from_client_identifier(Some("grok-pager")),
            ClientType::GrokPager
        );
    }
}
