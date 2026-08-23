//! Native Grok Build TUI backed by the Pi agent core.
//!
//! This binary is intentionally part of `xai-grok-pager-bin`, Grok Build's
//! production TUI composition package. The Pi crate is a protocol adapter only;
//! every terminal surface is created and rendered by `xai-grok-pager`.

#[path = "grok_pi/ask_user_extension.rs"]
mod ask_user_extension;
#[path = "grok_pi/auth_extension.rs"]
mod auth_extension;
#[path = "grok_pi/bash_extension.rs"]
mod bash_extension;
#[path = "grok_pi/btw_extension.rs"]
mod btw_extension;
#[path = "grok_pi/cli.rs"]
mod cli;
#[path = "grok_pi/config_skill.rs"]
mod config_skill;
#[path = "grok_pi/context_extension.rs"]
mod context_extension;
#[path = "grok_pi/export_extension.rs"]
mod export_extension;
#[path = "grok_pi/extension_self_heal.rs"]
mod extension_self_heal;
#[path = "grok_pi/goal_extension.rs"]
mod goal_extension;
#[path = "grok_pi/herdr_extension.rs"]
mod herdr_extension;
#[path = "grok_pi/home.rs"]
mod home;
#[path = "grok_pi/host_feature_extension.rs"]
mod host_feature_extension;
#[path = "grok_pi/loop_extension.rs"]
mod loop_extension;
#[path = "grok_pi/migrate_home.rs"]
mod migrate_home;
#[cfg(test)]
#[path = "grok_pi/model_manager_tests.rs"]
mod model_manager_tests;
#[path = "grok_pi/native_commands_extension.rs"]
mod native_commands_extension;
#[path = "grok_pi/pi_version.rs"]
mod pi_version;
#[path = "grok_pi/plan_mode_extension.rs"]
mod plan_mode_extension;
#[path = "grok_pi/recap_extension.rs"]
mod recap_extension;
#[path = "grok_pi/remote_tui_extension.rs"]
mod remote_tui_extension;
#[path = "grok_pi/rollback_extension.rs"]
mod rollback_extension;
#[path = "grok_pi/rpc_compat_extension.rs"]
mod rpc_compat_extension;
#[path = "grok_pi/runtime_config.rs"]
mod runtime_config;
#[path = "grok_pi/rust_tui_bridge_extension.rs"]
mod rust_tui_bridge_extension;
#[path = "grok_pi/session_paths.rs"]
mod session_paths;
#[path = "grok_pi/shortcut_manager_extension.rs"]
mod shortcut_manager_extension;
#[path = "grok_pi/subagent_extension.rs"]
mod subagent_extension;
#[path = "grok_pi/todo_extension.rs"]
mod todo_extension;
#[path = "grok_pi/tools_extension.rs"]
mod tools_extension;
#[path = "grok_pi/tree_bridge.rs"]
mod tree_bridge;
#[path = "grok_pi/tutorial_profile.rs"]
mod tutorial_profile;

use anyhow::{Context, Result};
use clap::Parser;
use pi_grok_adapter::{PiAgent, SubagentEventTransport};
use std::rc::Rc;
use tokio::task::LocalSet;
use tokio_util::sync::CancellationToken;
use xai_acp_lib::acp_channels;
use xai_grok_pager::{
    acp::{AcpConnection, ExternalLogoArt, ExternalUiProfile, ExternalWelcomeBrand},
    app::{ExternalRunConfig, PagerArgs, run_external},
    pi_resource_config::PiResourceCatalog,
    pi_resource_policy::ResourcePolicy,
};
use xai_grok_shell::host_features::{
    HostFeatureKey, HostFeatureManifest, PI_ASK_USER_QUESTION, PI_BTW, PI_GOAL, PI_HERDR, PI_LOOP,
    PI_SUBAGENTS, PI_TODO, PI_WORKFLOWS,
};

mod bundled_host_ui {
    include!(concat!(env!("OUT_DIR"), "/host_ui_catalog.rs"));
}

use ask_user_extension::write_ask_user_extension;
use auth_extension::write_auth_extension;
use bash_extension::write_bash_extension;
use btw_extension::write_btw_extension;
use cli::{Args, Command, normalize_compound_short_flags, pi_args_with_startup_flags};
use config_skill::{config_skill_enabled, sync_config_skill_cache};
use context_extension::write_context_extension;
use export_extension::write_export_extension;
use extension_self_heal::spawn_with_extension_self_heal;
#[cfg(test)]
use extension_self_heal::{bootstrap_with_deadline, disable_all_extensions};
use goal_extension::write_goal_extension;
use herdr_extension::{is_managed_pi_integration, write_herdr_extension};
use host_feature_extension::write_host_feature_extension;
use loop_extension::write_loop_extension;
use native_commands_extension::write_native_commands_extension;
use pi_version::ensure_compatible_pi_host;
use plan_mode_extension::write_plan_mode_extension;
use recap_extension::write_recap_extension;
use remote_tui_extension::write_remote_tui_extension;
use rpc_compat_extension::write_rpc_compat_extension;
use runtime_config::{
    bash_bridge_enabled, bash_control_meta_for_adapter, env_flag_default_off, env_flag_default_on,
    eval_v2_language, eval_v2_only_enabled, eval_v2_only_tool_policy_applies, eval_version,
    host_terminal_size, normal_f2_tool_policy_applies, resolve_bash_max_wait_mins,
};
use rust_tui_bridge_extension::write_rust_tui_bridge_extension;
use session_paths::pi_session_dir;
use shortcut_manager_extension::write_shortcut_manager_extension;
use subagent_extension::write_subagent_extension;
use todo_extension::write_todo_extension;
use tools_extension::{
    cli_tool_exclusions, configured_builtin_tools, disabled_builtin_tools_from_selected,
    has_no_tools_arg, merge_tool_exclusions, tool_name_allowed_by_cli, write_tools_extension,
};
use tree_bridge::write_navigate_tree_extension;

/// Grok pager commands that are meaningful when Pi is the ACP backend.
///
/// This is a composition policy, not an adapter feature. The commands below
/// are implemented by the production Grok pager or translated through its ACP
/// actions. Pi-advertised extension commands are merged dynamically.
const PI_GROK_NATIVE_COMMANDS: &[&str] = &[
    // Process and command discovery.
    "exit",
    "help",
    // Pi `/hotkeys` → native ShortcutsHelp modal (Ctrl+. surface).
    "hotkeys",
    // Upstream Pager-native onboarding modal; aliases `/tour` and `/onboarding`.
    "tutorial",
    // ACP operations with an explicit Pi implementation.
    "new",
    "compact",
    "model",
    "effort",
    "rename",
    "resume",
    // Pi `/session` stats via native Grok `/session-info` (+ alias `session`).
    "session-info",
    // Pi session entry tree via native ArgPicker + adapter navigate.
    "tree",
    // Branch map: user-messages-only fork view (native modal).
    "tree-map",
    // Pi message-level session fork (RPC get_fork_messages + fork).
    "fork",
    // Pi session clone at current leaf (RPC clone).
    "clone",
    // Pi resource reload (settings/extensions/skills/prompts/themes/context).
    "reload",
    // Process-local Pi extension notifications in a searchable native modal.
    "notify",
    // Native multi-session overview; idle rows come from pi/session/list.
    "dashboard",
    // Display-only session recap via injected Pi extension + adapter bridge.
    "recap",
    // Native /btw side questions (F2 pi_btw + pi-grok-btw extension).
    "btw",
    // Native Grok transcript/navigation surfaces over the Pi-backed session.
    "copy",
    "find",
    "jump",
    // Code review (edit/write file changes) — session + jump-style message pick.
    "review-session",
    "review-message",
    "transcript",
    "export",
    "expand",
    "queue",
    // Native Grok terminal/composer appearance controls.
    "multiline",
    "compact-mode",
    "eval-display",
    "vim-mode",
    "theme",
    "timestamps",
    "timeline",
    "toggle-mouse-reporting",
    // Pager-owned dictation writes to the native prompt; Pi still receives the
    // resulting prompt only when the user submits it.
    "voice",
    // Pager-native terminal diagnostics (`/doctor` + terminal-setup aliases).
    "doctor",
    // Pager debug overlays: `/debug [scroll|fps|log]`.
    "debug",
    // Pager-native Pi resource manager (`/pi-config`, `/pi-resources`).
    "pi-config",
    // Pager-native Pi provider/model manager with live reload.
    "pi-models",
    // Native Pi extension-shortcut manager (independent of remote-tui).
    "pi-shortcut-manager",
];

