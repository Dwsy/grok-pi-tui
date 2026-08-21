use clap::Parser;
use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
};

use crate::GROK_PI_VERSION;

#[derive(Debug, Parser)]
#[command(
    name = "grok-pi",
    version = GROK_PI_VERSION,
    about = "Run the Pi agent core in Grok Build's production TUI",
    after_help = "\
Pi-compatible aliases:
  -ns   Alias for --no-skills
  -nc   Alias for --no-context-files
  -ne   Alias for --no-extensions
  -nt   Alias for --no-tools
  -nbt  Alias for --no-builtin-tools
  -xt   Alias for --exclude-tools
  -na   Alias for --no-approve

Passthrough:
  Arguments after `--` are forwarded unchanged to Pi after `--mode rpc`.
  Prefer first-class flags above when available.

  grok-pi -- --model openai/gpt-4o --session-dir ~/.pi/agent/sessions

Examples:
  grok-pi --continue
  grok-pi --model anthropic/claude-sonnet-4-5 --thinking high
  grok-pi --session-dir ~/.pi/agent/sessions --session abc123
  grok-pi --tools read,bash,grep --offline
  grok-pi --no-builtin-tools -e ./my-extension.ts

Notes:
  TUI is Grok Pager; agent core is Pi (always `--mode rpc`).
  Runtime /model and /resume use native Grok surfaces, not Pi's TUI pickers.
  Stock Grok `--resume` is not exposed: use Welcome, /resume, or `--session <uuid>`.
  On quit, prints: To resume this session: grok-pi --session <uuid>

Extension startup recovery:
  If Pi RPC bootstrap fails, grok-pi retries with zero extensions. If that works,
  it automatically binary-searches the loaded extension list for a crashing extension.
  This is automatic recovery, not a separate CLI mode or flag.
  Single culprit: prints `Extension crash detected`, names the extension/path, and
  relaunches without that extension.
  Combination conflict: prints `Extension conflict detected` and relaunches with
  all extensions disabled.
  Scope: bootstrap-time extension failures only; crashes after bootstrap are not bisected.
  `-ne` / `--no-extensions` disables discovery only; explicit `-e` paths and
  grok-pi host bridge extensions still load.
  True zero-extension diagnostic: grok-pi -ne --no-bridge-extensions
  Permanent block: add `[pi.resources] block = [\"...\"]` to ~/.grok-pi/config.toml
  or the project .grok-pi/pi-resources.toml sidecar.
  Crash-report prompt sends extension name/package identity and crash kind only;
  it does not send absolute paths, stack traces, or session data.

Update (GitHub releases only):
  grok-pi update            Install latest from Dwsy/grok-pi
  grok-pi update --check    Print current vs latest
  Welcome Ctrl+U            Same install when an update is offered

Home (isolated from stock Grok ~/.grok):
  Default state dir: ~/.grok-pi  (override with GROK_HOME)
  Project config dir: <repo>/.grok-pi  (override with GROK_PROJECT_DIR)
  grok-pi migrate-home          Copy allowlisted prefs from ~/.grok
  grok-pi migrate-home --status Preview + marker
  grok-pi migrate-home --dry-run"
)]
pub(super) struct Args {
    #[command(subcommand)]
    pub(super) command: Option<Command>,

    /// Pi executable. By default, use the repository-bundled Pi CLI when present.
    #[arg(long, default_value = "pi")]
    pub(super) pi_bin: String,

    /// Argument inserted before `--mode rpc` (repeatable).
    #[arg(long = "pi-prefix-arg")]
    pub(super) pi_prefix_args: Vec<String>,

    /// Working directory for both Pi and the native Grok pager.
    #[arg(long)]
    pub(super) pi_cwd: Option<PathBuf>,

    /// Continue previous session.
    #[arg(short = 'c', long = "continue")]
    pub(super) continue_last_session: bool,

    /// Provider name (default: google).
    #[arg(long, value_name = "NAME")]
    pub(super) provider: Option<String>,

