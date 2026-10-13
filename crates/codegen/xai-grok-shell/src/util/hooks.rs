use std::path::Path;

use xai_grok_hooks::discovery::DiscoveryOptions;
use xai_grok_hooks::error::HookError;
use xai_grok_hooks::trust::DisabledHooks;
use xai_grok_hooks::trust::Trust;
use xai_grok_workspace::hook_inputs::ProcessHookInputs;
use xai_grok_workspace::permission::resolution::managed_settings;

pub(crate) fn process_hook_inputs() -> ProcessHookInputs {
    ProcessHookInputs::read(crate::claude_import::import_marker())
}

/// For a session that dispatches the hooks it discovers.
pub(crate) fn session_hook_inputs() -> (ProcessHookInputs, DisabledHooks) {
    ProcessHookInputs::read_with_disabled(managed_settings(), crate::claude_import::import_marker())
}

/// Vendor settings-file paths are classified at the call site so a directory there cannot load as a hook dir.
fn classify_grok_hook_source(path: PathBuf) -> HookSourceConfig {
    if path.is_dir() {
        HookSourceConfig::Directory(path)
    } else {
        HookSourceConfig::SettingsFile(path)
    }
}

fn include_claude_hooks(compat: &xai_grok_tools::types::compat::CompatConfig) -> bool {
    compat.claude.hooks
        && !crate::claude_import::is_claude_import_marked_with_log("discover_hook_source_paths")
}

fn include_cursor_hooks(compat: &xai_grok_tools::types::compat::CompatConfig) -> bool {
    compat.cursor.hooks
}

/// Global and project hook source paths.
/// The registry file is never a discovery source; Claude and Cursor sources are appended when their compat gates are on.
/// Vendor settings paths are pushed as settings files here and probed for presence by xai_grok_workspace::folder_trust::repo_configs_present; a new vendor path needs both.
pub(crate) fn discover_hook_source_paths(
    git_root: Option<&Path>,
    compat: &xai_grok_tools::types::compat::CompatConfig,
) -> HookSourcePaths {
    let grok = xai_grok_config::user_grok_home();
    let home = xai_dirs::home_dir();
    let include_claude = include_claude_hooks(compat);
    let include_cursor = include_cursor_hooks(compat);

    // An unreadable hooks-paths file keeps the fixed Grok sources; a hard resolve failure omits all Grok global sources
    let mut global: Vec<HookSourceConfig> =
        match resolve_global_hook_sources(grok.as_deref(), /* reject_symlinks */ false) {
            Ok(resolved) => {
                if let Some(e) = &resolved.configured_error {
                    tracing::warn!(
                        error = %e,
                        "hooks-paths unreadable; retaining fixed Grok hook discovery sources only"
                    );
                }
                resolved
                    .discovery_sources()
                    .map(|s| classify_grok_hook_source(s.path.clone()))
                    .collect()
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "global hook source resolve hard-failed; omitting Grok global sources"
                );
                Vec::new()
            }
        };

    if let Some(h) = home.as_deref() {
        if include_claude {
            global.push(HookSourceConfig::SettingsFile(
                h.join(".claude").join("settings.json"),
            ));
            global.push(HookSourceConfig::SettingsFile(
                h.join(".claude").join("settings.local.json"),
            ));
        }
        if include_cursor {
            global.push(HookSourceConfig::SettingsFile(
                h.join(".cursor").join("hooks.json"),
            ));
        }
    }

    let mut project = Vec::new();
    if let Some(root) = git_root {
        if include_claude {
            project.push(HookSourceConfig::SettingsFile(
                root.join(".claude").join("settings.json"),
            ));
            project.push(HookSourceConfig::SettingsFile(
                root.join(".claude").join("settings.local.json"),
            ));
        }
        project.push(classify_grok_hook_source(
            xai_grok_config::project_config_dir(root).join("hooks"),
        ));
        if include_cursor {
            project.push(HookSourceConfig::SettingsFile(
                root.join(".cursor").join("hooks.json"),
            ));
        }
    }

    HookSourcePaths { global, project }
}

/// Single load entry point: build compat-aware sources, gate project sources on trust, then load.
/// Every session-startup and mid-session reload site routes through here so the source policy stays in one place.
pub(crate) fn discover_hooks(
    inputs: &ProcessHookInputs,
    git_root: Option<&Path>,
    compat: &xai_grok_tools::types::compat::CompatConfig,
    trust: Trust,
) -> (xai_grok_hooks::discovery::HookRegistry, Vec<HookError>) {
    xai_grok_hooks::discovery::assemble_hooks(
        inputs.config_layers(),
        DiscoveryOptions {
            git_root,
            grok_home: inputs.grok_home(),
            home: inputs.home(),
            compat: compat.hooks(),
            claude_import: inputs.claude_import(),
            trust,
        },
    )
}