/// Block-character π mark for the native Grok welcome / minimal logo surface.
/// Matches Pi's static logo (`print_static_logo`): two-space indent + block art.
/// Kept as plain full-block art so it remains legible on terminals that cannot
/// render Grok's default braille logo. The welcome logo renderer pads rows to a
/// common visual width so per-line centering does not drift the glyph.
const PI_LOGO: &str = "\
  ██████\n\
  ██  ██\n\
  ████  ██\n\
  ██    ██\n\
";

/// Product version for `grok-pi --version` (release tag / git describe).
/// Not the upstream workspace crate version (`0.1.220-alpha.*`).
const GROK_PI_VERSION: &str = env!("GROK_PI_VERSION");
const PI_WELCOME_SUBTITLE: &str = "Pi agent core in Grok Build's native terminal UI";

fn main() -> Result<()> {
    // Isolate grok-pi state from stock Grok (`~/.grok`) before any library
    // pins `grok_home()` via OnceLock. User/test overrides of GROK_HOME win.
    home::ensure_default_grok_home();

    // Keep the exact production pager process hooks. In particular, Mermaid
    // rendering re-enters this binary with an internal worker argument and
    // therefore must be handled before clap parses the public `grok-pi` CLI.
    xai_grok_pager_minimal::install();
    if let Some(code) = xai_grok_pager::app::mermaid_worker::maybe_run_render_subprocess() {
        std::process::exit(code);
    }
    xai_crash_handler::install_terminal_restore_only();
    let _ = rustls::crypto::ring::default_provider().install_default();

    let mut args = Args::parse_from(normalize_compound_short_flags(std::env::args_os()));
    // Default host is system `pi` (min 0.80.10). Override with --pi-bin or PI_BIN.
    if args.pi_bin == "pi" {
        if let Ok(pi_bin) = std::env::var("PI_BIN") {
            if !pi_bin.trim().is_empty() {
                args.pi_bin = pi_bin;
            }
        }
    }
    if args.print_capabilities {
        println!(
            "{}",
            include_str!("../../../pi-grok-adapter/docs/capabilities.json")
        );
        return Ok(());
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to start the Grok pager Tokio runtime")?;
    if let Some(Command::Update {
        check,
        json,
        force,
        version,
    }) = args.command
    {
        return runtime.block_on(async move {
            xai_grok_update::run_pi_update(
                GROK_PI_VERSION,
                xai_grok_update::PiUpdateOptions {
                    check_only: check,
                    force,
                    version,
                    json,
                },
            )
            .await?;
            Ok(())
        });
    }
    if let Some(Command::MigrateHome {
        from,
        into,
        dry_run,
        force,
        include_auth,
        status,
    }) = args.command
    {
        return migrate_home::run_cli(from, into, dry_run, force, include_auth, status);
    }
    // One-shot safe copy when `~/.grok-pi` is empty and legacy `~/.grok` has data.
    match migrate_home::maybe_auto_migrate() {
        Ok(Some(report)) => {
            eprintln!(
                "grok-pi: migrated {} item(s) from {} → {}",
                report.copied_count(),
                home::display_home(&report.from),
                home::display_home(&report.to),
            );
            eprintln!("         re-run: grok-pi migrate-home --status");
        }
        Ok(None) => {}
        Err(err) => {
            // Never block startup on migration; user can run the subcommand.
            eprintln!("grok-pi: auto migrate-home skipped: {err}");
        }
    }
    runtime.block_on(LocalSet::new().run_until(run(args)))
}

async fn run(mut args: Args) -> Result<()> {
    let cwd = match args.pi_cwd.as_ref() {
        Some(path) => std::path::absolute(path).context("failed to resolve --pi-cwd")?,
        None => std::env::current_dir().context("failed to read current directory")?,
    };

    // Discover Pi theme JSON (embedded dark/light + ~/.pi/agent/themes + .pi/themes)
    // so `/theme` can list and apply them as `pi:<name>`.
    // Resource discovery adapts when cwd is Pi agent home (see PiResourceCatalog).
    let _theme_report = xai_grok_pager::theme::pi::init_discovery(&cwd);

    let config_skill_path = match sync_config_skill_cache(config_skill_enabled()) {
        Ok(path) => path,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "failed to sync grok-pi config skill; continuing without managed config skill"
            );
            None
        }
    };

    // Pi's --no-extensions controls auto-discovery only. Bundled host bridges
    // have a separate kill switch because they are passed as explicit paths.
    let bridge_extensions_enabled = !args.no_bridge_extensions;
    let navigate_tree_extension = bridge_extensions_enabled
        .then(|| write_navigate_tree_extension())
        .transpose()
        .context("failed to create Pi navigateTree bridge extension")?;
    // External host features are declarative: support metadata remains visible
    // in F2 even when disabled, while enabled state is resolved once at startup
    // from the same manifest that drives the composed settings registry.
    let host_feature_manifest =
        HostFeatureManifest::from_json_sources(bundled_host_ui::BUNDLED_HOST_UI_SOURCES)
            .map_err(anyhow::Error::msg)
            .context("invalid bundled extension grok-pi UI manifest")?;
    let host_feature_config = xai_grok_shell::config::load_effective_config().ok();
    let enabled_host_features = host_feature_manifest
        .iter()
        .filter(|spec| {
            bridge_extensions_enabled && spec.resolve_enabled(host_feature_config.as_ref())
        })
        .collect::<Vec<_>>();
    let host_feature_enabled =
        |key: HostFeatureKey| enabled_host_features.iter().any(|spec| spec.key == key);
    // F2 `[ui].pi_herdr` (default off). Outside Herdr the extension is a silent no-op.
    let herdr_extension = if host_feature_enabled(PI_HERDR) {
        Some(write_herdr_extension().context("failed to create Pi Herdr extension")?)
    } else {
        None
    };
    let bash_bridge_enabled = bash_bridge_enabled();
    // Eval and enhanced Bash share one physical extension bundle, but their
    // runtime switches are independent. Keep the bundle available whenever
    // bridge extensions are enabled; PI_GROK_BASH gates only Bash registration.
    let bash_extension = bridge_extensions_enabled
        .then(|| write_bash_extension())
        .transpose()
        .context("failed to create Pi Bash/Eval extension")?;
    let eval_v2_only = eval_v2_only_enabled();
    let eval_version = if eval_v2_only { "v2" } else { eval_version() };
    let eval_v2_language = eval_v2_language();
    // F2 `[ui].pi_subagents` (default on). Restart required — inject at startup only.
    let subagent_extension = if host_feature_enabled(PI_SUBAGENTS) {
        Some(write_subagent_extension().context("failed to create Pi subagent extension")?)
    } else {
        None
    };
    let subagent_transport = subagent_extension
        .as_ref()
        .map(|_| SubagentEventTransport::bind())
        .transpose()
        .context("failed to bind Pi subagent local socket")?;
    // F2 `[ui].pi_todo` (default on). Restart required — inject at startup only.
    let todo_extension = if host_feature_enabled(PI_TODO) {
        Some(write_todo_extension().context("failed to create Pi todo extension")?)
    } else {
        None
    };
    let workflows_enabled = enabled_host_features
        .iter()
        .any(|spec| spec.key == PI_WORKFLOWS);
    let workflow_extension = enabled_host_features
        .iter()
        .copied()
        .find(|spec| spec.key == PI_WORKFLOWS)
        .map(|_| write_host_feature_extension())
        .transpose()
        .context("failed to materialize registered Pi host feature")?;
    // F2 `[ui].pi_goal` (default off). Restart required — inject at startup only.
    // Control-file features keep narrow bootstrap factories; the manifest owns
    // their F2 metadata, persistence, and enablement resolution.
    let goal_extension = if host_feature_enabled(PI_GOAL) {
        Some(write_goal_extension().context("failed to create Pi goal extension")?)
    } else {
        None
    };
    // F2 `[ui].pi_loop` (default off). Restart required — inject at startup only.
    let loop_extension = if host_feature_enabled(PI_LOOP) {
        Some(write_loop_extension().context("failed to create Pi loop extension")?)
    } else {
        None
    };
    // F2 `[ui].pi_ask_user_question` (default off). Restart required — inject at startup only.
    let ask_user_extension = if host_feature_enabled(PI_ASK_USER_QUESTION) {
        Some(
            write_ask_user_extension()
                .context("failed to create Pi ask_user_question extension")?,
        )
    } else {
        None
    };
    // F2 `[ui].pi_btw` (default off). Restart required — inject at startup only.
    let btw_extension = if host_feature_enabled(PI_BTW) {
        Some(write_btw_extension().context("failed to create Pi btw extension")?)
    } else {
        None
    };
    let recap_extension = bridge_extensions_enabled
        .then(|| write_recap_extension())
        .transpose()
        .context("failed to create Pi recap extension")?;
    let context_extension = bridge_extensions_enabled
        .then(|| write_context_extension())
        .transpose()
        .context("failed to create Pi context breakdown extension")?;
    // Pi auth uses native OAuth/API-key components over Remote TUI and is
    // default-on (still needs Remote TUI). Broader pi-* selectors stay opt-in.
    let auth_extension = bridge_extensions_enabled
        .then(|| write_auth_extension())
        .transpose()
        .context("failed to create Pi auth extension")?;
    // Default-on Pi HTML export / gist share (host dist). Grok `/export` is Markdown.
    let export_extension = bridge_extensions_enabled
        .then(|| write_export_extension())
        .transpose()
        .context("failed to create Pi export extension")?;
    // Pi's interactive component internals are not a stable extension API.
    // Keep this experiment opt-in so a Pi upgrade cannot block the core RPC host.
    let native_commands_extension = (bridge_extensions_enabled
        && env_flag_default_off("PI_GROK_NATIVE_COMMANDS"))
    .then(|| write_native_commands_extension())
    .transpose()
    .context("failed to create Pi native commands extension")?;
    let remote_tui_enabled = bridge_extensions_enabled && env_flag_default_on("PI_GROK_REMOTE_TUI");
    let remote_tui_extension = if remote_tui_enabled {
        Some(write_remote_tui_extension().context("failed to create Pi remote-tui extension")?)
    } else {
        None
    };
    // RPC-compat is always injected with bridge extensions: argument-completion
    // enrichment for get_commands does not depend on Remote TUI. TUI mode rewrite
    // remains gated by PI_GROK_EXTENSION_TUI_COMPAT (set only when remote-tui on).
    let rpc_compat_extension = bridge_extensions_enabled
        .then(|| write_rpc_compat_extension())
        .transpose()
        .context("failed to create Pi RPC compatibility extension")?;
    let shortcut_manager_extension = if remote_tui_enabled {
        Some(
            write_shortcut_manager_extension()
                .context("failed to create Pi shortcut manager extension")?,
        )
    } else {
        None
    };
    // Optional experimental Rust TUI bridge (does not replace remote-tui).
    let rust_tui_bridge_extension =
        if remote_tui_enabled && env_flag_default_off("PI_GROK_RUST_TUI_BRIDGE") {
            Some(
                write_rust_tui_bridge_extension()
                    .context("failed to create Pi Rust TUI bridge extension")?,
            )
        } else {
            None
        };
    let plan_mode_extension = bridge_extensions_enabled
        .then(|| write_plan_mode_extension())
        .transpose()
        .context("failed to create Pi plan-mode extension")?;
    // Resolve session dir after first-class flags are merged so --session-dir
    // is visible whether it came from clap or from `--` passthrough.
    let mut pi_args = pi_args_with_startup_flags(
        std::mem::take(&mut args.pi_args),
        &args,
        navigate_tree_extension
            .as_ref()
            .map(|extension| extension.path()),
    );
    let pi_session_dir = pi_session_dir(&pi_args, &cwd);

    // The embedded config skill lives outside Pi's native auto-discovery tree,
    // so load it explicitly. Respect an explicit --no-skills from either the
    // first-class CLI or passthrough args; F2 pi_config_skill remains the
    // default-on source of truth otherwise.
    let skills_disabled = args.no_skills || pi_args.iter().any(|arg| arg == "--no-skills");
    if !skills_disabled && let Some(path) = config_skill_path.as_ref() {
        pi_args.extend(["--skill".to_string(), path.to_string_lossy().into_owned()]);
    }

    // ── Resource admission policy ────────────────────────────────────────────
    // Disable Pi's auto-discovery and load only policy-approved resources.
    // Bridge extensions (subagent, bash, recap, etc.) are appended separately
    // below and always load regardless of policy.
    let mut resource_policy = ResourcePolicy::load_from_config();
    // Feature-gated package blocks (assets/native_feature_conflicts.toml).
    // Enabled host features contribute their declared conflict keys; todo
    // additionally forces its native tool registration.
    for spec in &enabled_host_features {
        if let Some(feature_key) = spec.native_feature_key {
            resource_policy
                .enabled_native_features
                .push(feature_key.to_owned());
        }
    }
    if todo_extension.is_some() {
        resource_policy
            .forced_native_features
            .push("pi_todo".to_owned());
    }
    // Mirror Pi's --approve / --no-approve so the catalog's project-resource
    // discovery matches what Pi itself will trust for this run.
    // (Agent-home cwd is handled inside PiResourceCatalog::load_with_trust.)
    let trust_override = if args.approve {
        Some(true)
    } else if args.no_approve {
        Some(false)
    } else {
        None
    };
    let resource_catalog = PiResourceCatalog::load_with_trust(cwd.clone(), trust_override)
        .context("failed to load Pi resource catalog for admission policy")?;
    let launch_plan = resource_policy.evaluate(&resource_catalog);
    if let Some(summary) = launch_plan.blocked_summary() {
        tracing::warn!("{summary}");
    }

    // Filter explicit --extension paths (from -e / --extension / passthrough)
    // that the policy would block.  These were written into pi_args by
    // pi_args_with_startup_flags() before the catalog evaluation ran.
    {
        let mut filtered_args: Vec<String> = Vec::with_capacity(pi_args.len());
        let mut i = 0;
        while i < pi_args.len() {
            if pi_args[i] == "--extension" && i + 1 < pi_args.len() {
                let ext_path = &pi_args[i + 1];
                if let Some(reason) = resource_policy.check_explicit_path(ext_path) {
                    tracing::warn!(
                        "grok-pi resource policy blocked explicit extension {ext_path}: {reason}"
                    );
                    i += 2; // skip both --extension and its value
                    continue;
                }
            }
            filtered_args.push(pi_args[i].clone());
            i += 1;
        }
        pi_args = filtered_args;
    }

    // Pi loads explicit extensions in argument order. Install the mode facade
    // and its Remote TUI host before any third-party resource is loaded. They
    // bypass the user-resource policy just like the other host bridge files.
    let mut startup_extensions = Vec::new();
    for path in [
        rpc_compat_extension
            .as_ref()
            .map(|extension| extension.path()),
        herdr_extension.as_ref().map(|extension| extension.path()),
        remote_tui_extension
            .as_ref()
            .map(|extension| extension.source_path()),
        shortcut_manager_extension
            .as_ref()
            .map(|extension| extension.source_path()),
        rust_tui_bridge_extension
            .as_ref()
            .map(|extension| extension.path()),
    ]
    .into_iter()
    .flatten()
    {
        startup_extensions.extend([
            "--extension".to_string(),
            path.to_string_lossy().into_owned(),
        ]);
    }
    pi_args.splice(0..0, startup_extensions);

    // Disable Pi auto-discovery only for resources governed by the host
    // admission policy. Skills stay Pi-owned so settings changes and /reload
    // can enable or disable them without restarting grok-pi.
    // Respect the user's own --no-* CLI flags (both Clap and passthrough):
    // if they already disabled a category, don't re-add approved resources.
    let has_no_extensions = args.no_extensions || pi_args.iter().any(|a| a == "--no-extensions");
    let has_no_prompts = pi_args.iter().any(|a| a == "--no-prompt-templates");
    let has_no_themes = pi_args.iter().any(|a| a == "--no-themes");

    if !has_no_extensions {
        pi_args.push("--no-extensions".to_string());
        for path in &launch_plan.extensions {
            // The built-in bridge owns Herdr's authoritative `herdr:pi` source.
            // Skip only Herdr-managed auto-discovery; explicit --extension paths
            // were already preserved above and remain user-authoritative.
            if herdr_extension.is_some() && is_managed_pi_integration(path) {
                continue;
            }
            pi_args.extend([
                "--extension".to_string(),
                path.to_string_lossy().into_owned(),
            ]);
        }
    }
    if !has_no_prompts {
        pi_args.push("--no-prompt-templates".to_string());
        for path in &launch_plan.prompts {
            pi_args.extend([
                "--prompt-template".to_string(),
                path.to_string_lossy().into_owned(),
            ]);
        }
    }
    if !has_no_themes {
        pi_args.push("--no-themes".to_string());
        for path in &launch_plan.themes {
            pi_args.extend(["--theme".to_string(), path.to_string_lossy().into_owned()]);
        }
    }

    // Eval-v2-only is a strong F2 isolation mode: keep Pi's registry intact,
    // then let the host extension collapse the top-level active set to `eval`.
    // Explicit user --tools or --no-tools still wins; --no-builtin-tools remains
    // compatible because Eval is an extension tool, not an upstream Pi builtin.
    let eval_v2_only_tool_policy_applied =
        eval_v2_only_tool_policy_applies(&pi_args, bridge_extensions_enabled, eval_v2_only);

    // CLI tool restrictions (--tools, --no-tools, --no-builtin-tools,
    // --exclude-tools) are authoritative and always override normal F2 preferences.
    // When normal F2 owns the selection, disabled built-in names are also merged
    // into Pi's native --exclude-tools denylist. Eval-v2-only deliberately skips
    // this saved preference for the current process so nested Eval retains the
    // registry; explicit CLI exclusions remain authoritative. The tools extension
    // remains responsible for normal-mode activation of non-default tools.
    let f2_tools_enabled = normal_f2_tool_policy_applies(
        &pi_args,
        bridge_extensions_enabled,
        eval_v2_only_tool_policy_applied,
    );
    let selected_builtin_tools = f2_tools_enabled.then(configured_builtin_tools);
    let cli_exclusions = if let Some(selected) = selected_builtin_tools.as_deref() {
        let disabled = disabled_builtin_tools_from_selected(selected);
        merge_tool_exclusions(&mut pi_args, &disabled)
    } else {
        cli_tool_exclusions(&pi_args)
    };
    let tools_extension = f2_tools_enabled
        .then(|| write_tools_extension())
        .transpose()
        .context("failed to create Pi tools extension")?;
    let bash_bridge_runtime_enabled =
        bash_bridge_enabled && tool_name_allowed_by_cli(&pi_args, "bash");
    // Tree file rollback checkpoint extension: injected last so it can verify
    // that write/edit are still Pi builtin (not overridden by user extensions).
    // Only when F2 enabled and CLI hasn't disabled write/edit tools.
    let rollback_on = bridge_extensions_enabled
        && rollback_extension::rollback_enabled()
        && !has_no_tools_arg(&pi_args);
    let rollback_control_dir = if rollback_on {
        Some(rollback_extension::create_control_dir()?)
    } else {
        None
    };
    let rollback_ext = rollback_on
        .then(|| rollback_extension::write_rollback_extension())
        .transpose()
        .context("failed to create Pi rollback extension")?;
    // The subagent configuration screen needs a static catalog for extensions
    // that do not register a tool (for example, host-only Pi-Grok bridges).
    // Keep this strictly to extensions admitted for this process; a child only
    // loads an entry if its Markdown definition explicitly selects it.
    let mut subagent_extension_catalog = Vec::new();
    let mut add_subagent_extension = |label: &str, path: Option<std::path::PathBuf>| {
        if let Some(path) = path {
            if !subagent_extension_catalog
                .iter()
                .any(|entry: &serde_json::Value| entry["path"] == path.to_string_lossy().as_ref())
            {
                subagent_extension_catalog.push(serde_json::json!({
                    "path": path,
                    "label": label,
                    "description": "Pi-Grok extension admitted for this session",
                }));
            }
        }
    };
    add_subagent_extension(
        "grok-pi: navigate tree",
        navigate_tree_extension
            .as_ref()
            .map(|extension| extension.path().to_path_buf()),
    );
    add_subagent_extension(
        "grok-pi: RPC compatibility",
        rpc_compat_extension
            .as_ref()
            .map(|extension| extension.path().to_path_buf()),
    );
    add_subagent_extension(
        "grok-pi: Herdr",
        herdr_extension
            .as_ref()
            .map(|extension| extension.path().to_path_buf()),
    );
    add_subagent_extension(
        "grok-pi: remote TUI",
        remote_tui_extension
            .as_ref()
            .map(|extension| extension.source_path().to_path_buf()),
    );
    add_subagent_extension(
        "grok-pi: shortcut manager",
        shortcut_manager_extension
            .as_ref()
            .map(|extension| extension.source_path().to_path_buf()),
    );
    add_subagent_extension(
        "grok-pi: Rust TUI bridge",
        rust_tui_bridge_extension
            .as_ref()
            .map(|extension| extension.path().to_path_buf()),
    );
    add_subagent_extension(
        "grok-pi: subagents",
        subagent_extension
            .as_ref()
            .map(|extension| extension.source_path().to_path_buf()),
    );
    add_subagent_extension(
        "grok-pi: todo",
        todo_extension
            .as_ref()
            .map(|extension| extension.path().to_path_buf()),
    );
    add_subagent_extension(
        "grok-pi: workflows",
        workflow_extension
            .as_ref()
            .map(|extension| extension.path().to_path_buf()),
    );
    add_subagent_extension(
        "grok-pi: goal",
        goal_extension
            .as_ref()
            .map(|extension| extension.source_path().to_path_buf()),
    );
    add_subagent_extension(
        "grok-pi: loop",
        loop_extension
            .as_ref()
            .map(|extension| extension.source_path().to_path_buf()),
    );
    add_subagent_extension(
        "grok-pi: ask user",
        ask_user_extension
            .as_ref()
            .map(|extension| extension.source_path().to_path_buf()),
    );
    add_subagent_extension(
        "grok-pi: btw",
        btw_extension
            .as_ref()
            .map(|extension| extension.source_path().to_path_buf()),
    );
    add_subagent_extension(
        "grok-pi: recap",
        recap_extension
            .as_ref()
            .map(|extension| extension.source_path().to_path_buf()),
    );
    add_subagent_extension(
        "grok-pi: context breakdown",
        context_extension
            .as_ref()
            .map(|extension| extension.source_path().to_path_buf()),
    );
    add_subagent_extension(
        "grok-pi: auth",
        auth_extension
            .as_ref()
            .map(|extension| extension.source_path().to_path_buf()),
    );
    add_subagent_extension(
        "grok-pi: export",
        export_extension
            .as_ref()
            .map(|extension| extension.path().to_path_buf()),
    );
    add_subagent_extension(
        "grok-pi: native commands",
        native_commands_extension
            .as_ref()
            .map(|extension| extension.path().to_path_buf()),
    );
    add_subagent_extension(
        "grok-pi: Bash/Eval",
        bash_extension
            .as_ref()
            .map(|extension| extension.source_path().to_path_buf()),
    );
    add_subagent_extension(
        "grok-pi: tools",
        tools_extension
            .as_ref()
            .map(|extension| extension.path().to_path_buf()),
    );
    add_subagent_extension(
        "grok-pi: rollback",
        rollback_ext
            .as_ref()
            .map(|extension| extension.source_path().to_path_buf()),
    );
    add_subagent_extension(
        "grok-pi: plan mode",
        plan_mode_extension
            .as_ref()
            .map(|extension| extension.source_path().to_path_buf()),
    );
    for path in &launch_plan.extensions {
        let label = format!("Pi extension: {}", path.display());
        add_subagent_extension(&label, Some(path.to_path_buf()));
    }
    // remote_tui before auth/native-commands so custom() host exists first.
    for path in [
        subagent_extension
            .as_ref()
            .map(|extension| extension.source_path()),
        todo_extension.as_ref().map(|extension| extension.path()),
        workflow_extension
            .as_ref()
            .map(|extension| extension.path()),
        goal_extension
            .as_ref()
            .map(|extension| extension.source_path()),
        loop_extension
            .as_ref()
            .map(|extension| extension.source_path()),
        ask_user_extension
            .as_ref()
            .map(|extension| extension.source_path()),
        btw_extension
            .as_ref()
            .map(|extension| extension.source_path()),
        recap_extension
            .as_ref()
            .map(|extension| extension.source_path()),
        context_extension
            .as_ref()
            .map(|extension| extension.source_path()),
        auth_extension
            .as_ref()
            .map(|extension| extension.source_path()),
        export_extension.as_ref().map(|extension| extension.path()),
        native_commands_extension
            .as_ref()
            .map(|extension| extension.path()),
        bash_extension
            .as_ref()
            .map(|extension| extension.source_path()),
        tools_extension.as_ref().map(|extension| extension.path()),
        // Rollback extension observes the final built-in registrations.
        rollback_ext
            .as_ref()
            .map(|extension| extension.source_path()),
        // Plan gate runs after all tool registrations and owns no renderer/UI.
        plan_mode_extension
            .as_ref()
            .map(|extension| extension.source_path()),
    ]
    .into_iter()
    .flatten()
    {
        pi_args.extend([
            "--extension".to_string(),
            path.to_string_lossy().into_owned(),
        ]);
    }
    // Identifies this Pi child as running under the grok-pi host for user extensions.
    let mut env = vec![("PI_GROK".to_string(), "1".to_string())];
    // Only the host-owned eval-v2-only policy may widen Eval nested access.
    // Explicit --tools/--no-tools leave this marker off; native registry exclusions
    // such as --exclude-tools and --no-builtin-tools remain authoritative.
    env.push((
        "PI_GROK_EVAL_V2_ONLY".to_string(),
        if eval_v2_only_tool_policy_applied {
            "1"
        } else {
            "0"
        }
        .to_string(),
    ));
    if recap_extension.is_some() {
        env.push(("PI_GROK_RECAP".to_string(), "1".to_string()));
        // SAFETY: single-threaded startup; the adapter advertises this capability.
        unsafe {
            std::env::set_var("PI_GROK_RECAP", "1");
        }
    } else {
        // Avoid advertising recap when its host bridge is disabled.
        unsafe {
            std::env::remove_var("PI_GROK_RECAP");
        }
    }
    if let Some(selected) = selected_builtin_tools {
        env.push(("PI_GROK_BUILTIN_TOOLS".to_string(), selected));
    }
    if !cli_exclusions.is_empty() && tools_extension.is_some() {
        env.push(("PI_GROK_EXCLUDE_TOOLS".to_string(), cli_exclusions));
    }
    if subagent_extension.is_some() {
        env.push(("PI_GROK_SUBAGENTS".to_string(), "1".to_string()));
        env.push((
            "PI_GROK_SUBAGENT_SOCKET".to_string(),
            subagent_transport
                .as_ref()
                .expect("subagent transport exists with extension")
                .endpoint()
                .to_string(),
        ));
        env.push((
            "PI_GROK_SUBAGENT_EXTENSION_CATALOG".to_string(),
            serde_json::to_string(&subagent_extension_catalog)
                .expect("Pi-Grok extension catalog must be serializable"),
        ));
    } else {
        // Override inherited shell values so an explicitly loaded copy of the
        // bundled extension still respects the F2 off switch and cannot see a
        // stale child-extension catalog from a parent process.
        env.push(("PI_GROK_SUBAGENTS".to_string(), "0".to_string()));
        env.push((
            "PI_GROK_SUBAGENT_EXTENSION_CATALOG".to_string(),
            "[]".to_string(),
        ));
    }
    // Push every spec's startup env in both states so inherited shell values
    // cannot leak into the Pi child (e.g. a stale PI_GROK_TODO_VERSION=v2
    // surviving an F2 rollback to V1).
    for spec in host_feature_manifest.iter() {
        let enabled =
            bridge_extensions_enabled && enabled_host_features.iter().any(|e| e.key == spec.key);
        if let Some((key, value)) = spec.startup_env_override(enabled) {
            env.push((key.to_string(), value.to_string()));
        }
    }
    if let Some(extension) = goal_extension.as_ref() {
        env.push(("PI_GROK_GOAL".to_string(), "1".to_string()));
        env.push((
            "PI_GROK_GOAL_CONTROL".to_string(),
            extension.control_path().to_string_lossy().into_owned(),
        ));
        unsafe {
            std::env::set_var("PI_GROK_GOAL", "1");
        }
    } else {
        unsafe {
            std::env::remove_var("PI_GROK_GOAL");
        }
    }
    if let Some(extension) = loop_extension.as_ref() {
        env.push(("PI_GROK_LOOP".to_string(), "1".to_string()));
        env.push((
            "PI_GROK_LOOP_CONTROL".to_string(),
            extension.control_path().to_string_lossy().into_owned(),
        ));
        unsafe {
            std::env::set_var("PI_GROK_LOOP", "1");
        }
    } else {
        unsafe {
            std::env::remove_var("PI_GROK_LOOP");
        }
    }
    if let Some(extension) = ask_user_extension.as_ref() {
        let dir = extension.dir_path().to_string_lossy().into_owned();
        env.push(("PI_GROK_ASK_USER".to_string(), "1".to_string()));
        env.push(("PI_GROK_ASK_USER_DIR".to_string(), dir.clone()));
        // SAFETY: single-threaded startup; parent adapter reads the same dir.
        unsafe {
            std::env::set_var("PI_GROK_ASK_USER", "1");
            std::env::set_var("PI_GROK_ASK_USER_DIR", &dir);
        }
    } else {
        unsafe {
            std::env::remove_var("PI_GROK_ASK_USER");
            std::env::remove_var("PI_GROK_ASK_USER_DIR");
        }
    }
    if let Some(context_extension) = context_extension.as_ref() {
        env.push((
            "PI_GROK_CONTEXT_BREAKDOWN".to_string(),
            context_extension
                .breakdown_path()
                .to_string_lossy()
                .into_owned(),
        ));
    }
    if let Some(extension) = bash_extension.as_ref() {
        let inherited_bash_max_wait_mins = std::env::var("PI_GROK_BASH_MAX_WAIT_MINS").ok();
        let bash_max_wait_mins = resolve_bash_max_wait_mins(
            args.bash_max_wait_mins,
            inherited_bash_max_wait_mins.as_deref(),
        );
        env.push((
            "PI_GROK_BASH".to_string(),
            if bash_bridge_runtime_enabled {
                "1"
            } else {
                "0"
            }
            .to_string(),
        ));
        env.push(("PI_GROK_EVAL_VERSION".to_string(), eval_version.to_string()));
        env.push((
            "PI_GROK_EVAL_V2_LANGUAGE".to_string(),
            eval_v2_language.to_string(),
        ));
        env.push(("PI_GROK_BASH_MAX_WAIT_MINS".to_string(), bash_max_wait_mins));
        env.push((
            "PI_GROK_BASH_CONTROL_META".to_string(),
            if bash_bridge_runtime_enabled {
                extension.control_meta_path().to_string_lossy().into_owned()
            } else {
                String::new()
            },
        ));
    }
    if remote_tui_enabled {
        // The compatibility facade is safe only while this host can service
        // custom component requests.
        env.push(("PI_GROK_REMOTE_TUI".to_string(), "1".to_string()));
        env.push(("PI_GROK_EXTENSION_TUI_COMPAT".to_string(), "1".to_string()));
        // Pi RPC child has no real TTY; pass host size so Remote TUI is full-width
        // like interactive Pi (not a fixed 72-col probe box).
        if let Some((cols, rows)) = host_terminal_size() {
            env.push(("COLUMNS".to_string(), cols.to_string()));
            env.push(("LINES".to_string(), rows.to_string()));
            env.push(("PI_GROK_REMOTE_TUI_WIDTH".to_string(), cols.to_string()));
            env.push(("PI_GROK_REMOTE_TUI_ROWS".to_string(), rows.to_string()));
        }
        // Instance-scoped shortcut dispatch keyfile (parent adapter + Pi child).
        // Avoids global meta races when multiple grok-pi processes run.
        let shortcut_keys = std::env::temp_dir().join(format!(
            "pi-grok-shortcut-keys-host-{}.jsonl",
            std::process::id()
        ));
        if let Err(err) = std::fs::write(&shortcut_keys, b"") {
            tracing::warn!(%err, "failed to create shortcut dispatch keyfile");
        } else {
            let keys = shortcut_keys.to_string_lossy().into_owned();
            env.push(("PI_GROK_SHORTCUT_KEYS".to_string(), keys.clone()));
            // SAFETY: single-threaded startup; adapter reads same path.
            unsafe {
                std::env::set_var("PI_GROK_SHORTCUT_KEYS", &keys);
            }
        }
    }
    // Tree file rollback checkpoint extension env.
    if let Some(extension) = plan_mode_extension.as_ref() {
        env.push((
            "PI_GROK_PLAN_CONTROL".to_string(),
            extension.control_path().to_string_lossy().into_owned(),
        ));
    }
    if rollback_ext.is_some() {
        env.push(("PI_GROK_ROLLBACK".to_string(), "1".to_string()));
        env.push((
            "GROK_PI_ROLLBACK_STATE".to_string(),
            rollback_extension::rollback_state_root(),
        ));
        if let Some(ref control) = rollback_control_dir {
            env.push(("GROK_PI_ROLLBACK_CONTROL".to_string(), control.clone()));
        }
    }
    // Fail fast with OS-aware install hints before spawning the RPC host.
    // On Windows this also rewrites bare `pi` → absolute `pi.cmd` for CreateProcess.
    let (_pi_version, resolved_pi_bin) =
        ensure_compatible_pi_host(&args.pi_bin).context("Pi host version check failed")?;
    args.pi_bin = resolved_pi_bin;

    // ── Extension self-heal: spawn Pi, and if an extension crashes the RPC
    // child during bootstrap, binary-search the culprit (VSCode-style),
    // print a diagnostic, and relaunch without it. ──────────────────────────
    let (process, bootstrap, _pi_args) =
        spawn_with_extension_self_heal(&args, &cwd, pi_args, &env).await?;

    if btw_extension.is_some() {
        env.push(("PI_GROK_BTW".to_string(), "1".to_string()));
        unsafe {
            std::env::set_var("PI_GROK_BTW", "1");
        }
    } else {
        unsafe {
            std::env::remove_var("PI_GROK_BTW");
        }
    }
    let bash_control_meta =
        bash_control_meta_for_adapter(bash_bridge_runtime_enabled, bash_extension.as_ref());
    let context_breakdown = context_extension
        .as_ref()
        .map(|extension| extension.breakdown_path().to_path_buf());
    let plan_mode_control = plan_mode_extension
        .as_ref()
        .map(|extension| extension.control_path().to_path_buf());
    let goal_control = goal_extension
        .as_ref()
        .map(|extension| extension.control_path().to_path_buf());
    // Hold the NamedTempFiles so the extension paths remain valid.
    let _navigate_tree_extension = navigate_tree_extension;
    let _bash_extension = bash_extension;
    let _subagent_extension = subagent_extension;
    let _btw_extension = btw_extension;
    let _recap_extension = recap_extension;
    let _context_extension = context_extension;
    let _auth_extension = auth_extension;
    let _export_extension = export_extension;
    let _native_commands_extension = native_commands_extension;
    let _remote_tui_extension = remote_tui_extension;
    let _rpc_compat_extension = rpc_compat_extension;
    let _shortcut_manager_extension = shortcut_manager_extension;
    let _rust_tui_bridge_extension = rust_tui_bridge_extension;
    let _plan_mode_extension = plan_mode_extension;
    let _goal_extension = goal_extension;
    let _loop_extension = loop_extension;
    let _tools_extension = tools_extension;
    let _rollback_extension = rollback_ext;

    let initial_models = bootstrap.acp_models();
    let initial_commands = bootstrap.acp_commands(workflows_enabled);
    let session_id = bootstrap.session_id().to_string();
    let session_title = bootstrap
        .session_title()
        .map(str::to_owned)
        .or_else(|| Some("Pi".to_string()));

    let (client_channel, mut agent_channel) = acp_channels();
    let adapter = Rc::new(
        PiAgent::new(
            process.rpc,
            agent_channel.tx.clone(),
            bootstrap,
            pi_session_dir,
            bash_control_meta,
            context_breakdown,
            plan_mode_control,
            goal_control,
            subagent_transport,
            workflows_enabled,
            eval_v2_only_tool_policy_applied,
        )
        .context("failed to restore Pi plan-mode state")?,
    );

    let event_adapter = adapter.clone();
    tokio::task::spawn_local(async move {
        event_adapter.run_events(process.events).await;
    });

    let route_adapter = adapter.clone();
    tokio::task::spawn_local(async move {
        while let Some(message) = agent_channel.rx.recv().await {
            message.route_to_agent(route_adapter.clone(), |future| {
                tokio::task::spawn_local(future);
            });
        }
    });

    let command_profile = PI_GROK_NATIVE_COMMANDS
        .iter()
        .map(|name| (*name).to_string())
        .collect::<Vec<_>>();
    // Keep the upstream tutorial UI/state machine, but install grok-pi product
    // copy before the Pager constructs its slash registry or first modal.
    tutorial_profile::install();
    // External ACP skips shell `initialize`, so recap must be enabled here.
    // Adapter still implements initialize.meta.sessionRecap for non-external
    // paths; `/recap` stays hidden until this flag is true.
    let mut connection = AcpConnection::external(
        client_channel.tx,
        client_channel.rx,
        initial_models,
        initial_commands,
        CancellationToken::new(),
        ExternalUiProfile {
            agent_name: "Pi".to_string(),
            builtin_commands: command_profile.clone(),
            logo: Some(ExternalLogoArt {
                full: PI_LOGO,
                small: PI_LOGO,
            }),
            welcome_brand: Some(ExternalWelcomeBrand {
                title: "grok-pi",
                subtitle: PI_WELCOME_SUBTITLE,
                version: GROK_PI_VERSION,
            }),
            // Grok worktree product flow is not wired for Pi yet.
            hide_new_worktree: true,
            changelog_url: Some("https://github.com/Dwsy/grok-pi/blob/main/CHANGELOG.MD"),
            enable_voice_dictation: true,
            host_features: host_feature_manifest.clone(),
        },
    );
    connection.session_recap_available = true;

    let mut pager_args = PagerArgs::parse_from(["grok-pi"]);
    pager_args.cwd = Some(cwd.clone());
    pager_args.no_alt_screen = args.no_alt_screen;
    pager_args.minimal = args.minimal;
    pager_args.fullscreen = args.fullscreen;
    // Enable the Pi-specific update check (GitHub Releases only).
    // Set GROK_PI_NO_AUTO_UPDATE=1 to disable the background check.
    pager_args.no_auto_update = std::env::var_os("GROK_PI_NO_AUTO_UPDATE").is_some();

    // Skip Welcome when Pi already selected a concrete session (--continue,
    // --session path|uuid, --session-id, or --fork). Fresh starts stay on Welcome.
    let resume_existing_session = args.continue_last_session
        || args.session.is_some()
        || args.session_id.is_some()
        || args.fork.is_some();

    run_external(ExternalRunConfig {
        args: pager_args,
        connection,
        session_id,
        session_title,
        session_cwd: Some(cwd),
        resume_existing_session,
        // Ephemeral runs cannot be resumed from disk.
        emit_resume_hint: !args.no_session,
        resume_session_dir: args.session_dir.clone(),
        product_version: GROK_PI_VERSION.to_string(),
    })
    .await
}

