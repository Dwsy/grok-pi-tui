use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::command::canonical::{VolumeRule, with_volume_rule};
use crate::command::grants::{
    Expiry, Grant, GrantDecision, GrantId, GrantScope, GrantSubject, HostPattern,
};
use crate::command::policy::{DenyEntry, EnvPolicy, NetworkPolicy, ReadPolicy, SandboxPolicy};
use crate::command::protected::Protected;

use super::{
    Blocked, Capability, CommandExit, DecodeInput, Disposition, InformationalReason, OwnTargets,
    ProposalBounds, Replay, Violation, decode, refused_under_grant,
};

fn status(code: i32) -> CommandExit {
    CommandExit::code(code)
}

struct Scratch {
    root: PathBuf,
    ws: PathBuf,
}

impl Scratch {
    fn new(tag: &str) -> Scratch {
        let root =
            std::env::temp_dir().join(format!("grok-sandbox-decode-{tag}-{}", std::process::id()));
        let ws = root.join("ws");
        std::fs::create_dir_all(ws.join(".git").join("hooks")).unwrap();
        std::fs::create_dir_all(root.join("outside").join("dir")).unwrap();
        Scratch { root, ws }
    }

    fn policy(&self) -> SandboxPolicy {
        let ws = self.ws.clone();
        SandboxPolicy {
            read: ReadPolicy::AllExcept { deny: Vec::new() },
            write_roots: vec![ws.clone()],
            network: NetworkPolicy::Off,
            env: EnvPolicy::default(),
            protected: vec![Protected::Path {
                path: ws.join(".git").join("hooks"),
            }],
            build_cache_trees: Vec::new(),
            unread_git_metadata: Vec::new(),
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A decode with no argv: the command named nothing, so only a path under the cwd (the
/// workspace) is its own. Tests that expect a grant name the target through [`script`].
fn input<'a>(
    scratch: &'a Scratch,
    policy: &'a SandboxPolicy,
    exit: CommandExit,
    stderr: &'a str,
) -> DecodeInput<'a> {
    DecodeInput {
        exit,
        output: stderr.as_bytes(),
        partial_output: None,
        cwd: &scratch.ws,
        argv: &[],
        policy,
        ran_sandboxed: true,
        bounds: ProposalBounds {
            workspace_root: &scratch.ws,
            user_home: None,
            extra_bases: &[],
        },
    }
}

/// The shell call's argv for `text`: `bash -lc '<text>'` as the hub spawns it.
fn script(text: &str) -> Vec<OsString> {
    vec![OsString::from("-lc"), OsString::from(text)]
}

/// The Seatbelt EPERM line `touch` prints for a denied write.
fn touch_denied(target: &Path) -> String {
    format!("touch: {}: Operation not permitted\n", target.display())
}

/// `touch <target>` as the model typed it.
fn touch_script(target: &Path) -> Vec<OsString> {
    script(&format!("touch {}", target.display()))
}

#[test]
fn quick_rejects_return_none() {
    let scratch = Scratch::new("quick");
    let policy = scratch.policy();
    let denied = "touch: cannot touch '/etc/x': Permission denied\n";
    let ok = status(0);
    assert_eq!(None, decode(input(&scratch, &policy, ok, denied)));
    let not_found = status(127);
    assert_eq!(None, decode(input(&scratch, &policy, not_found, denied)));
    let not_exec = status(126);
    assert_eq!(None, decode(input(&scratch, &policy, not_exec, denied)));
    let one = status(1);
    let mut unsandboxed = input(&scratch, &policy, one, denied);
    unsandboxed.ran_sandboxed = false;
    assert_eq!(None, decode(unsandboxed));
}

#[test]
fn coarse_write_outside_the_workspace_proposes_the_nearest_existing_directory() {
    let scratch = Scratch::new("coarse");
    let policy = scratch.policy();
    let target = scratch
        .root
        .join("outside")
        .join("dir")
        .join("new")
        .join("file.txt");
    let stderr = format!(
        "touch: cannot touch '{}': Operation not permitted\n",
        target.display()
    );
    let argv = touch_script(&target);
    let one = status(1);
    let violation = decode(DecodeInput {
        partial_output: Some(true),
        argv: &argv,
        ..input(&scratch, &policy, one, &stderr)
    })
    .unwrap();
    assert_eq!(Blocked::FsWrite { path: target }, violation.blocked);
    assert_eq!(
        Some(GrantSubject::FsWriteRoot {
            root: scratch.root.join("outside").join("dir"),
        }),
        violation.proposed
    );
    assert_eq!(Disposition::Grantable, violation.disposition);
    assert_eq!(Some(true), violation.partial_output);
    assert_eq!(Replay::Rerun, violation.replay);
    assert_eq!(Some(1), violation.exit_code);
    assert!(violation.is_grantable());
    assert!(violation.produces_card());
    assert_eq!(stderr.trim(), violation.stderr_snippet);
}

#[test]
fn write_the_policy_already_allows_is_not_a_violation() {
    let scratch = Scratch::new("allowed");
    let policy = scratch.policy();
    let stderr = format!(
        "touch: cannot touch '{}': Permission denied\n",
        scratch.ws.join("file").display()
    );
    let one = status(1);
    assert_eq!(None, decode(input(&scratch, &policy, one, &stderr)));
}

/// On the run after a grant, a denial on the very target the widened
/// policy allows is the backend failing to apply the grant, and the model is told so — never
/// handed bash's error as if nothing had been granted.
#[test]
fn a_denial_on_a_target_the_widened_policy_allows_is_refused_under_the_grant() {
    let scratch = Scratch::new("refused-again");
    let policy = scratch.policy();
    let target = scratch.root.join("outside").join("dir").join("notes.txt");
    let widened = policy
        .clone()
        .with_grant(&Grant {
            id: GrantId::new("0192c1a0-0000-7000-8000-0000000000a6"),
            subject: GrantSubject::FsWriteRoot {
                root: target.clone(),
            },
            scope: GrantScope::Session,
            expires: Expiry::Never,
            decision: GrantDecision::Allow,
            granted_at: 0,
            granted_by: "hub:s1".to_owned(),
            via: None,
        })
        .unwrap();
    let stderr = format!("bash: {}: Operation not permitted\n", target.display());
    let one = status(1);
    // The first run's decode has a violation to card; the widened policy has none
    assert!(decode(input(&scratch, &policy, one, &stderr)).is_some());
    assert_eq!(None, decode(input(&scratch, &widened, one, &stderr)));

    let refused = refused_under_grant(input(&scratch, &widened, one, &stderr)).unwrap();
    assert_eq!(
        Blocked::FsWrite {
            path: target.clone(),
        },
        refused.blocked
    );
    assert_eq!(Some(1), refused.exit_code);
    assert_eq!(stderr.trim(), refused.stderr_snippet);
    let text = refused.text(Some("Seatbelt cannot express a grant on a firmlink alias"));
    assert!(
        text.starts_with(&format!(
            "sandbox: refused again under the grant: write to {} is allowed by the policy but Seatbelt cannot express a grant on a firmlink alias.",
            target.display()
        )),
        "{text}"
    );
    assert!(text.contains("Exit 1. stderr: bash:"), "{text}");
    assert!(
        refused
            .text(None)
            .contains("but the sandbox backend could not apply the grant to this target."),
        "{}",
        refused.text(None)
    );
    assert!(
        refused
            .text(Some("  "))
            .contains("could not apply the grant")
    );

    // A target the widened policy still refuses is a violation, not a refused grant
    let elsewhere = format!(
        "bash: {}: Operation not permitted\n",
        scratch.root.join("elsewhere").join("x").display()
    );
    assert_eq!(
        None,
        refused_under_grant(input(&scratch, &widened, one, &elsewhere))
    );
    // The quick rejects apply the same way
    let ok = status(0);
    assert_eq!(
        None,
        refused_under_grant(input(&scratch, &widened, ok, &stderr))
    );
    let mut unsandboxed = input(&scratch, &widened, one, &stderr);
    unsandboxed.ran_sandboxed = false;
    assert_eq!(None, refused_under_grant(unsandboxed));
}

fn write_grant(root: PathBuf) -> Grant {
    Grant {
        id: GrantId::new("0192c1a0-0000-7000-8000-0000000000c5"),
        subject: GrantSubject::FsWriteRoot { root },
        scope: GrantScope::Session,
        expires: Expiry::Never,
        decision: GrantDecision::Allow,
        granted_at: 0,
        granted_by: "hub:s1".to_owned(),
        via: None,
    }
}

/// Every path question the decoder asks compares as the volume does: under APFS a missing
/// grant root meets a later write spelled in another case, `/tmp` meets `/private/tmp`, and the
/// on-disk case meets the spelled one; compared byte for byte, none of them do. Linux checks
/// both answers; macOS has only the APFS one.
#[test]
fn every_path_question_folds_case_and_the_private_alias_as_the_volume_does() {
    let scratch = Scratch::new("volume-rule");
    let out = scratch.root.join("Out");
    std::fs::create_dir_all(out.join("x")).unwrap();
    let spelled = scratch.root.join("out");
    let base = scratch.policy();
    let one = status(1);
    let rules: &[VolumeRule] = if cfg!(target_os = "macos") {
        &[VolumeRule::Apfs]
    } else {
        &[VolumeRule::Exact, VolumeRule::Apfs]
    };
    for &rule in rules {
        let folds = rule == VolumeRule::Apfs;
        with_volume_rule(rule, || {
            // The card proposes the missing tree's top below a base; the next write spells it
            // in lower case
            let missing = out.join("NewDir").join("sub").join("file.txt");
            let bases = [out.clone()];
            let argv = touch_script(&missing);
            let stderr = touch_denied(&missing);
            let proposed = decode(DecodeInput {
                argv: &argv,
                bounds: ProposalBounds {
                    workspace_root: &scratch.ws,
                    user_home: None,
                    extra_bases: &bases,
                },
                ..input(&scratch, &base, one, &stderr)
            })
            .and_then(|violation| violation.proposed);
            let Some(GrantSubject::FsWriteRoot { root }) = proposed else {
                panic!("a write root is proposed: {proposed:?}");
            };
            assert_eq!(out.join("NewDir"), root);
            let granted_missing = base.clone().with_grant(&write_grant(root)).unwrap();
            let granted_on_disk = base.clone().with_grant(&write_grant(out.clone())).unwrap();
            let aliased_root = SandboxPolicy {
                write_roots: vec![PathBuf::from("/private/tmp/ws-fixture")],
                ..base.clone()
            };
            let read_denied = SandboxPolicy {
                read: ReadPolicy::AllExcept {
                    deny: vec![
                        DenyEntry::Path(scratch.root.join("Secret")),
                        DenyEntry::Glob {
                            root: PathBuf::from("/private/tmp/ws-fixture"),
                            tail: "**/*.pem".to_owned(),
                        },
                    ],
                },
                ..base.clone()
            };
            let glob_floor = SandboxPolicy {
                protected: vec![Protected::Glob {
                    glob: "/tmp/ws-fixture/.git/modules/**/hooks".to_owned(),
                }],
                ..base.clone()
            };
            let aliased_cwd = OwnTargets::of(
                Path::new("/tmp/ws-fixture/proj"),
                [OsString::from("touch /tmp/ws-fixture/named/new.txt")]
                    .iter()
                    .map(OsString::as_os_str),
                Path::new("/opt/ws-fixture/served"),
                &[],
                None,
            );
            let on_disk_argv = touch_script(&spelled.join("x").join("new.txt"));
            let on_disk_stderr = touch_denied(&out.join("x").join("new.txt"));
            let attributed_on_disk = decode(DecodeInput {
                argv: &on_disk_argv,
                ..input(&scratch, &base, one, &on_disk_stderr)
            })
            .is_some_and(|violation| violation.disposition == Disposition::Grantable);
            let spelled_stderr = touch_denied(&spelled.join("notes.txt"));
            let refused_spelled =
                refused_under_grant(input(&scratch, &granted_on_disk, one, &spelled_stderr))
                    .is_some();

            let rows = [
                (
                    "a write below a missing proposed root, spelled in another case",
                    granted_missing.would_allow(&Blocked::FsWrite {
                        path: spelled.join("newdir").join("sub").join("file.txt"),
                    }),
                ),
                (
                    "a /tmp write below a /private/tmp root",
                    aliased_root.would_allow(&Blocked::FsWrite {
                        path: PathBuf::from("/tmp/ws-fixture/x"),
                    }),
                ),
                (
                    "a denial quoting /private/tmp below a cwd spelled /tmp",
                    aliased_cwd.covers(Path::new("/private/tmp/ws-fixture/proj/out.txt")),
                ),
                (
                    "a denial quoting /private/tmp on a path the argv spelled /tmp",
                    aliased_cwd.covers(Path::new("/private/tmp/ws-fixture/named")),
                ),
                (
                    "a denial on the on-disk case of a path the argv spelled in another",
                    attributed_on_disk,
                ),
                (
                    "a replayed denial spelled in another case than the granted root",
                    refused_spelled,
                ),
                (
                    "a read below a deny spelled in another case",
                    !read_denied.would_allow(&Blocked::FsRead {
                        path: scratch.root.join("secret").join("key"),
                    }),
                ),
                (
                    "a /tmp read a /private/tmp glob deny matches in another case",
                    !read_denied.would_allow(&Blocked::FsRead {
                        path: PathBuf::from("/tmp/ws-fixture/certs/Server.PEM"),
                    }),
                ),
                (
                    "a floor path spelled in another case",
                    base.is_protected(&scratch.ws.join(".GIT").join("Hooks").join("pre-commit")),
                ),
                (
                    "a floor glob written /tmp, reached through /private/tmp in another case",
                    glob_floor.is_protected(Path::new(
                        "/private/tmp/ws-fixture/.GIT/modules/x/hooks/pre-commit",
                    )),
                ),
            ];
            let wrong: Vec<&str> = rows
                .iter()
                .filter(|(_, answer)| *answer != folds)
                .map(|(row, _)| *row)
                .collect();
            assert!(wrong.is_empty(), "under {rule:?}: {wrong:#?}");
        });
    }
}

#[test]
fn protected_target_is_reported_but_never_proposed() {
    let scratch = Scratch::new("protected");
    let policy = scratch.policy();
    let hook = scratch.ws.join(".git").join("hooks").join("pre-commit");
    let stderr = format!(
        "bash: line 1: {}: Operation not permitted\n",
        hook.display()
    );
    let one = status(1);
    let violation = decode(input(&scratch, &policy, one, &stderr)).unwrap();
    assert!(violation.protected_target());
    assert_eq!(
        Disposition::informational(InformationalReason::ProtectedTarget),
        violation.disposition
    );
    assert_eq!(None, violation.proposed);
    assert!(!violation.is_grantable());
    assert!(violation.produces_card());
    assert_eq!(
        format!(
            "sandbox denied: write to {} (protected). User kept it blocked. Exit 1. stderr: {}",
            hook.display(),
            stderr.trim()
        ),
        violation.kept_blocked_text()
    );
}

/// A printed denial plus `exit 1` names a
/// directory the command never reached for — the card must not offer it. The same line for a
/// path the command line names is grantable.
/// `cargo build` names no cache in its argv, yet a write into a curated
/// build-cache tree is grantable as the family — the daemon's table is the attribution — while
/// the same argv writing anywhere else it never named stays `unattributed`; a write into a
/// verified cache is allowed by the policy and so is no violation at all.
#[test]
fn a_build_cache_write_is_grantable_as_the_family_without_argv_attribution() {
    let scratch = Scratch::new("build-caches");
    let home = scratch.root.join("home");
    let src = home.join(".cargo/registry/src/index.crates.io-1949cf8c6b5b557f");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(home.join(".cargo/registry/cache")).unwrap();
    let mut policy = scratch.policy();
    policy.build_cache_trees = vec![home.join(".cargo/registry"), home.join(".npm")];
    policy.write_roots.push(home.join(".cargo/registry/cache"));
    let argv = script("cargo build");
    let one = status(1);
    let unpacked = src.join("serde-1.0.0/build.rs");
    let stderr = touch_denied(&unpacked);
    let violation = decode(DecodeInput {
        argv: &argv,
        ..input(&scratch, &policy, one, &stderr)
    })
    .unwrap();
    assert_eq!(Blocked::FsWrite { path: unpacked }, violation.blocked);
    assert_eq!(Some(GrantSubject::BuildCaches), violation.proposed);
    assert_eq!(Disposition::Grantable, violation.disposition);
    assert!(violation.is_grantable());
    // The same command, a path outside every tree stays unattributed
    let elsewhere = scratch.root.join("outside/dir/x");
    let stderr = touch_denied(&elsewhere);
    let violation = decode(DecodeInput {
        argv: &argv,
        ..input(&scratch, &policy, one, &stderr)
    })
    .unwrap();
    assert_eq!(
        Disposition::informational(InformationalReason::Unattributed),
        violation.disposition
    );
    // A verified cache is a default root: the policy allows it, so the OS refused, not the sandbox
    let verified = home.join(".cargo/registry/cache/serde-1.0.0.crate");
    let stderr = touch_denied(&verified);
    assert_eq!(
        None,
        decode(DecodeInput {
            argv: &argv,
            ..input(&scratch, &policy, one, &stderr)
        })
    );
    // Under the grant the same write is allowed and decodes to nothing
    let granted = policy
        .with_grant(&Grant {
            id: GrantId::new("0192c1a0-0000-7000-8000-0000000000bc"),
            subject: GrantSubject::BuildCaches,
            scope: GrantScope::Workspace {
                root: scratch.ws.clone(),
            },
            expires: Expiry::Ttl {
                seconds: 7 * 86_400,
            },
            decision: GrantDecision::Allow,
            granted_at: 0,
            granted_by: "desktop:test".to_owned(),
            via: None,
        })
        .unwrap();
    let stderr = touch_denied(&src.join("serde-1.0.0/build.rs"));
    assert_eq!(
        None,
        decode(DecodeInput {
            argv: &argv,
            ..input(&scratch, &granted, one, &stderr)
        })
    );
}

#[test]
fn a_grantable_path_the_command_never_named_is_unattributed() {
    let scratch = Scratch::new("unattributed");
    let policy = scratch.policy();
    let agents = scratch
        .root
        .join("outside")
        .join("dir")
        .join("LaunchAgents");
    std::fs::create_dir_all(&agents).unwrap();
    let plist = agents.join("com.evil.plist");
    let stderr = format!("bash: {}: Operation not permitted\n", plist.display());
    let one = status(1);
    // `cargo test` printed that line (a fabricated denial): nothing in its argv names the path
    let argv = script("cargo test -p xai-grok-sandbox");
    let violation = decode(DecodeInput {
        argv: &argv,
        ..input(&scratch, &policy, one, &stderr)
    })
    .unwrap();
    assert_eq!(
        Blocked::FsWrite {
            path: plist.clone()
        },
        violation.blocked
    );
    assert_eq!(None, violation.proposed);
    assert_eq!(
        Disposition::informational(InformationalReason::Unattributed),
        violation.disposition
    );
    assert!(!violation.is_grantable());
    assert!(violation.produces_card(), "the user still sees the denial");
    assert!(
        violation.kept_blocked_text().starts_with(&format!(
            "sandbox denied: write to {} (not a path this command named).",
            plist.display()
        )),
        "{}",
        violation.kept_blocked_text()
    );
    // The command that names it is offered the directory
    let argv = script(&format!("cp agent.plist {}", plist.display()));
    let violation = decode(DecodeInput {
        argv: &argv,
        ..input(&scratch, &policy, one, &stderr)
    })
    .unwrap();
    assert_eq!(
        Some(GrantSubject::FsWriteRoot { root: agents }),
        violation.proposed
    );
    assert_eq!(Disposition::Grantable, violation.disposition);
    // A denial under the cwd is the command's own without any argv (a relative path it wrote)
    let inside = scratch.ws.join("secret");
    let mut unread = scratch.policy();
    unread.read = ReadPolicy::Roots {
        roots: Vec::new(),
        deny: Vec::new(),
    };
    let stderr = format!("cat: {}: Operation not permitted\n", inside.display());
    let violation = decode(input(&scratch, &unread, one, &stderr)).unwrap();
    assert_eq!(
        Blocked::FsRead {
            path: inside.clone()
        },
        violation.blocked
    );
    assert_eq!(Disposition::Grantable, violation.disposition);
    // Under a profile `deny` it is final, like the floor: the card informs and offers nothing
    let mut read_denied = scratch.policy();
    read_denied.read = ReadPolicy::AllExcept {
        deny: vec![DenyEntry::Path(inside.clone())],
    };
    let violation = decode(input(&scratch, &read_denied, one, &stderr)).unwrap();
    assert_eq!(Blocked::FsRead { path: inside }, violation.blocked);
    assert_eq!(None, violation.proposed);
    assert_eq!(
        Disposition::informational(InformationalReason::ProfileDeny),
        violation.disposition
    );
    assert!(
        violation
            .kept_blocked_text()
            .contains("(denied by your sandbox.toml deny)"),
        "{}",
        violation.kept_blocked_text()
    );
}

/// A `read_only` root that is a symlink allows reads at its own spelling only: Seatbelt meets the
/// target's path, which no root allows, so the decode keeps the denial rather than dropping it.
#[cfg(unix)]
#[test]
fn a_symlinked_read_root_never_allows_its_target() {
    let scratch = Scratch::new("read-link");
    let target = scratch.root.join("outside").join("dir");
    let link = scratch.ws.join("docs");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let mut policy = scratch.policy();
    policy.read = ReadPolicy::Roots {
        roots: vec![link.clone(), scratch.ws.clone()],
        deny: Vec::new(),
    };
    for (path, allowed) in [
        (target.join("a.md"), false),
        (link.join("a.md"), false),
        (scratch.ws.join("src/lib.rs"), true),
    ] {
        let blocked = Blocked::FsRead { path: path.clone() };
        assert_eq!(allowed, policy.would_allow(&blocked), "{path:?}");
    }
    let stderr = format!(
        "cat: {}: Operation not permitted\n",
        target.join("a.md").display()
    );
    let violation = decode(input(&scratch, &policy, status(1), &stderr)).expect("kept");
    assert_eq!(
        Blocked::FsRead {
            path: target.join("a.md")
        },
        violation.blocked
    );
}

/// A protected *file* under a writable parent (`<ws>/.git/config` beside a writable `.git`) and
/// the first `mkdir <ws>/.grok` are the floor holding the line, not the OS: the user sees a
/// protected card and the model is told.
#[test]
fn a_protected_target_under_a_writable_parent_is_the_floor_not_the_os() {
    let scratch = Scratch::new("protected-under-writable");
    let mut policy = scratch.policy();
    let config = scratch.ws.join(".git").join("config");
    let grok = scratch.ws.join(".grok");
    std::fs::write(&config, "[core]\n").unwrap();
    policy.protected.push(Protected::Path {
        path: config.clone(),
    });
    policy
        .protected
        .push(Protected::Path { path: grok.clone() });
    assert!(
        policy.is_protected(&config)
            && !policy.would_allow(&Blocked::FsWrite {
                path: config.clone()
            })
    );
    let one = status(1);
    let stderr = format!(
        "bash: line 1: {}: Operation not permitted\n",
        config.display()
    );
    let violation = decode(input(&scratch, &policy, one, &stderr)).unwrap();
    assert_eq!(Blocked::FsWrite { path: config }, violation.blocked);
    assert!(violation.protected_target());
    assert_eq!(None, violation.proposed);
    assert!(violation.produces_card());
    // `.grok` does not exist yet: its nearest existing directory is the writable workspace
    let stderr = format!("mkdir: {}: Operation not permitted\n", grok.display());
    let violation = decode(input(&scratch, &policy, one, &stderr)).unwrap();
    assert_eq!(Blocked::FsWrite { path: grok }, violation.blocked);
    assert!(violation.protected_target());
    // An unprotected file beside them stays the OS's refusal
    let busy = scratch.ws.join(".git").join("index.lock");
    let stderr = format!(
        "bash: line 1: {}: Operation not permitted\n",
        busy.display()
    );
    assert_eq!(None, decode(input(&scratch, &policy, one, &stderr)));
}

/// `mv` is refused on the node it moves: a staged tree moved in as `.git`, or the git directory
/// moved away with its protected entries. Either way the card names `.git`, and it is the
/// floor's; a rename between two ordinary workspace paths stays the OS's refusal.
#[test]
fn a_rename_into_or_out_of_the_git_node_is_the_floor() {
    let scratch = Scratch::new("git-node-rename");
    let policy = scratch.policy();
    let git = scratch.ws.join(".git");
    let one = status(1);
    for stderr in [
        "mv: rename stage to .git: Operation not permitted\n".to_owned(),
        "mv: rename .git to old: Operation not permitted\n".to_owned(),
        format!(
            "mv: rename {} to {}: Operation not permitted\n",
            scratch.ws.join("stage").display(),
            git.display()
        ),
    ] {
        let violation = decode(input(&scratch, &policy, one, &stderr)).unwrap();
        assert_eq!(
            Blocked::FsWrite { path: git.clone() },
            violation.blocked,
            "{stderr}"
        );
        assert!(violation.protected_target(), "{stderr}");
        assert_eq!(None, violation.proposed, "{stderr}");
    }
    assert_eq!(
        None,
        decode(input(
            &scratch,
            &policy,
            one,
            "mv: rename a to b: Operation not permitted\n"
        ))
    );
}

/// A staged tree moved in as a missing ancestor of a floor entry (`tools` for a `tools/git/hooks`
/// hooks path) is refused on that node: the card names it, and it is the floor's.
#[test]
fn a_rename_into_a_missing_ancestor_of_the_floor_is_the_floor() {
    let scratch = Scratch::new("missing-ancestor-rename");
    let mut policy = scratch.policy();
    policy.protected.push(Protected::Path {
        path: scratch.ws.join("tools/git/hooks"),
    });
    let stderr = "mv: rename stage to tools: Operation not permitted\n";
    let violation = decode(input(&scratch, &policy, status(1), stderr)).unwrap();
    assert_eq!(
        Blocked::FsWrite {
            path: scratch.ws.join("tools")
        },
        violation.blocked
    );
    assert!(violation.protected_target());
    assert_eq!(None, violation.proposed);
}

/// A write whose nearest existing ancestor the policy already allows is the OS refusing (a mode
/// bit, a busy file), not the sandbox.
#[test]
fn a_denial_below_a_writable_root_is_the_os_not_the_sandbox() {
    let scratch = Scratch::new("os-denial");
    let policy = scratch.policy();
    let target = scratch.ws.join("missing").join("deeper").join("x");
    let one = status(1);
    assert_eq!(
        None,
        decode(input(&scratch, &policy, one, &touch_denied(&target)))
    );
}

#[test]
fn a_refused_hold_has_no_exit_to_report() {
    // The decider raises this mid-command: the command is still running when the owner refuses,
    // so the text says what the proxy did, not "Exit signal"
    let held = Violation {
        blocked: Blocked::Net {
            host: Some("example.com".to_owned()),
            port: Some(443),
        },
        proposed: Some(GrantSubject::NetHost {
            host: HostPattern::new("example.com"),
            port: None,
        }),
        disposition: Disposition::Grantable,
        partial_output: None,
        replay: Replay::Resume {
            hold_id: "hold-1".to_owned(),
        },
        exit_code: None,
        stderr_snippet: String::new(),
    };
    assert_eq!(
        "sandbox denied: connection to example.com:443 (not allowed). User kept it blocked. The proxy refused the connection (403) and the command went on.",
        held.kept_blocked_text()
    );
    assert_eq!(
        "sandbox denied: connection to example.com:443 (not allowed). Denied by policy. The proxy refused the connection (403) and the command went on.",
        held.denied_text("Denied by policy.")
    );
}

#[test]
fn coarse_unknown_never_cards() {
    let scratch = Scratch::new("unknown");
    let policy = scratch.policy();
    let stderr = "Error from server (Forbidden): pods is forbidden: permission denied\n";
    let one = status(1);
    let violation = decode(input(&scratch, &policy, one, stderr)).unwrap();
    assert_eq!(
        Blocked::Unknown {
            stderr_snippet: stderr.trim().to_owned(),
        },
        violation.blocked
    );
    assert!(!violation.produces_card());
    assert!(!violation.is_grantable());
    assert_eq!(
        format!(
            "sandbox denied: an operation the sandbox refused (target unknown). User kept it blocked. Exit 1. stderr: {}",
            stderr.trim()
        ),
        violation.kept_blocked_text()
    );
    // A remote refusing a key is not a denial at all
    let stderr = "git@github.com: Permission denied (publickey).\r\nfatal: Could not read from remote repository.\n";
    let git = status(128);
    assert_eq!(None, decode(input(&scratch, &policy, git, stderr)));
}

/// Python runs on without the byte-code it could not write, so a `__pycache__` denial alone is
/// no violation and no card; beside a real denial the card names the real target, and beside a
/// denial that names no path the decode is still `Unknown`.
#[test]
fn a_pycache_denial_never_becomes_the_card() {
    let scratch = Scratch::new("pycache");
    let policy = scratch.policy();
    let dir = scratch.root.join("outside").join("dir");
    let pyc = dir.join("__pycache__").join("tool.cpython-312.pyc");
    let noise = format!(
        "PermissionError: [Errno 1] Operation not permitted: '{}'\n",
        pyc.display()
    );
    assert_eq!(None, decode(input(&scratch, &policy, status(1), &noise)));

    let target = dir.join("out.txt");
    let stderr = format!("{noise}{}", touch_denied(&target));
    let argv = touch_script(&target);
    let violation = decode(DecodeInput {
        argv: &argv,
        ..input(&scratch, &policy, status(1), &stderr)
    })
    .unwrap();
    assert_eq!(Blocked::FsWrite { path: target }, violation.blocked);
    assert!(violation.produces_card());

    let vague = format!("{noise}rm: Permission denied\n");
    let violation = decode(input(&scratch, &policy, status(1), &vague)).unwrap();
    assert!(
        matches!(violation.blocked, Blocked::Unknown { .. }),
        "{violation:?}"
    );
    assert!(!violation.produces_card());
}

#[test]
fn merged_output_is_read_as_stderr_and_partial_output_is_the_callers() {
    let scratch = Scratch::new("merged");
    let policy = scratch.policy();
    let target = scratch.root.join("outside").join("dir").join("x");
    let output = format!(
        "building...\nstep 2 of 3\ntouch: cannot touch '{}': Operation not permitted\n",
        target.display()
    );
    let one = status(1);
    let violation = decode(input(&scratch, &policy, one, &output)).unwrap();
    assert_eq!(Blocked::FsWrite { path: target }, violation.blocked);
    assert_eq!(None, violation.partial_output);
    assert!(violation.stderr_snippet.starts_with("building..."));
    let json = serde_json::to_value(&violation).unwrap();
    assert!(
        json.get("partial_output").is_none(),
        "an unknown partial_output is omitted from the wire"
    );
}

#[test]
fn a_coarse_resolve_failure_decodes_with_no_port() {
    let scratch = Scratch::new("netresolve");
    let policy = scratch.policy();
    let six = status(6);
    let stderr = "curl: (6) Could not resolve host: example.com\n";
    let violation = decode(input(&scratch, &policy, six, stderr)).unwrap();
    assert_eq!(
        Blocked::Net {
            host: Some("example.com".to_owned()),
            port: None,
        },
        violation.blocked
    );
    // The proxy never saw this connection (it holds and asks about the ones it sees), so no
    // host grant can help: informational, nothing proposed
    assert_eq!(None, violation.proposed);
    assert_eq!(
        Disposition::informational(InformationalReason::UnproxiedNetwork),
        violation.disposition
    );
    assert!(!violation.is_grantable());
    assert!(violation.produces_card());
    assert_eq!(Replay::Rerun, violation.replay);
    assert_eq!(
        "sandbox denied: connection to example.com (not through the proxy). User kept it blocked. Exit 6. stderr: curl: (6) Could not resolve host: example.com",
        violation.kept_blocked_text()
    );
}

/// A connection the policy's proxy would have carried is a grant question for the proxy, not a
/// post-run violation; one the proxy never saw is informational.
#[test]
fn a_host_the_proxy_policy_allows_is_not_a_violation_and_a_bypassed_one_is_informational() {
    let scratch = Scratch::new("net-proxy");
    let mut policy = scratch.policy();
    policy.network = NetworkPolicy::Proxy { port: 8123 };
    let seven = status(7);
    // The proxy itself refusing is never a violation
    let stderr = "curl: (7) Failed to connect to 127.0.0.1 port 8123 after 0 ms: Couldn't connect to server\n";
    assert_eq!(None, decode(input(&scratch, &policy, seven, stderr)));
    // With the network off the kernel refused the connect: the card names the host and offers
    // nothing
    policy.network = NetworkPolicy::Off;
    let stderr =
        "curl: (7) Failed to connect to pypi.org port 443 after 3 ms: Couldn't connect to server\n";
    let violation = decode(input(&scratch, &policy, seven, stderr)).unwrap();
    assert_eq!(
        Blocked::Net {
            host: Some("pypi.org".to_owned()),
            port: Some(443),
        },
        violation.blocked
    );
    assert_eq!(None, violation.proposed);
    assert_eq!(
        Disposition::informational(InformationalReason::UnproxiedNetwork),
        violation.disposition
    );
    assert!(!violation.is_grantable());
}

#[test]
fn sandbox_exec_execvp_refusal_is_a_setuid_capability() {
    let scratch = Scratch::new("setuid");
    let policy = scratch.policy();
    let seventy_one = status(71);
    let stderr = "sandbox-exec: execvp() of '/usr/bin/sudo' failed: Operation not permitted\n";
    let violation = decode(input(&scratch, &policy, seventy_one, stderr)).unwrap();
    assert_eq!(
        Blocked::Capability {
            what: Capability::SetUid
        },
        violation.blocked
    );
    assert_eq!(None, violation.proposed);
    assert!(!violation.is_grantable());
    assert!(violation.produces_card());
    assert_eq!(Some(71), violation.exit_code);
    assert!(
        violation
            .kept_blocked_text()
            .starts_with("sandbox denied: capability set_uid (never allowed)."),
        "{}",
        violation.kept_blocked_text()
    );
}

#[test]
fn ssh_connect_refused_names_the_host_after_the_word_host() {
    let scratch = Scratch::new("ssh");
    let policy = scratch.policy();
    let status_255 = status(255);
    let stderr = "ssh: connect to host github.com port 22: Connection refused\n";
    let violation = decode(input(&scratch, &policy, status_255, stderr)).unwrap();
    assert_eq!(
        Blocked::Net {
            host: Some("github.com".to_owned()),
            port: Some(22),
        },
        violation.blocked
    );
}

#[test]
fn a_missing_tree_is_proposed_at_its_top_and_never_names_a_workspace_ancestor() {
    // The macOS pip example: `$PYTHONUSERBASE` beside the workspace, none of its parents exist, and
    // the nearest existing directory is the workspace's parent
    let scratch = Scratch::new("cap-pip");
    let policy = scratch.policy();
    let pyuser = scratch.root.join("pyuser");
    let target = pyuser
        .join("lib")
        .join("python3.12")
        .join("site-packages")
        .join("requests")
        .join("__init__.py");
    let stderr = format!(
        "PermissionError: [Errno 1] Operation not permitted: '{}'\n",
        target.display()
    );
    // pip names nothing on its command line; `$PYTHONUSERBASE` is how the call names the tree
    let argv = script("pip3 install --user requests");
    let bases = [pyuser.clone()];
    let one = status(1);
    let violation = decode(DecodeInput {
        argv: &argv,
        bounds: ProposalBounds {
            workspace_root: &scratch.ws,
            user_home: None,
            extra_bases: &bases,
        },
        ..input(&scratch, &policy, one, &stderr)
    })
    .unwrap();
    assert_eq!(
        Some(GrantSubject::FsWriteRoot { root: pyuser }),
        violation.proposed,
        "the highest missing ancestor — the tree pip is about to create — never the workspace's \
         parent"
    );
    assert_eq!(Blocked::FsWrite { path: target }, violation.blocked);
}

#[test]
fn the_home_directory_is_never_proposed_and_a_home_base_is_never_offered_whole() {
    let scratch = Scratch::new("cap-home");
    let policy = scratch.policy();
    let home = scratch.root.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let bounds = ProposalBounds {
        workspace_root: &scratch.ws,
        user_home: Some(&home),
        extra_bases: &[],
    };
    // `touch ~/x`: the only ancestor is the home directory itself, never offered; the one file
    // is. The model typed `~/x`; the home expands it
    let target = home.join("x");
    let stderr = touch_denied(&target);
    let argv = script("touch ~/x");
    let one = status(1);
    let violation = decode(DecodeInput {
        bounds,
        argv: &argv,
        ..input(&scratch, &policy, one, &stderr)
    })
    .unwrap();
    assert_eq!(
        Some(GrantSubject::FsWriteRoot {
            root: target.clone()
        }),
        violation.proposed
    );
    assert_eq!(Disposition::Grantable, violation.disposition);
    assert!(violation.is_grantable());

    // `~/Library/Python/3.14/lib/python/site-packages/pkg/mod.py` with `~/Library` present:
    // `~/Library` is a base the card never offers whole, so the first missing directory under it
    // is proposed
    let library = home.join("Library");
    std::fs::create_dir_all(&library).unwrap();
    let target = library
        .join("Python")
        .join("3.14")
        .join("lib")
        .join("python")
        .join("site-packages")
        .join("pkg")
        .join("mod.py");
    let stderr = touch_denied(&target);
    let argv = touch_script(&target);
    let violation = decode(DecodeInput {
        bounds,
        argv: &argv,
        ..input(&scratch, &policy, one, &stderr)
    })
    .unwrap();
    assert_eq!(
        Some(GrantSubject::FsWriteRoot {
            root: library.join("Python")
        }),
        violation.proposed
    );
    assert_eq!(Blocked::FsWrite { path: target }, violation.blocked);

    // `rm -rf ~/`: the home directory itself is the target — too broad to be the "one file"
    // fallback, so nothing is offered
    let stderr = format!("rm: {}: Operation not permitted\n", home.display());
    let argv = script("rm -rf ~/");
    let violation = decode(DecodeInput {
        bounds,
        argv: &argv,
        ..input(&scratch, &policy, one, &stderr)
    })
    .unwrap();
    assert_eq!(None, violation.proposed);
    assert_eq!(
        Disposition::informational(InformationalReason::ProtectedTarget),
        violation.disposition
    );
}

#[test]
fn a_top_level_system_directory_is_never_proposed() {
    let scratch = Scratch::new("cap-sys");
    let policy = scratch.policy();
    let one = status(1);
    // `/opt/newtool/bin/x`: `/opt` exists and is never proposed; `/opt/newtool` has too few
    // components, so `/opt/newtool/bin` (missing, specific enough) is the proposal
    let target = Path::new("/opt/grok-sandbox-test-newtool/bin/x");
    let argv = touch_script(target);
    let stderr = touch_denied(target);
    let violation = decode(DecodeInput {
        argv: &argv,
        ..input(&scratch, &policy, one, &stderr)
    })
    .unwrap();
    assert_eq!(
        Some(GrantSubject::FsWriteRoot {
            root: PathBuf::from("/opt/grok-sandbox-test-newtool/bin")
        }),
        violation.proposed
    );
    // `/etc/hosts`: `/etc` is never proposed, so the one file is
    let stderr = "bash: /etc/hosts: Operation not permitted\n";
    let argv = script("echo '127.0.0.1 x' >> /etc/hosts");
    let violation = decode(DecodeInput {
        argv: &argv,
        ..input(&scratch, &policy, one, stderr)
    })
    .unwrap();
    assert_eq!(
        Some(GrantSubject::FsWriteRoot {
            root: PathBuf::from("/etc/hosts")
        }),
        violation.proposed
    );
    assert_eq!(
        Blocked::FsWrite {
            path: PathBuf::from("/etc/hosts")
        },
        violation.blocked
    );
}

#[test]
fn violation_round_trips_through_json() {
    let violation = Violation {
        blocked: Blocked::FsWrite {
            path: PathBuf::from("/x/y"),
        },
        proposed: Some(GrantSubject::FsWriteRoot {
            root: PathBuf::from("/x"),
        }),
        disposition: Disposition::Grantable,
        partial_output: Some(true),
        replay: Replay::Rerun,
        exit_code: Some(1),
        stderr_snippet: "denied".to_owned(),
    };
    let json = serde_json::to_value(&violation).unwrap();
    assert_eq!(
        serde_json::json!({
            "blocked": {"kind": "fs_write", "path": "/x/y"},
            "proposed": {"kind": "fs_write_root", "root": "/x"},
            "disposition": {"kind": "grantable"},
            "partial_output": true,
            "replay": {"kind": "rerun"},
            "exit_code": 1,
            "stderr_snippet": "denied"
        }),
        json
    );
    assert_eq!(
        violation,
        serde_json::from_value::<Violation>(json).unwrap()
    );
    // A `Resume` replay (the proxy's) and a masked host round-trip too
    let held = Violation {
        blocked: Blocked::Net {
            host: None,
            port: Some(443),
        },
        proposed: Some(GrantSubject::NetHost {
            host: HostPattern::all(),
            port: Some(443),
        }),
        disposition: Disposition::Grantable,
        partial_output: None,
        replay: Replay::Resume {
            hold_id: "h-1".to_owned(),
        },
        exit_code: None,
        stderr_snippet: String::new(),
    };
    let json = serde_json::to_value(&held).unwrap();
    assert_eq!(
        Some(&serde_json::json!({"kind": "net", "port": 443})),
        json.get("blocked")
    );
    assert_eq!(held, serde_json::from_value::<Violation>(json).unwrap());
}
