//! Registry key → typed `Action` mapping for the grok-pi settings panel.
//!
//! The upstream settings modal keeps an equivalent table private to its own
//! module. Rather than widen upstream's visibility (and its merge surface),
//! this panel carries its own copy. `every_setting_has_a_dispatch_arm` keeps
//! the two in step with the registry.

use crate::app::actions::Action;
use crate::settings::{PagerLocalSnapshot, SettingKey, StringValidator};

/// Construct the typed `Action::Set*` for a Bool setting.
pub(super) fn action_for_bool(key: SettingKey, new: bool) -> Option<Action> {
    if let Some(spec) = xai_grok_shell::host_features::feature_spec_by_setting_key(key) {
        return Some(Action::SetHostFeatureBool {
            key: spec.key,
            enabled: new,
        });
    }
    use crate::app::actions::PiBuiltinTool;
    let action = match key {
        "compact_mode" => Action::SetCompactMode(new),
        "show_timestamps" => Action::SetTimestamps(new),
        "show_timeline" => Action::SetTimeline(new),
        "pi_builtin_tools.read" => Action::SetPiBuiltinTool {
            tool: PiBuiltinTool::Read,
            enabled: new,
        },
        "pi_builtin_tools.bash" => Action::SetPiBuiltinTool {
            tool: PiBuiltinTool::Bash,
            enabled: new,
        },
        "pi_builtin_tools.powershell" => Action::SetPiBuiltinTool {
            tool: PiBuiltinTool::PowerShell,
            enabled: new,
        },
        "pi_builtin_tools.edit" => Action::SetPiBuiltinTool {
            tool: PiBuiltinTool::Edit,
            enabled: new,
        },
        "pi_builtin_tools.write" => Action::SetPiBuiltinTool {
            tool: PiBuiltinTool::Write,
            enabled: new,
        },
        "pi_builtin_tools.grep" => Action::SetPiBuiltinTool {
            tool: PiBuiltinTool::Grep,
            enabled: new,
        },
        "pi_builtin_tools.find" => Action::SetPiBuiltinTool {
            tool: PiBuiltinTool::Find,
            enabled: new,
        },
        "pi_builtin_tools.ls" => Action::SetPiBuiltinTool {
            tool: PiBuiltinTool::Ls,
            enabled: new,
        },
        "pi_builtin_tools.eval" => Action::SetPiBuiltinTool {
            tool: PiBuiltinTool::Eval,
            enabled: new,
        },
        "pi_bash" => Action::SetPiBash(new),
        "pi_eval_v2_only" => Action::SetPiEvalV2Only(new),
        "psm_resume_index" => Action::SetPsmResumeIndex(new),
        "pi_tree_file_rollback" => Action::SetPiTreeFileRollback(new),
        "pi_tree_skip_summary_prompt" => Action::SetPiTreeSkipSummaryPrompt(new),
        "pi_ask_user_question_notifications" => Action::SetPiAskUserQuestionNotifications(new),
        "pi_cache_graph" => Action::SetPiCacheGraph(new),
        "pi_config_skill" => Action::SetPiConfigSkill(new),
        "pi_user_markdown" => Action::SetPiUserMarkdown(new),
        "pi_at_search_hidden" => Action::SetPiAtSearchHidden(new),
        "pi_keep_multi_agent" => Action::SetPiKeepMultiAgent(new),
        "pi_bash_command_format" => Action::SetPiBashCommandFormat(new),
        "write_edit_hover_popups" => Action::SetWriteEditHoverPopups(new),
        "show_other_tool_args" => Action::SetShowOtherToolArgs(new),
        "review_file_tree" => Action::SetReviewFileTree(new),
        "review_include_reads" => Action::SetReviewIncludeReads(new),
        "simple_mode" => Action::SetSimpleMode(new),
        "contextual_hints.undo" => Action::SetContextualHintUndo(new),
        "contextual_hints.plan_mode" => Action::SetContextualHintPlanMode(new),
        "contextual_hints.image_input" => Action::SetContextualHintImageInput(new),
        "contextual_hints.send_now" => Action::SetContextualHintSendNow(new),
        "contextual_hints.small_screen" => Action::SetContextualHintSmallScreen(new),
        "contextual_hints.word_select" => Action::SetContextualHintWordSelect(new),
        "contextual_hints.ssh_wrap" => Action::SetContextualHintSshWrap(new),
        "multiline_mode" => Action::SetMultilineMode(new),
        "vim_mode" => Action::SetVimMode(new),
        "session_recap" => Action::SetSessionRecap(new),
        "recap_mermaid" => Action::SetRecapMermaid(new),
        "progress_bar" => Action::SetProgressBar(new),
        "remote_tui_footer" => Action::SetRemoteTuiFooter(new),
        "voice_keybind_enabled" => Action::SetVoiceKeybindEnabled(new),
        "remember_tool_approvals" => Action::SetRememberToolApprovals(new),
        "toolset.ask_user_question.timeout_enabled" => {
            Action::SetAskUserQuestionTimeoutEnabled(new)
        }
        "show_thinking_blocks" => Action::SetShowThinkingBlocks(new),
        "thinking_border_colors" => Action::SetThinkingBorderColors(new),
        "group_tool_verbs" => Action::SetGroupToolVerbs(new),
        "collapsed_edit_blocks" => Action::SetCollapsedEditBlocks(new),
        "side_by_side_edit" => Action::SetSideBySideEdit(new),
        "prompt_suggestions" => Action::SetPromptSuggestions(new),
        "respect_manual_folds" => Action::SetRespectManualFolds(new),
        "page_flip_on_send" => Action::SetPageFlipOnSend(new),
        "confirm_before_rewind" => Action::SetConfirmBeforeRewind(new),
        "combine_queued_prompts" => Action::SetCombineQueuedPrompts(new),
        "invert_scroll" => Action::SetInvertScroll(new),
        "show_tips" => Action::SetShowTips(new),
        "auto_update" => Action::SetAutoUpdate(new),
        "display_refresh_auto_cadence" => Action::SetDisplayRefreshAutoCadence(new),
        _ => return None,
    };
    Some(action)
}

