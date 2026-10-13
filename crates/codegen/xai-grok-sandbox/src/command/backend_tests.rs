use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

#[cfg(unix)]
use crate::command::canonical::ServedRoot;
use crate::command::env::EnvGlobs;
#[cfg(unix)]
use crate::command::git_config::GitConfigEnv;
use crate::command::mode::SandboxMode;
#[cfg(unix)]
use crate::command::policy::PolicyError;
use crate::command::policy::{EnvPolicy, NetworkPolicy, ReadPolicy, SandboxPolicy};
#[cfg(unix)]
use crate::command::protected::{self, HARD_LINK_SCAN_LIMIT, ProtectedInputs};

use super::{
    BackendCapabilities, BackendName, CallId, CommandTag, OriginalArgv, RenderedPolicy,
    SandboxBackend, SandboxCommandError, WrapReceipt, wrap_for_mode,
};

/// A backend that renders the tag and the original argv as a fake SBPL profile, so the tests can
/// assert what `wrap_for_mode` handed it without `sandbox-exec`.
#[derive(Default)]
struct StubBackend {
    wraps: AtomicUsize,
    /// The explicit environment `cmd` carried when `wrap` was called.
    env_at_wrap: std::sync::Mutex<BTreeMap<OsString, Option<OsString>>>,
}

impl SandboxBackend for StubBackend {
    fn name(&self) -> BackendName {
        BackendName::Seatbelt
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities::default()
    }

    fn wrap(
        &self,
        cmd: &mut tokio::process::Command,
        original: &OriginalArgv,
        _policy: &SandboxPolicy,
        tag: &CommandTag,
    ) -> Result<WrapReceipt, SandboxCommandError> {
        self.wraps.fetch_add(1, Ordering::SeqCst);
        *self.env_at_wrap.lock().unwrap() = envs(cmd);
        let mut argv: Vec<OsString> = vec![OsString::from(tag.as_ref())];
        argv.push(original.program.clone().into_os_string());
        argv.extend(original.args.iter().cloned());
        *cmd = tokio::process::Command::new("/stub/wrapper");
        cmd.args(&argv);
        Ok(WrapReceipt {
            backend: BackendName::Seatbelt,
            rendered: RenderedPolicy::Sbpl {
                profile: format!("(stub {tag})"),
                params: argv
                    .iter()
                    .enumerate()
                    .map(|(index, arg)| {
                        (format!("ARG_{index}"), arg.to_string_lossy().into_owned())
                    })
                    .collect(),
            },
        })
    }
}

fn policy(exclude_globs: Vec<String>) -> SandboxPolicy {
    SandboxPolicy {
        read: ReadPolicy::AllExcept { deny: Vec::new() },
        write_roots: Vec::new(),
        network: NetworkPolicy::Proxy { port: 3128 },
        env: EnvPolicy {
            exclude_globs: EnvGlobs::new(exclude_globs).unwrap(),
            set: EnvPolicy::proxy_vars(3128),
        },
        protected: Vec::new(),
        build_cache_trees: Vec::new(),
        unread_git_metadata: Vec::new(),
    }
}

fn original() -> OriginalArgv {
    OriginalArgv {
        program: PathBuf::from("/bin/sh"),
        args: vec![OsString::from("-c"), OsString::from("echo hi")],
        cwd: PathBuf::from("/ws"),
    }
}

fn envs(cmd: &tokio::process::Command) -> BTreeMap<OsString, Option<OsString>> {
    cmd.as_std()
        .get_envs()
        .map(|(k, v)| (k.to_os_string(), v.map(|v| v.to_os_string())))
        .collect()
}

fn command_with_secret() -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("/bin/sh");
    cmd.args(["-c", "echo hi"]);
    cmd.env("TOOL_API_KEY", "leak");
    cmd
}

#[test]
fn off_leaves_the_command_untouched() {
    let backend = StubBackend::default();
    let mut cmd = command_with_secret();
    let receipt = wrap_for_mode(
        SandboxMode::Off,
        Some(&backend),
        &mut cmd,
        &original(),
        &policy(EnvPolicy::default_excludes()),
        &CommandTag::for_call(&CallId::tool("c1")),
    )
    .unwrap();
    assert_eq!(None, receipt);
    assert_eq!(Path::new("/bin/sh"), cmd.as_std().get_program());
    assert_eq!(
        BTreeMap::from([(OsString::from("TOOL_API_KEY"), Some(OsString::from("leak")))]),
        envs(&cmd)
    );
    assert_eq!(0, backend.wraps.load(Ordering::SeqCst));
}