    /// Model pattern or ID (supports "provider/id" and optional ":<thinking>").
    #[arg(long, value_name = "PATTERN")]
    pub(super) model: Option<String>,

    /// Comma-separated model patterns for cycling (globs and fuzzy matching).
    #[arg(long, value_name = "PATTERNS")]
    pub(super) models: Option<String>,

    /// Set thinking level: off, minimal, low, medium, high, xhigh, max.
    #[arg(long, value_name = "LEVEL")]
    pub(super) thinking: Option<String>,

    /// Use specific session file or partial UUID.
    #[arg(long, value_name = "PATH|ID")]
    pub(super) session: Option<String>,

    /// Use exact project session ID, creating it if missing.
    #[arg(long = "session-id", value_name = "ID")]
    pub(super) session_id: Option<String>,

    /// Fork specific session file or partial UUID into a new session.
    #[arg(long, value_name = "PATH|ID")]
    pub(super) fork: Option<String>,

    /// Directory for session storage and lookup.
    #[arg(long = "session-dir", value_name = "DIR")]
    pub(super) session_dir: Option<String>,

    /// System prompt (default: coding assistant prompt).
    #[arg(long, value_name = "TEXT")]
    pub(super) system_prompt: Option<String>,

    /// Append text or file contents to the system prompt (can be used multiple times).
    #[arg(long = "append-system-prompt", value_name = "TEXT")]
    pub(super) append_system_prompts: Vec<String>,

    /// Disable skills discovery and loading.
    #[arg(long)]
    pub(super) no_skills: bool,

    /// Disable AGENTS.md and CLAUDE.md discovery and loading.
    #[arg(long)]
    pub(super) no_context_files: bool,

    /// Load an extension file (can be used multiple times).
    #[arg(short = 'e', long = "extension", value_name = "PATH")]
    pub(super) extensions: Vec<String>,

    /// Disable extension discovery (explicit -e paths and host bridges still work).
    #[arg(long)]
    pub(super) no_extensions: bool,

    /// Disable grok-pi's bundled host bridge extensions.
    #[arg(long = "no-bridge-extensions")]
    pub(super) no_bridge_extensions: bool,

    /// Minutes before foreground Bash auto-backgrounds and each blocking task wait is capped.
    /// CLI overrides PI_GROK_BASH_MAX_WAIT_MINS; 0 or negative disables both behaviors.
    #[arg(long = "bash-max-wait-mins", value_name = "MIN", allow_hyphen_values = true)]
    pub(super) bash_max_wait_mins: Option<f64>,

    /// Comma-separated allowlist of tool names to enable.
    #[arg(short = 't', long = "tools", value_name = "TOOLS")]
    pub(super) tools: Option<String>,

    /// Comma-separated denylist of tool names to disable.
    #[arg(long = "exclude-tools", value_name = "TOOLS")]
    pub(super) exclude_tools: Option<String>,

    /// Disable all tools by default (built-in and extension).
    #[arg(long)]
    pub(super) no_tools: bool,

    /// Disable built-in tools by default but keep extension/custom tools enabled.
    #[arg(long = "no-builtin-tools")]
    pub(super) no_builtin_tools: bool,

    /// Don't save session (ephemeral).
    #[arg(long)]
    pub(super) no_session: bool,

    /// Set session display name.
    #[arg(short = 'n', long, value_name = "NAME")]
    pub(super) name: Option<String>,

    /// Trust project-local files for this run.
    #[arg(short = 'a', long = "approve", conflicts_with = "no_approve")]
    pub(super) approve: bool,

    /// Ignore project-local files for this run.
    #[arg(long = "no-approve", conflicts_with = "approve")]
    pub(super) no_approve: bool,

    /// Disable startup network operations (same as PI_OFFLINE=1).
    #[arg(long)]
    pub(super) offline: bool,