#[cfg(test)]
mod env_flag_tests {
    use super::runtime_config::{
        bash_bridge_enabled_from_config, eval_v2_only_enabled_from_config, eval_version_from_config,
    };
    use super::{
        Args, PI_GROK_NATIVE_COMMANDS, bash_control_meta_for_adapter, bootstrap_with_deadline,
        disable_all_extensions, env_flag_default_off, env_flag_default_on,
        eval_v2_only_tool_policy_applies, normal_f2_tool_policy_applies,
        resolve_bash_max_wait_mins,
    };
    use clap::Parser;

    #[tokio::test]
    async fn bootstrap_deadline_bounds_a_stuck_extension_startup() {
        let error = bootstrap_with_deadline(
            std::future::pending::<anyhow::Result<()>>(),
            std::time::Duration::from_millis(10),
        )
        .await
        .expect_err("pending bootstrap must time out");

        assert!(error.to_string().contains("Pi RPC bootstrap timed out"));
    }

    #[test]
    fn bash_bridge_defaults_on_and_honors_explicit_off() {
        assert!(bash_bridge_enabled_from_config(None));

        let missing: toml::Value = toml::from_str("[ui]\n").expect("parse missing config");
        assert!(bash_bridge_enabled_from_config(Some(&missing)));

        let enabled: toml::Value =
            toml::from_str("[ui]\npi_bash = true\n").expect("parse enabled config");
        assert!(bash_bridge_enabled_from_config(Some(&enabled)));

        let disabled: toml::Value =
            toml::from_str("[ui]\npi_bash = false\n").expect("parse disabled config");
        assert!(!bash_bridge_enabled_from_config(Some(&disabled)));
    }