/// Observe never wraps, with or without a backend: the proxy pointers are added and nothing is
/// removed, so the command runs with today's OS permissions and the proxy does the recording.
#[test]
fn observe_sets_the_proxy_pointers_and_removes_nothing() {
    let backend = StubBackend::default();
    let mut cmd = command_with_secret();
    let receipt = wrap_for_mode(
        SandboxMode::Observe,
        Some(&backend),
        &mut cmd,
        &original(),
        &policy(EnvPolicy::default_excludes()),
        &CommandTag::for_call(&CallId::tool("c1")),
    )
    .unwrap();
    assert_eq!(None, receipt);
    assert_eq!(Path::new("/bin/sh"), cmd.as_std().get_program());
    let envs = envs(&cmd);
    assert_eq!(
        Some(&Some(OsString::from("leak"))),
        envs.get(&OsString::from("TOOL_API_KEY"))
    );
    assert_eq!(
        Some(&Some(OsString::from("http://127.0.0.1:3128"))),
        envs.get(&OsString::from("HTTP_PROXY"))
    );
    assert_eq!(0, backend.wraps.load(Ordering::SeqCst));
}

#[test]
fn observe_without_a_backend_only_sets_env() {
    let mut cmd = command_with_secret();
    let receipt = wrap_for_mode(
        SandboxMode::Observe,
        None,
        &mut cmd,
        &original(),
        &policy(EnvPolicy::default_excludes()),
        &CommandTag::for_call(&CallId::tool("c1")),
    )
    .unwrap();
    assert_eq!(None, receipt);
    assert_eq!(Path::new("/bin/sh"), cmd.as_std().get_program());
    assert!(envs(&cmd).contains_key(&OsString::from("HTTPS_PROXY")));
}

#[test]
fn enforce_without_a_backend_refuses_to_run() {
    let mut cmd = command_with_secret();
    let error = wrap_for_mode(
        SandboxMode::Enforce,
        None,
        &mut cmd,
        &original(),
        &policy(EnvPolicy::default_excludes()),
        &CommandTag::for_call(&CallId::tool("c1")),
    )
    .unwrap_err();
    assert!(
        matches!(&error, SandboxCommandError::Unavailable { reason } if reason.contains("no per-command sandbox backend")),
        "{error}"
    );
}

#[test]
fn enforce_filters_the_environment_then_wraps() {
    let backend = StubBackend::default();
    let mut cmd = command_with_secret();
    let receipt = wrap_for_mode(
        SandboxMode::Enforce,
        Some(&backend),
        &mut cmd,
        &original(),
        &policy(EnvPolicy::default_excludes()),
        &CommandTag::for_call(&CallId::tool("c1")),
    )
    .unwrap()
    .unwrap();
    assert_eq!(BackendName::Seatbelt, receipt.backend);
    assert_eq!(
        RenderedPolicy::Sbpl {
            profile: "(stub grok-c1)".to_owned(),
            params: vec![
                ("ARG_0".to_owned(), "grok-c1".to_owned()),
                ("ARG_1".to_owned(), "/bin/sh".to_owned()),
                ("ARG_2".to_owned(), "-c".to_owned()),
                ("ARG_3".to_owned(), "echo hi".to_owned()),
            ],
        },
        receipt.rendered
    );
    assert_eq!(Path::new("/stub/wrapper"), cmd.as_std().get_program());
    assert_eq!(1, backend.wraps.load(Ordering::SeqCst));
    let at_wrap = backend.env_at_wrap.lock().unwrap().clone();
    assert_eq!(Some(&None), at_wrap.get(&OsString::from("TOOL_API_KEY")));
    assert_eq!(
        Some(&Some(OsString::from("http://127.0.0.1:3128"))),
        at_wrap.get(&OsString::from("HTTP_PROXY"))
    );
}

