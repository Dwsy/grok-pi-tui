//! Tab and sidebar-section taxonomy for the grok-pi settings panel.
//!
//! The registry ([`crate::settings::registry`]) owns *what* a setting is; this
//! module owns *where the panel draws it*. Each [`SettingCategory`] is one tab,
//! and within a tab the rows are grouped into ordered sections rendered as the
//! left sidebar.
//!
//! Section membership lives here rather than on `SettingMeta` so the upstream
//! registry stays untouched. [`section_for`] is total: unknown keys fall back
//! to [`OTHER_SECTION`], and `every_setting_has_a_section` fails the build if a
//! registered key ever lands there.

use crate::settings::{SettingCategory, SettingKey};

/// Fallback section for keys with no declared placement. A registry key
/// reaching this is a bug caught by `every_setting_has_a_section`.
pub const OTHER_SECTION: &str = "Other";

/// Short tab-bar label for a category. Tabs share one row, so these drop the
/// `&`-conjunctions that `SettingCategory::label` carries.
pub fn tab_label(category: SettingCategory) -> &'static str {
    match category {
        SettingCategory::Editor => "Editor",
        SettingCategory::Agent => "Agent",
        other => other.label(),
    }
}

/// Ordered sidebar sections for a tab. Order here is the render order;
/// sections with no visible rows are skipped at build time.
pub fn sections_for(category: SettingCategory) -> &'static [&'static str] {
    match category {
        SettingCategory::Appearance => &["Theme", "Display", "Thinking", "Tool output", "Prompt"],
        SettingCategory::Popups => &["Tool details"],
        SettingCategory::Mouse => &["Scrolling", "Selection"],
        SettingCategory::Editor => &["Input", "Voice"],
        SettingCategory::Agent => &[
            "Approval",
            "Built-in tools",
            "Pi features",
            "Sessions",
            "Review",
        ],
        SettingCategory::Privacy => &["Data sharing"],
        SettingCategory::Models => &["Defaults", "Recap", "Side questions"],
        SettingCategory::Session => &["Sessions"],
        SettingCategory::Advanced => &["Updates", "Contextual hints", "Diffs"],
    }
}

