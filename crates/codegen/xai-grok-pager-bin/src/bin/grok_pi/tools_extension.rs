use anyhow::{Context, Result};
use std::{fs::File, io::Write};
use tempfile::NamedTempFile;

/// Materialize the bridge extension that applies the F2-selected Pi built-in
/// tools without changing Pi's source. Registry-level exclusions are passed
/// through Pi's native `--exclude-tools` CLI mechanism.
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

#[cfg(windows)]
const BUILTIN_TOOL_NAMES: [&str; 9] = [
    "read", "bash", "powershell", "edit", "write", "grep", "find", "ls", "eval",
];
#[cfg(not(windows))]
const BUILTIN_TOOL_NAMES: [&str; 8] = [
    "read", "bash", "edit", "write", "grep", "find", "ls", "eval",
];
#[cfg(windows)]
const DEFAULT_BUILTIN_TOOLS: [&str; 5] = ["read", "bash", "powershell", "edit", "write"];
#[cfg(not(windows))]
const DEFAULT_BUILTIN_TOOLS: [&str; 4] = ["read", "bash", "edit", "write"];

pub(super) fn configured_builtin_tools() -> String {
    let Ok(config) = xai_grok_shell::config::load_effective_config() else {
        return DEFAULT_BUILTIN_TOOLS.join(",");
    };
    let Some(tools) = config
        .get("ui")
        .and_then(|ui| ui.get("pi_builtin_tools"))
        .and_then(toml::Value::as_table)
    else {
        return DEFAULT_BUILTIN_TOOLS.join(",");
    };
    BUILTIN_TOOL_NAMES
        .into_iter()
        .filter(|name| {
            tools
                .get(*name)
                .and_then(toml::Value::as_bool)
                .unwrap_or(DEFAULT_BUILTIN_TOOLS.contains(name))
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// Convert the F2 selected set into Pi's native registry-level denylist.
/// `setActiveTools()` alone only hides names from the model; `--exclude-tools`
/// also removes them from `getAllTools()` and prevents same-name extension
/// replacements (notably grok-pi's enhanced `bash`) from reappearing.
pub(super) fn disabled_builtin_tools_from_selected(selected: &str) -> String {
    let selected = selected
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>();
    BUILTIN_TOOL_NAMES
        .into_iter()
        .filter(|name| !selected.contains(name))
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

fn explicit_tools(args: &[String]) -> Option<String> {
    for (idx, arg) in args.iter().enumerate() {
        if let Some(value) = arg.strip_prefix("--tools=") {
            return Some(value.to_string());
        }
        if arg == "--tools" || arg == "-t" {
            return args.get(idx + 1).cloned();
        }
    }
    None
}

fn csv_contains(csv: &str, name: &str) -> bool {
    csv.split(',')
        .map(str::trim)
        .any(|candidate| candidate == name)
}

/// Whether a tool name is allowed by Pi's final CLI tool policy. This is used
/// for grok-pi-owned same-name bridges (notably enhanced Bash) so their host
/// control plane cannot remain enabled after Pi has excluded the tool itself.
/// `--no-builtin-tools` intentionally does not disable extension tools; that is
/// Pi's own distinction between builtin and extension/custom registrations.
pub(super) fn tool_name_allowed_by_cli(args: &[String], name: &str) -> bool {
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "--no-tools" | "-nt"))
    {
        return false;
    }
    if let Some(allowed) = explicit_tools(args) {
        return csv_contains(&allowed, name);
    }
    !excluded_tools(args).is_some_and(|excluded| csv_contains(&excluded, name))
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

fn push_unique_csv(out: &mut Vec<String>, csv: &str) {
    for name in csv
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        if !out.iter().any(|existing| existing == name) {
            out.push(name.to_string());
        }
    }
}

/// Merge host/F2 exclusions with any user `--exclude-tools`/`-xt` value and
/// normalize the Pi child argv to exactly one denylist flag. Pi's parser stores
/// only the last occurrence, so appending a second flag would silently discard
/// the user's earlier exclusions.
pub(super) fn merge_tool_exclusions(args: &mut Vec<String>, additional: &str) -> String {
    let mut exclusions = Vec::new();
    let mut kept = Vec::with_capacity(args.len() + 2);
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--exclude-tools" || arg == "-xt" {
            if let Some(value) = args.get(index + 1) {
                push_unique_csv(&mut exclusions, value);
                index += 2;
                continue;
            }
        } else if let Some(value) = arg.strip_prefix("--exclude-tools=") {
            push_unique_csv(&mut exclusions, value);
            index += 1;
            continue;
        }
        kept.push(arg.clone());
        index += 1;
    }
    push_unique_csv(&mut exclusions, additional);
    if !exclusions.is_empty() {
        kept.extend(["--exclude-tools".to_string(), exclusions.join(",")]);
    }
    *args = kept;
    exclusions.join(",")
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

    #[test]
    fn disabled_builtin_tools_become_registry_denylist() {
        #[cfg(windows)]
        assert_eq!(
            disabled_builtin_tools_from_selected("read,edit,write,grep,eval"),
            "bash,powershell,find,ls"
        );
        #[cfg(not(windows))]
        assert_eq!(
            disabled_builtin_tools_from_selected("read,edit,write,grep,eval"),
            "bash,find,ls"
        );
        #[cfg(windows)]
        assert_eq!(
            disabled_builtin_tools_from_selected("read,bash,powershell,edit,write"),
            "grep,find,ls,eval"
        );
        #[cfg(not(windows))]
        assert_eq!(
            disabled_builtin_tools_from_selected("read,bash,edit,write"),
            "grep,find,ls,eval"
        );
    }

    #[test]
    fn merges_user_and_f2_exclusions_into_one_pi_flag() {
        let mut args = vec![
            "--exclude-tools=write,bash".into(),
            "--foo".into(),
            "bar".into(),
        ];
        assert_eq!(
            merge_tool_exclusions(&mut args, "bash,find,ls"),
            "write,bash,find,ls"
        );
        assert_eq!(
            args,
            vec![
                "--foo".to_string(),
                "bar".to_string(),
                "--exclude-tools".to_string(),
                "write,bash,find,ls".to_string(),
            ]
        );

        let mut short = vec!["-xt".into(), "grep".into()];
        assert_eq!(merge_tool_exclusions(&mut short, "bash"), "grep,bash");
        assert_eq!(
            short,
            vec!["--exclude-tools".to_string(), "grep,bash".to_string()]
        );
    }

    #[test]
    fn final_cli_policy_controls_same_name_bridge_tools() {
        assert!(tool_name_allowed_by_cli(&[], "bash"));
        assert!(!tool_name_allowed_by_cli(
            &["--exclude-tools=bash,find".into()],
            "bash"
        ));
        assert!(!tool_name_allowed_by_cli(&["--no-tools".into()], "bash"));
        assert!(tool_name_allowed_by_cli(
            &["--no-builtin-tools".into()],
            "bash"
        ));
        assert!(tool_name_allowed_by_cli(
            &["--tools=read,bash".into()],
            "bash"
        ));
        assert!(!tool_name_allowed_by_cli(
            &["-t".into(), "read,edit".into()],
            "bash"
        ));
    }
}
