//! `/theme` (alias `/t`): switch the color theme.
//!
//! Toggles between available themes or switches to a named theme.
//! Selecting `auto` makes the theme follow the system appearance.
//! Selecting an explicit theme turns auto mode off.
//!
//! Supports Grok built-ins and Pi themes (`pi:<name>`).
//!
//! `run` dispatches `Action::SetTheme(<canonical>)` — the dispatcher
//! handles mutation + persistence + toast. `preview_arg` /
//! `cancel_preview` call `Theme::apply_kind` / `apply_custom` directly for
//! non-persisting visual previews (no toast/disk writes per keystroke).

use crate::app::actions::Action;
use crate::slash::command::{
    AppCtx, ArgItem, CommandExecCtx, CommandResult, SlashCommand, slash_meta,
};
use crate::slash::{ModeSupport, Remedy};
use crate::theme::{Theme, ThemeKind, cache as theme_cache, pi as pi_theme};

pub struct ThemeCommand;

/// Canonical name plus every alias, so `/theme transparent` still ranks the `terminal` row.
fn picker_match_text(kind: ThemeKind) -> String {
    std::iter::once(kind.display_name())
        .chain(kind.aliases().iter().copied())
        .collect::<Vec<_>>()
        .join(" ")
}

impl SlashCommand for ThemeCommand {
    slash_meta! {
        name: "theme",
        aliases: ["t"],
        description: "Switch the color theme",
        usage: "/theme <name>",
        takes_args: true,
        args_required: false,
        mode_support: ModeSupport::FullscreenOnly(Remedy::SwitchMode {
            why: "minimal renders with your terminal's own palette",
        }),
        arg_placeholder: "<theme>",
    }

    fn supports_preview(&self) -> bool {
        true
    }

    fn preview_state(&self) -> Option<String> {
        Some(Theme::current_display_id())
    }

    fn preview_arg(&self, arg: &str) {
        if let Some(kind) = ThemeKind::from_name(arg) {
            if kind.is_auto() {
                let resolved = theme_cache::resolve_auto();
                Theme::apply_kind(resolved);
            } else {
                Theme::apply_kind(kind);
            }
            return;
        }
        if pi_theme::is_pi_theme_id(arg) || arg.starts_with(pi_theme::PI_THEME_PREFIX) {
            let _ = pi_theme::apply_pi_theme(arg);
            return;
        }
        // Bare Pi name (e.g. user typed a discovered custom without prefix).
        let as_pi = pi_theme::theme_id(arg);
        if pi_theme::list_themes().iter().any(|t| t.name == arg) {
            let _ = pi_theme::apply_pi_theme(&as_pi);
        }
    }

    fn cancel_preview(&self, previous: &str) {
        if let Some(kind) = ThemeKind::from_name(previous) {
            Theme::apply_kind(kind);
            return;
        }
        if pi_theme::is_pi_theme_id(previous) {
            let _ = pi_theme::apply_pi_theme(previous);
        }
    }