    /// Use Grok's native inline terminal mode instead of the alternate screen.
    #[arg(long)]
    pub(super) no_alt_screen: bool,

    /// Start in Grok's native minimal/scrollback renderer.
    #[arg(long, conflicts_with = "fullscreen")]
    pub(super) minimal: bool,

    /// Start in Grok's native fullscreen renderer.
    #[arg(long, conflicts_with = "minimal")]
    pub(super) fullscreen: bool,

    /// Print the protocol boundary and exit without starting a terminal.
    #[arg(long)]
    pub(super) print_capabilities: bool,

    /// Remaining arguments are passed unchanged to Pi after `--mode rpc`.
    #[arg(last = true, allow_hyphen_values = true)]
    pub(super) pi_args: Vec<String>,
}

#[derive(Debug, clap::Subcommand)]
pub(super) enum Command {
    /// Check for or install grok-pi updates from GitHub Releases only.
    Update {
        /// Only report current vs latest; do not install.
        #[arg(long)]
        check: bool,
        /// Machine-readable status (requires `--check`).
        #[arg(long, requires = "check")]
        json: bool,
        /// Reinstall even when already on the latest version.
        #[arg(long)]
        force: bool,
        /// Install a specific version (e.g. `0.0.2` or `v0.0.2`).
        /// Named `--to` so it does not clash with clap's global `--version`.
        #[arg(long = "to", value_name = "VERSION")]
        version: Option<String>,
    },

    /// Copy allowlisted state from stock Grok home (`~/.grok`) into grok-pi home (`~/.grok-pi`).
    ///
    /// Migrates pager settings, config.toml, skills, hooks, and related prefs.
    /// Does not move Pi sessions (`~/.pi`) or stock install caches (`bin/`, downloads/).
    /// Source is never deleted (copy only).
    #[command(name = "migrate-home")]
    MigrateHome {
        /// Legacy home to read from (default: `$GROK_LEGACY_HOME` or `~/.grok`).
        #[arg(long, value_name = "DIR")]
        from: Option<PathBuf>,
        /// Destination home (default: `$GROK_HOME` or `~/.grok-pi`).
        #[arg(long, value_name = "DIR")]
        into: Option<PathBuf>,
        /// Print actions without writing files.
        #[arg(long)]
        dry_run: bool,
        /// Overwrite files that already exist at the destination.
        #[arg(long)]
        force: bool,
        /// Also copy `auth.json` (Grok cloud tokens; Pi auth is separate).
        #[arg(long)]
        include_auth: bool,
        /// Report what would migrate and whether the marker is present.
        #[arg(long)]
        status: bool,
    },
}

pub(super) fn normalize_compound_short_flags(
    args: impl IntoIterator<Item = OsString>,
) -> Vec<OsString> {
    let mut parse_options = true;
    args.into_iter()
        .map(|arg| {
            if parse_options && arg.as_os_str() == OsStr::new("--") {
                parse_options = false;
                return arg;
            }
            if !parse_options {
                return arg;
            }
            match arg.to_str() {
                Some("-ns") => OsString::from("--no-skills"),
                Some("-nc") => OsString::from("--no-context-files"),
                Some("-ne") => OsString::from("--no-extensions"),
                Some("-nt") => OsString::from("--no-tools"),
                Some("-nbt") => OsString::from("--no-builtin-tools"),
                Some("-xt") => OsString::from("--exclude-tools"),
                Some("-na") => OsString::from("--no-approve"),
                _ => arg,
            }
        })
        .collect()
}

