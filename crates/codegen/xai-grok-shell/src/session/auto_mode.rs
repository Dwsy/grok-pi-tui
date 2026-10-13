use agent_client_protocol as acp;
use xai_grok_telemetry::enums::PermissionMode;

use crate::util::config::auto_mode_session_active;

/// Whether a session asks to start in auto mode: `_meta.autoMode` (or `auto_mode`), else the default when yolo is off.
/// The auto gate is applied separately.
pub(crate) fn resolve_session_auto_mode(
    meta: Option<&acp::Meta>,
    default_auto_mode: bool,
    session_yolo_mode: bool,
) -> bool {
    meta.and_then(|m| m.get("autoMode").or_else(|| m.get("auto_mode")))
        .and_then(|v| v.as_bool())
        .unwrap_or(default_auto_mode && !session_yolo_mode)
}

/// The mode a session starts in, from its `_meta` and the launch default.
/// `auto_mode_enabled` is the auto gate. With it off, a session that asks for auto starts in ask.
pub fn session_permission_mode(
    meta: Option<&acp::Meta>,
    default: PermissionMode,
    auto_mode_enabled: bool,
) -> PermissionMode {
    let yolo = meta
        .and_then(|m| m.get("yoloMode"))
        .and_then(|v| v.as_bool())
        .unwrap_or(default.is_always_approve());
    let requested_auto = resolve_session_auto_mode(meta, default.is_auto(), yolo);
    let auto = auto_mode_session_active(auto_mode_enabled, requested_auto, yolo);
    PermissionMode::from_flags(yolo, auto)
}

/// The fields of an `x.ai/yolo_mode_changed` payload that set yolo and auto mode.
#[derive(Debug)]
pub struct PermissionModeChange {
    yolo_mode: Option<bool>,
    auto_mode: Option<bool>,
    permission_mode: Option<String>,
}

impl PermissionModeChange {
    /// Each field is read on its own, so a field with the wrong type is ignored and the rest still apply.
    pub fn from_params(params: &serde_json::Value) -> Self {
        Self {
            yolo_mode: params.get("yolo_mode").and_then(|v| v.as_bool()),
            auto_mode: params.get("auto_mode").and_then(|v| v.as_bool()),
            permission_mode: params
                .get("permission_mode")
                .and_then(|v| v.as_str())
                .map(str::to_owned),
        }
    }

    pub fn yolo_mode(&self) -> Option<bool> {
        self.yolo_mode
    }

    /// `Some(true)` turns auto mode on, `Some(false)` turns it off, `None` leaves it as it is.
    pub fn auto_change(&self) -> Option<bool> {
        let word = self.permission_mode.as_deref();
        let want_auto = self.auto_mode == Some(true) || word == Some("auto");

        // Turning auto on clears yolo, so a payload that also turns yolo on would undo its own yolo
        if want_auto && self.yolo_mode != Some(true) {
            return Some(true);
        }

        // `persist_permission_mode_and_notify` sends bare yolo toggles, and those must not clear auto
        let names_other_mode = matches!(word, Some("always-approve" | "ask" | "default"));
        (self.auto_mode == Some(false) || (names_other_mode && !want_auto)).then_some(false)
    }