    fn suggest_args(&self, _ctx: &AppCtx, _args_query: &str) -> Option<Vec<ArgItem>> {
        let current_id = Theme::current_display_id();
        let is_auto = theme_cache::is_auto_mode();
        let available = ThemeKind::available();
        let has_custom = theme_cache::has_custom();

        // Prepend "auto" (follow system appearance) as the first option.
        let auto_active = if is_auto { " (active)" } else { "" };
        let mut items = vec![ArgItem {
            display: "auto".to_string(),
            match_text: picker_match_text(ThemeKind::Auto),
            insert_text: "auto".to_string(),
            description: format!("auto (follow system){auto_active}"),
        }];

        // Transparent Pi themes right after auto for quick access.
        let transparent_names = ["transparent", "transparent-light"];
        for meta in pi_theme::list_themes() {
            if transparent_names.contains(&meta.name.as_str()) {
                let active = if !is_auto && current_id == meta.id {
                    " (active)"
                } else {
                    ""
                };
                items.push(ArgItem {
                    display: meta.id.clone(),
                    match_text: format!("{} {}", meta.id, meta.name),
                    insert_text: meta.id.clone(),
                    description: format!("Pi builtin{active}"),
                });
            }
        }

        // Concrete Grok themes — only show "(active)" when not in auto/custom.

        items.extend(available.iter().map(|kind| {
            let active = if !is_auto && !has_custom && kind.display_name() == current_id.as_str() {
                " (active)"
            } else {
                ""
            };
            ArgItem {
                display: kind.display_name().to_string(),
                match_text: picker_match_text(*kind),
                insert_text: kind.display_name().to_string(),
                description: format!("{}{active}", kind.display_name()),
            }
        }));

        // Remaining Pi themes (embedded + discovered), excluding transparents.
        for meta in pi_theme::list_themes() {
            if transparent_names.contains(&meta.name.as_str()) {
                continue;
            }
            let active = if !is_auto && current_id == meta.id {
                " (active)"
            } else {
                ""
            };
            let source = if meta.builtin {
                "Pi builtin".to_string()
            } else if let Some(path) = &meta.path {
                format!("Pi · {}", path.display())
            } else {
                "Pi".to_string()
            };
            items.push(ArgItem {
                display: meta.id.clone(),
                match_text: format!("{} {}", meta.id, meta.name),
                insert_text: meta.id.clone(),
                description: format!("{source}{active}"),
            });
        }

        Some(items)
    }