/// `Action::Preview*` for an Enum setting — driven by the chooser's Up/Down for
/// live preview and by Esc to revert. Preview actions never persist.
/// Settings with irreversible side effects deliberately have no preview.
pub(super) fn action_for_enum_preview(key: SettingKey, choice: &str) -> Option<Action> {
    match key {
        "theme" => Some(Action::PreviewTheme(choice.to_string())),
        "auto_dark_theme" => Some(Action::PreviewAutoDarkTheme(choice.to_string())),
        "auto_light_theme" => Some(Action::PreviewAutoLightTheme(choice.to_string())),
        _ => None,
    }
}

/// `Action::Set*` commit variant for an Enum setting. Commit actions persist to
/// disk and fire a toast. Junk canonicals fold to `None` so Enter no-ops
/// instead of mis-mapping.
pub(super) fn action_for_enum_commit(key: SettingKey, choice: &str) -> Option<Action> {
    use crate::app::actions::{PermissionModeKind, PlanModeKind};
    match key {
        "theme" => Some(Action::SetTheme(choice.to_string())),
        "auto_dark_theme" => Some(Action::SetAutoDarkTheme(choice.to_string())),
        "auto_light_theme" => Some(Action::SetAutoLightTheme(choice.to_string())),
        // Auto's feature gate is enforced in `set_permission_mode`, so the
        // panel and the Shift+Tab cycle never disagree.
        "permission_mode" => match choice {
            "always-approve" => Some(Action::SetPermissionMode(PermissionModeKind::AlwaysApprove)),
            "auto" => Some(Action::SetPermissionMode(PermissionModeKind::Auto)),
            "ask" => Some(Action::SetPermissionMode(PermissionModeKind::Ask)),
            "default" => Some(Action::SetPermissionMode(PermissionModeKind::Default)),
            _ => None,
        },
        "coding_data_sharing" => match choice {
            "opt-in" => Some(Action::SetCodingDataSharing { opted_in: true }),
            "opt-out" => Some(Action::SetCodingDataSharing { opted_in: false }),
            _ => None,
        },
        "plan_mode" => match choice {
            "on" => Some(Action::SetPlanMode(PlanModeKind::On)),
            "off" => Some(Action::SetPlanMode(PlanModeKind::Off)),
            _ => None,
        },
        "ctrl_o_tool_expansion" => Some(Action::SetCtrlOToolExpansion(choice.to_string())),
        "pi_eval" => match choice {
            "v1" | "v2" => Some(Action::SetPiEval(choice.to_string())),
            _ => None,
        },
        "pi_eval_v2_language" => match choice {
            "js" | "py" | "all" => Some(Action::SetPiEvalV2Language(choice.to_string())),
            _ => None,
        },
        "pi_eval_v2_display_mode" => match choice {
            "effects" | "legacy" => Some(Action::SetPiEvalV2DisplayMode(choice.to_string())),
            _ => None,
        },
        "pi_bash_run_display" => crate::appearance::ExecuteHeaderContent::from_canonical(choice)
            .map(Action::SetPiBashRunDisplay),
        "hunk_tracker_mode" => Some(Action::SetHunkTrackerMode(choice.to_string())),
        "screen_mode" => Some(Action::SetScreenMode(choice.to_string())),
        "voice_capture_mode" => Some(Action::SetVoiceCaptureMode(choice.to_string())),
        "voice_stt_language" => Some(Action::SetVoiceSttLanguage(choice.to_string())),
        "render_mermaid" => {
            crate::appearance::RenderMermaid::from_canonical(choice).map(Action::SetRenderMermaid)
        }
        "keep_text_selection" => crate::appearance::TextSelection::from_canonical(choice)
            .map(Action::SetKeepTextSelection),
        "scroll_mode" => {
            crate::appearance::ScrollMode::from_canonical(choice).map(Action::SetScrollMode)
        }
        "cancel_turn_key" => match choice {
            "esc" | "ctrl_c" => Some(Action::SetCancelTurnKey(choice.to_string())),
            _ => None,
        },
        "default_selected_permission" => {
            Some(Action::SetDefaultSelectedPermission(choice.to_string()))
        }
        _ => None,
    }
}