    #[test]
    fn bash_control_meta_is_hidden_when_bash_bridge_is_off() {
        let extension = super::write_bash_extension().expect("write Bash/Eval extension");
        assert!(bash_control_meta_for_adapter(false, Some(&extension)).is_none());
        assert_eq!(
            bash_control_meta_for_adapter(true, Some(&extension)).as_deref(),
            Some(extension.control_meta_path()),
        );
        assert!(bash_control_meta_for_adapter(true, None).is_none());
    }

    #[test]
    fn bash_max_wait_mins_prefers_cli_then_env_then_default() {
        assert_eq!(resolve_bash_max_wait_mins(None, None), "4.5");
        assert_eq!(resolve_bash_max_wait_mins(None, Some("3.75")), "3.75");
        assert_eq!(resolve_bash_max_wait_mins(Some(2.5), Some("3.75")), "2.5");
        assert_eq!(resolve_bash_max_wait_mins(Some(0.0), Some("3.75")), "0");
        assert_eq!(resolve_bash_max_wait_mins(Some(-1.0), None), "-1");
    }

    #[test]
    fn eval_bridge_defaults_v1_and_only_explicit_v2_opts_in() {
        assert_eq!(eval_version_from_config(None), "v1");

        let missing: toml::Value = toml::from_str("[ui]\n").expect("parse missing config");
        assert_eq!(eval_version_from_config(Some(&missing)), "v1");

        let v1: toml::Value = toml::from_str("[ui]\npi_eval = \"v1\"\n").expect("parse v1 config");
        assert_eq!(eval_version_from_config(Some(&v1)), "v1");

        let v2: toml::Value = toml::from_str("[ui]\npi_eval = \"v2\"\n").expect("parse v2 config");
        assert_eq!(eval_version_from_config(Some(&v2)), "v2");

        let invalid: toml::Value =
            toml::from_str("[ui]\npi_eval = \"both\"\n").expect("parse invalid config");
        assert_eq!(eval_version_from_config(Some(&invalid)), "v1");
    }

