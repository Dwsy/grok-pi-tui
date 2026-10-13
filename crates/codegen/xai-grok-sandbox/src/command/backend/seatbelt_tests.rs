use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::*;
use crate::command::backend::{CallId, wrap_for_mode};
use crate::command::canonical::{ServedRoot, VolumeRule, resolved_spellings, with_volume_rule};
use crate::command::env::EnvGlobs;
use crate::command::git_config::GitConfigEnv;
use crate::command::grants::{Expiry, Grant, GrantDecision, GrantId, GrantScope, GrantSubject};
use crate::command::mode::SandboxMode;
use crate::command::policy::{
    DenyEntry, EnvPolicy, GROK_HOME_SECRET_GLOBS, NetworkPolicy, PolicyInputs, ReadPolicy,
    SECRET_READ_DENY_DIRS, SECRET_READ_DENY_FILES, SandboxPolicy,
};
use crate::command::protected::{self, Protected, ProtectedInputs};

/// Paths that do not exist on any machine, so canonicalization is a no-op and the rendering is
/// byte-stable across hosts.
const HOME: &str = "/opt/ws-fixture/nobody-w1";

/// The fixture's `**/.env` deny as the anchored regexes for the match and what lies beneath it.
const ENV_GLOB: &str = "(regex #\"^/opt/ws-fixture/nobody-w1/(.*/)?\\.env$\")";
const ENV_GLOB_BENEATH: &str = "(regex #\"^/opt/ws-fixture/nobody-w1/(.*/)?\\.env/.*$\")";

fn tag() -> CommandTag {
    CommandTag::for_call(&CallId::tool("t"))
}

fn protected_path(path: PathBuf) -> Protected {
    Protected::Path { path }
}

/// The launch seam with this backend attached, as every spawn site reaches it under `Enforce`:
/// what every backend refuses alike, and the environment, are settled there before `wrap`.
fn wrap_through_the_seam(
    cmd: &mut tokio::process::Command,
    original: &OriginalArgv,
    policy: &SandboxPolicy,
) -> Result<Option<WrapReceipt>, SandboxCommandError> {
    let backend = SeatbeltBackend::new();
    let backend: &dyn SandboxBackend = &backend;
    wrap_for_mode(
        SandboxMode::Enforce,
        Some(backend),
        cmd,
        original,
        policy,
        &tag(),
    )
}

/// The command's explicit environment entries: `Some` for a value, `None` for a removal.
fn envs_of(cmd: &std::process::Command) -> BTreeMap<String, Option<String>> {
    cmd.get_envs()
        .map(|(name, value)| {
            (
                name.to_string_lossy().into_owned(),
                value.map(|value| value.to_string_lossy().into_owned()),
            )
        })
        .collect()
}

fn fixture_policy() -> SandboxPolicy {
    let home = Path::new(HOME);
    SandboxPolicy {
        read: ReadPolicy::AllExcept {
            deny: vec![
                DenyEntry::Path(home.join(".ssh")),
                DenyEntry::Glob {
                    root: PathBuf::from(HOME),
                    tail: "**/.env".to_owned(),
                },
            ],
        },
        write_roots: vec![home.join("proj")],
        network: NetworkPolicy::Off,
        env: EnvPolicy {
            exclude_globs: EnvGlobs::new(EnvPolicy::default_excludes()).unwrap(),
            set: BTreeMap::new(),
        },
        // the floor is the one source of what stays read-only inside a writable root
        protected: vec![
            protected_path(home.join("proj/.git/hooks")),
            protected_path(home.join("proj/.grok")),
            protected_path(home.join(".zshrc")),
        ],
        build_cache_trees: Vec::new(),
        unread_git_metadata: Vec::new(),
    }
}

fn generated_section(profile: &str) -> &str {
    profile
        .split_once("; grok: generated rules follow; default denials carry the command tag\n")
        .map_or(profile, |(_, tail)| tail)
}

/// Whether a deny in `profile` takes the path at param `name` away: any deny of it as a `subpath`,
/// or a `file-write*` deny naming it. A rename pin (`file-write-unlink` of the literal directory)
/// is neither; it only keeps the directory where it is.
fn denies_the_path(profile: &str, name: &str) -> bool {
    let param = format!("(param \"{name}\")");
    let subpath = format!("(subpath {param})");
    profile.lines().any(|line| {
        line.starts_with("(deny")
            && (line.contains(&subpath)
                || (line.starts_with("(deny file-write*") && line.contains(&param)))
    })
}