/// `Action::Set*` commit variant for a String setting. Model names resolve
/// through the snapshot; an empty buffer maps to `Action::Clear*`.
pub(super) fn action_for_string(
    key: SettingKey,
    value: String,
    snapshot: &PagerLocalSnapshot,
) -> Option<Action> {
    match key {
        "prompt_cursor" => Some(Action::SetPromptCursor(value)),
        "default_model" => {
            if value.is_empty() {
                Some(Action::ClearDefaultModel)
            } else {
                snapshot
                    .resolve_model_name(&value)
                    .map(Action::SetDefaultModel)
            }
        }
        "fork_secondary_model" => {
            if value.is_empty() {
                Some(Action::ClearForkSecondaryModel)
            } else {
                snapshot
                    .resolve_model_name(&value)
                    .map(Action::SetForkSecondaryModel)
            }
        }
        "recap_model" => {
            if value.is_empty() {
                Some(Action::ClearRecapModel)
            } else {
                snapshot
                    .resolve_model_name(&value)
                    .map(Action::SetRecapModel)
            }
        }
        "recap_model_2" => Some(if value.is_empty() {
            Action::ClearRecapModel2
        } else {
            Action::SetRecapModel2(value)
        }),
        "recap_model_3" => Some(if value.is_empty() {
            Action::ClearRecapModel3
        } else {
            Action::SetRecapModel3(value)
        }),
        "btw_model" => Some(if value.is_empty() {
            Action::ClearBtwModel
        } else {
            Action::SetBtwModel(value)
        }),
        "btw_model_2" => Some(if value.is_empty() {
            Action::ClearBtwModel2
        } else {
            Action::SetBtwModel2(value)
        }),
        "btw_model_3" => Some(if value.is_empty() {
            Action::ClearBtwModel3
        } else {
            Action::SetBtwModel3(value)
        }),
        _ => None,
    }
}