    #[test]
    fn eval_v2_only_defaults_off_and_honors_explicit_true() {
        assert!(!eval_v2_only_enabled_from_config(None));

        let missing: toml::Value = toml::from_str("[ui]\n").expect("parse missing config");
        assert!(!eval_v2_only_enabled_from_config(Some(&missing)));

        let enabled: toml::Value =
            toml::from_str("[ui]\npi_eval_v2_only = true\n").expect("parse enabled config");
        assert!(eval_v2_only_enabled_from_config(Some(&enabled)));

        let disabled: toml::Value =
            toml::from_str("[ui]\npi_eval_v2_only = false\n").expect("parse disabled config");
        assert!(!eval_v2_only_enabled_from_config(Some(&disabled)));
    }

    #[test]
    fn eval_v2_only_tool_policy_isolates_model_without_filtering_registry() {
        let args = Vec::new();
        assert!(eval_v2_only_tool_policy_applies(&args, true, true));
        assert!(args.is_empty());
        assert!(!normal_f2_tool_policy_applies(&args, true, true));
        assert!(normal_f2_tool_policy_applies(&args, true, false));

        let disabled = Vec::new();
        assert!(!eval_v2_only_tool_policy_applies(&disabled, true, false));

        let no_bridge = Vec::new();
        assert!(!eval_v2_only_tool_policy_applies(&no_bridge, false, true));
        assert!(!normal_f2_tool_policy_applies(&no_bridge, false, false));

        let explicit = vec!["--tools".to_string(), "read,eval".to_string()];
        assert!(!eval_v2_only_tool_policy_applies(&explicit, true, true));
        assert!(!normal_f2_tool_policy_applies(&explicit, true, false));

        let no_tools = vec!["--no-tools".to_string()];
        assert!(!eval_v2_only_tool_policy_applies(&no_tools, true, true));
        assert!(!normal_f2_tool_policy_applies(&no_tools, true, false));

        let no_builtins = vec!["--no-builtin-tools".to_string()];
        assert!(eval_v2_only_tool_policy_applies(&no_builtins, true, true));
        assert!(!normal_f2_tool_policy_applies(&no_builtins, true, true));
    }

