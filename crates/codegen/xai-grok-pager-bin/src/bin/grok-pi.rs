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
#[path = "grok_pi/context_extension.rs"]
mod context_extension;
#[path = "grok_pi/export_extension.rs"]
mod export_extension;
#[path = "grok_pi/goal_extension.rs"]
mod goal_extension;
#[path = "grok_pi/herdr_extension.rs"]
mod herdr_extension;
#[path = "grok_pi/home.rs"]
mod home;
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
#[path = "grok_pi/rust_tui_bridge_extension.rs"]
mod rust_tui_bridge_extension;
#[path = "grok_pi/session_paths.rs"]
mod session_paths;
#[path = "grok_pi/shortcut_manager_extension.rs"]
mod shortcut_manager_extension;
#[path = "grok_pi/subagent_extension.rs"]
mod subagent_extension;
#[path = "grok_pi/tools_extension.rs"]
mod tools_extension;
#[path = "grok_pi/tree_bridge.rs"]
mod tree_bridge;
#[path = "grok_pi/tutorial_profile.rs"]
mod tutorial_profile;
#[path = "grok_pi/workflow_extension.rs"]
mod workflow_extension;

use anyhow::{Context, Result};
use clap::Parser;
use pi_grok_adapter::{PiAgent, PiBootstrap, PiRpc, SpawnConfig};
use std::future::Future;
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