/// `Action::Set*` commit variant for an Int setting.
pub(super) fn action_for_int(key: SettingKey, value: i64) -> Option<Action> {
    match key {
        "max_thoughts_width" => Some(Action::SetMaxThoughtsWidth(value)),
        "scroll_speed" => Some(Action::SetScrollSpeed(value)),
        "scroll_lines" => Some(Action::SetScrollLines(value)),
        _ => None,
    }
}

/// Side-model slots route to the native searchable `/model` picker rather than
/// this panel's enum chooser, so their catalogs stay in one place.
pub(super) fn side_model_picker_action(key: SettingKey) -> Option<Action> {
    matches!(
        key,
        "recap_model"
            | "recap_model_2"
            | "recap_model_3"
            | "btw_model"
            | "btw_model_2"
            | "btw_model_3"
    )
    .then_some(Action::OpenSideModelPicker { slot_key: key })
}

/// Validate a String buffer against its registered validator. Returns
/// `Some(message)` on failure.
pub(super) fn validate_string(
    validator: StringValidator,
    buffer: &str,
    available_models: &[(String, agent_client_protocol::ModelId)],
) -> Option<String> {
    match validator {
        StringValidator::Any => None,
        StringValidator::PromptCursor => crate::appearance::PromptCursor::parse_config(buffer)
            .is_none()
            .then(|| {
                "Use native, block, underline, bar, or one single-column character".to_string()
            }),
        StringValidator::NonEmptyToken => {
            if buffer.is_empty() {
                Some("Value cannot be empty".to_string())
            } else if buffer.chars().any(char::is_whitespace) {
                Some("Value cannot contain whitespace".to_string())
            } else {
                None
            }
        }
        StringValidator::KnownModel => {
            // Empty is the "clear override" sentinel.
            if buffer.is_empty() {
                return None;
            }
            if available_models.is_empty() {
                return Some("Model catalog still loading — try again".to_string());
            }
            available_models
                .iter()
                .all(|(name, _)| !name.eq_ignore_ascii_case(buffer))
                .then(|| format!("Unknown model: \"{buffer}\""))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{SettingKind, SettingsRegistry};

    /// Every registry entry the panel can focus must map to an action, or
    /// Enter/Space silently does nothing.
    #[test]
    fn every_setting_has_a_dispatch_arm() {
        let registry = SettingsRegistry::defaults();
        let snapshot = PagerLocalSnapshot::default();
        let mut missing = Vec::new();
        for meta in registry.all() {
            let mapped = match &meta.kind {
                SettingKind::Bool { .. } => action_for_bool(meta.key, true).is_some(),
                SettingKind::Enum { choices, .. } => choices
                    .iter()
                    .all(|c| action_for_enum_commit(meta.key, c.canonical).is_some()),
                SettingKind::Int { .. } => action_for_int(meta.key, 1).is_some(),
                SettingKind::String { .. } => {
                    action_for_string(meta.key, "x".into(), &snapshot).is_some()
                        || side_model_picker_action(meta.key).is_some()
                }
                SettingKind::DynamicEnum { .. } => {
                    action_for_string(meta.key, String::new(), &snapshot).is_some()
                        || side_model_picker_action(meta.key).is_some()
                }
                // Groups open a sub-sheet; their children carry the actions.
                SettingKind::Group { .. } => true,
            };
            if !mapped {
                missing.push(meta.key);
            }
        }
        assert!(
            missing.is_empty(),
            "settings with no dispatch arm in views::pi_settings::actions: {missing:?}",
        );
    }
}