    #[test]
    fn default_on_when_unset() {
        // SAFETY: test-only env mutation in this unit test process.
        unsafe {
            std::env::remove_var("PI_GROK_TEST_FLAG_DEFAULT_ON");
        }
        assert!(env_flag_default_on("PI_GROK_TEST_FLAG_DEFAULT_ON"));
    }

    #[test]
    fn grok_pi_command_profile_includes_native_navigation() {
        assert!(PI_GROK_NATIVE_COMMANDS.contains(&"jump"));
        assert!(PI_GROK_NATIVE_COMMANDS.contains(&"review-session"));
        assert!(PI_GROK_NATIVE_COMMANDS.contains(&"review-message"));
        assert!(PI_GROK_NATIVE_COMMANDS.contains(&"voice"));
        assert!(PI_GROK_NATIVE_COMMANDS.contains(&"doctor"));
        assert!(PI_GROK_NATIVE_COMMANDS.contains(&"debug"));
        assert!(PI_GROK_NATIVE_COMMANDS.contains(&"hotkeys"));
        assert!(PI_GROK_NATIVE_COMMANDS.contains(&"tutorial"));
        assert!(PI_GROK_NATIVE_COMMANDS.contains(&"session-info"));
        assert!(PI_GROK_NATIVE_COMMANDS.contains(&"tree"));
        assert!(PI_GROK_NATIVE_COMMANDS.contains(&"fork"));
        assert!(PI_GROK_NATIVE_COMMANDS.contains(&"clone"));
        assert!(PI_GROK_NATIVE_COMMANDS.contains(&"reload"));
        assert!(PI_GROK_NATIVE_COMMANDS.contains(&"pi-shortcut-manager"));
    }