    fn run(&self, _ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        let trimmed = args.trim();
        let available = ThemeKind::available();

        // No args: toggle between available Grok themes (+ Pi if present).
        if trimmed.is_empty() {
            let mut cycle: Vec<String> = available
                .iter()
                .map(|k| k.display_name().to_string())
                .collect();
            for meta in pi_theme::list_themes() {
                cycle.push(meta.id);
            }
            if cycle.is_empty() {
                return CommandResult::Error("No themes available".into());
            }
            let current = Theme::current_display_id();
            let current_idx = cycle.iter().position(|k| k == &current).unwrap_or(0);
            let next = cycle[(current_idx + 1) % cycle.len()].clone();
            return CommandResult::Action(Action::SetTheme(next));
        }

        // Named Grok theme (including "auto").
        if let Some(kind) = ThemeKind::from_name(trimmed) {
            return CommandResult::Action(Action::SetTheme(kind.display_name().to_string()));
        }

        // Pi theme: `pi:name` or bare registered name.
        if pi_theme::is_pi_theme_id(trimmed) {
            if pi_theme::load_palette(trimmed).is_ok() {
                return CommandResult::Action(Action::SetTheme(trimmed.to_string()));
            }
            return CommandResult::Error(format!("Unknown Pi theme: {trimmed}"));
        }
        if pi_theme::list_themes().iter().any(|t| t.name == trimmed) {
            let id = pi_theme::theme_id(trimmed);
            return CommandResult::Action(Action::SetTheme(id));
        }

        let mut all_names: Vec<String> = vec!["auto".into()];
        all_names.extend(ThemeKind::ALL.iter().map(|k| k.display_name().to_string()));
        all_names.extend(pi_theme::list_themes().into_iter().map(|t| t.id));
        CommandResult::Error(format!(
            "Unknown theme: {}. Available: {}",
            trimmed,
            all_names.join(", ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{cache as theme_cache, system_appearance};

    /// Run a test with a clean in-memory state.
    /// Prevents disk reads by pre-loading the theme state.
    fn with_test_env(f: impl FnOnce()) {
        let _guard = theme_cache::test_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        theme_cache::reset_for_test();
        theme_cache::seed_auto_theme_defaults_for_test();
        system_appearance::clear_mock();
        pi_theme::reset_for_test();
        pi_theme::ensure_builtins();
        // Set LOADED=true so current_kind() doesn't try to read from disk.
        theme_cache::set(ThemeKind::GrokNight);
        f();
        system_appearance::clear_mock();
        pi_theme::reset_for_test();
        theme_cache::reset_for_test();
    }

    // -- suggest_args ---------------------------------------------------------

    #[test]
    fn suggest_args_prepends_auto_option() {
        with_test_env(|| {
            let cmd = ThemeCommand;
            let models = crate::acp::model_state::ModelState::default();
            let ctx = AppCtx {
                models: &models,
                cwd: std::path::Path::new("."),
                has_session_announcements: false,
                billing_surface_visible: true,
                usage_command_visible: true,
                workflows_available: true,
                saved_workflows: &[],
                workflow_runs: &[],
                screen_mode: crate::app::ScreenMode::Fullscreen,
                current_title: None,
            };
            let items = cmd.suggest_args(&ctx, "").expect("should return items");
            assert_eq!(items[0].insert_text, "auto");
            assert!(items[0].description.contains("follow system"));
            // auto + Grok available + every embedded/discovered Pi theme
            assert_eq!(
                items.len(),
                ThemeKind::available().len() + 1 + pi_theme::list_themes().len()
            );
            assert!(items.iter().any(|i| i.insert_text == "pi:dark"));
            assert!(items.iter().any(|i| i.insert_text == "pi:light"));
            assert!(items.iter().any(|i| i.insert_text == "pi:dark-tokyo-night"));
        });
    }

    #[test]
    fn suggest_args_auto_active_when_auto_mode() {
        with_test_env(|| {
            theme_cache::set_auto_mode(true);
            let cmd = ThemeCommand;
            let models = crate::acp::model_state::ModelState::default();
            let ctx = AppCtx {
                models: &models,
                cwd: std::path::Path::new("."),
                has_session_announcements: false,
                billing_surface_visible: true,
                usage_command_visible: true,
                workflows_available: true,
                saved_workflows: &[],
                workflow_runs: &[],
                screen_mode: crate::app::ScreenMode::Fullscreen,
                current_title: None,
            };
            let items = cmd.suggest_args(&ctx, "").expect("should return items");
            assert!(
                items[0].description.contains("(active)"),
                "auto should show (active), got: {}",
                items[0].description
            );
        });
    }

    #[test]
    fn suggest_args_auto_not_active_when_explicit() {
        with_test_env(|| {
            theme_cache::set_auto_mode(false);
            let cmd = ThemeCommand;
            let models = crate::acp::model_state::ModelState::default();
            let ctx = AppCtx {
                models: &models,
                cwd: std::path::Path::new("."),
                has_session_announcements: false,
                billing_surface_visible: true,
                usage_command_visible: true,
                workflows_available: true,
                saved_workflows: &[],
                workflow_runs: &[],
                screen_mode: crate::app::ScreenMode::Fullscreen,
                current_title: None,
            };
            let items = cmd.suggest_args(&ctx, "").expect("should return items");
            assert!(
                !items[0].description.contains("(active)"),
                "auto should not show (active), got: {}",
                items[0].description
            );
        });
    }

    #[test]
    fn suggest_args_explicit_active_when_not_auto() {
        with_test_env(|| {
            theme_cache::set_auto_mode(false);
            theme_cache::set(ThemeKind::GrokNight);
            let cmd = ThemeCommand;
            let models = crate::acp::model_state::ModelState::default();
            let ctx = AppCtx {
                models: &models,
                cwd: std::path::Path::new("."),
                has_session_announcements: false,
                billing_surface_visible: true,
                usage_command_visible: true,
                workflows_available: true,
                saved_workflows: &[],
                workflow_runs: &[],
                screen_mode: crate::app::ScreenMode::Fullscreen,
                current_title: None,
            };
            let items = cmd.suggest_args(&ctx, "").expect("should return items");
            let groknight = items
                .iter()
                .find(|i| i.insert_text == "groknight")
                .expect("groknight should be in list");
            assert!(
                groknight.description.contains("(active)"),
                "explicit theme should show (active), got: {}",
                groknight.description
            );
        });
    }

    #[test]
    fn suggest_args_no_explicit_active_when_auto() {
        with_test_env(|| {
            theme_cache::set_auto_mode(true);
            theme_cache::set(ThemeKind::GrokNight);
            let cmd = ThemeCommand;
            let models = crate::acp::model_state::ModelState::default();
            let ctx = AppCtx {
                models: &models,
                cwd: std::path::Path::new("."),
                has_session_announcements: false,
                billing_surface_visible: true,
                usage_command_visible: true,
                workflows_available: true,
                saved_workflows: &[],
                workflow_runs: &[],
                screen_mode: crate::app::ScreenMode::Fullscreen,
                current_title: None,
            };
            let items = cmd.suggest_args(&ctx, "").expect("should return items");
            // No concrete theme should show "(active)" in auto mode.
            for item in items.iter().skip(1) {
                assert!(
                    !item.description.contains("(active)"),
                    "{} should not show (active) in auto mode",
                    item.insert_text
                );
            }
        });
    }

    /// Typing an alias ranks its canonical row first; the row still inserts the canonical name.
    #[test]
    fn suggest_args_alias_ranks_canonical_row() {
        with_test_env(|| {
            let cmd = ThemeCommand;
            let models = crate::acp::model_state::ModelState::default();
            let ctx = AppCtx {
                models: &models,
                cwd: std::path::Path::new("."),
                has_session_announcements: false,
                billing_surface_visible: true,
                usage_command_visible: true,
                workflows_available: true,
                saved_workflows: &[],
                workflow_runs: &[],
                screen_mode: crate::app::ScreenMode::Fullscreen,
                current_title: None,
            };
            let items = cmd.suggest_args(&ctx, "").expect("should return items");
            let mut matcher = crate::slash::matcher::FuzzyMatcher::new();
            for (alias, canonical) in [
                ("transparent", "terminal"),
                ("dark", "groknight"),
                ("system", "auto"),
            ] {
                let hits = matcher.rank(&items, alias, items.len(), |item| &item.match_text);
                let (top, _) = hits
                    .first()
                    .unwrap_or_else(|| panic!("{alias} matched nothing"));
                assert_eq!(items[*top].insert_text, canonical, "top hit for {alias}");
            }
        });
    }

    // -- run (dispatches Action::SetTheme) ------------------------------------

    /// `/theme <name>` returns `Action::SetTheme(<canonical>)`; the dispatcher handles the in-memory state, the disk write, and the toast.
    #[test]
    fn run_explicit_dispatches_set_theme_action() {
        with_test_env(|| {
            let cmd = ThemeCommand;
            let models = crate::acp::model_state::ModelState::default();
            let bundle = crate::app::bundle::BundleState::default();
            let mut ctx = CommandExecCtx {
                models: &models,
                session_id: None,
                bundle_state: &bundle,
                screen_mode: crate::app::ScreenMode::Inline,
                billing_surface_visible: true,
                usage_command_visible: true,
                pager_state: crate::settings::PagerLocalSnapshot {
                    multiline_mode: false,
                    yolo_mode: false,
                    ..crate::settings::PagerLocalSnapshot::default()
                },
            };
            let result = cmd.run(&mut ctx, "groknight");
            match result {
                CommandResult::Action(Action::SetTheme(name)) => {
                    assert_eq!(name, "groknight");
                }
                other => panic!("expected Action::SetTheme(\"groknight\"), got {other:?}"),
            }
        });
    }

    /// While the terminal-theme rollout gate is off, a typed `/theme terminal` (or alias) is an unknown name whose error listing omits it, and it drops out of the suggestions.
    #[test]
    fn run_terminal_rejected_and_unlisted_while_gated_off() {
        with_test_env(|| {
            theme_cache::set_terminal_theme_enabled(false);
            let cmd = ThemeCommand;
            let models = crate::acp::model_state::ModelState::default();
            let bundle = crate::app::bundle::BundleState::default();
            let mut ctx = CommandExecCtx {
                models: &models,
                session_id: None,
                bundle_state: &bundle,
                screen_mode: crate::app::ScreenMode::Inline,
                billing_surface_visible: true,
                usage_command_visible: true,
                pager_state: crate::settings::PagerLocalSnapshot {
                    multiline_mode: false,
                    yolo_mode: false,
                    ..crate::settings::PagerLocalSnapshot::default()
                },
            };
            for name in ["terminal", "transparent"] {
                match cmd.run(&mut ctx, name) {
                    CommandResult::Error(msg) => {
                        assert!(msg.contains("Unknown theme"), "got: {msg}");
                        let listing = msg.split("Available:").nth(1).expect("listing");
                        assert!(!listing.contains("terminal"), "gated name listed: {msg}");
                    }
                    other => panic!("expected CommandResult::Error, got {other:?}"),
                }
            }
            let app_ctx = AppCtx {
                models: &models,
                cwd: std::path::Path::new("."),
                has_session_announcements: false,
                billing_surface_visible: true,
                usage_command_visible: true,
                workflows_available: true,
                saved_workflows: &[],
                workflow_runs: &[],
                screen_mode: crate::app::ScreenMode::Fullscreen,
                current_title: None,
            };
            let items = cmd.suggest_args(&app_ctx, "").expect("should return items");
            assert!(
                items.iter().all(|i| i.insert_text != "terminal"),
                "gated theme must not be suggested"
            );

            theme_cache::set_terminal_theme_enabled(true);
            match cmd.run(&mut ctx, "terminal") {
                CommandResult::Action(Action::SetTheme(name)) => assert_eq!(name, "terminal"),
                other => panic!("expected Action::SetTheme(\"terminal\"), got {other:?}"),
            }
        });
    }

    /// `/theme` (no args) toggles by dispatching `Action::SetTheme(<next>)`.
    /// Asserts first that `ThemeKind::available()` has at least 2 entries so a broken invariant fails loudly instead of being masked.

    #[test]
    fn run_toggle_dispatches_set_theme_action() {
        with_test_env(|| {
            theme_cache::set(ThemeKind::GrokNight);
            // Hard-fail with a clear message if the precondition breaks
            // `(0 + 1) % 0` in `run` would otherwise panic with `attempt to calculate the remainder with a divisor of zero`, a worse message

            assert!(
                ThemeKind::available().len() >= 2,
                "toggle test requires ≥2 available themes, got {}",
                ThemeKind::available().len(),
            );
            let cmd = ThemeCommand;
            let models = crate::acp::model_state::ModelState::default();
            let bundle = crate::app::bundle::BundleState::default();
            let mut ctx = CommandExecCtx {
                models: &models,
                session_id: None,
                bundle_state: &bundle,
                screen_mode: crate::app::ScreenMode::Inline,
                billing_surface_visible: true,
                usage_command_visible: true,
                pager_state: crate::settings::PagerLocalSnapshot {
                    multiline_mode: false,
                    yolo_mode: false,
                    ..crate::settings::PagerLocalSnapshot::default()
                },
            };
            let result = cmd.run(&mut ctx, "");
            match result {
                CommandResult::Action(Action::SetTheme(name)) => {
                    // available[0] is GrokNight; next is available[1]
                    let expected = ThemeKind::available()[1].display_name();
                    assert_eq!(name, expected);
                }
                other => panic!("expected Action::SetTheme(...), got {other:?}"),
            }
        });
    }

    #[test]
    fn run_auto_dispatches_set_theme_auto() {
        with_test_env(|| {
            let cmd = ThemeCommand;
            let models = crate::acp::model_state::ModelState::default();
            let bundle = crate::app::bundle::BundleState::default();
            let mut ctx = CommandExecCtx {
                models: &models,
                session_id: None,
                bundle_state: &bundle,
                screen_mode: crate::app::ScreenMode::Inline,
                billing_surface_visible: true,
                usage_command_visible: true,
                pager_state: crate::settings::PagerLocalSnapshot {
                    multiline_mode: false,
                    yolo_mode: false,
                    ..crate::settings::PagerLocalSnapshot::default()
                },
            };
            let result = cmd.run(&mut ctx, "auto");
            match result {
                CommandResult::Action(Action::SetTheme(name)) => {
                    assert_eq!(name, "auto");
                }
                other => panic!("expected Action::SetTheme(\"auto\"), got {other:?}"),
            }
        });
    }

    /// Aliases normalise to canonical `display_name` before dispatch.
    #[test]
    fn run_alias_normalises_to_canonical() {
        with_test_env(|| {
            let cmd = ThemeCommand;
            let models = crate::acp::model_state::ModelState::default();
            let bundle = crate::app::bundle::BundleState::default();
            let mut ctx = CommandExecCtx {
                models: &models,
                session_id: None,
                bundle_state: &bundle,
                screen_mode: crate::app::ScreenMode::Inline,
                billing_surface_visible: true,
                usage_command_visible: true,
                pager_state: crate::settings::PagerLocalSnapshot {
                    multiline_mode: false,
                    yolo_mode: false,
                    ..crate::settings::PagerLocalSnapshot::default()
                },
            };
            // "dark" is an alias for GrokNight.
            let result = cmd.run(&mut ctx, "dark");
            match result {
                CommandResult::Action(Action::SetTheme(name)) => {
                    assert_eq!(name, "groknight", "alias must normalise to canonical");
                }
                other => panic!("expected Action::SetTheme(\"groknight\"), got {other:?}"),
            }
        });
    }

    #[test]
    fn run_pi_theme_dispatches() {
        with_test_env(|| {
            let cmd = ThemeCommand;
            let models = crate::acp::model_state::ModelState::default();
            let bundle = crate::app::bundle::BundleState::default();
            let mut ctx = CommandExecCtx {
                models: &models,
                session_id: None,
                bundle_state: &bundle,
                screen_mode: crate::app::ScreenMode::Inline,
                billing_surface_visible: false,
                usage_command_visible: true,
                pager_state: crate::settings::PagerLocalSnapshot {
                    multiline_mode: false,
                    yolo_mode: false,
                    ..crate::settings::PagerLocalSnapshot::default()
                },
            };
            let result = cmd.run(&mut ctx, "pi:dark");
            match result {
                CommandResult::Action(Action::SetTheme(name)) => {
                    assert_eq!(name, "pi:dark");
                }
                other => panic!("expected Action::SetTheme(\"pi:dark\"), got {other:?}"),
            }
        });
    }

    // -- preview_arg ----------------------------------------------------------

    #[test]
    fn preview_auto_applies_resolved_theme() {
        with_test_env(|| {
            system_appearance::set_mock(Some(system_appearance::SystemAppearance::Light));
            let cmd = ThemeCommand;
            cmd.preview_arg("auto");
            // The default auto config maps Light to GrokDay
            assert_eq!(Theme::current_kind(), ThemeKind::GrokDay);
        });
    }

    #[test]
    fn preview_explicit_theme_applies_directly() {
        with_test_env(|| {
            theme_cache::set(ThemeKind::GrokNight);
            let cmd = ThemeCommand;
            cmd.preview_arg("grokday");
            assert_eq!(Theme::current_kind(), ThemeKind::GrokDay);
        });
    }

    #[test]
    fn preview_unknown_theme_is_no_op() {
        with_test_env(|| {
            theme_cache::set(ThemeKind::GrokNight);
            let cmd = ThemeCommand;
            cmd.preview_arg("nonexistent-theme");
            assert_eq!(
                Theme::current_kind(),
                ThemeKind::GrokNight,
                "unknown theme name must NOT change Theme::current_kind",
            );
        });
    }

    #[test]
    fn preview_pi_theme_sets_custom() {
        with_test_env(|| {
            theme_cache::set(ThemeKind::GrokNight);
            let cmd = ThemeCommand;
            cmd.preview_arg("pi:dark");
            assert!(theme_cache::has_custom());
            assert_eq!(Theme::current_display_id(), "pi:dark");
        });
    }

    // -- cancel_preview -------------------------------------------------------

    #[test]
    fn cancel_preview_restores_previous_kind() {
        with_test_env(|| {
            theme_cache::set(ThemeKind::GrokNight);
            let cmd = ThemeCommand;
            // Simulate user navigating into a different theme during preview.
            cmd.preview_arg("grokday");
            assert_eq!(Theme::current_kind(), ThemeKind::GrokDay);

            // Then Escape (or arg picker dismissal): restore.
            cmd.cancel_preview("groknight");
            assert_eq!(
                Theme::current_kind(),
                ThemeKind::GrokNight,
                "cancel_preview must restore the previous canonical",
            );
        });
    }

    #[test]
    fn cancel_preview_unknown_theme_is_no_op() {
        with_test_env(|| {
            theme_cache::set(ThemeKind::GrokDay);
            let cmd = ThemeCommand;
            cmd.cancel_preview("nonexistent-theme");
            assert_eq!(
                Theme::current_kind(),
                ThemeKind::GrokDay,
                "unknown previous must NOT change Theme::current_kind",
            );
        });
    }

    // -- error handling -------------------------------------------------------

    #[test]
    fn run_unknown_lists_auto_in_available() {
        with_test_env(|| {
            let cmd = ThemeCommand;
            let models = crate::acp::model_state::ModelState::default();
            let bundle = crate::app::bundle::BundleState::default();
            let mut ctx = CommandExecCtx {
                models: &models,
                session_id: None,
                bundle_state: &bundle,
                screen_mode: crate::app::ScreenMode::Inline,
                billing_surface_visible: true,
                usage_command_visible: true,
                pager_state: crate::settings::PagerLocalSnapshot {
                    multiline_mode: false,
                    yolo_mode: false,
                    ..crate::settings::PagerLocalSnapshot::default()
                },
            };
            let result = cmd.run(&mut ctx, "nonexistent");
            if let CommandResult::Error(msg) = result {
                assert!(msg.contains("auto"), "error should list auto: {msg}");
                assert!(
                    msg.contains("pi:dark"),
                    "error should list Pi themes: {msg}"
                );
            } else {
                panic!("expected Error, got: {result:?}");
            }
        });
    }

    #[test]
    fn run_truecolor_theme_dispatches_set_theme_action() {
        with_test_env(|| {
            let cmd = ThemeCommand;
            let models = crate::acp::model_state::ModelState::default();
            let bundle = crate::app::bundle::BundleState::default();
            let mut ctx = CommandExecCtx {
                models: &models,
                session_id: None,
                bundle_state: &bundle,
                screen_mode: crate::app::ScreenMode::Inline,
                billing_surface_visible: true,
                usage_command_visible: true,
                pager_state: crate::settings::PagerLocalSnapshot {
                    multiline_mode: false,
                    yolo_mode: false,
                    ..crate::settings::PagerLocalSnapshot::default()
                },
            };
            let result = cmd.run(&mut ctx, "tokyonight");
            match result {
                CommandResult::Action(Action::SetTheme(name)) => {
                    assert_eq!(
                        name, "tokyonight",
                        "truecolor themes must be accepted; clamping happens \
                         downstream in `Theme::apply_kind`",
                    );
                }
                other => panic!("expected Action::SetTheme(\"tokyonight\"), got {other:?}"),
            }
        });
    }
}