/// The sidebar section a setting belongs to, within its category's tab.
pub fn section_for(key: SettingKey) -> &'static str {
    match key {
        // -- Appearance ------------------------------------------------------
        "theme" | "auto_dark_theme" | "auto_light_theme" => "Theme",
        "compact_mode"
        | "screen_mode"
        | "show_timestamps"
        | "show_timeline"
        | "page_flip_on_send"
        | "progress_bar"
        | "remote_tui_footer"
        | "display_refresh_auto_cadence"
        | "render_mermaid" => "Display",
        "show_thinking_blocks" | "thinking_border_colors" | "max_thoughts_width" => "Thinking",
        "group_tool_verbs"
        | "collapsed_edit_blocks"
        | "side_by_side_edit"
        | "ctrl_o_tool_expansion"
        | "pi_bash_run_display"
        | "show_other_tool_args"
        | "respect_manual_folds" => "Tool output",
        "prompt_cursor" | "simple_mode" | "vim_mode" => "Prompt",

        // -- Popups ----------------------------------------------------------
        "write_edit_hover_popups" => "Tool details",

        // -- Mouse -----------------------------------------------------------
        "scroll_speed" | "scroll_mode" | "scroll_lines" | "invert_scroll" => "Scrolling",
        "keep_text_selection" => "Selection",

        // -- Editor ----------------------------------------------------------
        "combine_queued_prompts" | "multiline_mode" | "prompt_suggestions" | "pi_at_search_hidden" => "Input",
        "voice_keybind_enabled" | "voice_capture_mode" | "voice_stt_language" => "Voice",

        // -- Agent -----------------------------------------------------------
        "permission_mode"
        | "remember_tool_approvals"
        | "default_selected_permission"
        | "plan_mode" => "Approval",
        "pi_builtin_tools"
        | "pi_builtin_tools.read"
        | "pi_builtin_tools.bash"
        | "pi_builtin_tools.edit"
        | "pi_builtin_tools.write"
        | "pi_builtin_tools.grep"
        | "pi_builtin_tools.find"
        | "pi_builtin_tools.ls"
        | "pi_builtin_tools.eval" => "Built-in tools",
        "pi_bash"
        | "pi_eval"
        | "pi_eval_v2_only"
        | "pi_herdr"
        | "pi_subagents"
        | "pi_workflows"
        | "pi_todo"
        | "pi_goal"
        | "pi_loop"
        | "pi_btw"
        | "pi_cache_graph"
        | "pi_config_skill"
        | "pi_config"
        | "pi_user_markdown"
        | "pi_ask_user_question"
        | "pi_ask_user_question_notifications"
        | "toolset.ask_user_question.timeout_enabled" => "Pi features",
        "psm_resume_index"
        | "pi_tree_file_rollback"
        | "pi_tree_skip_summary_prompt"
        | "session_recap"
        | "recap_mermaid" => "Sessions",
        "review_file_tree" | "review_include_reads" => "Review",

        // -- Privacy ---------------------------------------------------------
        "coding_data_sharing" => "Data sharing",

        // -- Models ----------------------------------------------------------
        "default_model" | "fork_secondary_model" => "Defaults",
        "recap_models" | "recap_model" | "recap_model_2" | "recap_model_3" => "Recap",
        "btw_models" | "btw_model" | "btw_model_2" | "btw_model_3" => "Side questions",

        // -- Advanced --------------------------------------------------------
        "auto_update" | "show_tips" => "Updates",
        "contextual_hints"
        | "contextual_hints.undo"
        | "contextual_hints.plan_mode"
        | "contextual_hints.image_input"
        | "contextual_hints.send_now"
        | "contextual_hints.small_screen"
        | "contextual_hints.word_select"
        | "contextual_hints.ssh_wrap" => "Contextual hints",
        "hunk_tracker_mode" => "Diffs",

        _ => OTHER_SECTION,
    }
}

/// Widest declared section name across every tab. The sidebar column is sized
/// from this so the divider never shifts when switching tabs.
pub fn widest_section_name() -> usize {
    use unicode_width::UnicodeWidthStr;
    SettingCategory::ALL
        .iter()
        .flat_map(|cat| sections_for(*cat))
        .map(|name| name.width())
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::SettingsRegistry;

    #[test]
    fn every_setting_has_a_section() {
        let registry = SettingsRegistry::defaults();
        let orphans: Vec<&str> = registry
            .all()
            .iter()
            .filter(|meta| section_for(meta.key) == OTHER_SECTION)
            .map(|meta| meta.key)
            .collect();
        assert!(
            orphans.is_empty(),
            "settings with no declared sidebar section: {orphans:?} — \
             add them to views::pi_settings::layout::section_for",
        );
    }

    #[test]
    fn every_section_is_declared_for_its_category() {
        let registry = SettingsRegistry::defaults();
        for meta in registry.all() {
            let section = section_for(meta.key);
            let declared = sections_for(meta.category);
            assert!(
                declared.contains(&section),
                "`{}` maps to section `{section}`, which is not in \
                 sections_for({:?}) = {declared:?}",
                meta.key,
                meta.category,
            );
        }
    }

    #[test]
    fn declared_sections_are_unique_per_category() {
        for category in SettingCategory::ALL {
            let mut seen = std::collections::HashSet::new();
            for section in sections_for(*category) {
                assert!(
                    seen.insert(*section),
                    "duplicate section `{section}` in sections_for({category:?})",
                );
            }
        }
    }

    #[test]
    fn tab_labels_are_non_empty_and_unique() {
        let mut seen = std::collections::HashSet::new();
        for category in SettingCategory::ALL {
            let label = tab_label(*category);
            assert!(!label.is_empty(), "empty tab label for {category:?}");
            assert!(seen.insert(label), "duplicate tab label `{label}`");
        }
    }
}
