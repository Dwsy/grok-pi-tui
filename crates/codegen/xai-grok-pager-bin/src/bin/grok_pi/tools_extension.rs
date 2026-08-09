use anyhow::{Context, Result};
use std::{fs::File, io::Write};
use tempfile::NamedTempFile;

/// Materialize the bridge extension that applies the F2-selected Pi built-in
/// tools without changing Pi's source or filtering extension/custom tools.
pub(super) fn write_tools_extension() -> Result<NamedTempFile> {
    let mut file = tempfile::Builder::new()
        .prefix("pi-grok-tools-")
        .suffix(".ts")
        .tempfile()
        .context("create Pi tools extension tempfile")?;
    const SOURCE: &str = include_str!("../../../../../../extensions/pi-grok-tools/index.ts");
    file.write_all(SOURCE.as_bytes())
        .context("write Pi tools extension source")?;
    file.flush().context("flush Pi tools extension source")?;
    File::open(file.path())
        .and_then(|source| source.sync_all())
        .ok();
    Ok(file)
}

pub(super) fn configured_builtin_tools() -> String {
    let defaults = ["read", "bash", "edit", "write"];
    let Ok(config) = xai_grok_shell::config::load_effective_config() else {
        return defaults.join(",");
    };
    let Some(tools) = config
        .get("ui")
        .and_then(|ui| ui.get("pi_builtin_tools"))
        .and_then(toml::Value::as_table)
    else {
        return defaults.join(",");
    };
    [
        "read", "bash", "edit", "write", "grep", "find", "ls", "eval",
    ]
    .into_iter()
    .filter(|name| {
        tools
            .get(*name)
            .and_then(toml::Value::as_bool)
            .unwrap_or(matches!(*name, "read" | "bash" | "edit" | "write"))
    })
    .collect::<Vec<_>>()
    .join(",")
}

/// Whether the user passed an explicit `--tools` / `-t` allowlist.
/// When present, F2 preferences are skipped entirely — the allowlist is
/// authoritative and already excludes unlisted tools.
pub(super) fn has_explicit_tools_arg(args: &[String]) -> bool {
    args.iter()
        .any(|arg| arg == "--tools" || arg == "-t" || arg.starts_with("--tools="))
}

/// Whether the user passed `--no-tools` / `-nt` or `--no-builtin-tools` /
/// `-nbt`. Either flag disables all (or all builtin) tools; the F2
/// extension must NOT be injected because `setActiveTools()` would
/// re-enable tools the CLI explicitly disabled.
pub(super) fn has_no_tools_arg(args: &[String]) -> bool {
    args.iter().any(|arg| {
        matches!(
            arg.as_str(),
            "--no-tools" | "-nt" | "--no-builtin-tools" | "-nbt"
        )
    })
}

/// Extract the comma-separated tool names from `--exclude-tools` / `-xt`.
/// Returns `None` when the flag is absent.
pub(super) fn excluded_tools(args: &[String]) -> Option<String> {
    for (idx, arg) in args.iter().enumerate() {
        if let Some(value) = arg.strip_prefix("--exclude-tools=") {
            return (!value.is_empty()).then(|| value.to_string());
        }
        if arg == "--exclude-tools" || arg == "-xt" {
            return args.get(idx + 1).filter(|v| !v.is_empty()).cloned();
        }
    }
    None
}

/// Whether the F2 tools extension should be injected at all.
/// Returns `false` when CLI arguments make F2 preferences irrelevant:
/// - `--tools`/`-t`: explicit allowlist is authoritative
/// - `--no-tools`/`-nt`: all tools disabled
/// - `--no-builtin-tools`/`-nbt`: all builtins disabled
pub(super) fn should_inject_tools_extension(args: &[String]) -> bool {
    !has_explicit_tools_arg(args) && !has_no_tools_arg(args)
}

/// Comma-separated exclusion list to pass as `PI_GROK_EXCLUDE_TOOLS`.
/// Empty string when no `--exclude-tools` flag is present.
pub(super) fn cli_tool_exclusions(args: &[String]) -> String {
    excluded_tools(args).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_extension_source_is_loadable_typescript_module() {
        let file = write_tools_extension().expect("write tools extension");
        let source = std::fs::read_to_string(file.path()).expect("read extension");
        assert!(source.contains("PI_GROK_BUILTIN_TOOLS"));
        assert!(source.contains("setActiveTools"));
        assert!(source.contains("\"eval\""));
        assert_eq!(
            file.path().extension().and_then(|value| value.to_str()),
            Some("ts")
        );
    }

    #[test]
    fn detects_explicit_tools_allowlist() {
        assert!(has_explicit_tools_arg(&[
            "--tools".into(),
            "read,grep".into()
        ]));
        assert!(has_explicit_tools_arg(&["-t".into(), "read,grep".into()]));
        assert!(has_explicit_tools_arg(&["--tools=read,grep".into()]));
        assert!(!has_explicit_tools_arg(&[
            "--exclude-tools".into(),
            "bash".into()
        ]));
    }

    #[test]
    fn detects_no_tools_flags() {
        assert!(has_no_tools_arg(&["--no-tools".into()]));
        assert!(has_no_tools_arg(&["-nt".into()]));
        assert!(has_no_tools_arg(&["--no-builtin-tools".into()]));
        assert!(has_no_tools_arg(&["-nbt".into()]));
        assert!(!has_no_tools_arg(&["--tools".into(), "read".into()]));
        assert!(!has_no_tools_arg(&[
            "--exclude-tools".into(),
            "bash".into()
        ]));
    }

    #[test]
    fn extracts_excluded_tools() {
        assert_eq!(
            excluded_tools(&["--exclude-tools".into(), "bash,write".into()]),
            Some("bash,write".into())
        );
        assert_eq!(
            excluded_tools(&["-xt".into(), "grep".into()]),
            Some("grep".into())
        );
        assert_eq!(
            excluded_tools(&["--exclude-tools=bash,write".into()]),
            Some("bash,write".into())
        );
        assert_eq!(excluded_tools(&["--exclude-tools=".into()]), None);
        assert_eq!(excluded_tools(&["--tools".into(), "read".into()]), None);
    }

    #[test]
    fn injection_policy_respects_cli_tool_overrides() {
        assert!(should_inject_tools_extension(&[]));
        assert!(should_inject_tools_extension(&[
            "--exclude-tools=bash".into()
        ]));
        assert!(!should_inject_tools_extension(&["--tools=read".into()]));
        assert!(!should_inject_tools_extension(&["--no-tools".into()]));
        assert_eq!(
            cli_tool_exclusions(&["--exclude-tools=bash,write".into()]),
            "bash,write"
        );
    }
}
