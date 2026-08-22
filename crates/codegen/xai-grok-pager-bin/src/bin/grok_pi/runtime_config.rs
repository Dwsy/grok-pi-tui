use super::{
    bash_extension,
    tools_extension::{has_explicit_tools_arg, should_inject_tools_extension},
};

/// Best-effort host terminal size for Remote TUI viewport (Pi child has no TTY).
pub(super) fn host_terminal_size() -> Option<(u16, u16)> {
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
pub(super) fn env_flag_default_on(name: &str) -> bool {
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
pub(super) fn env_flag_default_off(name: &str) -> bool {
    match std::env::var(name) {
        Ok(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "on" | "yes"
        ),
        Err(_) => false,
    }
}

/// `[ui].pi_bash` — switch for grok-pi's enhanced Bash bridge only.
/// Eval version/visibility are independent. An explicitly-set `PI_GROK_BASH`
/// environment variable remains a process-local override; otherwise F2/TOML is
/// authoritative. Missing/invalid config defaults on.
pub(super) fn bash_bridge_enabled() -> bool {
    if std::env::var_os("PI_GROK_BASH").is_some() {
        return env_flag_default_on("PI_GROK_BASH");
    }
    let config = xai_grok_shell::config::load_effective_config().ok();
    bash_bridge_enabled_from_config(config.as_ref())
}

pub(super) fn bash_bridge_enabled_from_config(config: Option<&toml::Value>) -> bool {
    config
        .and_then(|root| root.get("ui"))
        .and_then(|ui| ui.get("pi_bash"))
        .and_then(toml::Value::as_bool)
        .unwrap_or(true)
}

const DEFAULT_BASH_MAX_WAIT_MINS: &str = "4.5";

pub(super) fn resolve_bash_max_wait_mins(cli: Option<f64>, inherited: Option<&str>) -> String {
    cli.map(|value| value.to_string())
        .or_else(|| inherited.map(str::to_owned))
        .unwrap_or_else(|| DEFAULT_BASH_MAX_WAIT_MINS.to_string())
}

/// Adapter background/kill RPC is valid only while the enhanced Bash half of
/// the shared Bash/Eval extension is active. Eval-only sessions still inject
/// the bundle, but must expose no Bash control metadata to the host adapter.
pub(super) fn bash_control_meta_for_adapter(
    bash_enabled: bool,
    extension: Option<&bash_extension::BashExtension>,
) -> Option<std::path::PathBuf> {
    extension
        .filter(|_| bash_enabled)
        .map(|extension| extension.control_meta_path().to_path_buf())
}

/// `[ui].pi_eval` — select the mutually exclusive Eval bridge generation.
/// Only an explicit `"v2"` opts into Eval Bridge v2; missing/invalid values preserve v1.
pub(super) fn eval_version() -> &'static str {
    let config = xai_grok_shell::config::load_effective_config().ok();
    eval_version_from_config(config.as_ref())
}

pub(super) fn eval_version_from_config(config: Option<&toml::Value>) -> &'static str {
    match config
        .and_then(|root| root.get("ui"))
        .and_then(|ui| ui.get("pi_eval"))
        .and_then(toml::Value::as_str)
    {
        Some("v2") => "v2",
        _ => "v1",
    }
}

/// `[ui].pi_eval_v2_language` — select Eval v2 language exposure.
/// Missing or invalid values preserve the pre-selector JavaScript-only default.
pub(super) fn eval_v2_language() -> &'static str {
    let config = xai_grok_shell::config::load_effective_config().ok();
    eval_v2_language_from_config(config.as_ref())
}

fn eval_v2_language_from_config(config: Option<&toml::Value>) -> &'static str {
    match config
        .and_then(|root| root.get("ui"))
        .and_then(|ui| ui.get("pi_eval_v2_language"))
        .and_then(toml::Value::as_str)
    {
        Some("py") => "py",
        Some("all") => "all",
        _ => "js",
    }
}

/// `[ui].pi_eval_v2_only` — force Eval v2 and isolate the top-level model to Eval.
pub(super) fn eval_v2_only_enabled() -> bool {
    let config = xai_grok_shell::config::load_effective_config().ok();
    eval_v2_only_enabled_from_config(config.as_ref())
}

pub(super) fn eval_v2_only_enabled_from_config(config: Option<&toml::Value>) -> bool {
    config
        .and_then(|root| root.get("ui"))
        .and_then(|ui| ui.get("pi_eval_v2_only"))
        .and_then(toml::Value::as_bool)
        .unwrap_or(false)
}

pub(super) fn eval_v2_only_tool_policy_applies(
    pi_args: &[String],
    bridge_extensions_enabled: bool,
    eval_v2_only: bool,
) -> bool {
    bridge_extensions_enabled
        && eval_v2_only
        && !has_explicit_tools_arg(pi_args)
        && !pi_args
            .iter()
            .any(|arg| matches!(arg.as_str(), "--no-tools" | "-nt"))
}

pub(super) fn normal_f2_tool_policy_applies(
    pi_args: &[String],
    bridge_extensions_enabled: bool,
    eval_v2_only_tool_policy_applied: bool,
) -> bool {
    bridge_extensions_enabled
        && !eval_v2_only_tool_policy_applied
        && should_inject_tools_extension(pi_args)
}
