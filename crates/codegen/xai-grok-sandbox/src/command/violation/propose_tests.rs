use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::command::grants::GrantSubject;
use crate::command::policy::{DenyEntry, EnvPolicy, NetworkPolicy, ReadPolicy, SandboxPolicy};
use crate::command::protected::Protected;
use crate::command::violation::{Blocked, Disposition, InformationalReason};

use super::{ProposalBounds, bases_from_env, is_too_broad, propose};
use crate::command::violation::proposed_subject;

struct Scratch {
    root: PathBuf,
    ws: PathBuf,
    home: PathBuf,
}

impl Scratch {
    fn new(tag: &str) -> Scratch {
        let root =
            std::env::temp_dir().join(format!("grok-sandbox-propose-{tag}-{}", std::process::id()));
        let ws = root.join("projects").join("ws");
        let home = root.join("home");
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::create_dir_all(home.join(".local")).unwrap();
        Scratch { root, ws, home }
    }

    fn policy(&self) -> SandboxPolicy {
        SandboxPolicy {
            read: ReadPolicy::AllExcept { deny: Vec::new() },
            write_roots: vec![self.ws.clone()],
            network: NetworkPolicy::Off,
            env: EnvPolicy::default(),
            protected: vec![Protected::Path {
                path: self.home.join(".local").join("bin"),
            }],
            build_cache_trees: vec![self.home.join(".cargo/registry"), self.home.join(".npm")],
            unread_git_metadata: Vec::new(),
        }
    }