#[test]
fn enforce_profile_renders_the_design_order_with_every_path_as_a_param() {
    let sbpl = render_enforce(&fixture_policy(), &tag()).expect("render");
    assert!(sbpl.profile.starts_with("; Copyright 2025 OpenAI"));
    assert!(sbpl.profile.contains("\n(deny default)\n"), "verbatim base");
    let generated = generated_section(&sbpl.profile);
    // The `**/.env` deny can match inside the write root, so it is carved out of the allow as a
    // floor glob would be
    let expected_head = format!(
        "\
(deny default (with message \"grok-t\"))
(allow file-read* (with message \"grok-t\"))
(allow file-write* (require-all (require-any (literal (param \"WRITABLE_ROOT_0\")) (subpath (param \"WRITABLE_ROOT_0\"))) (require-not (literal (param \"WRITABLE_ROOT_0_EXCLUDED_0_0\"))) (require-not (subpath (param \"WRITABLE_ROOT_0_EXCLUDED_0_0\"))) (require-not (literal (param \"WRITABLE_ROOT_0_EXCLUDED_1_0\"))) (require-not (subpath (param \"WRITABLE_ROOT_0_EXCLUDED_1_0\"))) (require-not {ENV_GLOB}) (require-not {ENV_GLOB_BENEATH})) (with message \"grok-t\"))
(deny file-write-unlink (require-all (literal (param \"WRITABLE_ROOT_0\")) (vnode-type DIRECTORY)) (with message \"grok-t\"))
; Copyright 2025 OpenAI"
    );
    assert!(
        generated.contains(&expected_head),
        "generated section:\n{generated}"
    );
    assert!(
        generated.contains("(allow user-preference-read)\n; grok: mandatory denies"),
        "preferences add-on precedes the mandatory block:\n{generated}"
    );
    // Every floor entry and every read deny is write-denied, may never be the source of a hard
    // link and is unreachable as a unix socket; the read denies themselves come last
    let expected_tail = format!(
        "\
; grok: mandatory denies; nothing below may be re-opened by a grant
(deny file-write* file-link (literal (param \"PROTECTED_0_0\")) (subpath (param \"PROTECTED_0_0\")) (with message \"grok-t\"))
(deny network-outbound (remote unix-socket (literal (param \"PROTECTED_0_0\"))) (remote unix-socket (subpath (param \"PROTECTED_0_0\"))) (with message \"grok-t\"))
(deny file-write* file-link (literal (param \"PROTECTED_1_0\")) (subpath (param \"PROTECTED_1_0\")) (with message \"grok-t\"))
(deny network-outbound (remote unix-socket (literal (param \"PROTECTED_1_0\"))) (remote unix-socket (subpath (param \"PROTECTED_1_0\"))) (with message \"grok-t\"))
(deny file-write* file-link (literal (param \"PROTECTED_2_0\")) (subpath (param \"PROTECTED_2_0\")) (with message \"grok-t\"))
(deny network-outbound (remote unix-socket (literal (param \"PROTECTED_2_0\"))) (remote unix-socket (subpath (param \"PROTECTED_2_0\"))) (with message \"grok-t\"))
(deny file-write* file-link (literal (param \"DENY_0_0\")) (subpath (param \"DENY_0_0\")) (with message \"grok-t\"))
(deny network-outbound (remote unix-socket (literal (param \"DENY_0_0\"))) (remote unix-socket (subpath (param \"DENY_0_0\"))) (with message \"grok-t\"))
(deny file-write* file-link {ENV_GLOB} {ENV_GLOB_BENEATH} (with message \"grok-t\"))
(deny network-outbound (remote unix-socket {ENV_GLOB}) (remote unix-socket {ENV_GLOB_BENEATH}) (with message \"grok-t\"))
(deny file-write* file-link (literal (param \"GIT_DIR_NODE_0\")) (with message \"grok-t\"))
(deny file-read* (literal (param \"DENY_0_0\")) (subpath (param \"DENY_0_0\")) (with message \"grok-t\"))
(deny file-read* {ENV_GLOB} {ENV_GLOB_BENEATH} (with message \"grok-t\"))
(deny mach-lookup (xpc-service-name-prefix \"\") (with message \"grok-t\"))
(deny system-fcntl (fcntl-command 80 110) (with message \"grok-t\"))
"
    );
    assert!(
        sbpl.profile.ends_with(&expected_tail),
        "profile tail:\n{}",
        sbpl.profile
    );
    let params: Vec<(String, String)> = [
        ("WRITABLE_ROOT_0", "/opt/ws-fixture/nobody-w1/proj"),
        (
            "WRITABLE_ROOT_0_EXCLUDED_0_0",
            "/opt/ws-fixture/nobody-w1/proj/.git/hooks",
        ),
        (
            "WRITABLE_ROOT_0_EXCLUDED_1_0",
            "/opt/ws-fixture/nobody-w1/proj/.grok",
        ),
        ("PROTECTED_0_0", "/opt/ws-fixture/nobody-w1/proj/.git/hooks"),
        ("PROTECTED_1_0", "/opt/ws-fixture/nobody-w1/proj/.grok"),
        ("PROTECTED_2_0", "/opt/ws-fixture/nobody-w1/.zshrc"),
        ("DENY_0_0", "/opt/ws-fixture/nobody-w1/.ssh"),
        ("GIT_DIR_NODE_0", "/opt/ws-fixture/nobody-w1/proj/.git"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect();
    assert_eq!(params, sbpl.params);
    // Only the anchored glob regex carries its root inline; every exact path goes through `-D`.
    for inlined in ["nobody-w1/proj", "/.ssh", "/.zshrc"] {
        assert!(
            !sbpl.profile.contains(inlined),
            "{inlined} must reach the profile as a param only"
        );
    }
}

/// Each write root's `.git` node is a literal deny in the mandatory block (a temp root's
/// `/private` alias included), never a subpath: a rename in or out of the node is refused and
/// every write inside it stays under the root's allow.
#[test]
fn each_write_roots_git_node_is_a_literal_deny_after_every_allow() {
    let mut policy = fixture_policy();
    policy.write_roots = vec![
        PathBuf::from(format!("{HOME}/proj")),
        PathBuf::from("/tmp/grok-w1-fixture-missing/ws"),
    ];
    let sbpl = render_enforce(&policy, &tag()).expect("render");
    let nodes: Vec<(&str, &str)> = sbpl
        .params
        .iter()
        .filter(|(name, _)| name.starts_with("GIT_DIR_NODE_"))
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    assert_eq!(
        vec![
            ("GIT_DIR_NODE_0", "/opt/ws-fixture/nobody-w1/proj/.git"),
            ("GIT_DIR_NODE_1", "/tmp/grok-w1-fixture-missing/ws/.git"),
            (
                "GIT_DIR_NODE_2",
                "/private/tmp/grok-w1-fixture-missing/ws/.git"
            ),
        ],
        nodes
    );
    let (_, mandatory) = sbpl
        .profile
        .split_once("; grok: mandatory denies")
        .expect("the mandatory block");
    for (name, _) in &nodes {
        let rule = format!(
            "(deny file-write* file-link (literal (param \"{name}\")) (with message \"grok-t\"))"
        );
        assert!(mandatory.contains(&rule), "{rule}\n{}", sbpl.profile);
        assert!(
            !sbpl
                .profile
                .contains(&format!("(subpath (param \"{name}\"))")),
            "{name} is denied as a literal only"
        );
    }
}

/// Every root's unlink pin comes after every root's allow: a build-cache tree granted after the
/// verified cache inside it would otherwise re-open rename of that cache under last-match.
#[test]
fn root_pins_follow_every_allow_so_a_parent_root_cannot_reopen_them() {
    let mut policy = fixture_policy();
    policy.write_roots = vec![
        PathBuf::from(format!("{HOME}/.cargo/registry/cache")),
        PathBuf::from(format!("{HOME}/.cargo/registry")),
    ];
    policy.protected.clear();
    let profile = render_enforce(&policy, &tag()).expect("render").profile;
    let last_allow = profile
        .rfind("(allow file-write* (require-all")
        .expect("root allows");
    for root in ["WRITABLE_ROOT_0", "WRITABLE_ROOT_1"] {
        let pin = format!(
            "(deny file-write-unlink (require-all (literal (param \"{root}\")) (vnode-type DIRECTORY))"
        );
        let at = profile.find(&pin).expect("the root is pinned");
        assert!(
            at > last_allow,
            "{root} pinned before a later allow:\n{profile}"
        );
    }
}

#[test]
fn tmp_root_renders_its_private_alias_as_a_second_root() {
    let mut policy = fixture_policy();
    policy.write_roots = vec![PathBuf::from("/tmp/grok-w1-fixture-missing/ws")];
    policy.protected.clear();
    let sbpl = render_enforce(&policy, &tag()).expect("render");
    let roots: Vec<&(String, String)> = sbpl
        .params
        .iter()
        .filter(|(k, _)| k.starts_with("WRITABLE_ROOT_"))
        .collect();
    assert_eq!(
        vec![
            &(
                "WRITABLE_ROOT_0".to_owned(),
                "/tmp/grok-w1-fixture-missing/ws".to_owned()
            ),
            &(
                "WRITABLE_ROOT_1".to_owned(),
                "/private/tmp/grok-w1-fixture-missing/ws".to_owned()
            ),
        ],
        roots
    );
    assert_eq!(2, sbpl.profile.matches("(deny file-write-unlink").count());
}

/// A write root, a restricted read root or a floor tree's exception swapped for a symlink after the
/// policy was built renders as spelled, with its `/private` alias: the renderer resolves none, so
/// the link's target never becomes a root or voids the tree.
#[test]
fn a_root_swapped_for_a_symlink_never_renders_its_target() {
    let base = dunce::canonicalize(scratch_dir("root-swapped")).expect("canonical scratch");
    let (root, target) = (base.join("lib"), base.join("config-git"));
    let (read_root, read_target) = (base.join("docs"), base.join("ssh-keys"));
    let (tree, except) = (base.join("grok"), base.join("grok/commands"));
    std::fs::create_dir_all(&target).expect("mkdir target");
    std::fs::create_dir_all(&read_target).expect("mkdir read target");
    std::fs::create_dir_all(&tree).expect("mkdir tree");
    let mut policy = fixture_policy();
    policy.write_roots = vec![root.clone()];
    policy.read = ReadPolicy::Roots {
        roots: vec![read_root.clone()],
        deny: policy.read.deny().to_vec(),
    };
    policy.protected.push(Protected::TreeExcept {
        tree: tree.clone(),
        except: except.clone(),
    });
    std::os::unix::fs::symlink(&target, &root).expect("ln -s");
    std::os::unix::fs::symlink(&read_target, &read_root).expect("ln -s");
    std::os::unix::fs::symlink(&tree, &except).expect("ln -s");
    assert!(
        !render_enforce(&policy, &tag())
            .expect("render")
            .params
            .iter()
            .any(|(name, value)| name.contains("_EXCEPT_") && Path::new(value) == tree),
        "the exception resolved to the tree it carves"
    );
    let sbpl = render_enforce(&policy, &tag()).expect("render");
    let first_value = |prefix: &str| {
        sbpl.params
            .iter()
            .find(|(name, _)| name.starts_with(prefix) && !name.contains("_EXCLUDED_"))
            .map(|(_, value)| value.as_str())
    };
    assert_eq!(root.to_str(), first_value("WRITABLE_ROOT_"));
    assert_eq!(read_root.to_str(), first_value("READABLE_ROOT_"));
    assert!(
        !sbpl
            .params
            .iter()
            .any(|(_, value)| value.ends_with("/config-git") || value.ends_with("/ssh-keys")),
        "{:?}",
        sbpl.params
    );
    let _ = std::fs::remove_dir_all(&base);
}

/// A link node renders as `(literal <link>)` under its parent's spellings and never resolves: a
/// stow-style `.config -> dotfiles` leaves a write root at `dotfiles` without any deny of its tree
/// (the floor entry inside it only pins it against rename).
#[test]
fn a_link_node_renders_as_a_literal_and_never_denies_its_target() {
    let base = dunce::canonicalize(scratch_dir("link-node")).expect("canonical scratch");
    let (link, dotfiles) = (base.join(".config"), base.join("dotfiles"));
    std::fs::create_dir_all(dotfiles.join("git")).expect("mkdir dotfiles");
    std::os::unix::fs::symlink(&dotfiles, &link).expect("ln -s");
    let mut policy = fixture_policy();
    policy.write_roots = vec![dotfiles.clone()];
    policy.protected = vec![
        Protected::Node { path: link.clone() },
        protected_path(dotfiles.join("git/config")),
    ];
    let sbpl = render_enforce(&policy, &tag()).expect("render");
    let link_params: Vec<&str> = sbpl
        .params
        .iter()
        .filter(|(_, value)| Path::new(value) == link)
        .map(|(name, _)| name.as_str())
        .collect();
    assert!(!link_params.is_empty(), "{:?}", sbpl.params);
    for name in link_params {
        let literal = format!("(literal (param \"{name}\"))");
        assert!(
            sbpl.profile
                .lines()
                .any(|line| line.starts_with("(deny file-write* file-link")
                    && line.contains(&literal)),
            "{}",
            sbpl.profile
        );
        assert!(
            !sbpl
                .profile
                .contains(&format!("(subpath (param \"{name}\"))"))
        );
    }
    let target_params: Vec<&str> = sbpl
        .params
        .iter()
        .filter(|(_, value)| Path::new(value).ends_with("dotfiles"))
        .map(|(name, _)| name.as_str())
        .collect();
    assert!(
        target_params.iter().any(|name| sbpl.profile.contains(&format!(
            "(deny file-write-unlink (require-all (vnode-type DIRECTORY) (literal (param \"{name}\"))) (with message \"grok-t\"))"
        ))),
        "{}",
        sbpl.profile
    );
    for name in target_params {
        assert!(
            !denies_the_path(&sbpl.profile, name),
            "{name} denies the link's target"
        );
        for deny in [
            format!("(deny file-write-unlink (subpath (param \"{name}\")))"),
            format!("(deny file-write* file-link (literal (param \"{name}\")))"),
        ] {
            assert!(denies_the_path(&deny, name), "{deny}");
        }
    }
    let _ = std::fs::remove_dir_all(&base);
}

/// The carve-outs of a pinned entry are one set of filters per policy root, shared by the root's
/// `/private` spelling: the `-D` params are registered once (under the root's first index), and
/// both allow rules name the same params. Every param rides `sandbox-exec`'s argv, so an aliased
/// root must not double the floor.
#[test]
fn an_aliased_roots_carve_outs_are_registered_once_and_shared_by_both_spellings() {
    let root = PathBuf::from("/tmp/grok-w1-fixture-missing/ws");
    let mut policy = fixture_policy();
    policy.read = ReadPolicy::AllExcept { deny: Vec::new() };
    policy.write_roots = vec![root.clone()];
    policy.protected = vec![protected_path(root.join(".grok"))];
    let sbpl = render_enforce(&policy, &tag()).expect("render");
    // One param family for the carve-out, under the root's first index, and the `-D` list in the
    // order the profile names them: the root, its carve-outs, then its alias
    let write_root_params: Vec<&str> = sbpl
        .params
        .iter()
        .map(|(name, _)| name.as_str())
        .filter(|name| name.starts_with("WRITABLE_ROOT_"))
        .collect();
    assert_eq!(
        vec![
            "WRITABLE_ROOT_0",
            "WRITABLE_ROOT_0_EXCLUDED_0_0",
            "WRITABLE_ROOT_0_EXCLUDED_0_1",
            "WRITABLE_ROOT_1",
        ],
        write_root_params,
        "{:?}",
        sbpl.params
    );
    let excluded: Vec<&str> = write_root_params
        .iter()
        .copied()
        .filter(|name| name.contains("_EXCLUDED_"))
        .collect();
    let allows: Vec<&str> = sbpl
        .profile
        .lines()
        .filter(|line| {
            line.starts_with(
                "(allow file-write* (require-all (require-any (literal (param \"WRITABLE_ROOT_",
            )
        })
        .collect();
    assert_eq!(2, allows.len(), "one allow per spelling:\n{}", sbpl.profile);
    for (spelling, allow) in ["WRITABLE_ROOT_0", "WRITABLE_ROOT_1"].iter().zip(&allows) {
        assert!(
            allow.contains(&format!("(literal (param \"{spelling}\"))")),
            "{allow}"
        );
        for name in &excluded {
            assert!(
                allow.contains(&format!("(require-not (literal (param \"{name}\")))")),
                "{spelling} must carve out {name}: {allow}"
            );
        }
    }
}

/// A glob entry renders as anchored regexes (the match and what lies beneath it), a
/// tree-except as one `require-all` filter; each is carved out of the root it reaches into and
/// denied last for writes and hard links. The own session root lies in the tree-except's
/// exception, so its allow carries no carve-out.
#[test]
fn glob_and_tree_except_floor_entries_render_as_regex_and_require_all_filters() {
    let home = Path::new(HOME);
    let sessions = home.join(".grok/sessions");
    let own = sessions.join("%2Fopt%2Fws-fixture%2Fnobody-w1%2Fproj");
    let mut policy = fixture_policy();
    policy.write_roots = vec![home.join("proj"), own.clone()];
    // The `.ssh` deny alone: a `**/.env` glob would reach into both roots and carve itself out
    // of their allows too, which the fixture test covers
    policy.read = ReadPolicy::AllExcept {
        deny: vec![DenyEntry::Path(home.join(".ssh"))],
    };
    policy.protected = vec![
        Protected::Glob {
            glob: format!("{HOME}/proj/.git/modules/**/hooks"),
        },
        Protected::TreeExcept {
            tree: sessions.clone(),
            except: own.clone(),
        },
    ];
    let sbpl = render_enforce(&policy, &tag()).expect("render");
    let modules = "(regex #\"^/opt/ws-fixture/nobody-w1/proj/\\.git/modules/(.*/)?hooks$\")";
    let beneath = "(regex #\"^/opt/ws-fixture/nobody-w1/proj/\\.git/modules/(.*/)?hooks/.*$\")";
    let tree_except = "(require-all (require-any (literal (param \"PROTECTED_1_TREE_0\")) (subpath (param \"PROTECTED_1_TREE_0\"))) (require-not (subpath (param \"PROTECTED_1_TREE_0_EXCEPT_0\"))))";
    let root = |n: u8| {
        format!(
            "(require-any (literal (param \"WRITABLE_ROOT_{n}\")) (subpath (param \"WRITABLE_ROOT_{n}\")))"
        )
    };
    for expected in [
        format!(
            "(allow file-write* (require-all {} (require-not {modules}) (require-not {beneath})) (with message \"grok-t\"))",
            root(0)
        ),
        format!(
            "(allow file-write* (require-all {}) (with message \"grok-t\"))",
            root(1)
        ),
        format!("(deny file-write* file-link {modules} {beneath} (with message \"grok-t\"))"),
        format!("(deny file-write* file-link {tree_except} (with message \"grok-t\"))"),
    ] {
        assert!(
            sbpl.profile.contains(&expected),
            "missing:\n{expected}\nin:\n{}",
            sbpl.profile
        );
    }
    assert_eq!(
        vec![
            ("WRITABLE_ROOT_0".to_owned(), format!("{HOME}/proj")),
            ("WRITABLE_ROOT_1".to_owned(), own.display().to_string()),
            (
                "PROTECTED_1_TREE_0".to_owned(),
                sessions.display().to_string()
            ),
            (
                "PROTECTED_1_TREE_0_EXCEPT_0".to_owned(),
                own.display().to_string()
            ),
            ("DENY_0_0".to_owned(), format!("{HOME}/.ssh")),
            ("GIT_DIR_NODE_0".to_owned(), format!("{HOME}/proj/.git")),
            (
                "GIT_DIR_NODE_1".to_owned(),
                own.join(".git").display().to_string()
            ),
            (
                "GLOB_PREFIX_NODE_0".to_owned(),
                format!("{HOME}/proj/.git/modules")
            ),
        ],
        sbpl.params
    );
}

/// A read deny is pinned as a floor entry is, in every shape: carved out of the write root it
/// reaches into, write- and link-denied, its glob prefix a node, its existing ancestors held
/// against rename — and then read-denied last. Without the pins, `mv .env aside` followed by a
/// read of `aside` would pass a deny that only a `file-read*` rule held. A deny the floor already
/// names is pinned once, by its floor entry.
#[test]
fn a_read_deny_is_pinned_like_a_floor_entry() {
    let ws = Path::new(HOME).join("proj");
    let sessions = ws.join("sessions");
    let mut policy = fixture_policy();
    policy.write_roots = vec![ws.clone()];
    policy.protected = vec![protected_path(ws.join(".grok"))];
    policy.read = ReadPolicy::AllExcept {
        deny: vec![
            DenyEntry::Path(ws.join("config/.env")),
            DenyEntry::Glob {
                root: ws.clone(),
                tail: "secrets/**/key".to_owned(),
            },
            DenyEntry::TreeExcept {
                tree: sessions.clone(),
                except: sessions.join("own"),
            },
            DenyEntry::Path(ws.join(".grok")),
        ],
    };
    let sbpl = render_enforce(&policy, &tag()).expect("render");
    let key = "(regex #\"^/opt/ws-fixture/nobody-w1/proj/secrets/(.*/)?key$\")";
    let key_beneath = "(regex #\"^/opt/ws-fixture/nobody-w1/proj/secrets/(.*/)?key/.*$\")";
    let tree = |prefix: &str| {
        format!(
            "(require-all (require-any (literal (param \"{prefix}_TREE_0\")) (subpath (param \"{prefix}_TREE_0\"))) (require-not (subpath (param \"{prefix}_TREE_0_EXCEPT_0\"))))"
        )
    };
    let (allow, mandatory) = sbpl
        .profile
        .split_once("; grok: mandatory denies")
        .expect("the mandatory block");
    // pinned entries are the floor's then the denies', so the `.env` deny is the root's second
    // carve-out and the tree its fourth
    for carved in [
        "(require-not (literal (param \"WRITABLE_ROOT_0_EXCLUDED_1_0\"))) (require-not (subpath (param \"WRITABLE_ROOT_0_EXCLUDED_1_0\")))".to_owned(),
        format!("(require-not {key}) (require-not {key_beneath})"),
        format!("(require-not {})", tree("WRITABLE_ROOT_0_EXCLUDED_3")),
    ] {
        assert!(
            allow.contains(&carved),
            "the deny is carved out of the root's allow: {carved}\n{allow}"
        );
    }
    for pinned in [
        "(deny file-write* file-link (literal (param \"DENY_0_0\")) (subpath (param \"DENY_0_0\")) (with message \"grok-t\"))".to_owned(),
        format!("(deny file-write* file-link {key} {key_beneath} (with message \"grok-t\"))"),
        format!(
            "(deny file-write* file-link {} (with message \"grok-t\"))",
            tree("DENY_2")
        ),
        "(deny file-write* file-link (literal (param \"DENY_NODE_0\")) (with message \"grok-t\"))".to_owned(),
        "(deny file-write* file-link (literal (param \"DENY_NODE_1\")) (with message \"grok-t\"))".to_owned(),
        "(deny file-read* (literal (param \"DENY_0_0\")) (subpath (param \"DENY_0_0\")) (with message \"grok-t\"))".to_owned(),
        format!("(deny file-read* {key} {key_beneath} (with message \"grok-t\"))"),
        format!("(deny file-read* {} (with message \"grok-t\"))", tree("DENY_2")),
    ] {
        assert!(
            mandatory.contains(&pinned),
            "missing:\n{pinned}\nin:\n{mandatory}"
        );
    }
    let named = |prefix: &str| -> Vec<PathBuf> {
        sbpl.params
            .iter()
            .filter(|(name, _)| name.starts_with(prefix))
            .map(|(_, value)| PathBuf::from(value))
            .collect()
    };
    assert_eq!(
        vec![ws.join("config/.env")],
        named("WRITABLE_ROOT_0_EXCLUDED_1_")
    );
    // a missing ancestor of the `.env` deny and the glob's literal prefix are nodes, as a
    // floor entry's would be; the tree needs none, its own literal filter meets a rename
    assert_eq!(
        vec![ws.join("config"), ws.join("secrets")],
        named("DENY_NODE_"),
        "{:?}",
        sbpl.params
    );
    // the `.grok` deny duplicates the floor entry: its pins are the floor's, its read rule its own
    assert_eq!(
        1,
        mandatory
            .matches("(deny file-write* file-link (literal (param \"PROTECTED_0_0\"))")
            .count()
    );
    assert!(
        !mandatory.contains("(deny file-write* file-link (literal (param \"DENY_3_0\"))"),
        "{mandatory}"
    );
    assert!(mandatory.contains(
        "(deny file-read* (literal (param \"DENY_3_0\")) (subpath (param \"DENY_3_0\")) (with message \"grok-t\"))"
    ));
    let lines: Vec<&str> = mandatory.lines().collect();
    let first_read_deny = lines
        .iter()
        .position(|line| line.starts_with("(deny file-read* "))
        .expect("a read deny");
    let last_pin = lines
        .iter()
        .rposition(|line| line.starts_with("(deny file-write"))
        .expect("a write deny");
    assert!(
        first_read_deny > last_pin,
        "read denies come after every pin:\n{mandatory}"
    );
}

/// The existing ancestors of a read deny inside a write root are held against rename, as a
/// floor entry's are: `config` cannot be moved aside while it holds the denied `.env`.
#[test]
fn a_read_denys_existing_ancestors_are_pinned_against_rename() {
    let base = dunce::canonicalize(scratch_dir("deny-ancestors")).expect("canonical scratch");
    let ws = base.join("ws");
    std::fs::create_dir_all(ws.join("config")).expect("mkdir");
    let mut policy = fixture_policy();
    policy.write_roots = vec![ws.clone()];
    policy.protected.clear();
    policy.read = ReadPolicy::AllExcept {
        deny: vec![DenyEntry::Path(ws.join("config/.env"))],
    };
    let sbpl = render_enforce(&policy, &tag()).expect("render");
    let pinned: Vec<PathBuf> = sbpl
        .params
        .iter()
        .filter(|(name, _)| name.starts_with("PROTECTED_ANCESTOR_"))
        .map(|(_, value)| PathBuf::from(value))
        .collect();
    for dir in [&ws.join("config"), &ws] {
        assert!(pinned.contains(dir), "{dir:?} in {pinned:?}");
    }
    assert!(
        !sbpl
            .params
            .iter()
            .any(|(name, _)| name.starts_with("DENY_NODE_")),
        "an existing ancestor is pinned, not a node: {:?}",
        sbpl.params
    );
    let _ = std::fs::remove_dir_all(&base);
}

/// Every pinned entry — floor and read deny, in every filter shape — is unreachable as a unix
/// socket through the same filters that select it for a file operation: a `literal` beside the
/// `subpath`, the anchored regexes of a glob, the `require-*` combinators of a tree-except
/// around `(remote unix-socket …)` leaves.
#[test]
fn the_unix_socket_deny_covers_every_filter_shape() {
    let home = Path::new(HOME);
    let mut policy = fixture_policy();
    policy.protected = vec![
        protected_path(home.join("proj/.grok")),
        Protected::Glob {
            glob: format!("{HOME}/proj/.git/modules/**/hooks"),
        },
        Protected::TreeExcept {
            tree: home.join(".grok/sessions"),
            except: home.join(".grok/sessions/own"),
        },
    ];
    policy.read = ReadPolicy::AllExcept {
        deny: vec![
            DenyEntry::Path(home.join(".ssh")),
            DenyEntry::Glob {
                root: PathBuf::from(HOME),
                tail: "**/.env".to_owned(),
            },
            DenyEntry::TreeExcept {
                tree: home.join(".config"),
                except: home.join(".config/grok"),
            },
        ],
    };
    let sbpl = render_enforce(&policy, &tag()).expect("render");
    let modules = "(regex #\"^/opt/ws-fixture/nobody-w1/proj/\\.git/modules/(.*/)?hooks$\")";
    let beneath = "(regex #\"^/opt/ws-fixture/nobody-w1/proj/\\.git/modules/(.*/)?hooks/.*$\")";
    let socket = |filter: &str| format!("(remote unix-socket {filter})");
    for expected in [
        format!(
            "(deny network-outbound {} {} (with message \"grok-t\"))",
            socket("(literal (param \"PROTECTED_0_0\"))"),
            socket("(subpath (param \"PROTECTED_0_0\"))")
        ),
        format!(
            "(deny network-outbound {} {} (with message \"grok-t\"))",
            socket(modules),
            socket(beneath)
        ),
        format!(
            "(deny network-outbound (require-all (require-any {} {}) (require-not {})) (with message \"grok-t\"))",
            socket("(literal (param \"PROTECTED_2_TREE_0\"))"),
            socket("(subpath (param \"PROTECTED_2_TREE_0\"))"),
            socket("(subpath (param \"PROTECTED_2_TREE_0_EXCEPT_0\"))")
        ),
        format!(
            "(deny network-outbound {} {} (with message \"grok-t\"))",
            socket("(literal (param \"DENY_0_0\"))"),
            socket("(subpath (param \"DENY_0_0\"))")
        ),
        format!(
            "(deny network-outbound {} {} (with message \"grok-t\"))",
            socket(ENV_GLOB),
            socket(ENV_GLOB_BENEATH)
        ),
        format!(
            "(deny network-outbound (require-all (require-any {} {}) (require-not {})) (with message \"grok-t\"))",
            socket("(literal (param \"DENY_2_TREE_0\"))"),
            socket("(subpath (param \"DENY_2_TREE_0\"))"),
            socket("(subpath (param \"DENY_2_TREE_0_EXCEPT_0\"))")
        ),
    ] {
        assert!(
            sbpl.profile.contains(&expected),
            "missing:\n{expected}\nin:\n{}",
            sbpl.profile
        );
    }
    // one socket deny per pinned entry, none selected by fewer filters than its write deny
    assert_eq!(
        policy.protected.len() + policy.read.deny().len(),
        sbpl.profile.matches("(deny network-outbound").count(),
        "{}",
        sbpl.profile
    );
    for line in sbpl.profile.lines() {
        if let Some(filters) = line.strip_prefix("(deny network-outbound ") {
            let socket_filters = filters.matches("(remote unix-socket ").count();
            let path_filters = ["(literal ", "(subpath ", "(regex "]
                .iter()
                .map(|shape| filters.matches(shape).count())
                .sum::<usize>();
            assert_eq!(path_filters, socket_filters, "{line}");
        }
    }
}

/// A glob entry's literal prefix is pinned against rename with its ancestors, so the tree the
/// pattern names cannot be moved aside, filled, and moved back; a prefix that does not exist
/// yet has nothing to pin.
#[test]
fn a_glob_entry_pins_its_existing_literal_prefix_against_rename() {
    let base = dunce::canonicalize(scratch_dir("glob-pin")).expect("canonical scratch");
    let ws = base.join("ws");
    let modules = ws.join(".git/modules");
    std::fs::create_dir_all(&modules).expect("mkdir");
    let mut policy = fixture_policy();
    policy.write_roots = vec![ws.clone()];
    policy.protected = vec![Protected::Glob {
        glob: format!("{}/**/hooks", modules.display()),
    }];
    let pinned = |policy: &SandboxPolicy| -> Vec<PathBuf> {
        let sbpl = render_enforce(policy, &tag()).expect("render");
        sbpl.params
            .iter()
            .filter(|(name, _)| name.starts_with("PROTECTED_ANCESTOR_"))
            .map(|(_, value)| PathBuf::from(value))
            .collect()
    };
    let with_modules = pinned(&policy);
    for dir in [&modules, &ws.join(".git"), &ws] {
        assert!(with_modules.contains(dir), "{dir:?} in {with_modules:?}");
    }
    let sbpl = render_enforce(&policy, &tag()).expect("render");
    assert!(
        sbpl.profile
            .contains("(deny file-write-unlink (require-all (vnode-type DIRECTORY) (literal (param \"PROTECTED_ANCESTOR_"),
        "{}",
        sbpl.profile
    );

    std::fs::remove_dir_all(&modules).expect("rmdir");
    assert!(!pinned(&policy).contains(&modules));
    let _ = std::fs::remove_dir_all(&base);
}

/// A glob's literal prefix inside a write root is denied as a node whether or not it exists yet:
/// a staged tree cannot be moved in as `.git/modules`, and a prefix that is the write root itself
/// (the own session directory's `permission*.toml`) adds nothing.
#[test]
fn a_glob_prefix_inside_a_write_root_is_a_literal_node_deny() {
    let mut policy = fixture_policy();
    let own = PathBuf::from(format!("{HOME}/.grok/own-session"));
    policy.write_roots = vec![PathBuf::from(format!("{HOME}/proj")), own.clone()];
    policy.protected = vec![
        Protected::Glob {
            glob: format!("{HOME}/proj/.git/modules/**/hooks"),
        },
        Protected::Glob {
            glob: format!("{}/permission*.toml", own.display()),
        },
    ];
    let sbpl = render_enforce(&policy, &tag()).expect("render");
    let nodes: Vec<&(String, String)> = sbpl
        .params
        .iter()
        .filter(|(name, _)| name.starts_with("GLOB_PREFIX_NODE_"))
        .collect();
    assert_eq!(
        vec![&(
            "GLOB_PREFIX_NODE_0".to_owned(),
            format!("{HOME}/proj/.git/modules")
        )],
        nodes
    );
    assert!(sbpl.profile.contains(
        "(deny file-write* file-link (literal (param \"GLOB_PREFIX_NODE_0\")) (with message \"grok-t\"))"
    ));
}

/// A glob spelled through the `/private` alias of its write root, or in another case, meets the
/// root as the volume compares them: its literal prefix is still a node deny, and pinned against
/// rename whenever it exists as spelled.
#[test]
fn a_glob_prefix_spelled_through_the_private_alias_or_another_case_is_a_node_and_pinned() {
    let name = format!("grok-seatbelt-alias-pin-{}", std::process::id());
    let root = Path::new("/tmp").join(&name).join("ws");
    let private = Path::new("/private/tmp").join(&name).join("ws");
    let other_case = Path::new("/private/tmp")
        .join(name.to_uppercase())
        .join("WS");
    std::fs::create_dir_all(private.join(".git/modules")).expect("mkdir");
    for glob_root in [&private, &other_case] {
        let mut policy = fixture_policy();
        policy.write_roots = vec![root.clone()];
        policy.protected = vec![Protected::Glob {
            glob: format!("{}/.git/modules/**/hooks", glob_root.display()),
        }];
        let sbpl =
            with_volume_rule(VolumeRule::Apfs, || render_enforce(&policy, &tag())).expect("render");
        let modules = glob_root.join(".git/modules");
        let param_for = |prefix: &str| -> Option<String> {
            sbpl.params
                .iter()
                .find(|(name, value)| name.starts_with(prefix) && Path::new(value) == modules)
                .map(|(name, _)| name.clone())
        };
        let node = param_for("GLOB_PREFIX_NODE_").expect("the prefix is a node");
        let node_rule = format!(
            "(deny file-write* file-link (literal (param \"{node}\")) (with message \"grok-t\"))"
        );
        assert!(sbpl.profile.contains(&node_rule), "{}", sbpl.profile);
        let pin = param_for("PROTECTED_ANCESTOR_");
        assert_eq!(
            protected::is_real_directory(&modules),
            pin.is_some(),
            "{glob_root:?}: {:?}",
            sbpl.params
        );
        if let Some(pin) = pin {
            let pin_rule = format!(
                "(deny file-write-unlink (require-all (vnode-type DIRECTORY) (literal (param \"{pin}\"))) (with message \"grok-t\"))"
            );
            assert!(sbpl.profile.contains(&pin_rule), "{}", sbpl.profile);
        }
    }
    let _ = std::fs::remove_dir_all(Path::new("/private/tmp").join(&name));
}

/// Under a `$HOME` grant every other missing ancestor of a floor entry is a literal node deny, so
/// a staged tree cannot be moved in as `~/.config/git`; a root's `.git` node is not rendered twice
/// and a root is never a node.
#[test]
fn a_missing_ancestor_inside_a_write_root_is_a_literal_node_deny() {
    let mut policy = fixture_policy();
    policy.write_roots = vec![PathBuf::from(HOME), PathBuf::from(format!("{HOME}/proj"))];
    policy.protected = vec![
        protected_path(PathBuf::from(format!("{HOME}/.config/git/config"))),
        protected_path(PathBuf::from(format!("{HOME}/proj/.git/hooks"))),
        protected_path(PathBuf::from(format!("{HOME}/.zshrc"))),
    ];
    let sbpl = render_enforce(&policy, &tag()).expect("render");
    let nodes: Vec<(&str, &str)> = sbpl
        .params
        .iter()
        .filter(|(name, _)| name.starts_with("ANCESTOR_NODE_"))
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    assert_eq!(
        vec![
            ("ANCESTOR_NODE_0", "/opt/ws-fixture/nobody-w1/.config/git"),
            ("ANCESTOR_NODE_1", "/opt/ws-fixture/nobody-w1/.config"),
        ],
        nodes
    );
    let (_, mandatory) = sbpl
        .profile
        .split_once("; grok: mandatory denies")
        .expect("the mandatory block");
    for (name, _) in &nodes {
        let rule = format!(
            "(deny file-write* file-link (literal (param \"{name}\")) (with message \"grok-t\"))"
        );
        assert!(mandatory.contains(&rule), "{rule}\n{}", sbpl.profile);
    }
}

/// An existing ancestor past a missing one is pinned too (`tools` cannot be moved away and
/// replaced while `tools/git` does not exist yet), and the missing `tools/git` is a node.
#[test]
fn an_existing_ancestor_past_a_missing_one_is_pinned_and_the_missing_one_is_a_node() {
    let base = dunce::canonicalize(scratch_dir("ancestor-gap")).expect("canonical scratch");
    let ws = base.join("ws");
    std::fs::create_dir_all(ws.join("tools")).expect("mkdir");
    let mut policy = fixture_policy();
    policy.write_roots = vec![ws.clone()];
    policy.protected = vec![protected_path(ws.join("tools/git/hooks"))];
    let sbpl = render_enforce(&policy, &tag()).expect("render");
    let named = |prefix: &str| -> Vec<PathBuf> {
        sbpl.params
            .iter()
            .filter(|(name, _)| name.starts_with(prefix))
            .map(|(_, value)| PathBuf::from(value))
            .collect()
    };
    let pinned = named("PROTECTED_ANCESTOR_");
    assert!(pinned.contains(&ws.join("tools")), "{pinned:?}");
    let nodes = named("ANCESTOR_NODE_");
    assert!(nodes.contains(&ws.join("tools/git")), "{nodes:?}");
    assert!(!nodes.contains(&ws.join("tools")), "{nodes:?}");
    let _ = std::fs::remove_dir_all(&base);
}

/// A protected file with a hard-link alias made before the profile is refused at the seam — the
/// alias is a second name under an allowed path for the same inode, and no path rule can tell
/// them apart — and the command is left untouched; without the alias the same policy wraps. The
/// check has one owner: with the alias in place the backend's own `wrap` still wraps, so a second
/// copy of it here would show up as a refusal.
#[test]
fn the_seam_not_the_backend_refuses_a_hard_linked_protected_file() {
    let base = dunce::canonicalize(scratch_dir("hard-link")).expect("canonical scratch");
    let ws = base.join("ws");
    std::fs::create_dir_all(ws.join(".git")).expect("mkdir");
    let config = ws.join(".git/config");
    std::fs::write(&config, "[core]\n").expect("config");
    let mut policy = fixture_policy();
    policy.write_roots = vec![ws.clone()];
    policy.protected = vec![protected_path(config.clone())];
    let mut cmd = tokio::process::Command::new("/usr/bin/true");
    wrap_through_the_seam(&mut cmd, &original_true(&ws), &policy)
        .expect("one link: wraps")
        .expect("enforce wraps");
    assert_eq!(Path::new(SANDBOX_EXEC), cmd.as_std().get_program());

    std::fs::hard_link(&config, ws.join("alias")).expect("ln");
    let mut cmd = tokio::process::Command::new("/usr/bin/true");
    cmd.env("GROK_W1_MARKER", "1");
    let err = wrap_through_the_seam(&mut cmd, &original_true(&ws), &policy)
        .expect_err("an alias refuses the wrap");
    assert!(
        matches!(
            &err,
            SandboxCommandError::Policy(PolicyError::HardLinkedProtected { path, nlink: 2, alias })
                if *path == config && *alias == ws.join("alias")
        ),
        "{err}"
    );
    assert_eq!(Path::new("/usr/bin/true"), cmd.as_std().get_program());
    assert!(
        cmd.as_std().get_envs().any(|(k, _)| k == "GROK_W1_MARKER"),
        "a refused wrap leaves the command untouched"
    );

    let mut cmd = tokio::process::Command::new("/usr/bin/true");
    SeatbeltBackend::new()
        .wrap(&mut cmd, &original_true(&ws), &policy, &tag())
        .expect("the backend does not repeat the seam's check");
    assert_eq!(Path::new(SANDBOX_EXEC), cmd.as_std().get_program());
    let _ = std::fs::remove_dir_all(&base);
}

/// A protected file with a link outside every write root and a write root too large to search
/// refuses at the seam as unverified, naming the root, not as an alias that was found.
#[test]
fn a_write_root_too_large_to_search_refuses_as_unverified() {
    let base = dunce::canonicalize(scratch_dir("hard-link-unsearched")).expect("canonical scratch");
    let (ws, home) = (base.join("ws"), base.join("home"));
    std::fs::create_dir_all(&ws).expect("mkdir ws");
    for index in 0..=protected::HARD_LINK_SCAN_LIMIT {
        std::fs::write(ws.join(format!("f{index}")), "").expect("fill ws");
    }
    std::fs::create_dir_all(home.join("dotfiles")).expect("mkdir home");
    std::fs::write(home.join(".bashrc"), "").expect("bashrc");
    std::fs::hard_link(home.join(".bashrc"), home.join("dotfiles/bashrc")).expect("ln");
    let mut policy = fixture_policy();
    policy.write_roots = vec![ws.clone()];
    policy.protected = vec![protected_path(home.join(".bashrc"))];
    let mut cmd = tokio::process::Command::new("/usr/bin/true");
    let err = wrap_through_the_seam(&mut cmd, &original_true(&ws), &policy)
        .expect_err("an unsearched root refuses the wrap");
    assert!(
        matches!(
            &err,
            SandboxCommandError::Policy(PolicyError::HardLinkUnverified { path, root })
                if *path == home.join(".bashrc") && *root == ws
        ),
        "{err}"
    );
    let _ = std::fs::remove_dir_all(&base);
}

/// An enrolment that fails (`/usr/bin/true` already gone, `ESRCH`) is no verdict on
/// `sandbox-exec`: the probe still reads the child's exit.
#[test]
fn a_failed_enrolment_does_not_fail_the_probe() {
    run_probe(sandbox_exec_probe(), |_| {
        Err::<(), _>(std::io::Error::from_raw_os_error(libc::ESRCH))
    })
    .expect("the verdict is the child's exit alone");
}

/// The probe's verdict names what happened: a child that exits non-zero is reported with its
/// status and what it wrote to stderr (a managed host's `sandbox_apply` refusal says why there),
/// one that never exits is killed and reported as such, and a program that cannot be spawned as
/// the spawn error — the three ways `sandbox_exec_ok` can be false.
#[test]
fn a_failed_probe_reports_the_exit_status_and_stderr_or_the_timeout() {
    let enroll = |_: &std::process::Child| Ok::<(), std::io::Error>(());
    let mut refused = std::process::Command::new("/bin/sh");
    refused
        .args([
            "-c",
            "echo 'sandbox_apply: Operation not permitted' >&2; exit 71",
        ])
        .stdin(xai_tty_utils::null_stdio())
        .stdout(xai_tty_utils::null_stdio())
        .stderr(std::process::Stdio::piped());
    let failure = run_probe(refused, enroll).expect_err("exit 71 fails the probe");
    assert!(
        matches!(&failure, ProbeFailure::Exit { status, stderr }
            if status.code() == Some(71) && stderr == "sandbox_apply: Operation not permitted"),
        "{failure:?}"
    );
    let text = failure.to_string();
    assert!(
        text.contains("71") && text.contains("sandbox_apply"),
        "{text}"
    );

    let mut missing = std::process::Command::new("/nonexistent/sandbox-exec");
    missing.stderr(std::process::Stdio::piped());
    let failure = run_probe(missing, enroll).expect_err("no binary fails the probe");
    assert!(
        matches!(&failure, ProbeFailure::Spawn(error) if error.kind() == std::io::ErrorKind::NotFound),
        "{failure:?}"
    );
}

/// The probe command itself: `sandbox-exec` by absolute path, the trivial profile, `/usr/bin/true`,
/// stderr captured for the verdict, stdin and stdout not opened by path.
#[test]
fn the_probe_command_is_sandbox_exec_with_the_trivial_profile_and_captured_stderr() {
    let cmd = sandbox_exec_probe();
    assert_eq!(Path::new(SANDBOX_EXEC), cmd.get_program());
    let args: Vec<&std::ffi::OsStr> = cmd.get_args().collect();
    assert_eq!(
        vec![
            std::ffi::OsStr::new("-p"),
            std::ffi::OsStr::new(TRIVIAL_PROFILE),
            std::ffi::OsStr::new("/usr/bin/true"),
        ],
        args
    );
}

/// The macOS probe for the `file-link` deny: under the real profile, `ln` of a
/// protected file into the writable root is refused and the file keeps its one link, so no alias
/// can be made from inside for a later write to reach it through an allowed path.
#[tokio::test]
async fn sandbox_exec_denies_a_hard_link_to_a_protected_file() {
    use std::os::unix::fs::MetadataExt as _;
    let base = dunce::canonicalize(scratch_dir("file-link")).expect("canonical scratch");
    let ws = base.join("ws");
    std::fs::create_dir_all(ws.join(".git")).expect("mkdir");
    let config = ws.join(".git/config");
    std::fs::write(&config, "[core]\n").expect("config");
    let policy = policy_for(vec![ws.clone()], vec![protected_path(config.clone())]);
    let alias = ws.join("config-alias");
    let mut cmd = tokio::process::Command::new("/bin/ln");
    let original = OriginalArgv {
        program: PathBuf::from("/bin/ln"),
        args: vec![
            config.clone().into_os_string(),
            alias.clone().into_os_string(),
        ],
        cwd: ws.clone(),
    };
    SeatbeltBackend::new()
        .wrap(&mut cmd, &original, &policy, &tag())
        .expect("wrap");
    let output = cmd.output().await.expect("spawn ln");
    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Operation not permitted"),
        "{output:?}"
    );
    assert!(!alias.exists(), "the alias must not be created");
    assert_eq!(
        1,
        std::fs::metadata(&config).expect("config").nlink(),
        "the protected file keeps its one link"
    );
    let _ = std::fs::remove_dir_all(&base);
}

/// The macOS probe for the glob prefix pin: under the real profile, `mv .git/modules` aside is
/// refused, so hooks cannot be written under another name and moved back into the pattern.
#[tokio::test]
async fn sandbox_exec_refuses_renaming_a_glob_entrys_literal_prefix() {
    let base = dunce::canonicalize(scratch_dir("glob-rename")).expect("canonical scratch");
    let ws = base.join("ws");
    let modules = ws.join(".git/modules");
    std::fs::create_dir_all(modules.join("lib")).expect("mkdir");
    let policy = policy_for(
        vec![ws.clone()],
        vec![Protected::Glob {
            glob: format!("{}/**/hooks", modules.display()),
        }],
    );
    let aside = ws.join(".git/modules-aside");
    let mut cmd = tokio::process::Command::new("/bin/mv");
    let original = OriginalArgv {
        program: PathBuf::from("/bin/mv"),
        args: vec![
            modules.clone().into_os_string(),
            aside.clone().into_os_string(),
        ],
        cwd: ws.clone(),
    };
    SeatbeltBackend::new()
        .wrap(&mut cmd, &original, &policy, &tag())
        .expect("wrap");
    let output = cmd.output().await.expect("spawn mv");
    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Operation not permitted"),
        "{output:?}"
    );
    assert!(modules.join("lib").is_dir(), "the prefix stays in place");
    assert!(!aside.exists(), "nothing was moved aside");
    let _ = std::fs::remove_dir_all(&base);
}

/// A policy that writes only `ws` and protects nothing, so the `.git` node rule alone decides.
fn git_node_policy(ws: &Path) -> SandboxPolicy {
    policy_for(vec![ws.to_path_buf()], Vec::new())
}

/// The one shape every filesystem test starts from: default read, no network, default env,
/// these write roots and floor entries; a test overrides the rest with `..`.
fn policy_for(write_roots: Vec<PathBuf>, protected: Vec<Protected>) -> SandboxPolicy {
    SandboxPolicy {
        read: ReadPolicy::AllExcept { deny: Vec::new() },
        write_roots,
        network: NetworkPolicy::Off,
        env: EnvPolicy::default(),
        protected,
        build_cache_trees: Vec::new(),
        unread_git_metadata: Vec::new(),
    }
}

async fn run_wrapped(
    policy: &SandboxPolicy,
    cwd: &Path,
    program: &str,
    args: &[&str],
) -> std::process::Output {
    let mut cmd = tokio::process::Command::new(program);
    let original = OriginalArgv {
        program: PathBuf::from(program),
        args: args.iter().map(OsString::from).collect(),
        cwd: cwd.to_path_buf(),
    };
    SeatbeltBackend::new()
        .wrap(&mut cmd, &original, policy, &tag())
        .expect("wrap");
    cmd.output().await.expect("spawn the wrapped command")
}

fn git_init(ws: &Path) {
    let status = std::process::Command::new("/usr/bin/git")
        .args(["init", "-q"])
        .current_dir(ws)
        .status()
        .expect("git init");
    assert!(status.success(), "git init outside the sandbox");
}

/// The macOS probe for the `.git` node: in a workspace with no `.git`, a staged tree cannot be
/// moved in as `.git`, so its `hooks/pre-commit` never lands where git would run it.
#[tokio::test]
async fn sandbox_exec_refuses_moving_a_staged_tree_in_as_git() {
    let base = dunce::canonicalize(scratch_dir("git-node-in")).expect("canonical scratch");
    let ws = base.join("ws");
    std::fs::create_dir_all(ws.join("stage/hooks")).expect("mkdir");
    std::fs::write(ws.join("stage/hooks/pre-commit"), "#!/bin/sh\n").expect("hook");
    let output = run_wrapped(&git_node_policy(&ws), &ws, "/bin/mv", &["stage", ".git"]).await;
    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Operation not permitted"),
        "{output:?}"
    );
    assert!(!ws.join(".git").exists(), "nothing was moved in as .git");
    assert!(ws.join("stage/hooks/pre-commit").is_file());
    let _ = std::fs::remove_dir_all(&base);
}

/// The macOS probe for the `.git` node: in a real repository the git directory cannot be moved
/// away (its protected entries would go with it and a fresh `.git` could be planted).
#[tokio::test]
async fn sandbox_exec_refuses_moving_the_git_directory_away() {
    let base = dunce::canonicalize(scratch_dir("git-node-out")).expect("canonical scratch");
    let ws = base.join("ws");
    std::fs::create_dir_all(&ws).expect("mkdir");
    git_init(&ws);
    let output = run_wrapped(&git_node_policy(&ws), &ws, "/bin/mv", &[".git", "old"]).await;
    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Operation not permitted"),
        "{output:?}"
    );
    assert!(ws.join(".git/HEAD").is_file(), "the git directory stays");
    assert!(!ws.join("old").exists());
    let _ = std::fs::remove_dir_all(&base);
}

/// The macOS probe for a missing glob prefix: in a repository with no submodules yet, a staged
/// tree cannot be moved in as `.git/modules` with hooks already under it.
#[tokio::test]
async fn sandbox_exec_refuses_moving_a_staged_tree_in_as_a_missing_glob_prefix() {
    let base = dunce::canonicalize(scratch_dir("glob-prefix-in")).expect("canonical scratch");
    let ws = base.join("ws");
    std::fs::create_dir_all(ws.join("staged/lib/hooks")).expect("mkdir");
    git_init(&ws);
    let modules = ws.join(".git/modules");
    let policy = SandboxPolicy {
        protected: vec![Protected::Glob {
            glob: format!("{}/**/hooks", modules.display()),
        }],
        ..git_node_policy(&ws)
    };
    let output = run_wrapped(&policy, &ws, "/bin/mv", &["staged", ".git/modules"]).await;
    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Operation not permitted"),
        "{output:?}"
    );
    assert!(!modules.exists(), "nothing was moved in as .git/modules");
    let _ = std::fs::remove_dir_all(&base);
}

/// The macOS probe for a missing ancestor: under a `$HOME` grant with no `~/.config/git`, a staged
/// tree cannot be moved in as it, so its `config` never lands where git reads it; an unrelated
/// directory beside it is still created.
#[tokio::test]
async fn sandbox_exec_refuses_moving_a_staged_tree_in_as_a_missing_protected_parent() {
    let base = dunce::canonicalize(scratch_dir("ancestor-in")).expect("canonical scratch");
    let home = base.join("home");
    std::fs::create_dir_all(home.join(".config")).expect("mkdir");
    std::fs::create_dir_all(home.join("stage")).expect("mkdir");
    std::fs::write(
        home.join("stage/config"),
        "[core]\n\tfsmonitor = /usr/bin/false\n",
    )
    .expect("config");
    let policy = SandboxPolicy {
        protected: vec![protected_path(home.join(".config/git/config"))],
        ..git_node_policy(&home)
    };
    let output = run_wrapped(&policy, &home, "/bin/mv", &["stage", ".config/git"]).await;
    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Operation not permitted"),
        "{output:?}"
    );
    assert!(!home.join(".config/git").exists(), "nothing was moved in");
    let output = run_wrapped(&policy, &home, "/bin/mkdir", &[".config/other"]).await;
    assert!(output.status.success(), "{output:?}");
    let _ = std::fs::remove_dir_all(&base);
}

/// The macOS probe for the pin past a gap: with `~/.config/git` missing, `~/.config` itself cannot
/// be moved away for a replacement holding `git/config` to be moved in after it.
#[tokio::test]
async fn sandbox_exec_refuses_moving_an_existing_parent_away_past_a_missing_one() {
    let base = dunce::canonicalize(scratch_dir("ancestor-out")).expect("canonical scratch");
    let home = base.join("home");
    std::fs::create_dir_all(home.join(".config")).expect("mkdir");
    let policy = SandboxPolicy {
        protected: vec![protected_path(home.join(".config/git/config"))],
        ..git_node_policy(&home)
    };
    let output = run_wrapped(&policy, &home, "/bin/mv", &[".config", "config-aside"]).await;
    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Operation not permitted"),
        "{output:?}"
    );
    assert!(home.join(".config").is_dir(), "the parent stays in place");
    assert!(!home.join("config-aside").exists());
    let _ = std::fs::remove_dir_all(&base);
}

/// The `.git` node deny costs nothing inside the directory: under the workspace's real floor,
/// `git status`, `git add` and `git commit` in an existing repository all succeed.
#[tokio::test]
async fn git_status_and_commit_work_inside_an_existing_repository() {
    let base = dunce::canonicalize(scratch_dir("git-node-inside")).expect("canonical scratch");
    let ws = base.join("ws");
    let grok_home = base.join("grok-home");
    std::fs::create_dir_all(&ws).expect("mkdir");
    std::fs::create_dir_all(&grok_home).expect("mkdir");
    git_init(&ws);
    std::fs::write(ws.join("file.txt"), "sandboxed\n").expect("file");
    let tmp = dunce::canonicalize(std::env::temp_dir()).expect("canonical temp dir");
    let policy = SandboxPolicy {
        write_roots: vec![ws.clone(), tmp],
        protected: protected::floor(&ProtectedInputs {
            workspace_root: &ServedRoot::pin(&ws),
            grok_home: &grok_home,
            user_home: Some(&base.join("home")),
            control_socket_dir: &grok_home.join("workspaced"),
            git_env: &GitConfigEnv::default(),
        }),
        ..git_node_policy(&ws)
    };
    let script = "git status --porcelain \
        && git add file.txt \
        && git -c user.name=grok -c user.email=grok@example.invalid -c commit.gpgsign=false \
           commit -q -m sandboxed-commit \
        && git log --oneline -1";
    let output = run_wrapped(&policy, &ws, "/bin/sh", &["-c", script]).await;
    assert!(output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("sandboxed-commit"),
        "{output:?}"
    );
    let _ = std::fs::remove_dir_all(&base);
}

fn allow_grant(subject: GrantSubject) -> Grant {
    Grant {
        id: GrantId::new("0192c1a0-0000-7000-8000-00000000b001"),
        subject,
        scope: GrantScope::Session,
        expires: Expiry::Never,
        decision: GrantDecision::Allow,
        granted_at: 0,
        granted_by: "test".to_owned(),
        via: None,
    }
}

/// A policy writing `ws` whose floor names `<home>/.kube/config`, with a write grant for the
/// missing `<home>/.kube` applied: the grant is left out until the directory exists.
fn missing_kube_grant_policy(ws: &Path, kube: &Path) -> SandboxPolicy {
    SandboxPolicy {
        protected: vec![protected_path(kube.join("config"))],
        ..git_node_policy(ws)
    }
    .with_grant(&allow_grant(GrantSubject::FsWriteRoot {
        root: kube.to_path_buf(),
    }))
    .expect("the grant is accepted and left out")
}

/// A granted write root that does not exist yet and holds a floor entry (`~/.kube` before its
/// `config`) is not rendered: nothing makes it writable, so it cannot be created into place.
#[test]
fn a_granted_but_missing_root_holding_a_floor_entry_is_not_rendered() {
    let base = dunce::canonicalize(scratch_dir("missing-grant-render")).expect("canonical scratch");
    let ws = base.join("ws");
    let kube = base.join("user-home/.kube");
    std::fs::create_dir_all(&ws).expect("mkdir");
    let sbpl = render_enforce(&missing_kube_grant_policy(&ws, &kube), &tag()).expect("render");
    let roots: Vec<&str> = sbpl
        .params
        .iter()
        .filter(|(name, _)| name.starts_with("WRITABLE_ROOT_"))
        .map(|(_, value)| value.as_str())
        .collect();
    assert!(
        !roots.iter().any(|root| root.ends_with("/user-home/.kube")),
        "{roots:?}"
    );
    let _ = std::fs::remove_dir_all(&base);
}

/// The macOS probe for the inert grant: while `~/.kube` is missing, a staged tree holding a
/// `config` cannot be moved in as it, although the user granted writes to it.
#[tokio::test]
async fn sandbox_exec_refuses_moving_a_staged_tree_in_as_a_granted_but_missing_root() {
    let base = dunce::canonicalize(scratch_dir("missing-grant-mv")).expect("canonical scratch");
    let ws = base.join("ws");
    let kube = base.join("user-home/.kube");
    std::fs::create_dir_all(ws.join("stage")).expect("mkdir");
    std::fs::create_dir_all(base.join("user-home")).expect("mkdir");
    std::fs::write(ws.join("stage/config"), "users: []\n").expect("config");
    let policy = missing_kube_grant_policy(&ws, &kube);
    let kube_arg = kube.to_string_lossy().into_owned();
    let output = run_wrapped(&policy, &ws, "/bin/mv", &["stage", &kube_arg]).await;
    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Operation not permitted"),
        "{output:?}"
    );
    assert!(!kube.exists(), "nothing was moved in as the missing root");
    let _ = std::fs::remove_dir_all(&base);
}

/// The macOS probe for the daemon-created session directory: once it exists (the daemon makes
/// it before the first command), a staged tree holding a grant file cannot replace it through
/// `rename(2)`, and it cannot be moved away; writing inside it still works.
#[tokio::test]
async fn sandbox_exec_refuses_replacing_or_moving_the_existing_own_session_directory() {
    let base = dunce::canonicalize(scratch_dir("own-session")).expect("canonical scratch");
    let ws = base.join("ws");
    let sessions = base.join("grok-home/sessions");
    let own = sessions.join("own");
    std::fs::create_dir_all(ws.join("stage")).expect("mkdir");
    std::fs::create_dir_all(&own).expect("mkdir");
    std::fs::write(ws.join("stage/sandbox_grants.toml"), "grants = []\n").expect("grants");
    let policy = SandboxPolicy {
        write_roots: vec![ws.clone(), own.clone()],
        protected: vec![
            Protected::TreeExcept {
                tree: sessions.clone(),
                except: own.clone(),
            },
            protected_path(own.join("sandbox_grants.toml")),
        ],
        ..git_node_policy(&ws)
    };
    let own_arg = own.to_string_lossy().into_owned();
    let replace = run_wrapped(
        &policy,
        &ws,
        "/usr/bin/perl",
        &[
            "-e",
            "rename($ARGV[0], $ARGV[1]) or die \"rename: $!\\n\"",
            "stage",
            &own_arg,
        ],
    )
    .await;
    assert!(!replace.status.success(), "{replace:?}");
    assert!(
        !own.join("sandbox_grants.toml").exists(),
        "no grant file was moved in"
    );
    let aside = ws.join("own-aside").to_string_lossy().into_owned();
    let moved = run_wrapped(&policy, &ws, "/bin/mv", &[&own_arg, &aside]).await;
    assert!(!moved.status.success(), "{moved:?}");
    assert!(own.is_dir(), "the session directory stays in place");
    let notes = own.join("notes.txt").to_string_lossy().into_owned();
    let write = run_wrapped(&policy, &ws, "/usr/bin/touch", &[&notes]).await;
    assert!(write.status.success(), "{write:?}");
    let _ = std::fs::remove_dir_all(&base);
}

/// The macOS probe for the root pins: a write root inside another write root (a verified cache
/// under a granted build-cache tree) still cannot be renamed from inside.
#[tokio::test]
async fn sandbox_exec_refuses_renaming_a_root_nested_in_a_later_root() {
    let base = dunce::canonicalize(scratch_dir("nested-root")).expect("canonical scratch");
    let parent = base.join("tree");
    let child = parent.join("cache");
    std::fs::create_dir_all(&child).expect("mkdir");
    let policy = SandboxPolicy {
        write_roots: vec![child.clone(), parent.clone()],
        ..git_node_policy(&parent)
    };
    let output = run_wrapped(&policy, &parent, "/bin/mv", &["cache", "cache-aside"]).await;
    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Operation not permitted"),
        "{output:?}"
    );
    assert!(child.is_dir(), "the nested root stays in place");
    assert!(!parent.join("cache-aside").exists());
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn the_proxy_policy_renders_the_loopback_port_and_nothing_else() {
    let mut policy = fixture_policy();
    policy.network = NetworkPolicy::Proxy { port: 8123 };
    let proxy = render_enforce(&policy, &tag()).expect("render").profile;
    assert!(proxy.contains(
        "(allow network-outbound (remote ip \"localhost:8123\") (with message \"grok-t\"))"
    ));
    // The only outbound allow is the loopback proxy port: no unix-socket allow-list, no
    // resolver (name lookups are the proxy's), nothing inbound.
    assert!(!proxy.contains("(allow network-outbound (remote unix-socket"));
    assert!(!proxy.contains("mDNSResponder"));
    assert!(!proxy.contains("DNSConfiguration"));
    assert!(!proxy.contains("(allow network-inbound"));
    assert!(!proxy.contains("(allow network-outbound (with message"));
    assert_eq!(1, proxy.matches("(allow network-outbound").count());

    policy.network = NetworkPolicy::Off;
    let off = render_enforce(&policy, &tag()).expect("render").profile;
    assert!(!off.contains("(allow network-outbound"));
}

#[test]
fn restricted_read_roots_ship_the_platform_defaults_and_skip_preferences() {
    let mut policy = fixture_policy();
    policy.read = ReadPolicy::Roots {
        roots: vec![PathBuf::from("/opt/ws-fixture/nobody-w1/proj")],
        deny: policy.read.deny().to_vec(),
    };
    let profile = render_enforce(&policy, &tag()).expect("render").profile;
    assert!(
        profile.contains("(allow file-read* file-test-existence\n  (subpath \"/Library/Apple\")")
    );
    assert!(profile.contains(
        "(allow file-read* (require-any (literal (param \"READABLE_ROOT_0\")) (subpath (param \"READABLE_ROOT_0\"))) (with message \"grok-t\"))"
    ));
    assert!(!profile.contains("(allow user-preference-read)"));
    assert!(!profile.contains("(allow file-read* (with message"));
}

/// The policy [`SandboxPolicy::build`] gives a workspace that is the home directory itself, with
/// the grok home inside it, in either read mode.
fn home_workspace_policy(home: &Path, default_read: bool) -> SandboxPolicy {
    let profile = crate::SandboxProfile {
        name: "workspace".to_owned(),
        read_only: Vec::new(),
        read_write: vec![home.to_path_buf()],
        deny: Vec::new(),
        write_deny: Vec::new(),
        default_read,
        restrict_network: false,
    };
    let grok_home = home.join(".grok");
    SandboxPolicy::build(PolicyInputs {
        workspace_root: &ServedRoot::pin(home),
        profile: &profile,
        grants: &[],
        proxy: None,
        tmp_dirs: &[],
        control_socket_dir: &grok_home.join("workspaced"),
        grok_home: &grok_home,
        user_home: Some(home),
        git_env: &GitConfigEnv::default(),
    })
    .expect("policy builds")
}

/// Every secret the policy read-denies renders as a `deny file-read*` after the last read allow
/// of either mode, the home-folder workspace under restricted roots included, so Seatbelt's last
/// match refuses the secret whatever root covers it.
#[test]
fn every_secret_read_deny_follows_every_read_allow_in_both_read_modes() {
    let home = Path::new(HOME);
    let grok_home = home.join(".grok");
    let mut wrong: Vec<String> = Vec::new();
    for default_read in [true, false] {
        let sbpl =
            render_enforce(&home_workspace_policy(home, default_read), &tag()).expect("render");
        let lines: Vec<&str> = sbpl.profile.lines().collect();
        let last_allow = lines
            .iter()
            .rposition(|line| line.contains("(allow file-read"))
            .expect("a read allow");
        let deny_line = |filter: &str| {
            lines
                .iter()
                .position(|line| line.starts_with("(deny file-read* ") && line.contains(filter))
        };
        let mut filters: Vec<(String, Option<String>)> = Vec::new();
        for rel in SECRET_READ_DENY_DIRS.iter().chain(SECRET_READ_DENY_FILES) {
            let filter = sbpl
                .params
                .iter()
                .find(|(name, value)| {
                    name.starts_with("DENY_") && Path::new(value) == home.join(rel)
                })
                .map(|(name, _)| format!("(literal (param \"{name}\"))"));
            filters.push(((*rel).to_owned(), filter));
        }
        for glob in GROK_HOME_SECRET_GLOBS {
            for spelling in resolved_spellings(&grok_home) {
                let regex = crate::deny::anchored_glob_regex(&spelling, glob).expect("utf-8");
                let filter = crate::deny::seatbelt_regex_filter(&regex);
                filters.push((format!(".grok/{glob}"), filter));
            }
        }
        for (secret, filter) in &filters {
            match filter.as_deref().and_then(deny_line) {
                Some(index) if index > last_allow => {}
                other => wrong.push(format!(
                    "default_read={default_read} {secret}: deny at {other:?}, last read allow at {last_allow}"
                )),
            }
        }
        if !default_read {
            assert!(
                sbpl.params
                    .iter()
                    .any(|(name, value)| name.starts_with("READABLE_ROOT_")
                        && Path::new(value) == home),
                "the home directory is a read root: {:?}",
                sbpl.params
            );
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// The macOS probe for the read denies under restricted roots: with the home directory itself as
/// the workspace, `cat` of an SSH key and of a grok-home auth file is refused while a file in the
/// workspace reads.
#[tokio::test]
async fn sandbox_exec_refuses_secret_reads_in_a_home_workspace_under_restricted_roots() {
    let home = dunce::canonicalize(scratch_dir("home-roots")).expect("canonical scratch");
    std::fs::create_dir_all(home.join(".ssh")).expect("mkdir .ssh");
    std::fs::create_dir_all(home.join(".grok")).expect("mkdir .grok");
    let key = home.join(".ssh/id_ed25519");
    let auth = home.join(".grok/auth.json");
    let readme = home.join("README");
    std::fs::write(&key, "fixture-key-material\n").expect("key");
    std::fs::write(&auth, "fixture-auth-material\n").expect("auth");
    std::fs::write(&readme, "fixture-readme\n").expect("readme");
    let policy = home_workspace_policy(&home, false);
    assert!(
        matches!(policy.read, ReadPolicy::Roots { .. }),
        "{:?}",
        policy.read
    );
    for secret in [&key, &auth] {
        let path = secret.to_str().expect("utf-8 scratch path");
        let output = run_wrapped(&policy, &home, "/bin/cat", &[path]).await;
        assert!(!output.status.success(), "{path}: {output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("Operation not permitted"),
            "{path}: {output:?}"
        );
        assert!(
            !String::from_utf8_lossy(&output.stdout).contains("material"),
            "{output:?}"
        );
    }
    let path = readme.to_str().expect("utf-8 scratch path");
    let output = run_wrapped(&policy, &home, "/bin/cat", &[path]).await;
    assert!(output.status.success(), "{output:?}");
    assert_eq!("fixture-readme\n", String::from_utf8_lossy(&output.stdout));
    let _ = std::fs::remove_dir_all(&home);
}

/// The macOS probe for the read-deny pins: a denied `.env` inside the workspace can be neither
/// moved aside (to be read under its new name), hard-linked, overwritten nor read, through a
/// path deny and through a glob deny alike; under the path deny its parent cannot be moved aside
/// either. A file beside it is moved, linked, read and written as before.
#[tokio::test]
async fn sandbox_exec_refuses_moving_linking_or_writing_a_read_denied_file() {
    let base = dunce::canonicalize(scratch_dir("deny-pins")).expect("canonical scratch");
    let ws = base.join("ws");
    std::fs::create_dir_all(ws.join("config")).expect("mkdir");
    let env_file = ws.join("config/.env");
    std::fs::write(&env_file, "fixture-secret-material\n").expect(".env");
    std::fs::write(ws.join("config/notes"), "fixture-notes\n").expect("notes");
    for deny in [
        DenyEntry::Path(env_file.clone()),
        DenyEntry::Glob {
            root: ws.clone(),
            tail: "**/.env".to_owned(),
        },
    ] {
        let policy = SandboxPolicy {
            read: ReadPolicy::AllExcept {
                deny: vec![deny.clone()],
            },
            ..git_node_policy(&ws)
        };
        let mut refused: Vec<(&str, Vec<&str>)> = vec![
            ("/bin/mv", vec!["config/.env", "config/aside"]),
            ("/bin/ln", vec!["config/.env", "config/alias"]),
            ("/bin/sh", vec!["-c", "echo overwritten > config/.env"]),
            ("/bin/cat", vec!["config/.env"]),
        ];
        if matches!(deny, DenyEntry::Path(_)) {
            refused.push(("/bin/mv", vec!["config", "config-aside"]));
        }
        for (program, args) in refused {
            let output = run_wrapped(&policy, &ws, program, &args).await;
            assert!(
                !output.status.success(),
                "{deny:?} {program} {args:?}: {output:?}"
            );
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("Operation not permitted"),
                "{deny:?} {program} {args:?}: {output:?}"
            );
            assert!(
                !String::from_utf8_lossy(&output.stdout).contains("material"),
                "{deny:?} {program} {args:?}: {output:?}"
            );
        }
        assert_eq!(
            "fixture-secret-material\n",
            std::fs::read_to_string(&env_file).expect("the denied file stays as it was")
        );
        for gone in ["config/aside", "config/alias", "config-aside"] {
            assert!(!ws.join(gone).exists(), "{deny:?}: {gone} must not appear");
        }
        let beside = run_wrapped(
            &policy,
            &ws,
            "/bin/sh",
            &[
                "-c",
                "mv config/notes config/notes-aside && ln config/notes-aside config/notes-alias \
                 && cat config/notes-alias && echo more >> config/notes-alias \
                 && mv config/notes-aside config/notes && rm config/notes-alias",
            ],
        )
        .await;
        assert!(beside.status.success(), "{deny:?}: {beside:?}");
        assert_eq!("fixture-notes\n", String::from_utf8_lossy(&beside.stdout));
        assert_eq!(
            "fixture-notes\nmore\n",
            std::fs::read_to_string(ws.join("config/notes")).expect("notes")
        );
        std::fs::write(ws.join("config/notes"), "fixture-notes\n").expect("reset notes");
    }
    let _ = std::fs::remove_dir_all(&base);
}

/// A deny glob anchored at a workspace whose own name holds `[` renders the root escaped as a
/// literal, so the regex matches the real directory instead of a character class.
#[test]
fn a_deny_glob_below_a_bracketed_root_escapes_the_root() {
    let mut policy = fixture_policy();
    policy.read = ReadPolicy::AllExcept {
        deny: vec![DenyEntry::Glob {
            root: PathBuf::from("/opt/ws-fixture/app[v2]"),
            tail: ".env*".to_owned(),
        }],
    };
    let sbpl = render_enforce(&policy, &tag()).expect("render");
    assert!(
        sbpl.profile
            .contains("(regex #\"^/opt/ws-fixture/app\\[v2\\]/\\.env[^/]*$\")"),
        "{}",
        sbpl.profile
    );
}

#[test]
fn relative_deny_glob_and_unsafe_tag_are_refused() {
    let mut policy = fixture_policy();
    policy.read = ReadPolicy::AllExcept {
        deny: vec![DenyEntry::Glob {
            root: PathBuf::from("relative"),
            tail: "**/.env".to_owned(),
        }],
    };
    let err = render_enforce(&policy, &tag()).expect_err("relative glob");
    assert!(
        matches!(&err, SandboxCommandError::Unrenderable { backend: BackendName::Seatbelt, reason } if reason.contains("must be absolute")),
        "{err}"
    );
    let err = render_enforce(
        &fixture_policy(),
        &CommandTag::for_call(&CallId::tool("x\")(allow default)(deny")),
    )
    .expect_err("tag");
    assert!(
        matches!(err, SandboxCommandError::Unrenderable { .. }),
        "{err}"
    );
}

/// `wrap` swaps in `sandbox-exec` and carries the command's environment as the seam left it,
/// filtering nothing itself: an explicit name the policy excludes stays, a removal stays a
/// removal, the policy's `set` entries are not added. The environment has one owner,
/// `wrap_for_mode`; through it the same command loses the excluded name and gains the proxy
/// pointer, and a second filter here would show up as a name missing from the first half.
#[test]
fn wrap_swaps_in_sandbox_exec_and_carries_the_environment_unfiltered() {
    let command = || {
        let mut cmd = tokio::process::Command::new("/bin/echo");
        cmd.arg("ignored-original-arg");
        cmd.env("GROK_W1_SECRET_TOKEN", "leak");
        cmd.env("GROK_W1_KEEP", "1");
        cmd.env_remove("GROK_W1_REMOVED");
        cmd
    };
    let original = OriginalArgv {
        program: PathBuf::from("/bin/echo"),
        args: vec![OsString::from("hi")],
        cwd: PathBuf::from("/"),
    };
    let mut policy = fixture_policy();
    policy
        .env
        .set
        .insert("HTTPS_PROXY".to_owned(), "http://127.0.0.1:8123".to_owned());
    let mut cmd = command();
    let receipt = SeatbeltBackend::new()
        .wrap(&mut cmd, &original, &policy, &tag())
        .expect("wrap");
    assert_eq!(BackendName::Seatbelt, receipt.backend);
    let std_cmd = cmd.as_std();
    assert_eq!(Path::new(SANDBOX_EXEC), std_cmd.get_program());
    let args: Vec<&std::ffi::OsStr> = std_cmd.get_args().collect();
    assert_eq!(Some(&std::ffi::OsStr::new("-p")), args.first());
    let separator = args
        .iter()
        .position(|a| *a == "--")
        .expect("argv separator");
    assert_eq!(
        vec!["/bin/echo", "hi"],
        args.get(separator + 1..)
            .expect("program after separator")
            .iter()
            .map(|a| a.to_str().unwrap_or_default())
            .collect::<Vec<_>>()
    );
    assert!(
        args.iter().any(|a| a
            .to_str()
            .is_some_and(|s| s.starts_with("WRITABLE_ROOT_0="))),
        "-D params present"
    );
    let env = envs_of(std_cmd);
    assert_eq!(
        Some(&Some("leak".to_owned())),
        env.get("GROK_W1_SECRET_TOKEN"),
        "the backend filters nothing: {env:?}"
    );
    assert_eq!(Some(&Some("1".to_owned())), env.get("GROK_W1_KEEP"));
    assert_eq!(
        Some(&None),
        env.get("GROK_W1_REMOVED"),
        "a removal is carried as a removal: {env:?}"
    );
    assert_eq!(
        None,
        env.get("HTTPS_PROXY"),
        "the policy's set entries are the seam's to add: {env:?}"
    );
    assert_eq!(Some(Path::new("/")), std_cmd.get_current_dir());

    let mut cmd = command();
    wrap_through_the_seam(&mut cmd, &original, &policy)
        .expect("seam")
        .expect("enforce wraps");
    let env = envs_of(cmd.as_std());
    assert_eq!(
        Some(&None),
        env.get("GROK_W1_SECRET_TOKEN"),
        "the seam removes the excluded name: {env:?}"
    );
    assert_eq!(Some(&Some("1".to_owned())), env.get("GROK_W1_KEEP"));
    assert_eq!(Some(&None), env.get("GROK_W1_REMOVED"));
    assert_eq!(
        Some(&Some("http://127.0.0.1:8123".to_owned())),
        env.get("HTTPS_PROXY")
    );
}

#[test]
fn writable_root_with_a_symlink_below_the_top_component_is_refused() {
    let base = scratch_dir("symlinked-root");
    let real = base.join("real");
    std::fs::create_dir_all(real.join("ws")).expect("mkdir");
    let link = base.join("link");
    std::os::unix::fs::symlink(&real, &link).expect("symlink");
    let mut policy = fixture_policy();
    policy.write_roots = vec![link.join("ws")];
    let mut cmd = tokio::process::Command::new("/usr/bin/true");
    let err = SeatbeltBackend::new()
        .wrap(&mut cmd, &original_true(&base), &policy, &tag())
        .expect_err("symlinked root");
    assert!(
        matches!(
            err,
            SandboxCommandError::Policy(PolicyError::SymlinkedRoot { .. })
        ),
        "{err}"
    );
    assert_eq!(Path::new("/usr/bin/true"), cmd.as_std().get_program());
    let _ = std::fs::remove_dir_all(&base);
}

/// A root component that cannot be inspected (its parent denies search, `EACCES`) might be a
/// link, so the wrap is refused with the same [`PolicyError`] a grant meets
/// (`symlink_free_spelling` is the one walk); a root that does not exist yet has nothing to
/// follow and wraps.
#[test]
fn a_root_whose_components_cannot_be_inspected_is_refused_and_a_missing_one_is_not() {
    use std::os::unix::fs::PermissionsExt as _;
    let base = dunce::canonicalize(scratch_dir("uninspectable-root")).expect("canonical scratch");
    let locked = base.join("locked");
    std::fs::create_dir_all(locked.join("ws")).expect("mkdir");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).expect("chmod");
    let mut policy = fixture_policy();
    policy.write_roots = vec![locked.join("ws")];
    let mut cmd = tokio::process::Command::new("/usr/bin/true");
    let refused = SeatbeltBackend::new().wrap(&mut cmd, &original_true(&base), &policy, &tag());
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    assert!(
        matches!(
            refused,
            Err(SandboxCommandError::Policy(
                PolicyError::UninspectableRoot { .. }
            ))
        ),
        "the policy's own refusal, not a rendering limit: {refused:?}"
    );
    assert_eq!(Path::new("/usr/bin/true"), cmd.as_std().get_program());

    policy.write_roots = vec![base.join("missing/ws")];
    let mut cmd = tokio::process::Command::new("/usr/bin/true");
    SeatbeltBackend::new()
        .wrap(&mut cmd, &original_true(&base), &policy, &tag())
        .expect("a missing root wraps");
    let _ = std::fs::remove_dir_all(&base);
}

/// Real `sandbox-exec` round trip: a write inside the root succeeds, a write outside and the
/// first-time `mkdir .grok` are denied, and the verbatim base plus our generated rules parse.
#[tokio::test]
async fn sandbox_exec_round_trip_confines_writes_to_the_root() {
    let base = dunce::canonicalize(scratch_dir("round-trip")).expect("canonical scratch");
    let ws = base.join("ws");
    let outside = base.join("outside");
    std::fs::create_dir_all(&ws).expect("mkdir ws");
    std::fs::create_dir_all(&outside).expect("mkdir outside");
    let policy = policy_for(vec![ws.clone()], vec![protected_path(ws.join(".grok"))]);
    let backend = SeatbeltBackend::new();
    let inside = run_touch(&backend, &policy, &ws, &ws.join("inside.txt")).await;
    assert!(inside.status.success(), "{inside:?}");
    assert!(ws.join("inside.txt").exists());

    let denied = run_touch(&backend, &policy, &ws, &outside.join("escape.txt")).await;
    assert!(!denied.status.success(), "{denied:?}");
    assert!(
        String::from_utf8_lossy(&denied.stderr).contains("Operation not permitted"),
        "{denied:?}"
    );
    assert!(!outside.join("escape.txt").exists());

    let mut cmd = tokio::process::Command::new("/bin/mkdir");
    let original = OriginalArgv {
        program: PathBuf::from("/bin/mkdir"),
        args: vec![ws.join(".grok").into_os_string()],
        cwd: ws.clone(),
    };
    backend
        .wrap(&mut cmd, &original, &policy, &tag())
        .expect("wrap");
    let mkdir = cmd.output().await.expect("spawn mkdir");
    assert!(!mkdir.status.success(), "{mkdir:?}");
    assert!(
        !ws.join(".grok").exists(),
        "protected dir must not be created"
    );
    let _ = std::fs::remove_dir_all(&base);
}

/// The wrapper is detached by the spawn site with `detach_command` after `wrap`, as every
/// launch does; `wrap` registers no detach of its own, or the second `setsid` would fail the
/// spawn. The running command leads its own process group: the one detach reached it.
#[tokio::test]
async fn the_spawn_sites_detach_is_the_wrappers_only_one() {
    let ws = dunce::canonicalize(scratch_dir("detach")).expect("canonical scratch");
    let mut policy = fixture_policy();
    policy.write_roots = vec![ws.clone()];
    policy.protected.clear();
    let mut cmd = tokio::process::Command::new("/bin/sleep");
    let original = OriginalArgv {
        program: PathBuf::from("/bin/sleep"),
        args: vec![OsString::from("30")],
        cwd: ws.clone(),
    };
    SeatbeltBackend::new()
        .wrap(&mut cmd, &original, &policy, &tag())
        .expect("wrap");
    cmd.kill_on_drop(true);
    xai_tty_utils::detach_command(&mut cmd);
    #[allow(clippy::disallowed_methods)] // enrolled via ProcessScope::enroll below
    let mut child = cmd.spawn().expect("the detached wrapper spawns");
    let scope = xai_tty_utils::ProcessScope::new();
    let _group = scope.enroll(&child).expect("the detached child enrols");
    let pid = libc::pid_t::try_from(child.id().expect("running")).expect("pid");
    // SAFETY: `getpgid` and `getpgrp` only read the process table.
    let (group, own_group) = unsafe { (libc::getpgid(pid), libc::getpgrp()) };
    child.start_kill().expect("kill");
    let _ = child.wait().await;
    assert_eq!(pid, group, "the command leads its own process group");
    assert_ne!(own_group, group);
    let _ = std::fs::remove_dir_all(&ws);
}

/// Opens a pipe without `O_CLOEXEC` (macOS `pipe` never sets it), the inheritance these tests
/// rely on; returns `(read_end, write_end)`.
fn leak_pipe() -> (libc::c_int, libc::c_int) {
    let mut pipe_fds = [0 as libc::c_int; 2];
    // SAFETY: `pipe` writes two descriptors into the two-element array.
    assert_eq!(0, unsafe { libc::pipe(pipe_fds.as_mut_ptr()) });
    let [read_end, write_end] = pipe_fds;
    (read_end, write_end)
}

fn close_pipe((read_end, write_end): (libc::c_int, libc::c_int)) {
    // SAFETY: closing the two descriptors `leak_pipe` opened.
    unsafe {
        libc::close(read_end);
        libc::close(write_end);
    }
}

/// A descriptor number far above the table's top at `wrap` time: a sweep bounded by what the
/// parent saw then, plus any margin, would miss a descriptor placed here between `wrap` and
/// the spawn. The probe lists up to [`FD_PROBE_END`].
const FAR_FD: libc::c_int = 3000;
const FD_PROBE_END: libc::c_int = 3100;

/// Places a second name for `fd` at `target` without close-on-exec (`dup2` clears it), raising
/// the soft descriptor limit past `target` first.
fn dup_far_above(fd: libc::c_int, target: libc::c_int) -> libc::c_int {
    let needed = libc::rlim_t::try_from(target).expect("fd number") + 1;
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `getrlimit` writes the two-field struct it is given.
    assert_eq!(0, unsafe {
        libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit)
    });
    if limit.rlim_cur < needed {
        assert!(
            limit.rlim_max >= needed,
            "hard descriptor limit {} below {needed}",
            limit.rlim_max
        );
        limit.rlim_cur = needed;
        // SAFETY: raises the soft limit within the hard limit; the struct is fully initialised.
        assert_eq!(
            0,
            unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) },
            "raise RLIMIT_NOFILE to {needed}: {}",
            std::io::Error::last_os_error()
        );
    }
    // SAFETY: `dup2` on two integer descriptors.
    assert_eq!(target, unsafe { libc::dup2(fd, target) });
    target
}

fn close_fd(fd: libc::c_int) {
    // SAFETY: closing a descriptor this test placed.
    unsafe {
        libc::close(fd);
    }
}

/// A pipe leaked before `wrap` is visible to an ordinary child and must be gone under the
/// wrapper; so must descriptors opened between `wrap` and the spawn, one of them placed far above
/// the table's top at `wrap` time: the hook lists the child's own table after `fork`, so nothing
/// the parent opened in the meantime, at whatever number, is missed. The child-side listing
/// must see every leaked descriptor.
#[tokio::test]
async fn wrapped_command_inherits_no_descriptor_above_stdio() {
    let before_wrap = leak_pipe();
    assert!(
        before_wrap.0 < FAR_FD && before_wrap.1 < FAR_FD,
        "the pipe {before_wrap:?} must sit below the far dup at {FAR_FD}, inside the probe range"
    );
    let far_before_wrap = dup_far_above(before_wrap.0, FAR_FD);
    let mut entries = [libc::proc_fdinfo {
        proc_fd: 0,
        proc_fdtype: 0,
    }; FD_LIST_CAPACITY];
    let listed: Vec<libc::c_int> = list_open_fds(&mut entries)
        .expect("proc_pidinfo lists this process")
        .iter()
        .map(|entry| entry.proc_fd)
        .collect();
    for fd in [before_wrap.0, before_wrap.1, far_before_wrap] {
        assert!(
            listed.contains(&fd),
            "listing must include {fd}: {listed:?}"
        );
    }

    let control = tokio::process::Command::new("/bin/sh")
        .args(["-c", &fd_probe()])
        .output()
        .await
        .expect("control child");
    let control = String::from_utf8_lossy(&control.stdout).trim().to_owned();
    for fd in [before_wrap.0, far_before_wrap] {
        assert!(
            control.contains(&format!(" {fd}")),
            "control child must see the leaked descriptor {fd}: {control}"
        );
    }
    // the table's top is low again when `wrap` runs
    close_fd(far_before_wrap);

    let ws = dunce::canonicalize(scratch_dir("fd-sweep")).expect("canonical scratch");
    let mut policy = fixture_policy();
    policy.write_roots = vec![ws.clone()];
    policy.protected.clear();
    assert_eq!(
        "open-above-stdio:[]",
        run_wrapped_probe(&policy, &ws, || {}).await,
        "pipe leaked before wrap"
    );
    let mut late: Option<((libc::c_int, libc::c_int), libc::c_int)> = None;
    let probed = run_wrapped_probe(&policy, &ws, || {
        let pipe = leak_pipe();
        late = Some((pipe, dup_far_above(pipe.0, FAR_FD)));
    })
    .await;
    let (late_pipe, late_far) = late.expect("the leak ran after wrap");
    close_pipe(late_pipe);
    close_fd(late_far);
    assert_eq!(
        "open-above-stdio:[]", probed,
        "descriptors opened between wrap and spawn, one at {FAR_FD}"
    );

    close_pipe(before_wrap);
    let _ = std::fs::remove_dir_all(&ws);
}

/// Lists every open descriptor in `3..=FD_PROBE_END` as seen by the child.
fn fd_probe() -> String {
    format!(
        "o=\"\"; for fd in $(/usr/bin/seq 3 {FD_PROBE_END}); do test -e /dev/fd/$fd && o=\"$o $fd\"; done; echo \"open-above-stdio:[$o]\""
    )
}

/// Wraps `/bin/sh -c <fd probe>` and returns its stdout. `after_wrap` runs after `wrap` and
/// before the spawn, where a parent-side enumeration could no longer see what it opens.
async fn run_wrapped_probe(policy: &SandboxPolicy, ws: &Path, after_wrap: impl FnOnce()) -> String {
    let mut cmd = tokio::process::Command::new("/bin/sh");
    let original = OriginalArgv {
        program: PathBuf::from("/bin/sh"),
        args: vec![OsString::from("-c"), OsString::from(fd_probe())],
        cwd: ws.to_path_buf(),
    };
    SeatbeltBackend::new()
        .wrap(&mut cmd, &original, policy, &tag())
        .expect("wrap");
    after_wrap();
    let output = cmd.output().await.expect("wrapped child");
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

async fn run_touch(
    backend: &SeatbeltBackend,
    policy: &SandboxPolicy,
    cwd: &Path,
    target: &Path,
) -> std::process::Output {
    let mut cmd = tokio::process::Command::new("/usr/bin/touch");
    let original = OriginalArgv {
        program: PathBuf::from("/usr/bin/touch"),
        args: vec![target.as_os_str().to_owned()],
        cwd: cwd.to_path_buf(),
    };
    backend
        .wrap(&mut cmd, &original, policy, &tag())
        .expect("wrap");
    cmd.output().await.expect("spawn touch")
}

fn original_true(cwd: &Path) -> OriginalArgv {
    OriginalArgv {
        program: PathBuf::from("/usr/bin/true"),
        args: Vec::new(),
        cwd: cwd.to_path_buf(),
    }
}

fn scratch_dir(label: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!(
        "grok-seatbelt-{label}-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}