use ask_user_extension::write_ask_user_extension;
use auth_extension::write_auth_extension;
use bash_extension::write_bash_extension;
use btw_extension::write_btw_extension;
use cli::{Args, Command, normalize_compound_short_flags, pi_args_with_startup_flags};
use context_extension::write_context_extension;
use export_extension::write_export_extension;
use goal_extension::write_goal_extension;
use herdr_extension::{is_managed_pi_integration, write_herdr_extension};
use loop_extension::write_loop_extension;
use native_commands_extension::write_native_commands_extension;
use pi_version::ensure_compatible_pi_host;
use plan_mode_extension::write_plan_mode_extension;
use recap_extension::write_recap_extension;
use remote_tui_extension::write_remote_tui_extension;
use rpc_compat_extension::write_rpc_compat_extension;
use rust_tui_bridge_extension::write_rust_tui_bridge_extension;
use session_paths::pi_session_dir;
use shortcut_manager_extension::write_shortcut_manager_extension;
use subagent_extension::write_subagent_extension;
use tools_extension::{
    cli_tool_exclusions, configured_builtin_tools, has_no_tools_arg, should_inject_tools_extension,
    write_tools_extension,
};
use tree_bridge::write_navigate_tree_extension;
use workflow_extension::write_workflow_extension;

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

    // Pi's --no-extensions controls auto-discovery only. Bundled host bridges
    // have a separate kill switch because they are passed as explicit paths.
    let bridge_extensions_enabled = !args.no_bridge_extensions;
    let navigate_tree_extension = bridge_extensions_enabled
        .then(|| write_navigate_tree_extension())
        .transpose()
        .context("failed to create Pi navigateTree bridge extension")?;
    // F2 `[ui].pi_herdr` (default off). Outside Herdr the extension is a silent no-op.
    let herdr_extension = if bridge_extensions_enabled && herdr_enabled() {
        Some(write_herdr_extension().context("failed to create Pi Herdr extension")?)
    } else {
        None
    };
    let bash_extension = if bridge_extensions_enabled && env_flag_default_on("PI_GROK_BASH") {
        Some(write_bash_extension().context("failed to create Pi Bash extension")?)
    } else {
        None
    };
    // F2 `[ui].pi_subagents` (default on). Restart required — inject at startup only.
    let subagent_extension = if bridge_extensions_enabled && subagents_enabled() {
        Some(write_subagent_extension().context("failed to create Pi subagent extension")?)
    } else {
        None
    };
    // F2 `[ui].pi_workflows` (default off). Restart required — inject at startup only.
    let workflow_extension = if bridge_extensions_enabled && workflows_enabled() {
        Some(write_workflow_extension().context("failed to create Pi workflow extension")?)
    } else {
        None
    };
    // F2 `[ui].pi_goal` (default off). Restart required — inject at startup only.
    let goal_extension = if bridge_extensions_enabled && goal_enabled() {
        Some(write_goal_extension().context("failed to create Pi goal extension")?)
    } else {
        None
    };
    // F2 `[ui].pi_loop` (default off). Restart required — inject at startup only.
    let loop_extension = if bridge_extensions_enabled && loop_enabled() {
        Some(write_loop_extension().context("failed to create Pi loop extension")?)
    } else {
        None
    };
    // F2 `[ui].pi_ask_user_question` (default off). Restart required — inject at startup only.
    let ask_user_extension = if bridge_extensions_enabled && ask_user_enabled() {
        Some(
            write_ask_user_extension()
                .context("failed to create Pi ask_user_question extension")?,
        )
    } else {
        None
    };
    // F2 `[ui].pi_btw` (default off). Restart required — inject at startup only.
    let btw_extension = if bridge_extensions_enabled && btw_enabled() {
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

    // ── Resource admission policy ────────────────────────────────────────────
    // Disable Pi's auto-discovery and load only policy-approved resources.
    // Bridge extensions (subagent, bash, recap, etc.) are appended separately
    // below and always load regardless of policy.
    let mut resource_policy = ResourcePolicy::load_from_config();
    // Feature-gated package blocks (assets/native_feature_conflicts.toml).
    if ask_user_extension.is_some() {
        resource_policy
            .enabled_native_features
            .push("pi_ask_user_question".to_owned());
    }
    if goal_extension.is_some() {
        resource_policy
            .enabled_native_features
            .push("pi_goal".to_owned());
    }
    if workflow_extension.is_some() {
        resource_policy
            .enabled_native_features
            .push("pi_workflows".to_owned());
    }
    if subagent_extension.is_some() {
        resource_policy
            .enabled_native_features
            .push("pi_subagents".to_owned());
    }
    if btw_extension.is_some() {
        resource_policy
            .enabled_native_features
            .push("pi_btw".to_owned());
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
            .map(|extension| extension.path()),
        shortcut_manager_extension
            .as_ref()
            .map(|extension| extension.path()),
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

    // CLI tool restrictions (--tools, --no-tools, --no-builtin-tools,
    // --exclude-tools) are authoritative and always override F2 preferences.
    // Skip the tools extension entirely when CLI disables all tools or all
    // builtins; for --exclude-tools, inject but pass the exclusion list so the
    // extension filters them out.
    let cli_exclusions = cli_tool_exclusions(&pi_args);
    let tools_extension = (bridge_extensions_enabled && should_inject_tools_extension(&pi_args))
        .then(|| write_tools_extension())
        .transpose()
        .context("failed to create Pi tools extension")?;
    let selected_builtin_tools = tools_extension.as_ref().map(|_| configured_builtin_tools());
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
            .map(|extension| extension.path().to_path_buf()),
    );
    add_subagent_extension(
        "grok-pi: shortcut manager",
        shortcut_manager_extension
            .as_ref()
            .map(|extension| extension.path().to_path_buf()),
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
            .map(|extension| extension.path().to_path_buf()),
    );
    add_subagent_extension(
        "grok-pi: recap",
        recap_extension
            .as_ref()
            .map(|extension| extension.path().to_path_buf()),
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
            .map(|extension| extension.path().to_path_buf()),
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
        "grok-pi: bash",
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
            .map(|extension| extension.path().to_path_buf()),
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
            .map(|extension| extension.path()),
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
        btw_extension.as_ref().map(|extension| extension.path()),
        recap_extension.as_ref().map(|extension| extension.path()),
        context_extension
            .as_ref()
            .map(|extension| extension.source_path()),
        auth_extension.as_ref().map(|extension| extension.path()),
        export_extension.as_ref().map(|extension| extension.path()),
        native_commands_extension
            .as_ref()
            .map(|extension| extension.path()),
        bash_extension
            .as_ref()
            .map(|extension| extension.source_path()),
        tools_extension.as_ref().map(|extension| extension.path()),
        // Rollback extension observes the final built-in registrations.
        rollback_ext.as_ref().map(|extension| extension.path()),
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
    if workflow_extension.is_some() {
        // Pi child reads this for extension factory; adapter also checks it
        // (and F2 config). Set child env + parent process env.
        env.push(("PI_GROK_WORKFLOWS".to_string(), "1".to_string()));
        // SAFETY: single-threaded startup; process-local flag for this session.
        unsafe {
            std::env::set_var("PI_GROK_WORKFLOWS", "1");
        }
    } else {
        // Avoid a stale parent env from a previous enable in the same shell.
        unsafe {
            std::env::remove_var("PI_GROK_WORKFLOWS");
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
        env.push(("PI_GROK_BASH".to_string(), "1".to_string()));
        env.push((
            "PI_GROK_BASH_CONTROL_META".to_string(),
            extension.control_meta_path().to_string_lossy().into_owned(),
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
    let bash_control_meta = bash_extension
        .as_ref()
        .map(|extension| extension.control_meta_path().to_path_buf());
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
    let initial_commands = bootstrap.acp_commands();
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

/// Best-effort host terminal size for Remote TUI viewport (Pi child has no TTY).
fn host_terminal_size() -> Option<(u16, u16)> {
    #[cfg(unix)]
    {
        // SAFETY: ioctl(TIOCGWINSZ) on stdout; fails cleanly when not a TTY.
        unsafe {
            let mut ws: libc::winsize = std::mem::zeroed();
            if libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) == 0
                && ws.ws_col > 0
                && ws.ws_row > 0
            {
                return Some((ws.ws_col, ws.ws_row));
            }
        }
    }
    None
}

/// Feature flags that default to ON. Explicit `0`/`false`/`off`/`no` disables.
/// Unset or any other value (including `1`) enables.
fn env_flag_default_on(name: &str) -> bool {
    match std::env::var(name) {
        Err(_) => true,
        Ok(value) => {
            let v = value.trim();
            !(v.eq_ignore_ascii_case("0")
                || v.eq_ignore_ascii_case("false")
                || v.eq_ignore_ascii_case("off")
                || v.eq_ignore_ascii_case("no"))
        }
    }
}

/// Experimental features default to OFF and require an explicit truthy value.
fn env_flag_default_off(name: &str) -> bool {
    match std::env::var(name) {
        Ok(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "on" | "yes"
        ),
        Err(_) => false,
    }
}

// ── Extension self-heal (VSCode-style binary search) ─────────────────────────
//
// When an extension crashes the Pi RPC child during bootstrap, grok-pi used to
// exit with an opaque error. Now we:
//
// 1. Confirm Pi boots with zero extensions (sanity check).
// 2. Binary-search the ordered `--extension` list to isolate the culprit.
// 3. Print a diagnostic naming the bad extension.
// 4. Relaunch without it (self-heal) so the user is never stuck.
//
// The user can run `grok-pi -ne --no-bridge-extensions` to skip all extensions.

const PI_BOOTSTRAP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

async fn bootstrap_with_deadline<T>(
    future: impl Future<Output = anyhow::Result<T>>,
    deadline: std::time::Duration,
) -> anyhow::Result<T> {
    tokio::time::timeout(deadline, future).await.map_err(|_| {
        anyhow::anyhow!(
            "Pi RPC bootstrap timed out after {} ms",
            deadline.as_millis()
        )
    })?
}

/// Spawn Pi with the full extension set. If bootstrap fails, run the
/// self-heal bisection and return a working process.
async fn spawn_with_extension_self_heal(
    args: &Args,
    cwd: &std::path::Path,
    pi_args: Vec<String>,
    env: &[(String, String)],
) -> Result<(pi_grok_adapter::PiProcess, PiBootstrap, Vec<String>)> {
    let config = SpawnConfig {
        program: args.pi_bin.clone(),
        prefix_args: args.pi_prefix_args.clone(),
        cwd: cwd.to_path_buf(),
        pi_args: pi_args.clone(),
        env: env.to_vec(),
    };

    let process = PiRpc::spawn(config).await?;
    match bootstrap_with_deadline(PiBootstrap::load(&process.rpc), PI_BOOTSTRAP_TIMEOUT).await {
        Ok(bootstrap) => return Ok((process, bootstrap, pi_args)),
        Err(error) => {
            process.rpc.kill().await;
            tracing::warn!(%error, "Pi bootstrap failed; starting extension self-heal");
        }
    }

    // Extract extension paths from pi_args (pairs: "--extension" <path>).
    let ext_paths = extract_extension_paths(&pi_args);
    if ext_paths.is_empty() {
        // No extensions to bisect — the failure is not extension-related.
        anyhow::bail!(
            "Pi RPC bootstrap failed and no extensions are loaded.\n\
             Try: grok-pi -ne --no-bridge-extensions"
        );
    }

    // Step 1: Confirm Pi boots with zero extensions.
    let no_ext_args = disable_all_extensions(&pi_args);
    let probe_config = SpawnConfig {
        program: args.pi_bin.clone(),
        prefix_args: args.pi_prefix_args.clone(),
        cwd: cwd.to_path_buf(),
        pi_args: no_ext_args.clone(),
        env: env.to_vec(),
    };
    let probe = PiRpc::spawn(probe_config).await?;
    match bootstrap_with_deadline(PiBootstrap::load(&probe.rpc), PI_BOOTSTRAP_TIMEOUT).await {
        Ok(_) => {
            probe.rpc.kill().await;
        }
        Err(e) => {
            probe.rpc.kill().await;
            anyhow::bail!(
                "Pi RPC bootstrap fails even with zero extensions.\n\
                 This is not an extension problem.\n\
                 Error: {e}"
            );
        }
    }

    // Step 2: Binary search for the culprit extension.
    let culprit = bisect_extension_culprit(args, cwd, &no_ext_args, env, &ext_paths).await;

    match culprit {
        Some(bad_path) => {
            let display = std::path::Path::new(&bad_path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| bad_path.clone());

            eprintln!();
            eprintln!("\x1b[1;31m✗ Extension crash detected\x1b[0m");
            eprintln!("  Culprit: \x1b[1m{display}\x1b[0m");
            eprintln!("  Path:    {bad_path}");
            eprintln!();
            eprintln!("  \x1b[1mSelf-healing:\x1b[0m relaunching without this extension.");
            eprintln!();
            eprintln!(
                "  To disable all extensions:  \x1b[1mgrok-pi -ne --no-bridge-extensions\x1b[0m"
            );
            eprintln!(
                "  To permanently block it, add to {}/config.toml:",
                home::display_home(&home::effective_grok_home())
            );
            eprintln!("    [pi.resources]");
            eprintln!("    block = [\"{bad_path}\"]");
            eprintln!("  Or project sidecar: .grok-pi/pi-resources.toml  block = [\"...\"]");
            eprintln!();

            // After successful bisection: Y / any key = report, only N = skip.
            prompt_and_maybe_report_ext_crash(&bad_path, "crash");

            // Step 3: Relaunch without the culprit.
            let healed_args = remove_extension_path(&pi_args, &bad_path);
            let heal_config = SpawnConfig {
                program: args.pi_bin.clone(),
                prefix_args: args.pi_prefix_args.clone(),
                cwd: cwd.to_path_buf(),
                pi_args: healed_args.clone(),
                env: env.to_vec(),
            };
            let process = PiRpc::spawn(heal_config).await?;
            let bootstrap =
                bootstrap_with_deadline(PiBootstrap::load(&process.rpc), PI_BOOTSTRAP_TIMEOUT)
                    .await
                    .context("self-heal relaunch still failed")?;
            Ok((process, bootstrap, healed_args))
        }
        None => {
            // Bisection couldn't isolate a single culprit (e.g. combination
            // conflict). Fall back to disabling all extensions.
            eprintln!();
            eprintln!("\x1b[1;31m✗ Extension conflict detected\x1b[0m");
            eprintln!("  Could not isolate a single culprit (possible combination conflict).");
            eprintln!();
            eprintln!("  \x1b[1mSelf-healing:\x1b[0m relaunching with all extensions disabled.");
            eprintln!("  To do this manually:  \x1b[1mgrok-pi -ne --no-bridge-extensions\x1b[0m");
            eprintln!();

            prompt_and_maybe_report_ext_crash("combo", "combo");

            let process = PiRpc::spawn(SpawnConfig {
                program: args.pi_bin.clone(),
                prefix_args: args.pi_prefix_args.clone(),
                cwd: cwd.to_path_buf(),
                pi_args: no_ext_args.clone(),
                env: env.to_vec(),
            })
            .await?;
            let bootstrap =
                bootstrap_with_deadline(PiBootstrap::load(&process.rpc), PI_BOOTSTRAP_TIMEOUT)
                    .await
                    .context("fallback no-extension launch failed")?;
            Ok((process, bootstrap, no_ext_args))
        }
    }
}

// ── Extension crash telemetry (privacy: name + package_dir only) ─────────────

const DEFAULT_EXT_TELEMETRY_URL: &str = "https://ext-crash-telemetry.dwsycode.workers.dev";

/// After bisection succeeds: interactive confirm then fire-and-forget POST.
///
/// Key semantics:
/// - `N` / `n` → do **not** report
/// - `Y` / any other key → report
/// Non-TTY → skip (never block CI / piped stdin).
fn prompt_and_maybe_report_ext_crash(path_or_label: &str, kind: &str) {
    use std::io::{IsTerminal, Write};

    if !std::io::stdin().is_terminal() {
        return;
    }

    let (ext_name, package_dir) = if kind == "combo" {
        ("combo".to_owned(), "combo".to_owned())
    } else {
        ext_identity_from_path(path_or_label)
    };

    eprint!(
        "  Report this {kind} to telemetry (name only: {package_dir})? [Y/n]  \
(N = no, any other key = yes) "
    );
    let _ = std::io::stderr().flush();

    let key = read_one_key_char();
    match key {
        Some('n') | Some('N') => {
            eprintln!("n — skipped report.");
            return;
        }
        Some(c) => eprintln!("{c} — reporting…"),
        None => eprintln!("— reporting…"),
    }

    let url = std::env::var("GROK_PI_EXT_TELEMETRY_URL")
        .or_else(|_| std::env::var("REPORT_URL"))
        .unwrap_or_else(|_| DEFAULT_EXT_TELEMETRY_URL.to_owned());
    let endpoint = format!("{}/v1/report", url.trim_end_matches('/'));
    let body = serde_json::json!({
        "ext_name": ext_name,
        "package_dir": package_dir,
        "kind": kind,
        "client": "grok-pi",
    })
    .to_string();

    // Token required server-side (fail closed). Prefer env, then ~/.grok-pi file.
    let token = std::env::var("REPORT_TOKEN")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(load_ext_telemetry_token_file);
    let Some(token) = token else {
        eprintln!(
            "  REPORT_TOKEN missing (set env or ~/.grok-pi/ext-telemetry.token); skip report."
        );
        return;
    };

    // Fire-and-forget so self-heal is not blocked on network.
    std::thread::spawn(move || {
        let mut cmd = std::process::Command::new("curl");
        cmd.args([
            "-sS",
            "-m",
            "5",
            "-X",
            "POST",
            &endpoint,
            "-H",
            "content-type: application/json",
            "-H",
            &format!("authorization: Bearer {token}"),
            "-d",
            &body,
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
        let _ = cmd.status();
    });
}

fn load_ext_telemetry_token_file() -> Option<String> {
    let home = home::effective_grok_home();
    let path = home.join("ext-telemetry.token");
    let text = std::fs::read_to_string(path).ok()?;
    let t = text.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_owned())
    }
}

/// Privacy-safe identity from an extension path (no absolute path returned).
fn ext_identity_from_path(input: &str) -> (String, String) {
    let raw = input.replace('\\', "/");
    let parts: Vec<&str> = raw.split('/').filter(|p| !p.is_empty()).collect();

    if let Some(nm) = parts.iter().rposition(|p| *p == "node_modules") {
        if nm + 1 < parts.len() {
            let a = parts[nm + 1];
            if a.starts_with('@') && nm + 2 < parts.len() {
                let name = parts[nm + 2].to_owned();
                let pkg = format!("{a}/{name}");
                return (name, pkg);
            }
            return (a.to_owned(), a.to_owned());
        }
    }

    if let Some(ei) = parts.iter().rposition(|p| *p == "extensions") {
        if ei + 1 < parts.len() && !parts[ei + 1].contains('.') {
            let d = parts[ei + 1].to_owned();
            return (d.clone(), d);
        }
    }

    let leaf = parts.last().copied().unwrap_or("unknown");
    let name = leaf
        .trim_end_matches(".ts")
        .trim_end_matches(".js")
        .trim_end_matches(".mjs")
        .to_owned();
    if parts.len() >= 2 {
        let parent = parts[parts.len() - 2];
        if parent != "node_modules" && !parent.starts_with('.') {
            return (name, parent.to_owned());
        }
    }
    (name.clone(), name)
}

/// Read a single key (raw mode on Unix). Falls back to first char of a line.
fn read_one_key_char() -> Option<char> {
    #[cfg(unix)]
    {
        if let Some(c) = read_one_key_raw_unix() {
            return Some(c);
        }
    }
    let mut line = String::new();
    match std::io::stdin().read_line(&mut line) {
        Ok(0) => None,
        Ok(_) => line.chars().next().filter(|c| *c != '\n' && *c != '\r'),
        Err(_) => None,
    }
}

#[cfg(unix)]
fn read_one_key_raw_unix() -> Option<char> {
    use std::io::Read;
    use std::os::fd::AsRawFd;

    let stdin = std::io::stdin();
    let fd = stdin.as_raw_fd();
    // SAFETY: termios get/set on the process stdin fd; restored before return.
    unsafe {
        let mut old: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(fd, &mut old) != 0 {
            return None;
        }
        let mut raw = old;
        raw.c_lflag &= !(libc::ICANON | libc::ECHO);
        raw.c_cc[libc::VMIN] = 1;
        raw.c_cc[libc::VTIME] = 0;
        if libc::tcsetattr(fd, libc::TCSANOW, &raw) != 0 {
            return None;
        }
        let mut buf = [0u8; 1];
        let n = {
            let mut lock = stdin.lock();
            lock.read(&mut buf).unwrap_or(0)
        };
        let _ = libc::tcsetattr(fd, libc::TCSANOW, &old);
        if n == 0 {
            return None;
        }
        Some(buf[0] as char)
    }
}

/// F2 `[ui].pi_herdr` — report lifecycle state to Herdr for this process.
/// Missing/invalid config defaults off; outside Herdr the extension is inert.
fn herdr_enabled() -> bool {
    let config = xai_grok_shell::config::load_effective_config().ok();
    herdr_enabled_from_config(config.as_ref())
}

fn herdr_enabled_from_config(config: Option<&toml::Value>) -> bool {
    config
        .and_then(|root| root.get("ui"))
        .and_then(|ui| ui.get("pi_herdr"))
        .and_then(toml::Value::as_bool)
        .unwrap_or(false)
}

/// F2 `[ui].pi_subagents` — enable built-in Pi child-session subagents.
/// Missing/invalid config preserves the product's existing default-on behavior.
fn subagents_enabled() -> bool {
    let config = xai_grok_shell::config::load_effective_config().ok();
    subagents_enabled_from_config(config.as_ref())
}

fn subagents_enabled_from_config(config: Option<&toml::Value>) -> bool {
    config
        .and_then(|root| root.get("ui"))
        .and_then(|ui| ui.get("pi_subagents"))
        .and_then(toml::Value::as_bool)
        .unwrap_or(true)
}

/// F2 `[ui].pi_workflows` — enable upstream Rhai workflows for this process.
fn workflows_enabled() -> bool {
    let Ok(config) = xai_grok_shell::config::load_effective_config() else {
        return false;
    };
    config
        .get("ui")
        .and_then(|ui| ui.get("pi_workflows"))
        .and_then(toml::Value::as_bool)
        .unwrap_or(false)
}

/// F2 `[ui].pi_goal` — enable Grok-style /goal for this process.
fn goal_enabled() -> bool {
    let Ok(config) = xai_grok_shell::config::load_effective_config() else {
        return false;
    };
    config
        .get("ui")
        .and_then(|ui| ui.get("pi_goal"))
        .and_then(toml::Value::as_bool)
        .unwrap_or(false)
}

/// F2 `[ui].pi_loop` — enable Grok-style /loop scheduler for this process.
fn loop_enabled() -> bool {
    let Ok(config) = xai_grok_shell::config::load_effective_config() else {
        return false;
    };
    config
        .get("ui")
        .and_then(|ui| ui.get("pi_loop"))
        .and_then(toml::Value::as_bool)
        .unwrap_or(false)
}

/// F2 `[ui].pi_btw` — enable native /btw for this process.
fn btw_enabled() -> bool {
    let Ok(config) = xai_grok_shell::config::load_effective_config() else {
        return false;
    };
    config
        .get("ui")
        .and_then(|ui| ui.get("pi_btw"))
        .and_then(toml::Value::as_bool)
        .unwrap_or(false)
}

/// F2 `[ui].pi_ask_user_question` — enable native Q&A for this process.
fn ask_user_enabled() -> bool {
    let Ok(config) = xai_grok_shell::config::load_effective_config() else {
        return false;
    };
    config
        .get("ui")
        .and_then(|ui| ui.get("pi_ask_user_question"))
        .and_then(toml::Value::as_bool)
        .unwrap_or(false)
}

/// Extract all `--extension <path>` values from pi_args.
fn extract_extension_paths(pi_args: &[String]) -> Vec<String> {
    let mut paths = Vec::new();
    let mut i = 0;
    while i < pi_args.len() {
        if pi_args[i] == "--extension" && i + 1 < pi_args.len() {
            paths.push(pi_args[i + 1].clone());
            i += 2;
        } else {
            i += 1;
        }
    }
    paths
}

/// Remove explicit extension paths and disable Pi extension auto-discovery.
fn disable_all_extensions(pi_args: &[String]) -> Vec<String> {
    let mut result = Vec::with_capacity(pi_args.len() + 1);
    let mut i = 0;
    while i < pi_args.len() {
        if pi_args[i] == "--extension" && i + 1 < pi_args.len() {
            i += 2;
        } else {
            result.push(pi_args[i].clone());
            i += 1;
        }
    }
    if !result.iter().any(|arg| arg == "--no-extensions") {
        result.push("--no-extensions".to_owned());
    }
    result
}

/// Remove a specific `--extension <path>` pair from pi_args.
fn remove_extension_path(pi_args: &[String], path: &str) -> Vec<String> {
    let mut result = Vec::with_capacity(pi_args.len());
    let mut i = 0;
    while i < pi_args.len() {
        if pi_args[i] == "--extension" && i + 1 < pi_args.len() && pi_args[i + 1] == path {
            i += 2;
        } else {
            result.push(pi_args[i].clone());
            i += 1;
        }
    }
    result
}

/// Binary search the extension list to find the one that crashes Pi.
/// Returns the path of the culprit, or None if isolation fails.
async fn bisect_extension_culprit(
    args: &Args,
    cwd: &std::path::Path,
    base_args: &[String],
    env: &[(String, String)],
    ext_paths: &[String],
) -> Option<String> {
    // If the full set passes, there's no culprit (shouldn't happen).
    if probe_extensions_ok(args, cwd, base_args, env, ext_paths).await {
        return None;
    }

    // Binary search: find the minimal prefix that fails.
    let mut lo = 0usize;
    let mut hi = ext_paths.len();
    while lo + 1 < hi {
        let mid = (lo + hi) / 2;
        if probe_extensions_ok(args, cwd, base_args, env, &ext_paths[..mid]).await {
            lo = mid;
        } else {
            hi = mid;
        }
    }

    // Verify the single extension at index `lo` is the culprit.
    let suspect = &ext_paths[lo];
    if !probe_extensions_ok(args, cwd, base_args, env, std::slice::from_ref(suspect)).await {
        return Some(suspect.clone());
    }

    // The suspect passes alone — it's a combination conflict.
    // Try each extension individually to find one that fails alone.
    for path in ext_paths {
        if !probe_extensions_ok(args, cwd, base_args, env, std::slice::from_ref(path)).await {
            return Some(path.clone());
        }
    }

    None
}

/// Probe whether Pi boots successfully with the given subset of extensions.
async fn probe_extensions_ok(
    args: &Args,
    cwd: &std::path::Path,
    base_args: &[String],
    env: &[(String, String)],
    subset: &[String],
) -> bool {
    let mut probe_args = base_args.to_vec();
    for path in subset {
        probe_args.extend(["--extension".to_string(), path.clone()]);
    }
    let config = SpawnConfig {
        program: args.pi_bin.clone(),
        prefix_args: args.pi_prefix_args.clone(),
        cwd: cwd.to_path_buf(),
        pi_args: probe_args,
        env: env.to_vec(),
    };
    let Ok(process) = PiRpc::spawn(config).await else {
        return false;
    };
    let ok = bootstrap_with_deadline(PiBootstrap::load(&process.rpc), PI_BOOTSTRAP_TIMEOUT)
        .await
        .is_ok();
    process.rpc.kill().await;
    ok
}

#[cfg(test)]
mod env_flag_tests {
    use super::{
        Args, PI_GROK_NATIVE_COMMANDS, bootstrap_with_deadline, disable_all_extensions,
        env_flag_default_off, env_flag_default_on, herdr_enabled_from_config,
        subagents_enabled_from_config,
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
    fn herdr_integration_defaults_off_and_honors_explicit_true() {
        assert!(!herdr_enabled_from_config(None));

        let missing: toml::Value = toml::from_str("[ui]\n").expect("parse missing config");
        assert!(!herdr_enabled_from_config(Some(&missing)));

        let enabled: toml::Value =
            toml::from_str("[ui]\npi_herdr = true\n").expect("parse enabled config");
        assert!(herdr_enabled_from_config(Some(&enabled)));

        let disabled: toml::Value =
            toml::from_str("[ui]\npi_herdr = false\n").expect("parse disabled config");
        assert!(!herdr_enabled_from_config(Some(&disabled)));
    }

    #[test]
    fn subagents_default_on_and_honor_explicit_off() {
        assert!(subagents_enabled_from_config(None));

        let missing: toml::Value = toml::from_str("[ui]\n").expect("parse missing config");
        assert!(subagents_enabled_from_config(Some(&missing)));

        let enabled: toml::Value =
            toml::from_str("[ui]\npi_subagents = true\n").expect("parse enabled config");
        assert!(subagents_enabled_from_config(Some(&enabled)));

        let disabled: toml::Value =
            toml::from_str("[ui]\npi_subagents = false\n").expect("parse disabled config");
        assert!(!subagents_enabled_from_config(Some(&disabled)));
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