    fn bounds<'a>(&'a self, extra: &'a [PathBuf]) -> ProposalBounds<'a> {
        ProposalBounds {
            workspace_root: &self.ws,
            user_home: Some(&self.home),
            extra_bases: extra,
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn write(path: PathBuf) -> Blocked {
    Blocked::FsWrite { path }
}

fn root_of(subject: Option<GrantSubject>) -> PathBuf {
    match subject {
        Some(GrantSubject::FsWriteRoot { root } | GrantSubject::FsRead { root }) => root,
        other => panic!("expected a directory proposal, got {other:?}"),
    }
}

/// A folder a floor glob can match beneath (`.git/modules/x`, which holds that submodule's
/// `hooks`) is never offered: the card informs instead of proposing a grant into the pattern.
#[test]
fn a_folder_beneath_a_floor_globs_literal_prefix_is_never_proposed() {
    let s = Scratch::new("glob-prefix");
    let modules = s.ws.join(".git").join("modules");
    std::fs::create_dir_all(modules.join("x").join("hooks")).unwrap();
    let mut policy = s.policy();
    policy.protected.push(Protected::Glob {
        glob: format!("{}/**/hooks", modules.display()),
    });
    let proposal = propose(
        &write(modules.join("x").join("packed-refs")),
        &policy,
        &s.bounds(&[]),
    );
    assert_eq!(None, proposal.subject);
    assert_eq!(
        Disposition::informational(InformationalReason::ProtectedTarget),
        proposal.disposition
    );
}

/// A folder spelled through a symlink is offered nowhere, whether it leads to `/`, the home, a
/// workspace ancestor or somewhere specific: the store refuses a linked spelling, and the card
/// would show a folder other than the one granted.
#[cfg(unix)]
#[test]
fn a_folder_spelled_through_a_symlink_is_never_proposed() {
    let s = Scratch::new("resolves-broad");
    let policy = s.policy();
    let parent = s.ws.parent().unwrap().to_path_buf();
    let specific = s.root.join("elsewhere/deep/dir");
    std::fs::create_dir_all(&specific).unwrap();
    for (name, target) in [
        ("to-root", PathBuf::from("/")),
        ("to-home", s.home.clone()),
        ("to-parent", parent),
        ("to-specific", specific),
    ] {
        let link = s.ws.join(name);
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let proposal = propose(&write(link.join("x")), &policy, &s.bounds(&[]));
        assert_eq!(None, proposal.subject, "{name}");
        assert_eq!(
            Disposition::informational(InformationalReason::ProtectedTarget),
            proposal.disposition,
            "{name}"
        );
    }
}

/// No grant drops a read deny, so a read or write it covers proposes nothing in either read mode:
/// a grok-home auth file (inside the floor's grok-home tree) is a protected target, a path under
/// a profile `deny` is denied by the profile. A folder holding a deny is never offered: the
/// proposal narrows toward the target, and informs when even the target holds one.
#[test]
fn a_path_no_grant_can_open_is_informational_in_both_read_modes() {
    let s = Scratch::new("read-floor");
    let grok_home = s.home.join(".grok");
    let deep = s.root.join("outside").join("deep");
    let denied = deep.join("dir");
    std::fs::create_dir_all(&grok_home).unwrap();
    std::fs::create_dir_all(&denied).unwrap();
    std::fs::create_dir_all(deep.join("sibling")).unwrap();
    let deny = vec![
        DenyEntry::Glob {
            root: grok_home.clone(),
            tail: "auth*".to_owned(),
        },
        DenyEntry::Path(denied.clone()),
    ];
    for read in [
        ReadPolicy::AllExcept { deny: deny.clone() },
        ReadPolicy::Roots {
            roots: vec![s.ws.clone()],
            deny,
        },
    ] {
        let mut policy = s.policy();
        policy.read = read;
        policy.protected.push(Protected::TreeExcept {
            tree: grok_home.clone(),
            except: grok_home.join("sessions/ws/commands"),
        });
        let propose_read =
            |path: PathBuf| propose(&Blocked::FsRead { path }, &policy, &s.bounds(&[]));
        let auth = propose_read(grok_home.join("auth.json"));
        assert_eq!(None, auth.subject, "{:?}", policy.read);
        assert_eq!(
            Disposition::informational(InformationalReason::ProtectedTarget),
            auth.disposition
        );
        let profile_deny = Disposition::informational(InformationalReason::ProfileDeny);
        for blocked in [
            Blocked::FsRead {
                path: denied.join("notes.txt"),
            },
            write(denied.join("notes.txt")),
            write(deep.clone()),
        ] {
            let proposal = propose(&blocked, &policy, &s.bounds(&[]));
            assert_eq!(None, proposal.subject, "{blocked:?}");
            assert_eq!(profile_deny, proposal.disposition, "{blocked:?}");
        }
        let beside = propose(&write(deep.join("new.txt")), &policy, &s.bounds(&[]));
        assert_eq!(deep.join("new.txt"), root_of(beside.subject));
        let nested = propose(&write(deep.join("sibling/x")), &policy, &s.bounds(&[]));
        assert_eq!(deep.join("sibling"), root_of(nested.subject));
    }
}

#[test]
fn a_specific_existing_directory_is_proposed_as_is() {
    let s = Scratch::new("existing");
    let dir = s.root.join("outside").join("deep").join("dir");
    std::fs::create_dir_all(&dir).unwrap();
    let proposal = propose(
        &write(dir.join("new").join("file")),
        &s.policy(),
        &s.bounds(&[]),
    );
    assert_eq!(dir, root_of(proposal.subject));
    assert_eq!(Disposition::Grantable, proposal.disposition);
}

#[test]
fn the_home_directory_is_never_proposed_and_a_fresh_tree_is_offered_at_its_top() {
    let s = Scratch::new("pyuser");
    let pyuser = s.home.join("pyuser");
    let target = pyuser
        .join("lib")
        .join("python3.14")
        .join("site-packages")
        .join("pkg");
    // The climb lands on the home directory, which is never offered: the highest missing
    // ancestor — the `$PYTHONUSERBASE` pip is about to create — is offered once
    let proposal = propose(&write(target.clone()), &s.policy(), &s.bounds(&[]));
    assert_eq!(pyuser, root_of(proposal.subject));
    // A file straight under the home directory offers the one file — never
    // the home directory
    let proposal = propose(
        &write(s.home.join("notes.txt")),
        &s.policy(),
        &s.bounds(&[]),
    );
    assert_eq!(s.home.join("notes.txt"), root_of(proposal.subject));
    assert_eq!(Disposition::Grantable, proposal.disposition);
    // Once the base exists it is still never offered whole: `$PYTHONUSERBASE` names it
    std::fs::create_dir_all(&pyuser).unwrap();
    let proposal = propose(&write(target.clone()), &s.policy(), &s.bounds(&[]));
    assert_eq!(
        pyuser,
        root_of(proposal.subject),
        "unknown to the env: the existing dir"
    );
    let extra = vec![pyuser.clone()];
    let proposal = propose(&write(target), &s.policy(), &s.bounds(&extra));
    assert_eq!(pyuser.join("lib"), root_of(proposal.subject));
}

#[test]
fn below_an_existing_base_the_highest_missing_ancestor_is_proposed() {
    let s = Scratch::new("local");
    let target = s
        .home
        .join(".local")
        .join("lib")
        .join("python3.14")
        .join("site-packages")
        .join("pkg");
    // `~/.local` exists and is specific enough by depth, but is never offered whole
    let proposal = propose(&write(target), &s.policy(), &s.bounds(&[]));
    assert_eq!(s.home.join(".local").join("lib"), root_of(proposal.subject));
    // Same for `~/Library`: a fresh `~/Library/Caches/pnpm` is offered at `Caches`, not `Library`
    std::fs::create_dir_all(s.home.join("Library")).unwrap();
    let proposal = propose(
        &write(s.home.join("Library").join("Caches").join("pnpm").join("x")),
        &s.policy(),
        &s.bounds(&[]),
    );
    assert_eq!(
        s.home.join("Library").join("Caches"),
        root_of(proposal.subject)
    );
    // An existing directory deeper than the base is specific: offered as is
    std::fs::create_dir_all(s.home.join("Library").join("Caches")).unwrap();
    let proposal = propose(
        &write(s.home.join("Library").join("Caches").join("pnpm").join("x")),
        &s.policy(),
        &s.bounds(&[]),
    );
    assert_eq!(
        s.home.join("Library").join("Caches"),
        root_of(proposal.subject)
    );
    // The protected `~/.local/bin` is never offered, whatever the rule would climb to
    let proposal = propose(
        &write(s.home.join(".local").join("bin").join("tool")),
        &s.policy(),
        &s.bounds(&[]),
    );
    assert_eq!(None, proposal.subject);
    assert_eq!(
        Disposition::informational(InformationalReason::ProtectedTarget),
        proposal.disposition
    );
}

#[test]
fn without_a_base_the_highest_missing_ancestor_that_is_specific_enough_is_proposed() {
    let s = Scratch::new("nobase");
    // `<tmp>/<scratch>` has two components: too broad; its missing child is the proposal
    let target = s.root.join("new").join("deeper").join("file");
    let proposal = propose(&write(target), &s.policy(), &s.bounds(&[]));
    assert_eq!(s.root.join("new"), root_of(proposal.subject));
}

#[test]
fn breadth_caps_hold_for_typed_folders_too() {
    let s = Scratch::new("caps");
    let bounds = s.bounds(&[]);
    for dir in ["/", "/Users", "/home", "/usr", "/opt", "/tmp/x"] {
        assert!(is_too_broad(Path::new(dir), &bounds), "{dir}");
    }
    assert!(is_too_broad(&s.home, &bounds));
    assert!(
        is_too_broad(s.ws.parent().unwrap(), &bounds),
        "a workspace ancestor"
    );
    assert!(!is_too_broad(&s.ws, &bounds));
    assert!(!is_too_broad(&s.home.join(".local").join("lib"), &bounds));
}

#[test]
fn env_bases_come_from_pythonuserbase_and_xdg_data_home_only() {
    let vars = [
        ("PYTHONUSERBASE", "/opt/ws-fixture/dev/pyuser"),
        ("XDG_DATA_HOME", "/opt/ws-fixture/dev/.local/share"),
        ("PATH", "/usr/bin"),
        ("HOME", "/opt/ws-fixture/dev"),
    ]
    .map(|(k, v)| (OsString::from(k), OsString::from(v)));
    assert_eq!(
        vec![
            PathBuf::from("/opt/ws-fixture/dev/pyuser"),
            PathBuf::from("/opt/ws-fixture/dev/.local/share")
        ],
        bases_from_env(vars)
    );
    // A relative value is ignored
    let vars = [(OsString::from("PYTHONUSERBASE"), OsString::from("pyuser"))];
    assert!(bases_from_env(vars).is_empty());
}

/// A connection read from a stopped command's output is one the proxy never
/// saw — the tool bypassed it — so no host grant can take effect and nothing is proposed; the
/// card informs. The proxy's decider constructs the grantable network violation, never this.
#[test]
fn a_decoded_connection_is_informational_and_proposes_nothing() {
    let s = Scratch::new("net");
    for blocked in [
        Blocked::Net {
            host: Some("pypi.org".to_owned()),
            port: Some(443),
        },
        Blocked::Net {
            host: None,
            port: Some(443),
        },
        Blocked::Net {
            host: None,
            port: None,
        },
    ] {
        let proposal = propose(&blocked, &s.policy(), &s.bounds(&[]));
        assert_eq!(None, proposal.subject, "{blocked:?}");
        assert_eq!(
            Disposition::informational(InformationalReason::UnproxiedNetwork),
            proposal.disposition,
            "{blocked:?}"
        );
    }
    let capability = propose(
        &Blocked::Capability {
            what: crate::command::violation::Capability::SetUid,
        },
        &s.policy(),
        &s.bounds(&[]),
    );
    assert_eq!(None, capability.subject);
    assert_eq!(
        Disposition::informational(InformationalReason::Capability),
        capability.disposition
    );
}

#[test]
fn proposed_subject_is_the_card_proposal_and_nothing_for_a_protected_target() {
    let s = Scratch::new("subject");
    let dir = s.root.join("outside").join("deep").join("dir");
    std::fs::create_dir_all(&dir).unwrap();
    let policy = s.policy();
    let bounds = s.bounds(&[]);
    let blocked = write(dir.join("a").join("b"));
    assert_eq!(
        propose(&blocked, &policy, &bounds).subject,
        proposed_subject(&blocked, &policy, &bounds)
    );
    assert_eq!(dir, root_of(proposed_subject(&blocked, &policy, &bounds)));
    let protected = write(s.home.join(".local").join("bin").join("tool"));
    assert_eq!(None, proposed_subject(&protected, &policy, &bounds));
    let capability = Blocked::Capability {
        what: crate::command::violation::Capability::SetUid,
    };
    assert_eq!(None, proposed_subject(&capability, &policy, &bounds));
}

/// A write anywhere in a curated build-cache tree proposes the one
/// `build_caches` grant — never the subpath the toolchain touched first, whether it exists (a
/// directory the climb would otherwise offer) or not (the first `mkdir`); a read there, and a
/// write anywhere else, propose what they always did.
#[test]
fn a_write_into_a_build_cache_tree_proposes_the_family() {
    let s = Scratch::new("build-caches");
    let policy = s.policy();
    let bounds = s.bounds(&[]);
    let src = s
        .home
        .join(".cargo/registry/src/index.crates.io-1949cf8c6b5b557f");
    std::fs::create_dir_all(&src).unwrap();
    for path in [
        src.join("serde-1.0.0/build.rs"),
        s.home.join(".npm/_npx/abc/node_modules/.bin/tool"),
        s.home.join(".npm"),
    ] {
        let proposal = propose(&write(path.clone()), &policy, &bounds);
        assert_eq!(
            Some(GrantSubject::BuildCaches),
            proposal.subject,
            "{}",
            path.display()
        );
        assert_eq!(Disposition::Grantable, proposal.disposition);
        assert_eq!(
            Some(GrantSubject::BuildCaches),
            proposed_subject(&write(path), &policy, &bounds)
        );
    }
    let read = Blocked::FsRead {
        path: src.join("serde-1.0.0/build.rs"),
    };
    assert!(
        matches!(
            propose(&read, &policy, &bounds).subject,
            Some(GrantSubject::FsRead { .. })
        ),
        "a read in a cache tree is a read grant, not the family"
    );
    let beside = s.home.join(".cargo-not-a-cache/x");
    assert!(
        matches!(
            propose(&write(beside), &policy, &bounds).subject,
            Some(GrantSubject::FsWriteRoot { .. })
        ),
        "the family is the trees, not a name prefix"
    );
    // A protected path is the floor first, even under a policy that put it in a tree
    let mut protected_tree = policy.clone();
    protected_tree.build_cache_trees.push(s.home.join(".local"));
    let proposal = propose(
        &write(s.home.join(".local/bin/tool")),
        &protected_tree,
        &bounds,
    );
    assert_eq!(None, proposal.subject);
    assert_eq!(
        Disposition::informational(InformationalReason::ProtectedTarget),
        proposal.disposition
    );
}

/// When the climb refuses — the target's existing ancestor is the workspace's
/// parent — the one file is proposed; the ancestor directory never is.
#[test]
fn when_the_climb_refuses_the_one_file_is_proposed() {
    let s = Scratch::new("file-scoped");
    let beside = s.ws.parent().unwrap().join("outside.txt");
    let proposal = propose(&write(beside.clone()), &s.policy(), &s.bounds(&[]));
    assert_eq!(beside, root_of(proposal.subject));
    assert_eq!(Disposition::Grantable, proposal.disposition);
    let read = Blocked::FsRead {
        path: s.home.join("notes.txt"),
    };
    let proposal = propose(&read, &s.policy(), &s.bounds(&[]));
    assert_eq!(
        Some(GrantSubject::FsRead {
            root: s.home.join("notes.txt"),
        }),
        proposal.subject,
        "the home directory is never proposed; the file is"
    );
}
