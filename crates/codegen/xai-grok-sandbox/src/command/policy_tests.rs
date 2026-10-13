use super::*;
use crate::SandboxProfile;
use crate::command::grant_store::allows_not_denied;
use crate::command::grants::GrantSubject;
use crate::command::grants::{Expiry, GrantDecision, GrantId, GrantScope, HostPattern};
use crate::command::violation::Blocked;

fn profile(ws: &Path, grok_home: &Path) -> SandboxProfile {
    SandboxProfile {
        name: "workspace".to_owned(),
        read_only: vec![],
        read_write: vec![ws.to_path_buf(), grok_home.to_path_buf()],
        deny: vec![PathBuf::from("/opt/secret"), PathBuf::from("**/*.pem")],
        write_deny: vec![],
        default_read: true,
        restrict_network: false,
    }
}

fn grant(subject: GrantSubject) -> Grant {
    Grant {
        id: GrantId::new("0192c1a0-0000-7000-8000-000000000002"),
        subject,
        scope: GrantScope::Session,
        expires: Expiry::Never,
        decision: GrantDecision::Allow,
        granted_at: 0,
        granted_by: "cli".to_owned(),
        via: None,
    }
}

struct Fixture {
    ws: PathBuf,
    grok_home: PathBuf,
    home: PathBuf,
    tmp: PathBuf,
    git_env: GitConfigEnv,
}