    #[test]
    fn native_commands_default_off() {
        // SAFETY: test-only env mutation in this unit test process.
        unsafe {
            std::env::remove_var("PI_GROK_NATIVE_COMMANDS");
        }
        assert!(!env_flag_default_off("PI_GROK_NATIVE_COMMANDS"));
    }

    #[test]
    fn experimental_flags_require_an_explicit_opt_in() {
        // SAFETY: test-only env mutation in this unit test process.
        unsafe {
            std::env::set_var("PI_GROK_NATIVE_COMMANDS", "yes");
        }
        assert!(env_flag_default_off("PI_GROK_NATIVE_COMMANDS"));
        unsafe {
            std::env::remove_var("PI_GROK_NATIVE_COMMANDS");
        }
    }

    #[test]
    fn zero_extension_probe_disables_auto_discovery() {
        let args = [
            "--extension",
            "bridge-a.ts",
            "--model",
            "provider/model",
            "--extension",
            "bridge-b.ts",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();

        assert_eq!(
            disable_all_extensions(&args),
            vec![
                "--model".to_owned(),
                "provider/model".to_owned(),
                "--no-extensions".to_owned(),
            ]
        );
        assert_eq!(
            disable_all_extensions(&["--no-extensions".to_owned()]),
            vec!["--no-extensions".to_owned()]
        );
    }

    #[test]
    fn extension_discovery_and_host_bridges_have_independent_switches() {
        let discovery = Args::try_parse_from(["grok-pi", "--no-extensions"]).expect("parse args");
        assert!(discovery.no_extensions);
        assert!(!discovery.no_bridge_extensions);

        let bridges =
            Args::try_parse_from(["grok-pi", "--no-bridge-extensions"]).expect("parse args");
        assert!(!bridges.no_extensions);
        assert!(bridges.no_bridge_extensions);
    }

    #[test]
    fn off_values_disable() {
        for value in ["0", "false", "OFF", "No"] {
            unsafe {
                std::env::set_var("PI_GROK_TEST_FLAG_DEFAULT_ON", value);
            }
            assert!(
                !env_flag_default_on("PI_GROK_TEST_FLAG_DEFAULT_ON"),
                "{value}"
            );
        }
        unsafe {
            std::env::set_var("PI_GROK_TEST_FLAG_DEFAULT_ON", "1");
        }
        assert!(env_flag_default_on("PI_GROK_TEST_FLAG_DEFAULT_ON"));
        unsafe {
            std::env::remove_var("PI_GROK_TEST_FLAG_DEFAULT_ON");
        }
    }
}