    /// The mode this payload moves `current` to, or `None` when it names neither yolo nor auto.
    /// `auto_mode_enabled` is the auto gate. With it off, a request for auto turns auto off.
    pub fn apply(
        &self,
        current: PermissionMode,
        auto_mode_enabled: bool,
    ) -> Option<PermissionMode> {
        let auto_change = self.auto_change();
        if self.yolo_mode.is_none() && auto_change.is_none() {
            return None;
        }

        let mut yolo = self.yolo_mode.unwrap_or(current.is_always_approve());
        let mut auto = current.is_auto();
        match auto_change {
            Some(true) if auto_mode_enabled => (yolo, auto) = (false, true),
            Some(true) | Some(false) => auto = false,
            None => {}
        }

        Some(PermissionMode::from_flags(yolo, auto))
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use serde_json::json;
    use xai_grok_telemetry::enums::PermissionMode::{AlwaysApprove, Ask, Auto};

    use super::*;

    fn change(params: serde_json::Value) -> PermissionModeChange {
        PermissionModeChange::from_params(&params)
    }

    fn start(
        meta: Option<serde_json::Value>,
        default: PermissionMode,
        auto_mode_enabled: bool,
    ) -> PermissionMode {
        let meta = meta.and_then(|meta| meta.as_object().cloned());
        session_permission_mode(meta.as_ref(), default, auto_mode_enabled)
    }

    #[rstest]
    #[case::auto_word(json!({"yolo_mode": false, "permission_mode": "auto"}), Some(true))]
    #[case::auto_flag(json!({"auto_mode": true}), Some(true))]
    #[case::auto_word_with_auto_flag_off(json!({"permission_mode": "auto", "auto_mode": false}), Some(true))]
    #[case::auto_word_with_yolo_on(json!({"yolo_mode": true, "permission_mode": "auto"}), None)]
    #[case::auto_word_with_yolo_on_and_auto_flag_off(json!({"yolo_mode": true, "permission_mode": "auto", "auto_mode": false}), Some(false))]
    #[case::ask_word(json!({"yolo_mode": false, "permission_mode": "ask"}), Some(false))]
    #[case::always_approve_word(json!({"yolo_mode": true, "permission_mode": "always-approve"}), Some(false))]
    #[case::bare_yolo_toggle(json!({"yolo_mode": false}), None)]
    #[case::unknown_word(json!({"permission_mode": "weird"}), None)]
    fn auto_change_follows_the_flag_the_word_and_yolo(
        #[case] params: serde_json::Value,
        #[case] expected: Option<bool>,
    ) {
        assert_eq!(expected, change(params).auto_change());
    }

    #[rstest]
    #[case::always_approve_word_from_auto(json!({"yolo_mode": true, "permission_mode": "always-approve"}), Auto, Some(AlwaysApprove))]
    #[case::auto_flag_from_always_approve(json!({"auto_mode": true}), AlwaysApprove, Some(Auto))]
    #[case::ask_word_from_auto(json!({"yolo_mode": false, "permission_mode": "ask"}), Auto, Some(Ask))]
    #[case::bare_yolo_toggle_keeps_auto(json!({"yolo_mode": false}), Auto, Some(Auto))]
    // The TUI's auto kill switch sends no yolo_mode, so a sibling session's always-approve stays
    #[case::kill_switch_keeps_always_approve(json!({"auto_mode": false, "permission_mode": "ask"}), AlwaysApprove, Some(AlwaysApprove))]
    #[case::wrong_typed_auto_flag_is_ignored(json!({"yolo_mode": false, "auto_mode": "true"}), AlwaysApprove, Some(Ask))]
    #[case::unknown_word_changes_nothing(json!({"permission_mode": "weird"}), Ask, None)]
    fn apply_moves_the_current_mode(
        #[case] params: serde_json::Value,
        #[case] current: PermissionMode,
        #[case] expected: Option<PermissionMode>,
    ) {
        assert_eq!(expected, change(params).apply(current, true));
    }

    #[rstest]
    #[case::from_ask(Ask, Ask)]
    #[case::from_auto(Auto, Ask)]
    #[case::from_always_approve(AlwaysApprove, AlwaysApprove)]
    fn apply_turns_auto_off_for_an_auto_request_with_the_gate_off(
        #[case] current: PermissionMode,
        #[case] expected: PermissionMode,
        #[values(json!({"auto_mode": true}), json!({"permission_mode": "auto"}))]
        params: serde_json::Value,
    ) {
        assert_eq!(Some(expected), change(params).apply(current, false));
    }

    #[rstest]
    fn apply_gives_always_approve_for_yolo_on(
        #[values(
            json!({"yolo_mode": true}),
            json!({"yolo_mode": true, "auto_mode": true}),
            json!({"yolo_mode": true, "permission_mode": "auto"})
        )]
        params: serde_json::Value,
        #[values(Ask, Auto, AlwaysApprove)] current: PermissionMode,
        #[values(true, false)] auto_mode_enabled: bool,
    ) {
        assert_eq!(
            Some(AlwaysApprove),
            change(params).apply(current, auto_mode_enabled)
        );
    }

    #[rstest]
    #[case::yolo_meta(Some(json!({"yoloMode": true})), Ask, AlwaysApprove)]
    #[case::yolo_meta_off(Some(json!({"yoloMode": false})), AlwaysApprove, Ask)]
    #[case::always_approve_default(None, AlwaysApprove, AlwaysApprove)]
    #[case::auto_default(None, Auto, Auto)]
    #[case::auto_meta(Some(json!({"autoMode": true})), Ask, Auto)]
    #[case::yolo_and_auto_meta(Some(json!({"yoloMode": true, "autoMode": true})), Ask, AlwaysApprove)]
    fn session_permission_mode_reads_meta_then_the_default(
        #[case] meta: Option<serde_json::Value>,
        #[case] default: PermissionMode,
        #[case] expected: PermissionMode,
    ) {
        assert_eq!(expected, start(meta, default, true));
    }

    #[rstest]
    #[case::auto_meta(Some(json!({"autoMode": true})), Ask, Ask)]
    #[case::auto_default(None, Auto, Ask)]
    #[case::yolo_meta(Some(json!({"yoloMode": true})), Ask, AlwaysApprove)]
    #[case::always_approve_default(None, AlwaysApprove, AlwaysApprove)]
    fn session_permission_mode_turns_only_auto_off_with_the_gate_off(
        #[case] meta: Option<serde_json::Value>,
        #[case] default: PermissionMode,
        #[case] expected: PermissionMode,
    ) {
        assert_eq!(expected, start(meta, default, false));
    }
}