/// A protected file with a hard-link alias under a write root refuses the wrap in `wrap_for_mode`
/// itself, so every backend inherits the check: the backend is never asked and the command is
/// left untouched. Without the alias the same policy wraps.
#[cfg(unix)]
#[test]
fn enforce_refuses_a_hard_linked_protected_file_before_any_backend_wraps() {
    let base = std::env::temp_dir().join(format!(
        "xai-sandbox-backend-hard-link-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let ws = dunce::canonicalize(&base).unwrap().join("ws");
    std::fs::create_dir_all(ws.join(".git")).unwrap();
    let config = ws.join(".git/config");
    std::fs::write(&config, "[core]\n").unwrap();
    let mut policy = policy(EnvPolicy::default_excludes());
    policy.write_roots = vec![ws.clone()];
    policy.protected = vec![crate::command::protected::Protected::Path {
        path: config.clone(),
    }];
    std::fs::hard_link(&config, ws.join("alias")).unwrap();
    let backend = StubBackend::default();
    let mut cmd = command_with_secret();
    let error = wrap_for_mode(
        SandboxMode::Enforce,
        Some(&backend),
        &mut cmd,
        &original(),
        &policy,
        &CommandTag::for_call(&CallId::tool("c1")),
    )
    .unwrap_err();
    assert!(
        matches!(
            &error,
            SandboxCommandError::Policy(crate::command::policy::PolicyError::HardLinkedProtected {
                path,
                nlink: 2,
                alias,
            }) if *path == config && *alias == ws.join("alias")
        ),
        "{error}"
    );
    let reason = error.to_string();
    assert!(reason.contains(&config.display().to_string()), "{reason}");
    assert!(
        reason.contains(&ws.join("alias").display().to_string()),
        "{reason}"
    );
    assert!(reason.contains("remove the extra link"), "{reason}");
    assert_eq!(0, backend.wraps.load(Ordering::SeqCst));
    assert_eq!(Path::new("/bin/sh"), cmd.as_std().get_program());
    assert!(envs(&cmd).contains_key(&OsString::from("TOOL_API_KEY")));

    std::fs::remove_file(ws.join("alias")).unwrap();
    let mut cmd = command_with_secret();
    wrap_for_mode(
        SandboxMode::Enforce,
        Some(&backend),
        &mut cmd,
        &original(),
        &policy,
        &CommandTag::for_call(&CallId::tool("c1")),
    )
    .unwrap();
    assert_eq!(1, backend.wraps.load(Ordering::SeqCst));
    let _ = std::fs::remove_dir_all(&base);
}

/// A git file the floor had to read and could not (a `.git/config` past the read limit) leaves
/// the hooks path it may set unknown: enforce refuses in `wrap_for_mode` itself, before any
/// backend or the hard-link check, with what to do; observe records and runs as before, and off
/// is untouched. With the list empty the same policy wraps.
#[test]
fn enforce_refuses_a_policy_with_unread_git_metadata_and_observe_runs() {
    let unread = crate::command::git_config::GitMetadataUnread::TooLarge {
        path: PathBuf::from("/ws/.git/config"),
        limit: 1024 * 1024,
    };
    let mut policy = policy(EnvPolicy::default_excludes());
    policy.unread_git_metadata = vec![unread.clone()];
    let backend = StubBackend::default();
    let tag = CommandTag::for_call(&CallId::tool("c1"));
    let mut cmd = command_with_secret();
    let error = wrap_for_mode(
        SandboxMode::Enforce,
        Some(&backend),
        &mut cmd,
        &original(),
        &policy,
        &tag,
    )
    .unwrap_err();
    assert!(
        matches!(
            &error,
            SandboxCommandError::Policy(crate::command::policy::PolicyError::GitMetadataUnread {
                unread: reported
            }) if *reported == unread
        ),
        "{error}"
    );
    let message = error.to_string();
    assert!(message.contains("/ws/.git/config"), "{message}");
    assert!(message.contains("larger than 1048576 bytes"), "{message}");
    assert!(message.contains("observe mode"), "{message}");
    assert_eq!(0, backend.wraps.load(Ordering::SeqCst));
    assert_eq!(Path::new("/bin/sh"), cmd.as_std().get_program());
    assert!(envs(&cmd).contains_key(&OsString::from("TOOL_API_KEY")));

    for mode in [SandboxMode::Observe, SandboxMode::Off] {
        let mut cmd = command_with_secret();
        let receipt =
            wrap_for_mode(mode, Some(&backend), &mut cmd, &original(), &policy, &tag).unwrap();
        assert_eq!(None, receipt);
        assert_eq!(0, backend.wraps.load(Ordering::SeqCst));
    }

    policy.unread_git_metadata.clear();
    let mut cmd = command_with_secret();
    wrap_for_mode(
        SandboxMode::Enforce,
        Some(&backend),
        &mut cmd,
        &original(),
        &policy,
        &tag,
    )
    .unwrap();
    assert_eq!(1, backend.wraps.load(Ordering::SeqCst));
}

/// The same refusal for a file the floor names only by pattern: a submodule hook
/// (`.git/modules/<name>/hooks/pre-commit`) with a hard-link alias under a write root.
#[cfg(unix)]
#[test]
fn enforce_refuses_a_hard_linked_submodule_hook_the_floor_names_by_glob() {
    let base = std::env::temp_dir().join(format!(
        "xai-sandbox-backend-hard-link-glob-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let ws = dunce::canonicalize(&base).unwrap().join("ws");
    let hooks = ws.join(".git/modules/x/hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    std::fs::write(ws.join("pre-commit.sh"), "#!/bin/sh\n").unwrap();
    std::fs::hard_link(ws.join("pre-commit.sh"), hooks.join("pre-commit")).unwrap();
    let mut policy = policy(EnvPolicy::default_excludes());
    policy.write_roots = vec![ws.clone()];
    policy.protected = vec![crate::command::protected::Protected::Glob {
        glob: format!("{}/.git/modules/**/hooks", ws.display()),
    }];
    let backend = StubBackend::default();
    let mut cmd = command_with_secret();
    let error = wrap_for_mode(
        SandboxMode::Enforce,
        Some(&backend),
        &mut cmd,
        &original(),
        &policy,
        &CommandTag::for_call(&CallId::tool("c1")),
    )
    .unwrap_err();
    assert!(
        matches!(
            &error,
            SandboxCommandError::Policy(crate::command::policy::PolicyError::HardLinkedProtected {
                path,
                nlink: 2,
                alias,
            }) if *path == hooks.join("pre-commit") && *alias == ws.join("pre-commit.sh")
        ),
        "{error}"
    );
    assert_eq!(0, backend.wraps.load(Ordering::SeqCst));
    let _ = std::fs::remove_dir_all(&base);
}

/// Home files linked only to each other leave no link to write through, so enforce wraps with a
/// workspace too large to search. A third name outside the floor that cannot be searched for,
/// and a protected folder too large to list, each refuse with what to do before any backend is
/// asked.
#[cfg(unix)]
#[test]
fn enforce_runs_with_linked_home_files_and_refuses_what_it_cannot_rule_out() {
    let base = std::env::temp_dir().join(format!(
        "xai-sandbox-backend-hard-link-home-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let base = dunce::canonicalize(&base).unwrap();
    let (ws, home) = (base.join("ws"), base.join("home"));
    std::fs::create_dir_all(ws.join("src")).unwrap();
    for index in 0..=HARD_LINK_SCAN_LIMIT {
        std::fs::write(ws.join(format!("src/f{index}")), "").unwrap();
    }
    std::fs::create_dir_all(home.join("dotfiles")).unwrap();
    std::fs::write(home.join(".bashrc"), "").unwrap();
    std::fs::hard_link(home.join(".bashrc"), home.join(".bash_profile")).unwrap();
    let mut policy = policy(EnvPolicy::default_excludes());
    policy.write_roots = vec![ws.clone()];
    policy.protected = protected::floor(&ProtectedInputs {
        workspace_root: &ServedRoot::pin(&ws),
        grok_home: &home.join(".grok"),
        user_home: Some(&home),
        control_socket_dir: &base.join("ctl"),
        git_env: &GitConfigEnv::default(),
    });
    let backend = StubBackend::default();
    let tag = CommandTag::for_call(&CallId::tool("c1"));
    let enforce = |cmd: &mut tokio::process::Command| {
        wrap_for_mode(
            SandboxMode::Enforce,
            Some(&backend),
            cmd,
            &original(),
            &policy,
            &tag,
        )
    };
    enforce(&mut command_with_secret()).unwrap();
    assert_eq!(1, backend.wraps.load(Ordering::SeqCst));

    std::fs::hard_link(home.join(".bashrc"), home.join("dotfiles/bashrc")).unwrap();
    let mut cmd = command_with_secret();
    let error = enforce(&mut cmd).unwrap_err();
    assert!(
        matches!(
            &error,
            SandboxCommandError::Policy(PolicyError::HardLinkUnverified { root, .. })
                if *root == ws
        ),
        "{error}"
    );
    let message = error.to_string();
    assert!(message.contains("remove the extra link"), "{message}");
    assert!(message.contains("observe mode"), "{message}");
    assert_eq!(Path::new("/bin/sh"), cmd.as_std().get_program());

    std::fs::remove_file(home.join("dotfiles/bashrc")).unwrap();
    std::fs::create_dir_all(ws.join(".idea")).unwrap();
    for index in 0..=HARD_LINK_SCAN_LIMIT {
        std::fs::write(ws.join(format!(".idea/f{index}")), "").unwrap();
    }
    let error = enforce(&mut command_with_secret()).unwrap_err();
    assert!(
        matches!(
            &error,
            SandboxCommandError::Policy(PolicyError::ProtectedTreeUnchecked { tree })
                if *tree == ws.join(".idea")
        ),
        "{error}"
    );
    assert!(error.to_string().contains("observe mode"), "{error}");
    assert_eq!(1, backend.wraps.load(Ordering::SeqCst));
    let _ = std::fs::remove_dir_all(&base);
}

/// A search for a leftover link walks the workspace without following its symlinks: links to a
/// tree too large to search, in the workspace and in its protected hooks directory, and a link
/// to the protected file itself, leave the command to run.
#[cfg(unix)]
#[test]
fn enforce_runs_past_symlinks_out_of_the_workspace() {
    let base = std::env::temp_dir().join(format!(
        "xai-sandbox-backend-symlinked-out-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let base = dunce::canonicalize(&base).unwrap();
    let (ws, home, outside) = (base.join("ws"), base.join("home"), base.join("outside"));
    std::fs::create_dir_all(ws.join(".git/hooks")).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    for index in 0..=HARD_LINK_SCAN_LIMIT {
        std::fs::write(outside.join(format!("f{index}")), "").unwrap();
    }
    std::fs::create_dir_all(home.join("dotfiles")).unwrap();
    std::fs::write(home.join(".bashrc"), "").unwrap();
    std::fs::hard_link(home.join(".bashrc"), home.join("dotfiles/bashrc")).unwrap();
    std::os::unix::fs::symlink(&outside, ws.join("node_modules")).unwrap();
    std::os::unix::fs::symlink(&outside, ws.join(".git/hooks/shared")).unwrap();
    std::os::unix::fs::symlink(home.join(".bashrc"), ws.join("bashrc")).unwrap();
    let mut policy = policy(EnvPolicy::default_excludes());
    policy.write_roots = vec![ws.clone()];
    policy.protected = protected::floor(&ProtectedInputs {
        workspace_root: &ServedRoot::pin(&ws),
        grok_home: &home.join(".grok"),
        user_home: Some(&home),
        control_socket_dir: &base.join("ctl"),
        git_env: &GitConfigEnv::default(),
    });
    let backend = StubBackend::default();
    let tag = CommandTag::for_call(&CallId::tool("c1"));
    wrap_for_mode(
        SandboxMode::Enforce,
        Some(&backend),
        &mut command_with_secret(),
        &original(),
        &policy,
        &tag,
    )
    .unwrap();
    assert_eq!(1, backend.wraps.load(Ordering::SeqCst));
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn command_tag_is_the_call_id_with_the_grok_prefix() {
    let tag = CommandTag::for_call(&CallId::tool("tc-42"));
    assert_eq!("grok-tc-42", tag.as_ref());
    assert_eq!("grok-tc-42", tag.to_string());
    assert_eq!(Some(CallId::tool("tc-42")), tag.call_id());
}

#[test]
fn a_tool_id_spelled_like_a_shell_init_spawn_stays_a_tool_call() {
    for id in ["shell-init:login-env", "tool:x", "tool:shell-init:y"] {
        let call = CallId::tool(id);
        assert_eq!(format!("tool:{id}"), call.to_string());
        assert_eq!(call, CallId::parse(&call.to_string()));
        assert_eq!(Some(call.clone()), CommandTag::for_call(&call).call_id());
    }
    for label in ["login-env", "tool:x"] {
        let init = CallId::shell_init(label);
        assert_eq!(format!("shell-init:{label}"), init.to_string());
        assert_eq!(init, CallId::parse(&init.to_string()));
    }
    assert_eq!("tc-5", CallId::tool("tc-5").to_string());
}