impl Fixture {
    fn new(tag: &str) -> Fixture {
        let root =
            std::env::temp_dir().join(format!("xai-sandbox-policy-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        // macOS temp dirs sit behind the `/var` firmlink; compare canonical paths throughout
        let root = dunce::canonicalize(&root).unwrap();
        let ws = root.join("ws");
        let grok_home = root.join("home").join(".grok");
        let home = root.join("home");
        let tmp = root.join("tmp");
        let own_session = xai_grok_config::sessions_cwd_dir_in(&grok_home, &ws.to_string_lossy());
        for dir in [&ws, &grok_home, &tmp, &own_session.join("commands")] {
            std::fs::create_dir_all(dir).unwrap();
        }
        Fixture {
            ws,
            grok_home,
            home,
            tmp,
            git_env: GitConfigEnv::default(),
        }
    }

    fn build(&self, grants: &[Grant], proxy: Option<ProxyEndpoint>) -> SandboxPolicy {
        self.build_with_home(grants, proxy, Some(&self.home))
    }

    /// A host with no home directory: no cache table applies.
    fn build_without_home(&self) -> SandboxPolicy {
        self.build_with_home(&[], None, None)
    }

    fn build_with_home(
        &self,
        grants: &[Grant],
        proxy: Option<ProxyEndpoint>,
        user_home: Option<&Path>,
    ) -> SandboxPolicy {
        let profile = profile(&self.ws, &self.grok_home);
        let tmp_dirs = [ServedRoot::pin(&self.tmp)];
        SandboxPolicy::build(PolicyInputs {
            workspace_root: &ServedRoot::pin(&self.ws),
            profile: &profile,
            grants,
            proxy,
            tmp_dirs: &tmp_dirs,
            control_socket_dir: &self.grok_home.join("workspaced"),
            grok_home: &self.grok_home,
            user_home,
            git_env: &self.git_env,
        })
        .expect("policy builds")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(root) = self.ws.parent() {
            let _ = std::fs::remove_dir_all(root);
        }
    }
}

/// A git file the floor reads for `core.hooksPath` and cannot (here `.git/config` past the read
/// limit) rides on the policy as `unread_git_metadata`, in observe as in enforce, so
/// `wrap_for_mode` can refuse to enforce; the floor keeps every entry the layout gave, and a
/// readable config leaves the list empty. The list survives the policy's JSON round trip and an
/// older policy without the field reads back empty.
#[test]
fn a_git_file_the_floor_could_not_read_rides_on_the_policy() {
    let f = Fixture::new("unread-git");
    std::fs::create_dir_all(f.ws.join(".git")).unwrap();
    let config = f.ws.join(".git/config");
    std::fs::write(&config, "[core]\n\thooksPath = tools/hooks\n").unwrap();
    let policy = f.build(&[], None);
    assert!(policy.unread_git_metadata.is_empty(), "{policy:?}");
    assert!(policy.is_protected(&f.ws.join("tools/hooks")));

    let mut oversized = b"[core]\n\thooksPath = tools/hooks\n".to_vec();
    oversized.extend(std::iter::repeat_n(b'#', 1024 * 1024));
    std::fs::write(&config, oversized).unwrap();
    let policy = f.build(&[], None);
    assert_eq!(
        vec![crate::command::git_config::GitMetadataUnread::TooLarge {
            path: config.clone(),
            limit: 1024 * 1024,
        }],
        policy.unread_git_metadata
    );
    assert!(policy.is_protected(&config));
    assert!(policy.is_protected(&f.ws.join(".git/hooks/pre-commit")));
    assert!(!policy.is_protected(&f.ws.join("tools/hooks")));

    let json = serde_json::to_string(&policy).unwrap();
    let back: SandboxPolicy = serde_json::from_str(&json).unwrap();
    assert_eq!(policy, back);
    let mut without: serde_json::Value = serde_json::from_str(&json).unwrap();
    without
        .as_object_mut()
        .unwrap()
        .remove("unread_git_metadata")
        .unwrap();
    let older: SandboxPolicy = serde_json::from_value(without).unwrap();
    assert!(older.unread_git_metadata.is_empty());
}

/// The floor reads the user's global git config where the daemon's environment puts it
/// (`PolicyInputs::git_env`): `GIT_CONFIG_GLOBAL` names the one file read for `core.hooksPath`
/// and protected in place of `~/.gitconfig`, and a relative value rides on the policy as unread.
#[test]
fn the_policy_reads_the_global_git_config_where_the_daemons_environment_puts_it() {
    let mut f = Fixture::new("git-env");
    let root = f.home.parent().unwrap().to_path_buf();
    let (global, global_hooks, home_hooks) = (
        root.join("dotfiles/gitconfig"),
        root.join("global-hooks"),
        root.join("home-hooks"),
    );
    let hooks_path = |hooks: &Path| format!("[core]\n\thooksPath = {}\n", hooks.display());
    std::fs::create_dir_all(global.parent().unwrap()).unwrap();
    std::fs::write(&global, hooks_path(&global_hooks)).unwrap();
    std::fs::write(f.home.join(".gitconfig"), hooks_path(&home_hooks)).unwrap();

    let policy = f.build(&[], None);
    assert!(policy.is_protected(&home_hooks.join("pre-commit")));
    assert!(!policy.is_protected(&global));
    assert!(!policy.is_protected(&global_hooks.join("pre-commit")));

    f.git_env = GitConfigEnv {
        config_global: Some(global.clone().into_os_string()),
        xdg_config_home: None,
    };
    let policy = f.build(&[], None);
    assert!(policy.is_protected(&global), "{:?}", policy.protected);
    assert!(policy.is_protected(&global_hooks.join("pre-commit")));
    assert!(!policy.is_protected(&home_hooks.join("pre-commit")));
    assert!(policy.unread_git_metadata.is_empty());

    f.git_env.config_global = Some("rel/gitconfig".into());
    let policy = f.build(&[], None);
    assert_eq!(
        1,
        policy.unread_git_metadata.len(),
        "{:?}",
        policy.unread_git_metadata
    );
}

/// The child's default write roots: the workspace, the temp dirs, the *verified* build caches and
/// the `commands` directory of this workspace's *own* session directory — never the whole grok
/// home, the `sessions` tree or the rest of the own directory (its grant files, their lock, the
/// writer's temp names, the client permission files), which the floor protects.
#[test]
fn workspace_tmp_session_command_dir_and_verified_caches_are_the_default_write_roots() {
    let f = Fixture::new("roots");
    let policy = f.build(&[], None);
    let own_session = xai_grok_config::sessions_cwd_dir_in(&f.grok_home, &f.ws.to_string_lossy());
    let commands = own_session.join("commands");
    assert!(policy.write_roots.contains(&f.ws));
    assert!(policy.write_roots.contains(&f.tmp));
    assert!(policy.write_roots.contains(&commands));
    assert!(!policy.write_roots.contains(&own_session));
    assert!(policy.write_roots.contains(&f.home.join(".npm/_cacache")));
    assert!(
        !policy.write_roots.contains(&f.home.join(".npm")),
        "a cache tree is the family's, not a default root"
    );
    assert!(
        !policy.write_roots.contains(&f.grok_home),
        "the whole grok home is never a child write root"
    );
    assert!(
        !policy.write_roots.contains(&f.grok_home.join("sessions")),
        "the sessions tree is the floor's, not a write root"
    );
    let write = |path: PathBuf| policy.would_allow(&Blocked::FsWrite { path });
    assert!(write(commands.join("out.log")));
    assert!(!write(own_session.join("events.jsonl")));
    assert!(!write(own_session.join("permission_desktop.toml")));
    assert!(!write(own_session.join("sandbox_grants.toml")));
    assert!(!write(own_session.join("sandbox_grants.toml.lock")));
    assert!(
        !write(own_session.join(".4242.0.tmp")),
        "the atomic writer's temp file, renamed over a grant file"
    );
    let other = xai_grok_config::sessions_cwd_dir_in(&f.grok_home, "/opt/ws-fixture/other");
    assert!(!write(other.join("events.jsonl")));
    assert!(!write(other.join("sandbox_grants.toml")));
    assert!(!policy.would_allow(&Blocked::FsRead {
        path: other.join("events.jsonl")
    }));
    assert!(policy.would_allow(&Blocked::FsRead {
        path: own_session.join("events.jsonl")
    }));
}

/// A hook source the process-wide profile write-denies — here a `hooks-paths` entry inside the
/// workspace — is in the command's floor: grok runs the hooks it defines unsandboxed, so no
/// command writes it and no grant opens it.
#[test]
fn a_configured_hook_source_is_protected_like_the_grok_home_hooks() {
    let f = Fixture::new("hook-sources");
    let team_hooks = f.ws.join("team-hooks");
    std::fs::create_dir_all(&team_hooks).unwrap();
    let mut profile = profile(&f.ws, &f.grok_home);
    profile.write_deny = vec![xai_grok_config::GlobalHookSource {
        path: team_hooks.clone(),
        kind: xai_grok_config::GlobalHookSourceKind::ConfiguredSource,
    }];
    let tmp_dirs = [ServedRoot::pin(&f.tmp)];
    let policy = SandboxPolicy::build(PolicyInputs {
        workspace_root: &ServedRoot::pin(&f.ws),
        profile: &profile,
        grants: &[],
        proxy: None,
        tmp_dirs: &tmp_dirs,
        control_socket_dir: &f.grok_home.join("workspaced"),
        grok_home: &f.grok_home,
        user_home: Some(&f.home),
        git_env: &GitConfigEnv::default(),
    })
    .expect("policy builds");
    let hook = team_hooks.join("format.json");
    assert!(!policy.would_allow(&Blocked::FsWrite { path: hook }));
    assert!(protected::is_ungrantable(&team_hooks, &policy.protected));
    assert!(policy.would_allow(&Blocked::FsWrite {
        path: f.ws.join("src/main.rs")
    }));
}

/// The session command directory is a write root only once it and the own session directory exist
/// as directories: missing, a command could create either as a symlink to a folder of its own; a
/// symlink already in either place is never written through.
#[cfg(unix)]
#[test]
fn a_missing_or_symlinked_session_command_directory_is_not_a_write_root() {
    let f = Fixture::new("session-dir");
    let own_session = xai_grok_config::sessions_cwd_dir_in(&f.grok_home, &f.ws.to_string_lossy());
    let commands = own_session.join("commands");
    let write =
        |policy: &SandboxPolicy, path: PathBuf| policy.would_allow(&Blocked::FsWrite { path });
    std::fs::remove_dir(&commands).unwrap();
    let policy = f.build(&[], None);
    assert!(!policy.write_roots.contains(&commands), "missing");
    assert!(!write(&policy, commands.clone()));

    let planted = f.ws.parent().unwrap().join("planted");
    std::fs::create_dir_all(planted.join("commands")).unwrap();
    std::os::unix::fs::symlink(&planted, &commands).unwrap();
    let policy = f.build(&[], None);
    assert!(
        !policy
            .write_roots
            .iter()
            .any(|root| root.starts_with(&planted)),
        "{:?}",
        policy.write_roots
    );

    std::fs::remove_file(&commands).unwrap();
    std::fs::remove_dir(&own_session).unwrap();
    std::os::unix::fs::symlink(&planted, &own_session).unwrap();
    let policy = f.build(&[], None);
    assert!(
        !policy
            .write_roots
            .iter()
            .any(|root| root.starts_with(&planted)),
        "{:?}",
        policy.write_roots
    );
    assert!(!write(&policy, planted.join("sandbox_grants.toml")));
}

/// A grok home no glob can spell (`…[1]`) keeps the session command directory: the floor names
/// the own directory as a tree, so a permission file created later or a writer's temp name is
/// protected however the directory is spelled.
#[test]
fn a_grok_home_unspellable_as_a_glob_keeps_the_session_command_directory() {
    for tag in ["session-glob", "session-glob[1]"] {
        let f = Fixture::new(tag);
        let own_session =
            xai_grok_config::sessions_cwd_dir_in(&f.grok_home, &f.ws.to_string_lossy());
        let policy = f.build(&[], None);
        let write = |path: PathBuf| policy.would_allow(&Blocked::FsWrite { path });
        assert!(
            policy.write_roots.contains(&own_session.join("commands")),
            "{tag}: {:?}",
            policy.write_roots
        );
        for name in ["permission_desktop.toml", ".4242.7.tmp", "events.jsonl"] {
            assert!(!write(own_session.join(name)), "{tag}: {name}");
        }
        assert!(write(own_session.join("commands/out.log")), "{tag}");
    }
}

/// Only the checksum-verified caches — and the lock and log files no
/// toolchain executes — are writable by default; the unpacked-source and executable subpaths of
/// the same trees are not, and a write there proposes the one `build_caches` grant.
#[test]
fn verified_caches_are_writable_by_default_and_the_unpacked_ones_are_not() {
    let f = Fixture::new("split");
    let policy = f.build(&[], None);
    let write = |path: PathBuf| policy.would_allow(&Blocked::FsWrite { path });
    for rel in [
        // cargo's lock files sit directly under ~/.cargo (seen on macOS)
        ".cargo/.package-cache",
        ".cargo/.global-cache-journal",
        ".npm/_cacache/index-v5/00/x",
        ".npm/_logs/2026-09-22T10_00_00_000Z-debug-0.log",
        ".cache/pip/http/0/x",
        ".cache/pip/http-v2/0/x",
        "Library/Caches/pip/http-v2/0/x",
    ] {
        assert!(write(f.home.join(rel)), "{rel} is a verified cache");
    }
    for rel in [
        // cached archives unpacked without their checksum, and the checksums themselves
        ".cargo/registry/cache/index.crates.io-1949cf8c6b5b557f/serde-1.0.0.crate",
        ".cargo/registry/index/index.crates.io-1949cf8c6b5b557f/.cache/se/rd/serde",
        "go/pkg/mod/cache/download/golang.org/x/text/@v/v0.3.0.zip",
        "go/pkg/mod/cache/download/golang.org/x/text/@v/v0.3.0.ziphash",
        // built wheels install unverified
        ".cache/pip/wheels/ab/cd/x-1.0-py3-none-any.whl",
        ".cache/pip/selfcheck/x.json",
        "Library/Caches/pip/wheels/ab/x.whl",
    ] {
        let path = f.home.join(rel);
        assert!(!write(path.clone()), "{rel} is not verified on read");
        assert!(
            policy.in_build_caches(&path),
            "{rel} takes the build_caches grant"
        );
    }
    for rel in [
        ".cargo/registry/src/index.crates.io-1949cf8c6b5b557f/serde-1.0.0/build.rs",
        ".cargo/git/checkouts/x-abc/deadbeef/src/lib.rs",
        ".cargo/git/db/x-abc",
        ".npm/_npx/abc/node_modules/.bin/tool",
        ".cache/pre-commit/repo0/py_env/bin/python",
        ".cache/uv/archive-v0/x/lib.py",
        "go/pkg/mod/golang.org/x/text@v0.3.0/unicode/norm/tables.go",
        ".pnpm-store/v3/files/00/x",
        ".gradle/caches/modules-2/files-2.1/x.jar",
        ".m2/repository/org/x/1.0/x-1.0.jar",
        "Library/Caches/Homebrew/x.tar.gz",
    ] {
        let path = f.home.join(rel);
        assert!(!write(path.clone()), "{rel} takes the build_caches grant");
        assert!(policy.in_build_caches(&path), "{rel} is in a family tree");
        assert!(
            !policy.is_protected(&path),
            "{rel} is never floor-protected"
        );
    }
    assert!(
        !policy.in_build_caches(&f.home.join("notes.txt")),
        "the family is the curated trees, not the home"
    );
}

/// One `build_caches` grant widens to every tree of the family; the floor entries inside
/// `~/.cargo` (binaries, config, credentials, `env` — one source of truth) stay
/// read-only, and neither table names a protected path, so the grant can never be refused as one.
#[test]
fn a_build_caches_grant_widens_to_every_tree_and_keeps_the_floor() {
    let f = Fixture::new("family");
    let policy = f.build(&[], None);
    for rel in BUILD_CACHE_TREES.iter().chain(VERIFIED_BUILD_CACHES) {
        assert!(
            !policy.is_protected(&f.home.join(rel)),
            "{rel} is in a cache table and must not be in the floor"
        );
    }
    let widened = policy
        .clone()
        .with_grant(&grant(GrantSubject::BuildCaches))
        .expect("the family is never protected");
    let write = |path: PathBuf| widened.would_allow(&Blocked::FsWrite { path });
    for rel in BUILD_CACHE_TREES {
        assert!(widened.write_roots.contains(&f.home.join(rel)), "{rel}");
        assert!(write(f.home.join(rel).join("anything")), "{rel}");
    }
    let cargo = f.home.join(".cargo");
    assert!(
        !widened.write_roots.contains(&cargo),
        "~/.cargo itself is in neither table"
    );
    for read_only in [
        "bin",
        "config.toml",
        "config",
        "credentials.toml",
        "credentials",
        "env",
    ] {
        assert!(
            !write(cargo.join(read_only).join("x")) && !write(cargo.join(read_only)),
            "{read_only} must stay read-only inside ~/.cargo"
        );
        assert!(widened.is_protected(&cargo.join(read_only)));
    }
    let twice = widened
        .clone()
        .with_grant(&grant(GrantSubject::BuildCaches))
        .unwrap();
    assert_eq!(widened.write_roots, twice.write_roots, "idempotent");
    let no_home = f.build_without_home();
    assert!(no_home.build_cache_trees.is_empty());
    let unchanged = no_home
        .clone()
        .with_grant(&grant(GrantSubject::BuildCaches))
        .unwrap();
    assert_eq!(no_home.write_roots, unchanged.write_roots);
}

/// [`WritableLocations`] holds every place a command may write or may have written: the
/// temporary directories, every build-cache tree (granted or not), the workspace, each recorded
/// policy's write roots (kept after a later policy drops one), the sessions tree, every folder a
/// session directory names and every write root in a grants file of either scope, deny and
/// expired rows included. Nothing else is held; grants that cannot be read are an error.
#[test]
fn writable_locations_hold_every_place_a_command_may_write_or_may_have_written() {
    let f = Fixture::new("writable");
    let writable = WritableLocations::new(Some(&f.home), std::slice::from_ref(&f.tmp));
    let (recorded, global, folder) = (
        f.home.join("recorded"),
        f.home.join("global-grant"),
        f.home.join("folder-grant"),
    );
    let other = f.home.join("other");
    for dir in [&recorded, &global, &folder, &other] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let session =
        xai_grok_config::ensure_sessions_cwd_dir_in(&f.grok_home, other.to_str().unwrap()).unwrap();
    let row = |root: &Path, rest: &str| {
        let root = toml::Value::from(root.to_str().unwrap());
        format!("[[grant]]\nsubject = {{ kind = \"fs_write_root\", root = {root} }}\n{rest}\n")
    };
    let global_file = f.grok_home.join(protected::GLOBAL_GRANTS_FILENAME);
    std::fs::write(&global_file, row(&global, "decision = \"deny\"")).unwrap();
    let folder_file = session.join(protected::GLOBAL_GRANTS_FILENAME);
    let expired = "expires = { kind = \"at\", at = 1 }";
    std::fs::write(&folder_file, row(&folder, expired)).unwrap();
    let with_recorded = f.build(
        &[grant(GrantSubject::FsWriteRoot {
            root: recorded.clone(),
        })],
        None,
    );
    let holds = |path: &Path| writable.holds(path, &f.ws, &f.grok_home).unwrap();
    assert!(holds(&f.tmp.join("a")), "seeded before any policy");
    assert!(!holds(&recorded.join("e")), "not recorded yet");
    writable.record(&with_recorded);
    writable.record(&f.build(&[], None));

    for held in [
        f.tmp.join("a"),
        f.home.join(".cargo/registry/b"),
        f.home.join("Library/Caches/c"),
        f.ws.join("d"),
        recorded.join("e"),
        f.grok_home.join("sessions/f"),
        other.join("g"),
        global.join("h"),
        folder.join("i"),
    ] {
        assert!(holds(&held), "{held:?}");
    }
    for free in [
        f.home.join("dotfiles/grok.toml"),
        f.home.join(".cargo/config.toml"),
    ] {
        assert!(!holds(&free), "{free:?}");
    }

    std::fs::write(&folder_file, "[[grant]\n").unwrap();
    let error = writable
        .holds(&f.home.join("dotfiles/grok.toml"), &f.ws, &f.grok_home)
        .unwrap_err();
    assert!(error.to_string().contains("is not TOML"), "{error}");
}

/// A deny row of the other write kind closes the overlap and nothing else: a path deny inside or
/// around a build-cache tree leaves that tree out of the family's allow, whole, and the other
/// trees open; the family's deny leaves out a path allow into or around a tree and keeps one
/// elsewhere. The rows go through [`allows_not_denied`] as the store hands them over.
#[test]
fn a_deny_of_the_other_write_kind_closes_the_overlap() {
    let f = Fixture::new("cross-kind");
    let deny = |subject| Grant {
        decision: GrantDecision::Deny,
        ..grant(subject)
    };
    let write_root = |root: PathBuf| GrantSubject::FsWriteRoot { root };
    let cargo = f.home.join(".cargo");
    let npm_file = f.home.join(".npm/z");
    for denied in [cargo.join("registry/src/x"), cargo.clone()] {
        let rows = [
            deny(write_root(denied.clone())),
            grant(GrantSubject::BuildCaches),
        ];
        let policy = f.build(&allows_not_denied(&rows), None);
        let write = |path: PathBuf| policy.would_allow(&Blocked::FsWrite { path });
        assert!(!write(cargo.join("registry/cache/y")), "{denied:?}");
        assert!(!write(denied.join("z")), "{denied:?}");
        assert!(
            write(npm_file.clone()),
            "{denied:?} leaves the other trees open"
        );
        assert_eq!(
            denied == cargo,
            !write(cargo.join("git/y")),
            "{denied:?} closes every tree it holds"
        );
    }

    let into = f.home.join(".npm/lib-x");
    let around = f.home.join("go");
    let elsewhere = f.home.join("projects/tool");
    let rows = [
        grant(write_root(into.clone())),
        grant(write_root(around.clone())),
        grant(write_root(elsewhere.clone())),
        deny(GrantSubject::BuildCaches),
    ];
    let policy = f.build(&allows_not_denied(&rows), None);
    let write = |path: PathBuf| policy.would_allow(&Blocked::FsWrite { path });
    assert!(!write(into.join("y")), "a path allow into a tree");
    assert!(!write(around.join("bin/y")), "a path allow around a tree");
    assert!(
        write(elsewhere.join("y")),
        "a path allow clear of every tree"
    );
}

#[test]
fn protected_subpaths_are_carved_out_of_the_workspace_root() {
    let f = Fixture::new("carve");
    let policy = f.build(&[], None);
    assert!(policy.write_roots.contains(&f.ws));
    let carve_outs: Vec<&Protected> = policy
        .protected
        .iter()
        .filter(|entry| entry.reaches_into(&f.ws))
        .collect();
    let path = |p: PathBuf| Protected::Path { path: p };
    assert!(carve_outs.contains(&&path(f.ws.join(".git/hooks"))));
    assert!(carve_outs.contains(&&path(f.ws.join(".git/config"))));
    assert!(carve_outs.contains(&&path(f.ws.join(".git/info"))));
    assert!(carve_outs.contains(&&path(f.ws.join(".grok"))));
    assert!(
        carve_outs.contains(&&Protected::Glob {
            glob: format!("{}/.git/modules/**/hooks", f.ws.display())
        }),
        "{carve_outs:?}"
    );
    assert!(policy.would_allow(&Blocked::FsWrite {
        path: f.ws.join("src/main.rs")
    }));
    for protected in [
        ".git/hooks/pre-commit",
        ".git/config",
        ".git/info/exclude",
        ".git/modules/lib/hooks/post-checkout",
        ".git/modules/lib/config",
        ".grok/settings.toml",
        "src/../.git/hooks/pre-commit",
    ] {
        assert!(
            !policy.would_allow(&Blocked::FsWrite {
                path: f.ws.join(protected)
            }),
            "{protected} is the floor's"
        );
        assert!(policy.is_protected(&f.ws.join(protected)), "{protected}");
    }
    for writable in [".git/index", ".git/HEAD", ".git/modules/lib/index"] {
        assert!(
            policy.would_allow(&Blocked::FsWrite {
                path: f.ws.join(writable)
            }),
            "{writable} stays writable"
        );
    }
}

/// Each write root's `.git` node is protected — nothing may create, remove or rename it — while
/// what lies inside stays writable (the floor still holds `.git/hooks`, `config` and `info`).
#[test]
fn every_write_roots_git_node_is_protected_and_its_contents_stay_writable() {
    let f = Fixture::new("git-node");
    let policy = f.build(&[], None);
    let nodes = policy.git_dir_nodes();
    assert_eq!(policy.write_roots.len(), nodes.len());
    for root in &policy.write_roots {
        let node = root.join(".git");
        assert!(nodes.contains(&node), "{node:?}");
        assert!(policy.is_protected(&node), "{node:?}");
        assert!(policy.is_protected(&root.join("sub/../.git")), "{root:?}");
    }
    let write = |path: PathBuf| policy.would_allow(&Blocked::FsWrite { path });
    assert!(!write(f.ws.join(".git")));
    assert!(write(f.ws.join(".git/objects/ab/cdef")));
    assert!(write(f.ws.join(".git/index.lock")));
    assert!(!write(f.ws.join(".git/hooks/pre-commit")));
    assert!(!policy.is_protected(&f.ws.join(".gitignore")));
    assert!(!policy.is_protected(&f.ws.join("sub/.git")));
}

/// A floor glob's literal prefix inside a write root (`<ws>/.git/modules`) is a node, as the `.git`
/// node is: a refused `mkdir` or rename of it is the floor, never the OS. Beneath it no grant
/// applies, whether or not the folder matches the glob.
#[test]
fn a_floor_globs_literal_prefix_is_a_node_and_nothing_beneath_it_is_grantable() {
    let f = Fixture::new("glob-prefix");
    std::fs::create_dir_all(f.ws.join(".git/modules/lib/hooks")).unwrap();
    let policy = f.build(&[], None);
    let modules = f.ws.join(".git/modules");
    let nodes = policy.glob_prefix_nodes();
    assert!(nodes.contains(&modules), "{nodes:?}");
    assert!(policy.is_protected(&modules));
    let write = |path: PathBuf| policy.would_allow(&Blocked::FsWrite { path });
    assert!(!write(modules.clone()));
    assert!(write(modules.join("lib/index")));
    for root in [
        modules.join("lib"),
        modules.join("lib/hooks"),
        modules.join("new"),
    ] {
        let refused = policy
            .clone()
            .with_grant(&grant(GrantSubject::FsWriteRoot { root: root.clone() }));
        assert!(
            matches!(refused, Err(PolicyError::Protected { .. })),
            "{root:?}: {refused:?}"
        );
    }
    let beside = policy.clone().with_grant(&grant(GrantSubject::FsWriteRoot {
        root: f.ws.join("target"),
    }));
    assert!(beside.is_ok(), "{beside:?}");
}

/// Under a `$HOME` grant a missing ancestor of a floor entry is a protected node (a staged tree
/// moved in as `~/.config/git` would bring its `config`); an existing directory, a write root
/// and the same path outside every root are not.
#[test]
fn a_missing_ancestor_of_a_floor_entry_inside_a_write_root_is_protected() {
    let f = Fixture::new("missing-ancestor");
    std::fs::create_dir_all(f.home.join(".config")).unwrap();
    let config_git = f.home.join(".config/git");
    let policy = f.build(&[], None);
    assert!(
        !policy.is_protected(&config_git),
        "outside every write root"
    );
    let home_grant = policy
        .with_grant(&grant(GrantSubject::FsWriteRoot {
            root: f.home.clone(),
        }))
        .unwrap();
    let nodes = home_grant.missing_ancestor_nodes();
    assert!(nodes.contains(&config_git), "{nodes:?}");
    assert!(!nodes.contains(&f.home), "a root is pinned on its own");
    assert!(home_grant.is_protected(&config_git));
    let write = |path: PathBuf| home_grant.would_allow(&Blocked::FsWrite { path });
    assert!(!write(config_git.clone()));
    assert!(!home_grant.is_protected(&f.home.join(".config")));
    assert!(write(f.home.join(".config/other")));

    std::fs::create_dir_all(&config_git).unwrap();
    assert!(
        !home_grant.is_protected(&config_git),
        "an existing directory is pinned, not a node"
    );
    assert!(home_grant.is_protected(&config_git.join("config")));
}

/// A `core.hooksPath` at or above a write root, the home or `/` would put the whole tree in the
/// floor and refuse every write and grant beneath it; the hook files git would run from it are
/// protected instead: `.`, `..`, `~`, `/`, the workspace spelled absolutely, the grok home over
/// the own session directory, `~/.cargo` over the verified caches, a shared temp root. A hooks
/// directory beneath them stays a tree.
#[test]
fn a_hooks_path_over_a_write_root_protects_its_hook_files_not_the_tree() {
    let f = Fixture::new("hooks-over-roots");
    std::fs::create_dir_all(f.ws.join(".git")).unwrap();
    let root = f.ws.parent().unwrap().to_path_buf();
    let own_session = xai_grok_config::sessions_cwd_dir_in(&f.grok_home, &f.ws.to_string_lossy());
    let shared_tmp = default_tmp_dirs(None, None)
        .iter()
        .map(|dir| canonical_path(dir))
        .find(|dir| !is_within(&root, dir))
        .expect("a shared temp root outside the fixture");
    let with_hooks_path = |value: &str| {
        let config = format!("[core]\n\thooksPath = \"{value}\"\n");
        std::fs::write(f.ws.join(".git/config"), config).unwrap();
        f.build(&[], None)
    };
    for (value, hooks) in [
        (".".to_owned(), f.ws.clone()),
        ("..".to_owned(), root.clone()),
        ("~".to_owned(), f.home.clone()),
        ("/".to_owned(), PathBuf::from("/")),
        (f.ws.display().to_string(), f.ws.clone()),
        ("~/.grok".to_owned(), f.grok_home.clone()),
        ("~/.cargo".to_owned(), f.home.join(".cargo")),
        (shared_tmp.display().to_string(), shared_tmp.clone()),
    ] {
        let policy = with_hooks_path(&value);
        let write = |path: PathBuf| policy.would_allow(&Blocked::FsWrite { path });
        assert!(write(f.ws.join("src/main.rs")), "{value}");
        assert!(write(own_session.join("commands/out.log")), "{value}");
        assert!(write(f.home.join(".cargo/.global-cache")), "{value}");
        for root in [f.home.join("other"), shared_tmp.join("scratch")] {
            assert!(
                !protected::is_ungrantable(&root, &policy.protected),
                "{value}: {}",
                root.display()
            );
        }
        for hook in ["pre-commit", "post-checkout", "p4-pre-submit"] {
            assert!(!write(hooks.join(hook)), "{value}: {hook}");
            assert!(
                protected::is_protected(&hooks.join(hook), &policy.protected),
                "{value}: {hook}"
            );
        }
    }
    let policy = with_hooks_path("tools/hooks");
    let write = |path: PathBuf| policy.would_allow(&Blocked::FsWrite { path });
    assert!(policy.protected.contains(&Protected::Path {
        path: f.ws.join("tools/hooks")
    }));
    assert!(!write(f.ws.join("tools/hooks/lib/common.sh")));
    assert!(write(f.ws.join("src/main.rs")));
}

/// macOS spells `$TMPDIR` as `/var/folders/…` while the stored root is `/private/var/folders/…`;
/// a denial quoted in the command's spelling must still match the widened root (the replayed
/// command's "refused under grant" reading depends on it).
#[cfg(target_os = "macos")]
#[test]
fn would_allow_matches_a_blocked_path_spelled_through_the_tmp_alias() {
    let f = Fixture::new("alias");
    let granted = f
        .build(&[], None)
        .with_grant(&Grant {
            id: GrantId::new("0192c1a0-0000-7000-8000-0000000000a7"),
            subject: GrantSubject::FsWriteRoot {
                root: PathBuf::from("/var/folders/grok-alias-fixture/dir"),
            },
            scope: GrantScope::Session,
            expires: Expiry::Never,
            decision: GrantDecision::Allow,
            granted_at: 0,
            granted_by: "hub:s1".to_owned(),
            via: None,
        })
        .expect("grant outside the floor");
    assert!(
        granted.write_roots.contains(&PathBuf::from(
            "/private/var/folders/grok-alias-fixture/dir"
        )),
        "the grant root is stored in its canonical spelling"
    );
    assert!(granted.would_allow(&Blocked::FsWrite {
        path: PathBuf::from("/var/folders/grok-alias-fixture/dir/notes.txt"),
    }));
    assert!(granted.would_allow(&Blocked::FsWrite {
        path: PathBuf::from("/private/var/folders/grok-alias-fixture/dir/notes.txt"),
    }));
    assert!(!granted.would_allow(&Blocked::FsWrite {
        path: PathBuf::from("/var/folders/grok-alias-fixture/elsewhere.txt"),
    }));
}

#[test]
fn read_policy_denies_secret_stores_profile_denies_and_auth_globs() {
    let f = Fixture::new("read");
    let policy = f.build(&[], None);
    let denied = |p: PathBuf| !policy.would_allow(&Blocked::FsRead { path: p });
    assert!(denied(f.home.join(".ssh/id_ed25519")));
    assert!(denied(f.home.join(".netrc")));
    assert!(denied(f.home.join(".docker/config.json")));
    assert!(denied(f.home.join(".config/gh/hosts.yml")));
    assert!(denied(f.grok_home.join("workspaced/control.sock")));
    assert!(denied(PathBuf::from("/opt/secret/x")));
    assert!(denied(f.ws.join("certs/server.pem")));
    assert!(denied(f.grok_home.join("auth.json")));
    assert!(denied(f.grok_home.join("credentials-x.toml")));
    assert!(!denied(f.grok_home.join("config.toml")));
    assert!(!denied(f.ws.join("src/main.rs")));
}

#[test]
fn proxy_endpoint_sets_network_and_env() {
    let f = Fixture::new("proxy");
    let policy = f.build(&[], Some(ProxyEndpoint { port: 4321 }));
    assert_eq!(NetworkPolicy::Proxy { port: 4321 }, policy.network);
    assert_eq!(
        Some("http://127.0.0.1:4321"),
        policy.env.set.get("HTTPS_PROXY").map(String::as_str)
    );
    assert!(policy.env.excludes("aws_secret_access_key"));
    assert!(policy.env.excludes("DYLD_INSERT_LIBRARIES"));
    assert!(!policy.env.excludes("PATH"));
}

#[test]
fn proxy_vars_carry_the_credentialed_url_in_both_spellings_and_survive_the_excludes() {
    let url = "http://grok:tok3n@127.0.0.1:4321";
    let set = EnvPolicy::proxy_vars_at(url);
    for name in ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY"] {
        assert_eq!(Some(url), set.get(name).map(String::as_str), "{name}");
        assert_eq!(
            Some(url),
            set.get(&name.to_ascii_lowercase()).map(String::as_str),
            "{name}"
        );
    }
    assert_eq!(
        set.get("NO_PROXY"),
        set.get("no_proxy"),
        "loopback stays direct in both spellings"
    );
    assert_eq!(8, set.len());
    // A credential in the pointer is not a variable name the excludes could strip
    let env = EnvPolicy {
        exclude_globs: EnvGlobs::new(EnvPolicy::default_excludes()).unwrap(),
        set,
    };
    assert!(env.set.keys().all(|name| !env.excludes(name)));
}

#[test]
fn no_proxy_means_network_off_and_no_proxy_env() {
    let f = Fixture::new("noproxy");
    let policy = f.build(&[], None);
    assert_eq!(NetworkPolicy::Off, policy.network);
    assert!(policy.env.set.is_empty());
    assert!(!policy.would_allow(&Blocked::Net {
        host: Some("example.com".to_owned()),
        port: Some(443)
    }));
}

/// A grant that covers protected subpaths widens the write roots and leaves the floor
/// where it was, so the entries beneath the granted root are exactly the carve-outs the renderer
/// takes out of that root's allow. The grant of `$HOME` covers the rc files, the secret stores,
/// the persistence trees and the grok home's floor.
#[test]
fn write_grant_widens_and_keeps_protected_carve_outs() {
    let f = Fixture::new("grant");
    let outside = f.home.join("proj2");
    std::fs::create_dir_all(outside.join(".git")).unwrap();
    let policy = f.build(&[], None);
    let floor_before = policy.protected.clone();
    let path = outside.join("x");
    assert!(!policy.would_allow(&Blocked::FsWrite { path: path.clone() }));
    let widened = policy
        .with_grant(&grant(GrantSubject::FsWriteRoot {
            root: outside.clone(),
        }))
        .unwrap();
    assert!(widened.would_allow(&Blocked::FsWrite { path }));
    assert!(widened.write_roots.contains(&outside));
    assert!(
        !widened.protected.iter().any(|e| e.reaches_into(&outside)),
        "a tree with no floor inside it needs no carve-out"
    );

    let home_grant = widened
        .with_grant(&grant(GrantSubject::FsWriteRoot {
            root: f.home.clone(),
        }))
        .unwrap();
    assert!(home_grant.write_roots.contains(&f.home));
    assert_eq!(
        floor_before, home_grant.protected,
        "a grant never edits the floor"
    );
    let carve_outs: Vec<&Protected> = home_grant
        .protected
        .iter()
        .filter(|entry| entry.reaches_into(&f.home))
        .collect();
    let expect_carved = |rel: &str| {
        let target = f.home.join(rel);
        assert!(
            carve_outs.iter().any(|entry| entry.covers(&target)),
            "{rel} must be carved out of the $HOME grant: {carve_outs:?}"
        );
        assert!(
            !home_grant.would_allow(&Blocked::FsWrite {
                path: target.clone()
            }),
            "{rel} stays read-only under the $HOME grant"
        );
        assert!(home_grant.is_protected(&target), "{rel}");
    };
    for rel in [
        ".bashrc",
        ".zshenv",
        ".ssh/id_ed25519",
        ".cargo/bin/cargo",
        "Library/LaunchAgents/com.evil.plist",
        ".grok/workspaced.toml",
        ".grok/hooks/pre-tool",
        ".grok/sessions/%2Fopt%2Fother/sandbox_grants.toml",
    ] {
        expect_carved(rel);
    }
    let own_session = xai_grok_config::sessions_cwd_dir_in(&f.grok_home, &f.ws.to_string_lossy());
    for name in [
        "permission_desktop.toml",
        "sandbox_grants.toml",
        ".4242.0.tmp",
    ] {
        let rel = own_session.join(name);
        expect_carved(&rel.strip_prefix(&f.home).unwrap().to_string_lossy());
    }
    assert!(
        carve_outs
            .iter()
            .any(|entry| matches!(entry, Protected::TreeExcept { .. })),
        "the sessions tree-except is one of the carve-outs"
    );
    for rel in [
        "notes.txt",
        "proj2/src/main.rs",
        ".cargo/registry/cache/x",
        ".npm/_cacache/y",
    ] {
        assert!(
            home_grant.would_allow(&Blocked::FsWrite {
                path: f.home.join(rel)
            }),
            "{rel} is writable under the $HOME grant"
        );
    }
}

/// The grant files' lock sidecars are neither readable nor writable to a command: one that could
/// open a sidecar could hold its `flock` and stall every grant edit.
#[test]
fn a_command_can_neither_read_nor_write_the_grant_lock_sidecars() {
    let f = Fixture::new("grant-lock");
    let policy = f.build(&[], None);
    let own = xai_grok_config::sessions_cwd_dir_in(&f.grok_home, &f.ws.to_string_lossy());
    for lock in [
        f.grok_home.join("sandbox_grants.toml.lock"),
        own.join("sandbox_grants.toml.lock"),
    ] {
        assert!(policy.is_protected(&lock), "{lock:?}");
        assert!(
            !policy.would_allow(&Blocked::FsRead { path: lock.clone() }),
            "{lock:?}"
        );
    }
}

/// A deny row never widens: whatever its subject, the policy after it is the policy before it.
#[test]
fn a_deny_row_leaves_the_policy_unchanged() {
    let f = Fixture::new("deny-row");
    let policy = f.build(&[], None);
    for subject in [
        GrantSubject::FsWriteRoot {
            root: f.home.join("elsewhere"),
        },
        GrantSubject::FsRead {
            root: f.home.clone(),
        },
        GrantSubject::BuildCaches,
    ] {
        let mut deny = grant(subject.clone());
        deny.decision = GrantDecision::Deny;
        assert_eq!(
            policy,
            policy.clone().with_grant(&deny).unwrap(),
            "{subject:?}"
        );
    }
}

/// A granted write root that does not exist yet and holds a floor entry (`~/.kube` before
/// `~/.kube/config`) is left out, so a staged tree cannot be moved in as it with that entry
/// inside; once the directory exists the grant applies. A missing root with no floor entry
/// under it applies at once.
#[test]
fn a_missing_granted_root_holding_a_floor_entry_is_left_out() {
    let f = Fixture::new("missing-root");
    let policy = f.build(&[], None);
    let kube = f.home.join(".kube");
    let widened = policy
        .clone()
        .with_grant(&grant(GrantSubject::FsWriteRoot { root: kube.clone() }))
        .unwrap();
    assert_eq!(policy.write_roots, widened.write_roots);
    let fresh = f.home.join("fresh-tree");
    let widened = policy
        .clone()
        .with_grant(&grant(GrantSubject::FsWriteRoot {
            root: fresh.clone(),
        }))
        .unwrap();
    assert!(widened.write_roots.contains(&fresh));

    std::fs::create_dir_all(&kube).unwrap();
    let widened = policy
        .with_grant(&grant(GrantSubject::FsWriteRoot { root: kube.clone() }))
        .unwrap();
    assert!(widened.write_roots.contains(&kube));
}

/// A deny glob that does not parse covers every path (the renderer refuses it, and the
/// classification must not read a read it would deny as allowed).
#[test]
fn an_unparsable_deny_glob_covers_every_path() {
    let entry = DenyEntry::Glob {
        root: PathBuf::from("/opt/ws-fixture"),
        tail: "[unclosed".to_owned(),
    };
    assert!(entry.covers(Path::new("/opt/ws-fixture/anything")));
}

/// A deny glob anchored at a workspace whose own path carries glob metacharacters (`app[v2]`)
/// matches below that directory: the root is taken as written, only the tail is a pattern.
#[test]
fn a_deny_glob_below_a_bracketed_workspace_still_matches() {
    let ws = Path::new("/opt/ws-fixture/app[v2]");
    let entry = absolute_deny_entry(Path::new(".env*"), &ServedRoot::pin(ws), None).unwrap();
    assert_eq!(
        DenyEntry::Glob {
            root: ws.to_path_buf(),
            tail: ".env*".to_owned()
        },
        entry
    );
    assert!(entry.covers(&ws.join(".env.local")));
    assert!(!entry.covers(&ws.join("src/main.rs")));
    assert!(!entry.covers(Path::new("/opt/ws-fixture/appv/.env")));
}

/// Deny entries compare as APFS does: `AUTH.json` meets `auth*`, `.SSH` meets `.ssh`.
#[cfg(target_os = "macos")]
#[test]
fn deny_entries_match_case_insensitively_on_macos() {
    let glob = DenyEntry::Glob {
        root: PathBuf::from("/opt/ws-fixture/u/.grok"),
        tail: "auth*".to_owned(),
    };
    assert!(glob.covers(Path::new("/opt/ws-fixture/u/.grok/AUTH.json")));
    let path = DenyEntry::Path(PathBuf::from("/opt/ws-fixture/u/.ssh"));
    assert!(path.covers(Path::new("/opt/ws-fixture/u/.SSH/id_ed25519")));
}

/// A read grant around the profile's path denies leaves them denied however the profile spells
/// them: through the `/private` firmlinks or in another case, as [`DenyEntry::covers`] compares,
/// and whichever spelling the command reads them through.
#[cfg(target_os = "macos")]
#[test]
fn a_read_grant_keeps_denies_spelled_through_an_alias_or_another_case() {
    let f = Fixture::new("alias-read-grant");
    let data = f.home.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let aliased = data
        .strip_prefix("/private")
        .map(|rest| Path::new("/").join(rest))
        .expect("the fixture's canonical temp root sits under /private");
    let mut profile = profile(&f.ws, &f.grok_home);
    profile.deny = vec![aliased.join("secret"), f.home.join("DATA/keys")];
    let base = build_with_profile(&f, &profile, Some(&f.home)).unwrap();
    let widened = base
        .clone()
        .with_grant(&grant(GrantSubject::FsRead { root: data.clone() }))
        .unwrap();
    assert_eq!(
        base.read.deny(),
        widened.read.deny(),
        "no read grant drops a deny"
    );
    for dir in [&data, &aliased] {
        for path in [dir.join("secret/a"), dir.join("keys/b"), dir.join("KEYS/c")] {
            assert!(
                !widened.would_allow(&Blocked::FsRead { path: path.clone() }),
                "{path:?}"
            );
        }
        assert!(widened.would_allow(&Blocked::FsRead {
            path: dir.join("open/d")
        }));
    }
}

#[test]
fn grant_inside_the_floor_is_refused() {
    let f = Fixture::new("floor");
    let policy = f.build(&[], None);
    let err = policy
        .clone()
        .with_grant(&grant(GrantSubject::FsWriteRoot {
            root: f.ws.join(".git/hooks"),
        }))
        .unwrap_err();
    assert!(matches!(err, PolicyError::Protected { .. }), "{err}");
    let err = policy
        .clone()
        .with_grant(&grant(GrantSubject::FsWriteRoot {
            root: f.grok_home.join("sessions/%2Fx/permission.toml"),
        }))
        .unwrap_err();
    assert!(matches!(err, PolicyError::Protected { .. }), "{err}");
    let err = policy
        .with_grant(&grant(GrantSubject::FsRead {
            root: f.grok_home.join("hooks"),
        }))
        .unwrap_err();
    assert!(matches!(err, PolicyError::Protected { .. }), "{err}");
}

/// The grant boundary canonicalises once — a `..`-spelled root cannot step around the floor,
/// and what is stored is the folded spelling the floor and the decoder compare against.
#[test]
fn grant_roots_are_canonicalised_before_the_floor_is_asked() {
    let f = Fixture::new("canon");
    let outside = f.home.join("proj4");
    std::fs::create_dir_all(&outside).unwrap();
    let policy = f.build(&[], None);
    let err = policy
        .clone()
        .with_grant(&grant(GrantSubject::FsWriteRoot {
            root: f.ws.join("src/../.git/hooks"),
        }))
        .unwrap_err();
    assert!(matches!(err, PolicyError::Protected { .. }), "{err}");
    let err = policy
        .clone()
        .with_grant(&grant(GrantSubject::FsRead {
            root: f.home.join("Documents/../.ssh"),
        }))
        .unwrap_err();
    assert!(matches!(err, PolicyError::Protected { .. }), "{err}");
    let widened = policy
        .with_grant(&grant(GrantSubject::FsWriteRoot {
            root: f.home.join("elsewhere/../proj4/./"),
        }))
        .unwrap();
    assert!(
        widened.write_roots.contains(&outside),
        "{:?}",
        widened.write_roots
    );
    assert!(widened.would_allow(&Blocked::FsWrite {
        path: f.home.join("proj4/../proj4/out.txt")
    }));
}

#[test]
fn relative_grant_root_is_refused() {
    let f = Fixture::new("relative");
    let err = f
        .build(&[], None)
        .with_grant(&grant(GrantSubject::FsWriteRoot {
            root: PathBuf::from("node_modules"),
        }))
        .unwrap_err();
    assert!(matches!(err, PolicyError::NotAbsolute { .. }), "{err}");
}

#[cfg(unix)]
#[test]
fn symlink_below_top_level_in_grant_root_is_refused() {
    let f = Fixture::new("symlink");
    let target = f.home.join("real");
    std::fs::create_dir_all(&target).unwrap();
    let link = f.home.join("link");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let err = f
        .build(&[], None)
        .with_grant(&grant(GrantSubject::FsWriteRoot {
            root: link.join("sub"),
        }))
        .unwrap_err();
    assert!(matches!(err, PolicyError::SymlinkedRoot { .. }), "{err}");
}

/// A component of a granted root that cannot be inspected (its parent denies search) might be a
/// link, so the grant is refused; a component that does not exist ends the walk, so a missing
/// root is still granted.
#[cfg(unix)]
#[test]
fn a_grant_root_whose_components_cannot_be_inspected_is_refused() {
    use std::os::unix::fs::PermissionsExt as _;
    let f = Fixture::new("uninspectable-grant");
    let locked = f.home.join("locked");
    std::fs::create_dir_all(locked.join("sub")).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let blocked = std::fs::symlink_metadata(locked.join("sub")).is_err();
    let refused = f
        .build(&[], None)
        .with_grant(&grant(GrantSubject::FsWriteRoot {
            root: locked.join("sub"),
        }));
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    // A process that can still inspect it (running as root) cannot produce the condition
    if blocked {
        assert!(
            matches!(refused, Err(PolicyError::UninspectableRoot { .. })),
            "{refused:?}"
        );
    }
    let fresh = f.home.join("missing/tree");
    let widened = f
        .build(&[], None)
        .with_grant(&grant(GrantSubject::FsWriteRoot {
            root: fresh.clone(),
        }))
        .unwrap();
    assert!(widened.write_roots.contains(&fresh));
}

/// A live grant the floor or the symlink check now refuses (its root became a link after it was
/// given) is left out of the build; the other grants still apply and nothing is refused.
#[cfg(unix)]
#[test]
fn a_live_grant_that_no_longer_applies_is_left_out_of_the_build() {
    let f = Fixture::new("stale-grant");
    let target = f.home.join("real");
    let kept = f.home.join("kept");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::create_dir_all(&kept).unwrap();
    let link = f.home.join("link");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let grants = [
        grant(GrantSubject::FsWriteRoot {
            root: link.join("sub"),
        }),
        grant(GrantSubject::FsWriteRoot { root: kept.clone() }),
    ];
    let policy = f.build(&grants, None);
    assert!(
        policy.write_roots.contains(&kept),
        "{:?}",
        policy.write_roots
    );
    assert!(
        !policy
            .write_roots
            .iter()
            .any(|root| root.starts_with(&target) || root.starts_with(&link)),
        "{:?}",
        policy.write_roots
    );
}

/// A curated cache path with a symlinked component (relocated to another volume, or planted by
/// a command while it was missing) is left out of the write roots and the build-cache trees
/// rather than followed or refusing every command; the other caches stay writable.
#[cfg(unix)]
#[test]
fn a_symlinked_cache_path_is_left_out_of_the_policy() {
    let f = Fixture::new("cache-link");
    std::fs::create_dir_all(f.home.join("elsewhere")).unwrap();
    std::os::unix::fs::symlink(f.home.join("elsewhere"), f.home.join(".cargo")).unwrap();
    let policy = build_with_profile(&f, &profile(&f.ws, &f.grok_home), Some(&f.home)).unwrap();
    let under_cargo = |root: &PathBuf| root.starts_with(f.home.join(".cargo"));
    assert!(
        !policy.write_roots.iter().any(under_cargo),
        "{:?}",
        policy.write_roots
    );
    assert!(
        !policy.build_cache_trees.iter().any(under_cargo),
        "{:?}",
        policy.build_cache_trees
    );
    assert!(policy.write_roots.contains(&f.home.join(".npm/_cacache")));
    assert!(policy.build_cache_trees.contains(&f.home.join(".npm")));
}

/// A refused grant leaves the policy as it was, so `build` applies each live grant in place.
#[test]
fn a_refused_grant_leaves_the_policy_as_it_was() {
    let f = Fixture::new("refused-in-place");
    let mut policy = f.build(&[], None);
    let before = policy.clone();
    for subject in [
        GrantSubject::FsWriteRoot {
            root: f.ws.join(".grok"),
        },
        GrantSubject::FsWriteRoot {
            root: PathBuf::from("relative/out"),
        },
        GrantSubject::FsWriteRoot {
            root: PathBuf::from("/opt/secret/sub"),
        },
        GrantSubject::FsRead {
            root: PathBuf::from("/opt/secret"),
        },
    ] {
        assert!(
            policy.apply_grant(&grant(subject.clone())).is_err(),
            "{subject:?}"
        );
        assert_eq!(before, policy, "{subject:?}");
    }
    let out = PathBuf::from("/opt/fixture/refused-in-place/out");
    policy
        .apply_grant(&grant(GrantSubject::FsWriteRoot { root: out.clone() }))
        .unwrap();
    assert!(policy.write_roots.contains(&canonical_path(&out)));
}

/// A profile `deny` holds like the floor: a read or write grant at or beneath one is refused, a
/// grant around one leaves it denied, and a path it covers is written by no write root.
#[test]
fn a_grant_at_or_under_a_profile_deny_is_refused_and_one_around_it_keeps_it() {
    let f = Fixture::new("readgrant");
    let policy = f.build(&[], None);
    let refused = |subject: GrantSubject| policy.clone().with_grant(&grant(subject)).err();
    for subject in [
        GrantSubject::FsRead {
            root: PathBuf::from("/opt/secret"),
        },
        GrantSubject::FsRead {
            root: PathBuf::from("/opt/secret/sub"),
        },
        GrantSubject::FsWriteRoot {
            root: PathBuf::from("/opt/secret/sub"),
        },
        GrantSubject::FsWriteRoot {
            root: f.ws.join("keys/a.pem"),
        },
    ] {
        assert!(
            matches!(
                refused(subject.clone()),
                Some(PolicyError::ProfileDenied { .. })
            ),
            "{subject:?}"
        );
    }
    let around = policy
        .clone()
        .with_grant(&grant(GrantSubject::FsRead {
            root: PathBuf::from("/opt"),
        }))
        .unwrap();
    assert_eq!(policy.read, around.read, "no read grant drops a deny");
    assert!(!around.would_allow(&Blocked::FsRead {
        path: PathBuf::from("/opt/secret/x")
    }));
    assert!(!policy.would_allow(&Blocked::FsWrite {
        path: f.ws.join("a.pem")
    }));
    assert!(policy.would_allow(&Blocked::FsWrite {
        path: f.ws.join("a.txt")
    }));
    assert!(policy.holds_read_deny(Path::new("/opt")));
    assert!(!policy.holds_read_deny(Path::new("/opt/secret")));
    assert!(!policy.holds_read_deny(Path::new("/usr")));
}

/// A read grant on the home directory itself is allowed, but the secret stores inside it stay
/// denied: the floor wins over a broad read grant.
#[test]
fn read_grant_on_home_keeps_the_protected_denies() {
    let f = Fixture::new("readhome");
    let policy = f.build(&[], None);
    let widened = policy
        .with_grant(&grant(GrantSubject::FsRead {
            root: f.home.clone(),
        }))
        .unwrap();
    for rel in [
        ".ssh/id_ed25519",
        ".aws/credentials",
        ".netrc",
        ".kube/config",
    ] {
        assert!(
            !widened.would_allow(&Blocked::FsRead {
                path: f.home.join(rel)
            }),
            "{rel} stays denied"
        );
    }
    assert!(widened.would_allow(&Blocked::FsRead {
        path: f.home.join("notes.txt")
    }));
}

/// How a cell of the secret-read table widens reads over the home directory.
#[derive(Clone, Copy, Debug)]
enum HomeWidening {
    None,
    WorkspaceIsHome,
    ReadOnlyAboveHome,
    GrantHome,
    GrantConfig,
}

/// Every secret the policy read-denies, as the path a command would open: a file inside each
/// store, each credential file, one match of each grok-home glob.
fn secret_read_probes(home: &Path, grok_home: &Path) -> Vec<PathBuf> {
    SECRET_READ_DENY_DIRS
        .iter()
        .map(|rel| home.join(rel).join("id_ed25519"))
        .chain(SECRET_READ_DENY_FILES.iter().map(|rel| home.join(rel)))
        .chain(
            GROK_HOME_SECRET_GLOBS
                .iter()
                .map(|glob| grok_home.join(glob.replace('*', ".json"))),
        )
        .collect()
}

/// The read denies hold in both read modes under every widening that reaches a secret: a
/// home-folder workspace, a `read_only` root above the home, a read grant of `~` or of an
/// ancestor of the entry. Each cell pairs a shape with a secret the shape reaches, and the
/// shape's control sibling must stay readable, so a cell passes only because the deny holds.
#[test]
fn secret_reads_stay_denied_in_every_read_mode_under_every_widening() {
    let f = Fixture::new("secret-table");
    for rel in [".ssh", ".aws", ".gnupg", "Library/Keychains"] {
        assert!(SECRET_READ_DENY_DIRS.contains(&rel), "{rel} left the table");
    }
    for rel in [
        ".netrc",
        ".npmrc",
        ".pypirc",
        ".docker/config.json",
        ".kube/config",
        ".config/gh/hosts.yml",
        ".git-credentials",
    ] {
        assert!(
            SECRET_READ_DENY_FILES.contains(&rel),
            "{rel} left the table"
        );
    }
    assert_eq!(GROK_HOME_SECRET_GLOBS, ["auth*", "credentials*"]);
    let config = f.home.join(".config");
    let probes = secret_read_probes(&f.home, &f.grok_home);
    let mut cells = 0;
    let mut wrong: Vec<String> = Vec::new();
    for default_read in [true, false] {
        for widening in [
            HomeWidening::None,
            HomeWidening::WorkspaceIsHome,
            HomeWidening::ReadOnlyAboveHome,
            HomeWidening::GrantHome,
            HomeWidening::GrantConfig,
        ] {
            // Restricted roots with nothing widened never reach the home directory
            if !default_read && matches!(widening, HomeWidening::None) {
                continue;
            }
            let ws = match widening {
                HomeWidening::WorkspaceIsHome => &f.home,
                _ => &f.ws,
            };
            let mut profile = profile(ws, &f.grok_home);
            profile.default_read = default_read;
            if matches!(widening, HomeWidening::ReadOnlyAboveHome) {
                profile.read_only = f.home.parent().map(Path::to_path_buf).into_iter().collect();
            }
            let grants: Vec<Grant> = match widening {
                HomeWidening::GrantHome => vec![grant(GrantSubject::FsRead {
                    root: f.home.clone(),
                })],
                HomeWidening::GrantConfig => vec![grant(GrantSubject::FsRead {
                    root: config.clone(),
                })],
                _ => Vec::new(),
            };
            let policy = SandboxPolicy::build(PolicyInputs {
                workspace_root: &ServedRoot::pin(ws),
                profile: &profile,
                grants: &grants,
                proxy: None,
                tmp_dirs: &[],
                control_socket_dir: &f.grok_home.join("workspaced"),
                grok_home: &f.grok_home,
                user_home: Some(&f.home),
                git_env: &GitConfigEnv::default(),
            })
            .expect("policy builds");
            let reach = match widening {
                HomeWidening::GrantConfig => &config,
                _ => &f.home,
            };
            let shape = format!("default_read={default_read} {widening:?}");
            let control = reach.join("notes.txt");
            if !policy.would_allow(&Blocked::FsRead {
                path: control.clone(),
            }) {
                wrong.push(format!("{shape}: control {} unreadable", control.display()));
            }
            for probe in probes.iter().filter(|probe| probe.starts_with(reach)) {
                cells += 1;
                if policy.would_allow(&Blocked::FsRead {
                    path: probe.clone(),
                }) {
                    wrong.push(format!("{shape}: {} readable", probe.display()));
                }
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "{} of {cells} cells wrong:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
    assert!(cells >= 7 * probes.len(), "{cells} cells");
}

/// No grant, not even "all hosts", changes the kernel network policy. The
/// proxy's decider applies network grants; the policy stays `Off` or `Proxy`.
#[test]
fn network_grants_never_change_the_kernel_policy() {
    let f = Fixture::new("net");
    let net = Blocked::Net {
        host: Some("registry.npmjs.org".to_owned()),
        port: Some(443),
    };
    for proxy in [None, Some(ProxyEndpoint { port: 3128 })] {
        let policy = f.build(&[], proxy);
        let before = policy.network.clone();
        let host_only = policy
            .clone()
            .with_grant(&grant(GrantSubject::NetHost {
                host: HostPattern::new("registry.npmjs.org"),
                port: Some(443),
            }))
            .unwrap();
        assert_eq!(before, host_only.network);
        assert!(!host_only.would_allow(&net));
        let all = policy
            .with_grant(&grant(GrantSubject::NetHost {
                host: HostPattern::all(),
                port: None,
            }))
            .unwrap();
        assert_eq!(before, all.network, "all-hosts grant is a proxy decision");
        assert!(!all.would_allow(&net));
    }
}

#[test]
fn live_grants_are_applied_at_build() {
    let f = Fixture::new("build-grants");
    let outside = f.home.join("proj3");
    std::fs::create_dir_all(&outside).unwrap();
    let grants = [grant(GrantSubject::FsWriteRoot {
        root: outside.clone(),
    })];
    let policy = f.build(&grants, None);
    assert!(policy.would_allow(&Blocked::FsWrite {
        path: outside.join("x")
    }));
}

#[test]
fn capabilities_and_unknown_are_never_allowed() {
    let f = Fixture::new("cap");
    let policy = f.build(&[], None);
    assert!(!policy.would_allow(&Blocked::Capability {
        what: crate::command::violation::Capability::Ptrace
    }));
    assert!(!policy.would_allow(&Blocked::Unknown {
        stderr_snippet: String::new(),
    }));
}

fn deny_globs(policy: &SandboxPolicy) -> Vec<String> {
    policy
        .read
        .deny()
        .iter()
        .filter_map(|entry| match entry {
            DenyEntry::Glob { root, tail } => Some(root.join(tail).to_string_lossy().into_owned()),
            DenyEntry::Path(_) | DenyEntry::TreeExcept { .. } => None,
        })
        .collect()
}

#[test]
fn every_deny_glob_is_absolute_after_home_and_workspace_anchoring() {
    let f = Fixture::new("globs");
    let mut profile = profile(&f.ws, &f.grok_home);
    profile.deny = vec![
        PathBuf::from("~/keys/**"),
        PathBuf::from("secrets/**"),
        PathBuf::from("/srv/**/*.key"),
    ];
    let policy = SandboxPolicy::build(PolicyInputs {
        workspace_root: &ServedRoot::pin(&f.ws),
        profile: &profile,
        grants: &[],
        proxy: None,
        tmp_dirs: &[],
        control_socket_dir: &f.grok_home.join("workspaced"),
        grok_home: &f.grok_home,
        user_home: Some(&f.home),
        git_env: &GitConfigEnv::default(),
    })
    .unwrap();
    let globs = deny_globs(&policy);
    assert!(
        globs.iter().all(|g| Path::new(g).is_absolute()),
        "{globs:?}"
    );
    assert!(globs.contains(&f.home.join("keys/**").to_string_lossy().into_owned()));
    assert!(globs.contains(&f.ws.join("secrets/**").to_string_lossy().into_owned()));
    assert!(globs.contains(&"/srv/**/*.key".to_owned()));
    assert!(!policy.would_allow(&Blocked::FsRead {
        path: f.home.join("keys/a")
    }));
    assert!(!policy.would_allow(&Blocked::FsRead {
        path: f.ws.join("secrets/b")
    }));
}

fn build_with_profile(
    f: &Fixture,
    profile: &SandboxProfile,
    user_home: Option<&Path>,
) -> Result<SandboxPolicy, PolicyError> {
    SandboxPolicy::build(PolicyInputs {
        workspace_root: &ServedRoot::pin(&f.ws),
        profile,
        grants: &[],
        proxy: None,
        tmp_dirs: &[],
        control_socket_dir: &f.grok_home.join("workspaced"),
        grok_home: &f.grok_home,
        user_home,
        git_env: &GitConfigEnv::default(),
    })
}

#[test]
fn plain_relative_and_home_denies_are_anchored_like_globs() {
    let f = Fixture::new("plain-deny");
    let mut profile = profile(&f.ws, &f.grok_home);
    profile.deny = vec![
        PathBuf::from(".env"),
        PathBuf::from("certs/server.key"),
        PathBuf::from("~/keys"),
        PathBuf::from("/opt/secret"),
    ];
    let policy = build_with_profile(&f, &profile, Some(&f.home)).unwrap();
    let deny = policy.read.deny();
    for path in [
        f.ws.join(".env"),
        f.ws.join("certs/server.key"),
        f.home.join("keys"),
        PathBuf::from("/opt/secret"),
    ] {
        assert!(
            deny.contains(&DenyEntry::Path(path.clone())),
            "{path:?} in {deny:?}"
        );
    }
    assert!(
        deny.iter().all(|entry| match entry {
            DenyEntry::Path(path) => path.is_absolute(),
            DenyEntry::Glob { root, .. } => root.is_absolute(),
            DenyEntry::TreeExcept { tree, .. } => tree.is_absolute(),
        }),
        "{deny:?}"
    );
    let denied = |path: PathBuf| !policy.would_allow(&Blocked::FsRead { path });
    assert!(denied(f.ws.join(".env")));
    assert!(denied(f.ws.join("certs/server.key")));
    assert!(denied(f.home.join("keys/id")));
    assert!(!denied(f.ws.join("src/.env.example")));

    let err = build_with_profile(&f, &profile, None).unwrap_err();
    assert!(matches!(err, PolicyError::NotAbsolute { .. }), "{err}");
}

/// Relative and `~` denies are anchored where the workspace and the home resolve, as the write
/// roots are: a workspace served through a symlink still denies its `.env` where the kernel
/// meets it.
#[cfg(unix)]
#[test]
fn denies_are_anchored_at_the_resolved_workspace_and_home() {
    let f = Fixture::new("resolved-deny");
    let ws_link = f.ws.with_file_name("ws-link");
    let home_link = f.home.with_file_name("home-link");
    std::os::unix::fs::symlink(&f.ws, &ws_link).unwrap();
    std::os::unix::fs::symlink(&f.home, &home_link).unwrap();
    let mut profile = profile(&f.ws, &f.grok_home);
    profile.deny = [".env", "secrets/**", "~/keys"].map(PathBuf::from).to_vec();
    let policy = SandboxPolicy::build(PolicyInputs {
        workspace_root: &ServedRoot::pin(&ws_link),
        profile: &profile,
        grants: &[],
        proxy: None,
        tmp_dirs: &[],
        control_socket_dir: &f.grok_home.join("workspaced"),
        grok_home: &f.grok_home,
        user_home: Some(&home_link),
        git_env: &GitConfigEnv::default(),
    })
    .unwrap();
    let deny = policy.read.deny();
    let glob = DenyEntry::Glob {
        root: f.ws.clone(),
        tail: "secrets/**".to_owned(),
    };
    for entry in [
        DenyEntry::Path(f.ws.join(".env")),
        DenyEntry::Path(f.home.join("keys")),
        glob,
    ] {
        assert!(deny.contains(&entry), "{entry:?} in {deny:?}");
    }
}

/// A stow-style `~/.config -> dotfiles` holding the git config pins the link itself and the config
/// where it leads, never the tree: the rest of `~/dotfiles` stays grantable and writable.
#[cfg(unix)]
#[test]
fn a_linked_git_config_directory_pins_the_link_and_leaves_its_target_grantable() {
    let f = Fixture::new("stow-link");
    let dotfiles = f.home.join("dotfiles");
    std::fs::create_dir_all(dotfiles.join("git")).unwrap();
    std::fs::write(dotfiles.join("git/config"), "[user]\n\tname = me\n").unwrap();
    std::os::unix::fs::symlink("dotfiles", f.home.join(".config")).unwrap();
    let write = grant(GrantSubject::FsWriteRoot {
        root: dotfiles.clone(),
    });
    let policy = f.build(&[write], None);
    let link = f.home.join(".config");
    assert!(
        policy
            .protected
            .contains(&Protected::Node { path: link.clone() }),
        "{:?}",
        policy.protected
    );
    assert!(contains_path(&policy.write_roots, &dotfiles), "{policy:?}");
    for (path, allowed) in [
        (dotfiles.join("zsh/rc"), true),
        (link.join("zsh/rc"), true),
        (dotfiles.join("git/config"), false),
        (link.join("git/config"), false),
        (link.clone(), false),
    ] {
        let blocked = Blocked::FsWrite { path: path.clone() };
        assert_eq!(allowed, policy.would_allow(&blocked), "{path:?}");
    }
}

/// A secret store, floor entry or `read_only` root spelled through a symlink holds at its target
/// (a read root in the workspace stays folded), no read grant opens a secret, and a `read_write`
/// root in the workspace reached through a link is left out, never widened to its target.
#[cfg(unix)]
#[test]
fn an_entry_spelled_through_a_symlink_holds_at_its_target() {
    let f = Fixture::new("secret-link");
    let kube = f.home.join(".kube");
    std::fs::create_dir_all(&kube).unwrap();
    std::fs::write(kube.join("config-prod"), "").unwrap();
    std::os::unix::fs::symlink("config-prod", kube.join("config")).unwrap();
    let ssh = f.home.join("dotfiles/ssh");
    std::fs::create_dir_all(&ssh).unwrap();
    std::os::unix::fs::symlink(&ssh, f.home.join(".ssh")).unwrap();
    let vscode = f.ws.with_file_name("shared-vscode");
    std::fs::create_dir_all(&vscode).unwrap();
    std::os::unix::fs::symlink(&vscode, f.ws.join(".vscode")).unwrap();
    let policy = f.build(&[], None);
    for secret in [kube.join("config-prod"), ssh.join("id_ed25519")] {
        assert!(policy.is_read_floor(&secret), "{secret:?}");
        assert!(policy.is_protected(&secret), "{secret:?}");
        let read = grant(GrantSubject::FsRead {
            root: secret.parent().unwrap().to_path_buf(),
        });
        let widened = policy.clone().with_grant(&read).unwrap_or(policy.clone());
        assert!(
            !widened.would_allow(&Blocked::FsRead {
                path: secret.clone()
            }),
            "{secret:?}"
        );
    }
    assert!(policy.is_protected(&vscode.join("settings.json")));
    let notes = f.home.join("dotfiles/notes");
    std::fs::create_dir_all(&notes).unwrap();
    std::os::unix::fs::symlink(&notes, f.home.join("notes")).unwrap();
    let out = f.ws.with_file_name("shared-out");
    std::fs::create_dir_all(&out).unwrap();
    std::os::unix::fs::symlink(&out, f.ws.join("out")).unwrap();
    let mut restricted = profile(&f.ws, &f.grok_home);
    restricted.default_read = false;
    restricted.read_only = vec![f.home.join("notes"), f.ws.join("src/../.vscode")];
    let outside = f.ws.join("../home").join("notes");
    restricted
        .read_write
        .extend([outside, f.ws.join("src/../out")]);
    let policy = build_with_profile(&f, &restricted, Some(&f.home)).unwrap();
    std::fs::remove_file(f.ws.join("out")).unwrap();
    std::fs::create_dir(f.ws.join("out")).unwrap();
    let unlinked = build_with_profile(&f, &restricted, Some(&f.home)).unwrap();
    for (writes, root, held) in [
        (&policy.write_roots, &notes, true),
        (&policy.write_roots, &f.home.join("notes"), false),
        (&policy.write_roots, &f.ws.join("out"), false),
        (&policy.write_roots, &out, false),
        (&unlinked.write_roots, &f.ws.join("out"), true),
    ] {
        assert_eq!(held, writes.contains(root), "{root:?} in {writes:?}");
    }
    let ReadPolicy::Roots { roots, .. } = &policy.read else {
        panic!("{:?}", policy.read);
    };
    for (root, held) in [
        (notes, true),
        (f.home.join("notes"), false),
        (f.ws.join(".vscode"), true),
        (vscode, false),
    ] {
        assert_eq!(held, roots.contains(&root), "{root:?} in {roots:?}");
    }
}

/// The profile's grok-home entry is matched in its canonical spelling, so a symlinked or
/// `..`-spelled grok home is still left out and the session command directory is the one write
/// root there.
#[cfg(unix)]
#[test]
fn a_differently_spelled_grok_home_in_the_profile_still_narrows_to_the_command_dir() {
    let f = Fixture::new("grok-home-spelling");
    let link = f.home.with_file_name("home-link");
    std::os::unix::fs::symlink(&f.home, &link).unwrap();
    for spelling in [link.join(".grok"), f.grok_home.join("sessions/..")] {
        let mut profile = profile(&f.ws, &f.grok_home);
        profile.read_write = vec![f.ws.clone(), spelling.clone()];
        let policy = build_with_profile(&f, &profile, Some(&f.home)).unwrap();
        let own_session =
            xai_grok_config::sessions_cwd_dir_in(&f.grok_home, &f.ws.to_string_lossy());
        assert!(
            policy.write_roots.contains(&own_session.join("commands")),
            "{spelling:?}"
        );
        assert!(!policy.write_roots.contains(&own_session), "{spelling:?}");
        assert!(
            !policy.write_roots.contains(&f.grok_home),
            "{spelling:?} widened to the whole grok home: {:?}",
            policy.write_roots
        );
    }
}

/// A workspace served from inside the grok home (a grok-managed worktree) is the one writable
/// tree there: the MCP config, extensions, the trust files, a name grok adds later and the
/// worktree's session directory — its command directory included — are not writable.
#[test]
fn a_worktree_inside_the_grok_home_is_its_only_writable_tree() {
    let f = Fixture::new("worktree-in-home");
    let worktree = f.grok_home.join("worktrees/repo/feature");
    let session = xai_grok_config::sessions_cwd_dir_in(&f.grok_home, &worktree.to_string_lossy());
    for dir in [&worktree, &session.join("commands")] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let profile = profile(&worktree, &f.grok_home);
    let policy = SandboxPolicy::build(PolicyInputs {
        workspace_root: &ServedRoot::pin(&worktree),
        profile: &profile,
        grants: &[],
        proxy: None,
        tmp_dirs: &[],
        control_socket_dir: &f.grok_home.join("workspaced"),
        grok_home: &f.grok_home,
        user_home: Some(&f.home),
        git_env: &GitConfigEnv::default(),
    })
    .unwrap();
    let write = |path: PathBuf| policy.would_allow(&Blocked::FsWrite { path });
    assert!(write(worktree.join("src/main.rs")));
    for path in [
        f.grok_home.join("mcp.json"),
        f.grok_home.join("extensions/x/run.sh"),
        f.grok_home.join("config.toml"),
        f.grok_home.join("a-name-grok-adds-later"),
        session.join("commands/out.log"),
        session.join("sandbox_grants.toml"),
    ] {
        assert!(!write(path.clone()), "{path:?}");
        assert!(policy.is_ungrantable(&path), "{path:?}");
    }
    assert!(
        !policy
            .write_roots
            .iter()
            .any(|root| root.starts_with(&session)),
        "{:?}",
        policy.write_roots
    );
}

#[test]
fn home_glob_without_a_home_directory_is_not_absolute() {
    let f = Fixture::new("nohome");
    let mut profile = profile(&f.ws, &f.grok_home);
    profile.deny = vec![PathBuf::from("~/keys/**")];
    let err = SandboxPolicy::build(PolicyInputs {
        workspace_root: &ServedRoot::pin(&f.ws),
        profile: &profile,
        grants: &[],
        proxy: None,
        tmp_dirs: &[],
        control_socket_dir: &f.grok_home.join("workspaced"),
        grok_home: &f.grok_home,
        user_home: None,
        git_env: &GitConfigEnv::default(),
    })
    .unwrap_err();
    assert!(matches!(err, PolicyError::NotAbsolute { .. }), "{err}");
}

#[test]
fn default_tmp_dirs_are_absolute_deduplicated_and_include_tmpdir() {
    let home = PathBuf::from("/opt/ws-fixture/u");
    let tmpdir = std::env::temp_dir();
    let dirs = default_tmp_dirs(Some(&tmpdir), Some(&home));
    assert!(dirs.iter().all(|d| d.is_absolute()), "{dirs:?}");
    assert!(dirs.contains(&tmpdir));
    let mut sorted = dirs.clone();
    sorted.dedup();
    assert_eq!(dirs, sorted, "no duplicate entries");
    if cfg!(target_os = "macos") {
        assert!(dirs.contains(&PathBuf::from("/private/tmp")));
    } else if cfg!(unix) {
        assert!(dirs.contains(&PathBuf::from("/tmp")));
    }
    let relative = default_tmp_dirs(Some(Path::new("relative/tmp")), Some(&home));
    assert!(!relative.iter().any(|d| d.ends_with("relative/tmp")));
}

#[test]
fn policy_round_trips_through_json() {
    let f = Fixture::new("json");
    let policy = f.build(&[], None);
    let text = serde_json::to_string(&policy).unwrap();
    let back: SandboxPolicy = serde_json::from_str(&text).unwrap();
    assert_eq!(policy, back);
}

/// A workspace served by the spelling `<root>/link/ws`: `link` points at the fixture's root, so
/// the spelling resolves to `f.ws`. `<grok_home>/elsewhere` is where a command writing the link's
/// parent could point it instead.
#[cfg(unix)]
struct LinkedRoot {
    f: Fixture,
    link: PathBuf,
    spelled: PathBuf,
    elsewhere: PathBuf,
    control_socket_dir: PathBuf,
}

#[cfg(unix)]
impl LinkedRoot {
    fn new(tag: &str) -> LinkedRoot {
        let f = Fixture::new(tag);
        let parent = f.ws.parent().unwrap().to_path_buf();
        let link = parent.join("link");
        std::os::unix::fs::symlink(&parent, &link).unwrap();
        let elsewhere = f.grok_home.join("elsewhere");
        std::fs::create_dir_all(elsewhere.join("ws")).unwrap();
        LinkedRoot {
            spelled: link.join("ws"),
            control_socket_dir: f.grok_home.join("workspaced"),
            f,
            link,
            elsewhere,
        }
    }

    /// Where the spelling resolves while the link points at `elsewhere`.
    fn moved(&self) -> PathBuf {
        self.elsewhere.join("ws")
    }

    fn point_link_at(&self, target: &Path) {
        std::fs::remove_file(&self.link).unwrap();
        std::os::unix::fs::symlink(target, &self.link).unwrap();
    }

    /// The real workspace moved aside and a link to [`LinkedRoot::moved`] put in its place.
    fn replace_real_with_link(&self) {
        std::fs::rename(&self.f.ws, self.f.ws.with_file_name("ws-away")).unwrap();
        std::os::unix::fs::symlink(self.moved(), &self.f.ws).unwrap();
    }

    fn inputs<'a>(&'a self, root: &'a ServedRoot, profile: &'a SandboxProfile) -> PolicyInputs<'a> {
        PolicyInputs {
            workspace_root: root,
            profile,
            grants: &[],
            proxy: None,
            tmp_dirs: &[],
            control_socket_dir: &self.control_socket_dir,
            grok_home: &self.f.grok_home,
            user_home: Some(&self.f.home),
            git_env: &self.f.git_env,
        }
    }
}

/// The roots of a policy's allows and denies: write, cache, read and deny roots.
#[cfg(unix)]
fn rule_roots(policy: &SandboxPolicy) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = policy.write_roots.clone();
    paths.extend(policy.build_cache_trees.iter().cloned());
    if let ReadPolicy::Roots { roots, .. } = &policy.read {
        paths.extend(roots.iter().cloned());
    }
    for entry in policy.read.deny() {
        match entry {
            DenyEntry::Path(path) | DenyEntry::Glob { root: path, .. } => paths.push(path.clone()),
            DenyEntry::TreeExcept { tree, except } => paths.extend([tree.clone(), except.clone()]),
        }
    }
    paths
}

/// Every path a policy names: [`rule_roots`] and the floor.
#[cfg(unix)]
fn policy_paths(policy: &SandboxPolicy) -> Vec<PathBuf> {
    let mut paths = rule_roots(policy);
    for entry in &policy.protected {
        match entry {
            Protected::Path { path } | Protected::Node { path } => paths.push(path.clone()),
            Protected::Glob { glob } => paths.push(PathBuf::from(glob)),
            Protected::TreeExcept { tree, except } => paths.extend([tree.clone(), except.clone()]),
        }
    }
    paths
}

/// [`ServedRoot`] invariant 1: past the check nothing resolves the root again. With the link
/// retargeted after the pin, every rule anchored at the workspace sits at the pinned real path
/// and none under the spelling or where it points now: the write, read and deny roots, the
/// profile's entries spelled under the root, the floor and the session command directory's
/// standing. With the real path swapped for a link instead, the root's own rules stay the real
/// path as pinned.
#[cfg(unix)]
#[test]
fn a_pinned_policy_never_resolves_the_spelling_again() {
    let l = LinkedRoot::new("pin-once");
    let real = &l.f.ws;
    let own = xai_grok_config::sessions_cwd_dir_in(&l.f.grok_home, &l.spelled.to_string_lossy());
    std::fs::create_dir_all(own.join(protected::SESSION_COMMAND_DIRNAME)).unwrap();
    let mut under_root = profile(&l.spelled, &l.f.grok_home);
    under_root.default_read = false;
    under_root.read_only = vec![l.spelled.join("docs")];
    under_root.read_write.push(l.spelled.join("build"));
    under_root.deny = vec![PathBuf::from(".env"), l.spelled.join("secret")];
    under_root.write_deny = vec![xai_grok_config::GlobalHookSource {
        path: l.spelled.join("team-hooks"),
        kind: xai_grok_config::GlobalHookSourceKind::ConfiguredSource,
    }];
    let root = ServedRoot::pin(&l.spelled);
    l.point_link_at(&l.elsewhere);
    let policy = SandboxPolicy::build_at_pin(&l.inputs(&root, &under_root)).unwrap();
    for write_root in [
        real.clone(),
        real.join("build"),
        canonical_path(&own).join(protected::SESSION_COMMAND_DIRNAME),
    ] {
        assert!(
            policy.write_roots.contains(&write_root),
            "{write_root:?} in {:?}",
            policy.write_roots
        );
    }
    let ReadPolicy::Roots { roots, deny } = &policy.read else {
        panic!("{:?}", policy.read);
    };
    assert!(roots.contains(real), "{roots:?}");
    assert!(roots.contains(&real.join("docs")), "{roots:?}");
    for entry in [
        DenyEntry::Path(real.join(".env")),
        DenyEntry::Path(real.join("secret")),
    ] {
        assert!(deny.contains(&entry), "{entry:?} in {deny:?}");
    }
    assert!(policy.is_protected(&real.join(".grok/settings.toml")));
    assert!(policy.is_protected(&real.join("team-hooks/format.json")));
    for path in policy_paths(&policy) {
        assert!(
            !is_within(&path, &l.spelled) && !is_within(&path, &l.elsewhere),
            "{path:?} in {policy:?}"
        );
    }

    let l = LinkedRoot::new("pin-once-real");
    let mut profile = profile(&l.spelled, &l.f.grok_home);
    profile.default_read = false;
    profile.deny = vec![PathBuf::from(".env")];
    let root = ServedRoot::pin(&l.spelled);
    l.replace_real_with_link();
    let policy = SandboxPolicy::build_at_pin(&l.inputs(&root, &profile)).unwrap();
    assert!(
        policy.write_roots.contains(&l.f.ws),
        "{:?}",
        policy.write_roots
    );
    assert!(
        matches!(&policy.read, ReadPolicy::Roots { roots, .. } if roots.contains(&l.f.ws)),
        "{:?}",
        policy.read
    );
    assert!(
        policy
            .read
            .deny()
            .contains(&DenyEntry::Path(l.f.ws.join(".env")))
    );
    for path in rule_roots(&policy) {
        assert!(!is_within(&path, &l.elsewhere), "{path:?} in {policy:?}");
    }
}

/// [`ServedRoot`] invariant 2: the spelling keys the folder's session directory, the one the hub
/// names by the text it opened, and no rule sits under the spelling itself.
#[cfg(unix)]
#[test]
fn the_spelling_keys_the_session_directory_and_nothing_else() {
    let l = LinkedRoot::new("pin-key");
    let root = ServedRoot::pin(&l.spelled);
    let keyed = |ws: &Path| {
        canonical_path(&xai_grok_config::sessions_cwd_dir_in(
            &l.f.grok_home,
            &ws.to_string_lossy(),
        ))
    };
    let own = keyed(&l.spelled);
    assert_ne!(own, keyed(&l.f.ws));
    let protected_inputs = ProtectedInputs {
        workspace_root: &root,
        grok_home: &l.f.grok_home,
        user_home: Some(&l.f.home),
        control_socket_dir: &l.control_socket_dir,
        git_env: &l.f.git_env,
    };
    assert_eq!(own, protected::own_session_dir(&protected_inputs));
    let profile = profile(&l.spelled, &l.f.grok_home);
    let policy = SandboxPolicy::build(l.inputs(&root, &profile)).unwrap();
    let sessions = DenyEntry::TreeExcept {
        tree: canonical_path(&l.f.grok_home).join("sessions"),
        except: own.clone(),
    };
    assert!(policy.read.deny().contains(&sessions), "{:?}", policy.read);
    let grants_file = Protected::Path {
        path: own.join(protected::GLOBAL_GRANTS_FILENAME),
    };
    assert!(policy.protected.contains(&grants_file), "{policy:?}");
    for path in policy_paths(&policy) {
        assert!(!is_within(&path, &l.spelled), "{path:?} in {policy:?}");
    }
}

/// [`ServedRoot`] invariant 3: once the spelling resolves anywhere but the pinned real path (the
/// link retargeted or removed, or the real path swapped for a link, for a folder opened through
/// the link or at its real path), the build refuses with the one reason naming both paths.
#[cfg(unix)]
#[test]
fn a_moved_root_refuses_the_build_with_one_reason() {
    type Move = fn(&LinkedRoot) -> PathBuf;
    let cases: [(&str, bool, Move); 4] = [
        ("link retargeted", true, |l| {
            l.point_link_at(&l.elsewhere);
            l.moved()
        }),
        ("link removed", true, |l| {
            std::fs::remove_file(&l.link).unwrap();
            l.spelled.clone()
        }),
        ("real path replaced", true, |l| {
            l.replace_real_with_link();
            l.moved()
        }),
        ("real path replaced, opened there", false, |l| {
            l.replace_real_with_link();
            l.moved()
        }),
    ];
    for (index, (case, through_link, move_root)) in cases.into_iter().enumerate() {
        let l = LinkedRoot::new(&format!("pin-moved-{index}"));
        let spelled = if through_link {
            l.spelled.clone()
        } else {
            l.f.ws.clone()
        };
        let root = ServedRoot::pin(&spelled);
        let profile = profile(&spelled, &l.f.grok_home);
        if let Err(error) = SandboxPolicy::build(l.inputs(&root, &profile)) {
            panic!("{case}: builds before the move: {error}");
        }
        let now = move_root(&l);
        let error = SandboxPolicy::build(l.inputs(&root, &profile)).unwrap_err();
        let expected = RootMoved {
            pinned: l.f.ws.clone(),
            now: now.clone(),
        };
        assert!(
            matches!(&error, PolicyError::RootMoved(moved) if *moved == expected),
            "{case}: {error:?}"
        );
        assert_eq!(
            format!(
                "the folder's real path changed from {} to {}; re-open the folder",
                l.f.ws.display(),
                now.display()
            ),
            error.to_string(),
            "{case}"
        );
    }
}

/// [`ServedRoot`] invariant 4: the refusal outlives the move (the link put back builds nothing
/// under that pin) until the folder is served again and pins where the spelling resolves then;
/// a temporary directory's pin refuses the same way, the profile's entry for it included.
#[cfg(unix)]
#[test]
fn a_moved_root_stays_refused_until_pinned_again() {
    let l = LinkedRoot::new("pin-latch");
    let profile = profile(&l.spelled, &l.f.grok_home);
    let root = ServedRoot::pin(&l.spelled);
    l.point_link_at(&l.elsewhere);
    let moved = RootMoved {
        pinned: l.f.ws.clone(),
        now: l.moved(),
    };
    let first = SandboxPolicy::build(l.inputs(&root, &profile)).unwrap_err();
    assert!(
        matches!(&first, PolicyError::RootMoved(m) if *m == moved),
        "{first:?}"
    );
    l.point_link_at(l.f.ws.parent().unwrap());
    let restored = SandboxPolicy::build(l.inputs(&root, &profile)).unwrap_err();
    assert!(
        matches!(&restored, PolicyError::RootMoved(m) if *m == moved),
        "{restored:?}"
    );

    let served_again = ServedRoot::pin(&l.spelled);
    let policy = SandboxPolicy::build(l.inputs(&served_again, &profile)).unwrap();
    assert!(
        policy.write_roots.contains(&l.f.ws),
        "{:?}",
        policy.write_roots
    );
    l.point_link_at(&l.elsewhere);
    let served_there = ServedRoot::pin(&l.spelled);
    let policy = SandboxPolicy::build(l.inputs(&served_there, &profile)).unwrap();
    assert!(
        policy.write_roots.contains(&l.moved()) && !policy.write_roots.contains(&l.f.ws),
        "{:?}",
        policy.write_roots
    );

    l.point_link_at(l.f.ws.parent().unwrap());
    let (root, tmp) = (
        ServedRoot::pin(&l.f.ws),
        [ServedRoot::pin(l.link.join("tmp"))],
    );
    let profile = SandboxProfile {
        read_write: vec![l.f.ws.clone(), l.link.join("tmp")],
        ..profile
    };
    let inputs = || PolicyInputs {
        tmp_dirs: &tmp,
        ..l.inputs(&root, &profile)
    };
    let policy = SandboxPolicy::build(inputs()).unwrap();
    assert!(policy.write_roots.contains(&l.f.tmp), "{policy:?}");
    let moved = RootMoved {
        pinned: l.f.tmp.clone(),
        now: l.elsewhere.join("tmp"),
    };
    for target in [&l.elsewhere, l.f.ws.parent().unwrap()] {
        l.point_link_at(target);
        let error = SandboxPolicy::build(inputs()).unwrap_err();
        assert!(
            matches!(&error, PolicyError::TmpDirMoved(m) if *m == moved),
            "{error:?}"
        );
        assert!(error.to_string().ends_with("restart the daemon"), "{error}");
    }
}

/// [`ServedRoot`] invariant 5: a root opened through a link that has not moved is no refusal; it
/// gets the policy of its real path (the same write roots, the same rules under the workspace),
/// the session directory being the one thing keyed by the spelling.
#[cfg(unix)]
#[test]
fn a_root_opened_through_a_link_builds_at_its_real_path() {
    let l = LinkedRoot::new("pin-link");
    let build = |spelled: &Path| {
        let root = ServedRoot::pin(spelled);
        let profile = profile(spelled, &l.f.grok_home);
        SandboxPolicy::build(l.inputs(&root, &profile))
            .unwrap_or_else(|error| panic!("{spelled:?}: {error}"))
    };
    let through = build(&l.spelled);
    let direct = build(&l.f.ws);
    assert!(
        through.write_roots.contains(&l.f.ws),
        "{:?}",
        through.write_roots
    );
    let outside_grok_home = |policy: &SandboxPolicy| -> Vec<PathBuf> {
        let roots = policy.write_roots.iter();
        roots
            .filter(|root| !is_within(root, &l.f.grok_home))
            .cloned()
            .collect()
    };
    assert_eq!(outside_grok_home(&direct), outside_grok_home(&through));
    let under_workspace = |policy: &SandboxPolicy| -> Vec<PathBuf> {
        let paths = policy_paths(policy).into_iter();
        paths.filter(|path| is_within(path, &l.f.ws)).collect()
    };
    assert_eq!(under_workspace(&direct), under_workspace(&through));
    assert!(!under_workspace(&through).is_empty());
}