pub(super) fn pi_args_with_startup_flags(
    mut pi_args: Vec<String>,
    args: &Args,
    bridge_extension: Option<&Path>,
) -> Vec<String> {
    if args.continue_last_session {
        pi_args.push("--continue".to_string());
    }
    if let Some(provider) = &args.provider {
        pi_args.extend(["--provider".to_string(), provider.clone()]);
    }
    if let Some(model) = &args.model {
        pi_args.extend(["--model".to_string(), model.clone()]);
    }
    if let Some(models) = &args.models {
        pi_args.extend(["--models".to_string(), models.clone()]);
    }
    if let Some(thinking) = &args.thinking {
        pi_args.extend(["--thinking".to_string(), thinking.clone()]);
    }
    if let Some(session) = &args.session {
        pi_args.extend(["--session".to_string(), session.clone()]);
    }
    if let Some(session_id) = &args.session_id {
        pi_args.extend(["--session-id".to_string(), session_id.clone()]);
    }
    if let Some(fork) = &args.fork {
        pi_args.extend(["--fork".to_string(), fork.clone()]);
    }
    if let Some(session_dir) = &args.session_dir {
        pi_args.extend(["--session-dir".to_string(), session_dir.clone()]);
    }
    if let Some(system_prompt) = &args.system_prompt {
        pi_args.extend(["--system-prompt".to_string(), system_prompt.clone()]);
    }
    for append_system_prompt in &args.append_system_prompts {
        pi_args.extend([
            "--append-system-prompt".to_string(),
            append_system_prompt.clone(),
        ]);
    }
    if args.no_skills {
        pi_args.push("--no-skills".to_string());
    }
    if args.no_context_files {
        pi_args.push("--no-context-files".to_string());
    }
    for extension in &args.extensions {
        pi_args.extend(["--extension".to_string(), extension.clone()]);
    }
    // Explicit --extension paths still load under --no-extensions.
    if let Some(path) = bridge_extension {
        pi_args.extend([
            "--extension".to_string(),
            path.to_string_lossy().into_owned(),
        ]);
    }
    if args.no_extensions {
        pi_args.push("--no-extensions".to_string());
    }
    if let Some(tools) = &args.tools {
        pi_args.extend(["--tools".to_string(), tools.clone()]);
    }
    if let Some(exclude_tools) = &args.exclude_tools {
        pi_args.extend(["--exclude-tools".to_string(), exclude_tools.clone()]);
    }
    if args.no_tools {
        pi_args.push("--no-tools".to_string());
    }
    if args.no_builtin_tools {
        pi_args.push("--no-builtin-tools".to_string());
    }
    if args.no_session {
        pi_args.push("--no-session".to_string());
    }
    if let Some(name) = &args.name {
        pi_args.extend(["--name".to_string(), name.clone()]);
    }
    if args.approve {
        pi_args.push("--approve".to_string());
    }
    if args.no_approve {
        pi_args.push("--no-approve".to_string());
    }
    if args.offline {
        pi_args.push("--offline".to_string());
    }
    pi_args
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    use std::ffi::OsString;

    #[test]
    fn help_documents_extension_startup_recovery() {
        let help = Args::command().render_long_help().to_string();

        assert!(help.contains("Extension startup recovery:"));
        assert!(help.contains("automatically binary-searches"));
        assert!(help.contains("bootstrap-time extension failures only"));
        assert!(help.contains("grok-pi -ne --no-bridge-extensions"));
        assert!(help.contains("does not send absolute paths, stack traces, or session data"));
    }

    #[test]
    fn continue_flag_is_forwarded_to_pi() {
        let args = Args::try_parse_from(["grok-pi", "--continue"]).unwrap();
        assert!(args.continue_last_session);
        assert_eq!(
            pi_args_with_startup_flags(args.pi_args.clone(), &args, None),
            vec!["--continue"]
        );
    }

    #[test]
    fn short_continue_flag_is_forwarded_to_pi() {
        let args = Args::try_parse_from(["grok-pi", "-c"]).unwrap();
        assert!(args.continue_last_session);
        assert_eq!(
            pi_args_with_startup_flags(args.pi_args.clone(), &args, None),
            vec!["--continue"]
        );
    }

    #[test]
    fn pi_startup_flags_are_forwarded_to_pi() {
        let args = Args::try_parse_from(normalize_compound_short_flags(
            [
                "grok-pi",
                "--provider",
                "openai",
                "--model",
                "openai/gpt-4o",
                "--models",
                "openai/*,anthropic/*",
                "--thinking",
                "high",
                "--session",
                "sess-abc",
                "--session-id",
                "exact-id",
                "--fork",
                "fork-src",
                "--session-dir",
                "/tmp/sessions",
                "--system-prompt",
                "base prompt",
                "--append-system-prompt",
                "first addition",
                "--append-system-prompt",
                "second addition",
                "-ns",
                "-nc",
                "-e",
                "first.ts",
                "--extension",
                "second.ts",
                "-ne",
                "-t",
                "read,bash",
                "-xt",
                "write",
                "-nt",
                "-nbt",
                "--no-session",
                "-n",
                "named-session",
                "-a",
                "--offline",
            ]
            .into_iter()
            .map(OsString::from),
        ))
        .unwrap();

        assert_eq!(
            pi_args_with_startup_flags(
                args.pi_args.clone(),
                &args,
                Some(Path::new("/tmp/bridge.ts")),
            ),
            vec![
                "--provider",
                "openai",
                "--model",
                "openai/gpt-4o",
                "--models",
                "openai/*,anthropic/*",
                "--thinking",
                "high",
                "--session",
                "sess-abc",
                "--session-id",
                "exact-id",
                "--fork",
                "fork-src",
                "--session-dir",
                "/tmp/sessions",
                "--system-prompt",
                "base prompt",
                "--append-system-prompt",
                "first addition",
                "--append-system-prompt",
                "second addition",
                "--no-skills",
                "--no-context-files",
                "--extension",
                "first.ts",
                "--extension",
                "second.ts",
                "--extension",
                "/tmp/bridge.ts",
                "--no-extensions",
                "--tools",
                "read,bash",
                "--exclude-tools",
                "write",
                "--no-tools",
                "--no-builtin-tools",
                "--no-session",
                "--name",
                "named-session",
                "--approve",
                "--offline",
            ]
        );
    }

    #[test]
    fn no_bridge_extensions_is_host_only() {
        let args = Args::try_parse_from(["grok-pi", "--no-bridge-extensions"]).unwrap();
        assert!(args.no_bridge_extensions);
        assert_eq!(
            pi_args_with_startup_flags(args.pi_args.clone(), &args, None),
            Vec::<String>::new()
        );
    }

    #[test]
    fn bash_max_wait_mins_is_host_only_and_accepts_off_values() {
        let args = Args::try_parse_from(["grok-pi", "--bash-max-wait-mins", "4.25"]).unwrap();
        assert_eq!(args.bash_max_wait_mins, Some(4.25));
        assert!(pi_args_with_startup_flags(args.pi_args.clone(), &args, None).is_empty());

        let off = Args::try_parse_from(["grok-pi", "--bash-max-wait-mins", "-1"]).unwrap();
        assert_eq!(off.bash_max_wait_mins, Some(-1.0));
    }

    #[test]
    fn no_approve_short_flag_is_forwarded_to_pi() {
        let args = Args::try_parse_from(normalize_compound_short_flags(
            ["grok-pi", "-na"].into_iter().map(OsString::from),
        ))
        .unwrap();
        assert!(args.no_approve);
        assert_eq!(
            pi_args_with_startup_flags(args.pi_args.clone(), &args, None),
            vec!["--no-approve"]
        );
    }

    #[test]
    fn compound_short_flags_after_double_dash_are_not_rewritten() {
        assert_eq!(
            normalize_compound_short_flags(
                [
                    "grok-pi", "--", "-ns", "-nc", "-ne", "-nt", "-nbt", "-xt", "-na"
                ]
                .into_iter()
                .map(OsString::from),
            ),
            [
                "grok-pi", "--", "-ns", "-nc", "-ne", "-nt", "-nbt", "-xt", "-na"
            ]
            .into_iter()
            .map(OsString::from)
            .collect::<Vec<_>>(),
        );
    }
}
