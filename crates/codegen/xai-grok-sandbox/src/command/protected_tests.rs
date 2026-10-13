#[cfg(unix)]
use std::os::fd::AsFd as _;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
#[cfg(unix)]
use std::sync::Arc;
#[cfg(unix)]
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};

use super::*;
use crate::command::canonical::{VolumeRule, with_volume_rule};
use crate::command::git_config::{GitEntries, git_entries_in};

/// The git entries with git's default places for the global config.
fn git_entries(ws: &Path, user_home: Option<&Path>, write_roots: &[PathBuf]) -> GitEntries {
    git_entries_in(ws, user_home, write_roots, &GitConfigEnv::default())
}

fn inputs<'a>(ws: &'a ServedRoot, grok_home: &'a Path, home: &'a Path) -> ProtectedInputs<'a> {
    ProtectedInputs {
        workspace_root: ws,
        grok_home,
        user_home: Some(home),
        control_socket_dir: Path::new("/run/grok/ctl"),
        git_env: &GitConfigEnv {
            config_global: None,
            xdg_config_home: None,
        },
    }
}

fn path(p: impl Into<PathBuf>) -> Protected {
    Protected::Path { path: p.into() }
}

fn glob(g: impl Into<String>) -> Protected {
    Protected::Glob { glob: g.into() }
}

fn scratch(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "xai-sandbox-protected-{}-{tag}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    dunce::canonicalize(&root).unwrap()
}

/// Pins the table. A change here is a change to what the card says can never
/// be allowed and to what every backend renders last; update the desktop copy and the user guide
/// with it. Paths that exist nowhere, so canonicalisation leaves the spelling alone.
#[test]
fn protected_table_is_pinned() {
    let ws = Path::new("/opt/ws-fixture/ws");
    let grok_home = Path::new("/opt/ws-fixture/u/.grok");
    let home = Path::new("/opt/ws-fixture/u");
    let got = floor(&inputs(&ServedRoot::pin(ws), grok_home, home));
    let own_session = xai_grok_config::sessions_cwd_dir_in(grok_home, "/opt/ws-fixture/ws");
    let mut expected: Vec<Protected> = [
        // the workspace's own agent/editor config trees
        "/opt/ws-fixture/ws/.grok",
        "/opt/ws-fixture/ws/.cursor",
        "/opt/ws-fixture/ws/.claude",
        "/opt/ws-fixture/ws/.vscode",
        "/opt/ws-fixture/ws/.idea",
        // the top-level git directory
        "/opt/ws-fixture/ws/.git/hooks",
        "/opt/ws-fixture/ws/.git/config",
        "/opt/ws-fixture/ws/.git/config.worktree",
        "/opt/ws-fixture/ws/.git/info",
        // grok home: trust boundary files, hook sources, grants, the daemon's settings
        "/opt/ws-fixture/u/.grok/config.toml",
        "/opt/ws-fixture/u/.grok/trusted_folders.toml",
        "/opt/ws-fixture/u/.grok/managed_config.toml",
        "/opt/ws-fixture/u/.grok/requirements.toml",
        "/opt/ws-fixture/u/.grok/sandbox.toml",
        "/opt/ws-fixture/u/.grok/hooks",
        "/opt/ws-fixture/u/.grok/hooks-paths",
        "/opt/ws-fixture/u/.grok/sandbox_grants.toml",
        "/opt/ws-fixture/u/.grok/sandbox_grants.toml.lock",
        "/opt/ws-fixture/u/.grok/workspaced.toml",
        // what grok itself runs, starts or loads into a prompt
        "/opt/ws-fixture/u/.grok/bin",
        "/opt/ws-fixture/u/.grok/downloads",
        "/opt/ws-fixture/u/.grok/vendor",
        "/opt/ws-fixture/u/.grok/plugins",
        "/opt/ws-fixture/u/.grok/installed-plugins",
        "/opt/ws-fixture/u/.grok/plugin-data",
        "/opt/ws-fixture/u/.grok/marketplace-cache",
        "/opt/ws-fixture/u/.grok/skills",
        "/opt/ws-fixture/u/.grok/agents",
        "/opt/ws-fixture/u/.grok/personas",
        "/opt/ws-fixture/u/.grok/roles",
        "/opt/ws-fixture/u/.grok/rules",
        "/opt/ws-fixture/u/.grok/workflows",
        "/opt/ws-fixture/u/.grok/bundled",
        "/opt/ws-fixture/u/.grok/memory",
        "/opt/ws-fixture/u/.grok/memory-v2",
        "/opt/ws-fixture/u/.grok/docs",
        "/opt/ws-fixture/u/.grok/mcp.json",
        "/opt/ws-fixture/u/.grok/lsp.json",
        "/opt/ws-fixture/u/.grok/extensions",
        "/opt/ws-fixture/u/.grok/pager.toml",
        // shell rc files and what they load: the next login runs them
        "/opt/ws-fixture/u/.bashrc",
        "/opt/ws-fixture/u/.bash_aliases",
        "/opt/ws-fixture/u/.bash_login",
        "/opt/ws-fixture/u/.bash_logout",
        "/opt/ws-fixture/u/.bash_profile",
        "/opt/ws-fixture/u/.profile",
        "/opt/ws-fixture/u/.pam_environment",
        "/opt/ws-fixture/u/.zshenv",
        "/opt/ws-fixture/u/.zprofile",
        "/opt/ws-fixture/u/.zshrc",
        "/opt/ws-fixture/u/.zlogin",
        "/opt/ws-fixture/u/.zlogout",
        "/opt/ws-fixture/u/.config/fish/config.fish",
        // git and cargo configuration: the next build or git action runs them
        "/opt/ws-fixture/u/.gitconfig",
        "/opt/ws-fixture/u/.config/git/config",
        "/opt/ws-fixture/u/.cargo/config.toml",
        "/opt/ws-fixture/u/.cargo/config",
        "/opt/ws-fixture/u/.cargo/credentials.toml",
        "/opt/ws-fixture/u/.cargo/credentials",
        "/opt/ws-fixture/u/.cargo/env",
        "/opt/ws-fixture/u/.gradle/init.gradle",
        "/opt/ws-fixture/u/.gradle/gradle.properties",
        // package-manager and editor configuration: the next install or editor start reads them
        "/opt/ws-fixture/u/.m2/settings.xml",
        "/opt/ws-fixture/u/.pip/pip.conf",
        "/opt/ws-fixture/u/.config/pip/pip.conf",
        "/opt/ws-fixture/u/.yarnrc",
        "/opt/ws-fixture/u/.yarnrc.yml",
        "/opt/ws-fixture/u/.vimrc",
        "/opt/ws-fixture/u/.tmux.conf",
        // PATH entries and auto-sourced trees
        "/opt/ws-fixture/u/.local/bin",
        "/opt/ws-fixture/u/.cargo/bin",
        "/opt/ws-fixture/u/.config/fish/conf.d",
        "/opt/ws-fixture/u/.config/fish/functions",
        "/opt/ws-fixture/u/.gradle/init.d",
        "/opt/ws-fixture/u/.config/nvim",
        // login-session persistence
        "/opt/ws-fixture/u/.config/autostart",
        "/opt/ws-fixture/u/.config/systemd/user",
        "/opt/ws-fixture/u/.config/environment.d",
        // macOS persistence
        "/opt/ws-fixture/u/Library/LaunchAgents",
        "/opt/ws-fixture/u/Library/LaunchDaemons",
        "/opt/ws-fixture/u/Library/Application Support/com.apple.backgroundtaskmanagementagent",
        // toolchains, shell frameworks, editor and CLI plugins: run next, unsandboxed
        "/opt/ws-fixture/u/.rustup/toolchains",
        "/opt/ws-fixture/u/.nvm",
        "/opt/ws-fixture/u/.oh-my-zsh",
        "/opt/ws-fixture/u/.vim",
        "/opt/ws-fixture/u/.vscode/extensions",
        "/opt/ws-fixture/u/.docker/cli-plugins",
        // secret stores (also read-denied)
        "/opt/ws-fixture/u/.ssh",
        "/opt/ws-fixture/u/.aws",
        "/opt/ws-fixture/u/.gnupg",
        "/opt/ws-fixture/u/Library/Keychains",
        "/opt/ws-fixture/u/Library/Application Support/Google/Chrome",
        "/opt/ws-fixture/u/Library/Application Support/Firefox",
        "/opt/ws-fixture/u/.config/google-chrome",
        "/opt/ws-fixture/u/.config/chromium",
        "/opt/ws-fixture/u/.mozilla",
        "/opt/ws-fixture/u/.netrc",
        "/opt/ws-fixture/u/.npmrc",
        "/opt/ws-fixture/u/.pypirc",
        "/opt/ws-fixture/u/.docker/config.json",
        "/opt/ws-fixture/u/.kube/config",
        "/opt/ws-fixture/u/.config/gh/hosts.yml",
        "/opt/ws-fixture/u/.git-credentials",
        // the daemon's own endpoint
        "/run/grok/ctl",
    ]
    .into_iter()
    .map(path)
    .collect();
    // every submodule's git directory, present or future
    for entry in ["hooks", "config", "config.worktree", "info"] {
        expected.push(glob(format!("/opt/ws-fixture/ws/.git/modules/**/{entry}")));
    }
    // every linked worktree's pointer files and per-worktree config, present or future
    for file in ["commondir", "gitdir", "config.worktree"] {
        expected.push(glob(format!("/opt/ws-fixture/ws/.git/worktrees/*/{file}")));
    }
    // the whole grok home, every session directory and all of the own one but its command
    // directory; the own grant files named too, for the hard-link check
    let commands = own_session.join("commands");
    for tree in [grok_home.to_path_buf(), grok_home.join("sessions")] {
        expected.push(Protected::TreeExcept {
            tree,
            except: commands.clone(),
        });
    }
    for name in [
        "sandbox_grants.toml",
        "sandbox_grants.toml.lock",
        "permission.toml",
    ] {
        expected.push(path(own_session.join(name)));
    }
    expected.sort();
    assert_eq!(expected, got);
}

/// The whole grok home is protected but the own session's command directory, so a name grok adds
/// later is covered too. A workspace served from inside it (a grok-managed worktree) is then the
/// one writable tree there: the MCP config, extensions, the session directories and any new name
/// stay protected, the command directory included.
#[test]
fn the_grok_home_is_a_floor_tree_but_the_command_dir_or_a_workspace_inside_it() {
    let home = Path::new("/opt/ws-fixture/u");
    let grok_home = home.join(".grok");
    let ws = Path::new("/opt/ws-fixture/ws");
    let got = floor(&inputs(&ServedRoot::pin(ws), &grok_home, home));
    let own_session = xai_grok_config::sessions_cwd_dir_in(&grok_home, "/opt/ws-fixture/ws");
    assert!(is_protected(&grok_home.join("extensions/new/run.sh"), &got));
    assert!(is_protected(&grok_home.join("sessions/other/x"), &got));
    assert!(is_protected(&own_session.join("notes.md"), &got));
    assert!(!is_protected(&own_session.join("commands/out.log"), &got));

    let worktree = grok_home.join("worktrees/repo/feature");
    let worktree_session =
        xai_grok_config::sessions_cwd_dir_in(&grok_home, &worktree.to_string_lossy());
    let got = floor(&inputs(&ServedRoot::pin(&worktree), &grok_home, home));
    assert!(!is_protected(&worktree.join("src/main.rs"), &got));
    for inside in [
        grok_home.join("mcp.json"),
        grok_home.join("extensions/new/run.sh"),
        grok_home.join("config.toml"),
        grok_home.join("a-name-grok-adds-later"),
        grok_home.join("worktrees/repo/other/x"),
        grok_home.join("bin/grok"),
        grok_home.join("plugins/p/run.sh"),
        worktree_session.join("commands/out.log"),
        worktree_session.join("sandbox_grants.toml"),
    ] {
        assert!(is_protected(&inside, &got), "{inside:?}");
    }

    let got = floor(&inputs(&ServedRoot::pin(&grok_home), &grok_home, home));
    let home_session =
        xai_grok_config::sessions_cwd_dir_in(&grok_home, &grok_home.to_string_lossy());
    for inside in [
        grok_home.join("mcp.json"),
        home_session.join("notes.md"),
        grok_home.join("sessions/other/x"),
    ] {
        assert!(is_protected(&inside, &got), "{inside:?}");
    }
}

#[test]
fn control_socket_dir_is_protected_even_without_a_home() {
    let got = floor(&ProtectedInputs {
        workspace_root: &ServedRoot::pin(Path::new("/opt/ws-fixture/ws")),
        grok_home: Path::new("/opt/ws-fixture/gh"),
        user_home: None,
        control_socket_dir: Path::new("/run/grok/ctl"),
        git_env: &GitConfigEnv::default(),
    });
    assert!(got.contains(&path("/run/grok/ctl")));
    assert!(
        !got.iter()
            .any(|entry| entry.covers(Path::new("/opt/ws-fixture/u/.bashrc")))
    );
}

/// The secret stores, the `PATH` entries and the macOS
/// persistence trees are in the floor, so neither a write nor a read grant can open them and no
/// card ever offers them.
#[test]
fn secret_stores_path_dirs_and_persistence_trees_are_in_the_floor() {
    let home = Path::new("/opt/ws-fixture/u");
    let got = floor(&inputs(
        &ServedRoot::pin(Path::new("/opt/ws-fixture/ws")),
        Path::new("/opt/ws-fixture/u/.grok"),
        home,
    ));
    for rel in [
        ".ssh",
        ".aws",
        ".gnupg",
        ".local/bin",
        ".cargo/bin",
        ".config/fish/conf.d",
        ".netrc",
        "Library/LaunchAgents",
        "Library/LaunchDaemons",
        "Library/Application Support/com.apple.backgroundtaskmanagementagent",
    ] {
        assert!(
            is_protected(&home.join(rel).join("x"), &got),
            "{rel} must be protected"
        );
    }
    for rel in [".zshenv", ".zprofile", ".bash_profile", ".profile"] {
        assert!(
            is_protected(&home.join(rel), &got),
            "{rel} must be protected"
        );
    }
    assert!(!is_protected(&home.join("src/x"), &got));
    assert!(!is_protected(&home.join(".cargo/registry/cache/x"), &got));
}

#[test]
fn covers_matches_components_not_prefix_bytes() {
    let entry = path("/opt/ws-fixture/ws/.git/hooks");
    assert!(entry.covers(Path::new("/opt/ws-fixture/ws/.git/hooks")));
    assert!(entry.covers(Path::new("/opt/ws-fixture/ws/.git/hooks/pre-commit")));
    assert!(!entry.covers(Path::new("/opt/ws-fixture/ws/.git/hooksx")));
    assert!(!entry.covers(Path::new("/opt/ws-fixture/ws/.git")));
}

/// Before `git init`, a case variant of a missing floor entry names the same directory on APFS,
/// so it is protected on macOS; on a case-sensitive filesystem it is a different directory.
#[test]
fn a_case_variant_of_a_missing_floor_entry_is_protected_where_apfs_folds_case() {
    let ws = scratch("case-variant");
    let grok_home = ws.join("home/.grok");
    let home = ws.join("home");
    let floor = floor(&inputs(&ServedRoot::pin(&ws), &grok_home, &home));
    assert!(is_protected(&ws.join(".git/hooks/pre-commit"), &floor));
    for variant in [
        ws.join(".GIT/Hooks/pre-commit"),
        ws.join(".git/CONFIG"),
        ws.join(".git/modules/lib/HOOKS/post-checkout"),
        ws.join(".Grok/config.toml"),
        home.join(".SSH/config"),
    ] {
        assert_eq!(
            cfg!(target_os = "macos"),
            is_protected(&variant, &floor),
            "{variant:?}"
        );
    }
    assert!(!is_protected(&ws.join(".gitignore"), &floor));
    let _ = std::fs::remove_dir_all(&ws);
}

/// `is_protected` asks in the given spelling and in the canonical one, so `..` cannot step
/// around the floor.
#[test]
fn is_protected_folds_dots_before_asking() {
    let floor = vec![path("/opt/ws-fixture/ws/.git/hooks")];
    assert!(is_protected(
        Path::new("/opt/ws-fixture/ws/src/../.git/hooks/pre-commit"),
        &floor
    ));
    assert!(!is_protected(
        Path::new("/opt/ws-fixture/ws/.git/hooks/../index"),
        &floor
    ));
}

/// The session directories are one tree-except entry: every present or future sibling is covered,
/// and so is all of the own directory — its grant files, their lock, temp names, a
/// `permission_<client>.toml` written later — but its command directory.
#[test]
fn every_session_directory_is_protected_but_the_own_command_directory() {
    let grok_home = Path::new("/opt/ws-fixture/u/.grok");
    let got = floor(&inputs(
        &ServedRoot::pin(Path::new("/opt/ws-fixture/ws")),
        grok_home,
        Path::new("/opt/ws-fixture/u"),
    ));
    let own = xai_grok_config::sessions_cwd_dir_in(grok_home, "/opt/ws-fixture/ws");
    let other = xai_grok_config::sessions_cwd_dir_in(grok_home, "/opt/ws-fixture/other");
    assert!(is_protected(&other, &got));
    assert!(is_protected(&other.join("permission_cli.toml"), &got));
    assert!(is_protected(&other.join("events.jsonl"), &got));
    assert!(is_protected(
        &other.join("nested/deep/transcript.jsonl"),
        &got
    ));
    assert!(is_protected(
        &grok_home.join("sessions/anything-at-all"),
        &got
    ));
    assert!(is_protected(&grok_home.join("sessions"), &got));
    for name in [
        "",
        "events.jsonl",
        "permission.toml",
        "permission_desktop.toml",
        "permission.toml.bak",
        "sandbox_grants.toml",
        "sandbox_grants.toml.lock",
        ".1234.5.tmp",
        "nested/permission.toml",
    ] {
        assert!(is_protected(&own.join(name), &got), "{name}");
    }
    assert!(!is_protected(&own.join("commands"), &got));
    assert!(!is_protected(&own.join("commands/permission.toml"), &got));
}

/// The own directory's grant files are enumerated for the hard-link check, never globbed (a grok
/// home no glob can spell needs no special case), their names matched as the volume compares
/// them: under APFS `Permission_desktop.toml` is one.
#[test]
fn the_own_grant_files_are_enumerated_as_the_volume_compares_names() {
    let root = scratch("unspellable-home");
    let grok_home = root.join("u[1]/.grok");
    let ws = root.join("ws");
    let own = own_session_dir(&inputs(&ServedRoot::pin(&ws), &grok_home, &root));
    std::fs::create_dir_all(&own).unwrap();
    std::fs::write(own.join("Permission_desktop.toml"), "").unwrap();
    std::fs::write(own.join("notes.toml"), "").unwrap();
    for rule in [VolumeRule::Exact, VolumeRule::Apfs] {
        with_volume_rule(rule, || {
            let got = floor(&inputs(&ServedRoot::pin(&ws), &grok_home, &root));
            assert!(
                !got.iter().any(
                    |entry| matches!(entry, Protected::Glob { glob } if glob.contains("permission"))
                ),
                "{got:?}"
            );
            assert_eq!(
                rule == VolumeRule::Apfs,
                got.contains(&path(own.join("Permission_desktop.toml"))),
                "{rule:?}: {got:?}"
            );
            assert!(!got.contains(&path(own.join("notes.toml"))), "{got:?}");
        });
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// [`read_nofollow`] reads a regular file of the daemon's user within the bound and refuses the
/// rest without following or blocking: a symlink (even to that file), a FIFO, a file past the
/// bound, another user's file. [`FileOwner::Any`] takes another user's file.
#[cfg(unix)]
#[test]
fn read_nofollow_refuses_a_symlink_a_fifo_an_oversized_or_a_foreign_file() {
    let root = scratch("nofollow");
    let file = root.join("state.toml");
    std::fs::write(&file, "x = 1\n").unwrap();
    assert_eq!(
        "x = 1\n",
        read_nofollow(&file, 64, FileOwner::Daemon).unwrap()
    );
    let refusal = |path: &Path, max_bytes, owner| {
        let error = read_nofollow(path, max_bytes, owner).unwrap_err();
        (error.kind(), error.to_string())
    };
    let refused = |reason: &str| (std::io::ErrorKind::InvalidInput, reason.to_owned());

    let link = root.join("link.toml");
    std::os::unix::fs::symlink(&file, &link).unwrap();
    assert_eq!(
        refused("the file is a symlink"),
        refusal(&link, 64, FileOwner::Any)
    );
    let fifo = root.join("fifo.toml");
    let made = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap();
    assert!(made.success());
    assert_eq!(
        refused("the file is not a regular file"),
        refusal(&fifo, 64, FileOwner::Any)
    );
    assert_eq!(
        std::io::ErrorKind::FileTooLarge,
        refusal(&file, 3, FileOwner::Daemon).0
    );

    let own_uid = std::os::unix::fs::MetadataExt::uid(&std::fs::metadata(&file).unwrap());
    let foreign = if own_uid == 0 {
        let foreign = root.join("foreign.toml");
        std::fs::write(&foreign, "y = 2\n").unwrap();
        std::os::unix::fs::chown(&foreign, Some(65534), None).unwrap();
        foreign
    } else {
        PathBuf::from("/etc/passwd")
    };
    assert_eq!(
        refused("the file is owned by another user"),
        refusal(&foreign, 1 << 20, FileOwner::Daemon)
    );
    assert!(read_nofollow(&foreign, 1 << 20, FileOwner::Any).is_ok());
    let _ = std::fs::remove_dir_all(&root);
}

/// Threads that create one lock sidecar in a fresh directory at the same moment all open the one
/// owner-only file (a bare `O_CREAT` that loses the race fails with `ENOENT` on macOS), and a
/// symlink in its place is still refused, never followed to create its target.
#[cfg(unix)]
#[test]
fn racing_creators_of_one_lock_sidecar_all_open_it() {
    const RACERS: usize = 8;
    let root = scratch("create-race");
    let lock = OsStr::new("x.lock");
    for round in 0..200 {
        let path = root.join(round.to_string());
        std::fs::create_dir(&path).unwrap();
        let dir = open_dir_nofollow_at(None, &path, FileOwner::Daemon).unwrap();
        let start = std::sync::Barrier::new(RACERS);
        let opened: Vec<_> = std::thread::scope(|scope| {
            let racers: Vec<_> = (0..RACERS)
                .map(|_| {
                    scope.spawn(|| {
                        start.wait();
                        open_nofollow_at(dir.as_fd(), lock, true, FileOwner::Daemon)
                    })
                })
                .collect();
            racers
                .into_iter()
                .map(|racer| racer.join().unwrap())
                .collect()
        });
        let files: Vec<_> = opened
            .into_iter()
            .map(|file| {
                let meta = file
                    .unwrap_or_else(|error| panic!("round {round}: {error}"))
                    .metadata()
                    .unwrap();
                (meta.dev(), meta.ino(), meta.permissions().mode() & 0o777)
            })
            .collect();
        let (first, rest) = files.split_first().unwrap();
        assert!(
            rest.iter().all(|file| file == first),
            "round {round}: {files:?}"
        );
        assert_eq!(0o600, first.2, "round {round}");
    }

    let path = root.join("link");
    std::fs::create_dir(&path).unwrap();
    std::os::unix::fs::symlink(path.join("target.lock"), path.join("x.lock")).unwrap();
    let dir = open_dir_nofollow_at(None, &path, FileOwner::Daemon).unwrap();
    let error = open_nofollow_at(dir.as_fd(), lock, true, FileOwner::Daemon).unwrap_err();
    assert_eq!(
        (
            std::io::ErrorKind::InvalidInput,
            "the file is a symlink".to_owned()
        ),
        (error.kind(), error.to_string())
    );
    assert!(!path.join("target.lock").exists());
    let _ = std::fs::remove_dir_all(&root);
}

/// A writer in this process re-taking a held directory the moment each write let it go (a burst
/// of mode writes) cannot starve another: its turn comes at the next release. The burst holds the
/// directory a tenth of the wait per write (one hold plus a loaded box's stalls must fit in it).
#[cfg(unix)]
#[test]
fn a_directory_writer_is_not_starved_by_one_re_taking_the_lock_back_to_back() {
    const WAIT: Duration = Duration::from_secs(2);
    let root = scratch("held-turns");
    let dir = root.join(".grok");
    std::fs::create_dir(&dir).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let held = Arc::new(AtomicUsize::new(0));
    let burst = std::thread::spawn({
        let (root, dir, stop, held) = (root.clone(), dir.clone(), stop.clone(), held.clone());
        move || {
            while !stop.load(Relaxed) {
                let dir = HeldDir::open(&root, &dir, FileOwner::Any, false).unwrap();
                dir.lock(WAIT).expect("the burst's own writes go through");
                held.fetch_add(1, Relaxed);
                std::thread::sleep(WAIT / 10);
            }
        }
    });
    while held.load(Relaxed) == 0 {
        std::thread::sleep(Duration::from_millis(1));
    }
    let before = held.load(Relaxed);
    for round in 0..3 {
        let dir = HeldDir::open(&root, &dir, FileOwner::Any, false).unwrap();
        dir.lock(WAIT)
            .unwrap_or_else(|error| panic!("round {round}: {error}"));
    }
    stop.store(true, Relaxed);
    burst.join().unwrap();
    assert!(held.load(Relaxed) > before, "the writes were taken in turn");
    let _ = std::fs::remove_dir_all(&root);
}

/// Windows: a file symlink in the path's place is opened as the link, never its target, and
/// refused — to read, and to create through a dangling one (the lock sidecar's open). Skipped
/// on a host that may not create symlinks (no Developer Mode, no privilege).
#[cfg(windows)]
#[test]
fn open_nofollow_refuses_a_symlink_on_windows() {
    let root = scratch("nofollow-windows");
    let file = root.join("state.toml");
    std::fs::write(&file, "x = 1\n").unwrap();
    let link = root.join("link.toml");
    if let Err(error) = std::os::windows::fs::symlink_file(&file, &link) {
        eprintln!("skipped: this host cannot create a symlink: {error}");
        return;
    }
    let error = read_nofollow(&link, 64, FileOwner::Any).unwrap_err();
    assert_eq!(
        (std::io::ErrorKind::InvalidInput, "the file is a symlink"),
        (error.kind(), error.to_string().as_str())
    );

    let target = root.join("created-through.toml");
    let dangling = root.join("dangling.toml");
    std::os::windows::fs::symlink_file(&target, &dangling).unwrap();
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    let error = open_nofollow(&dangling, &mut options, FileOwner::Daemon).unwrap_err();
    assert_eq!(std::io::ErrorKind::InvalidInput, error.kind(), "{error}");
    assert!(!target.exists(), "the open created the link's target");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn tree_except_covers_component_wise() {
    let entry = Protected::TreeExcept {
        tree: PathBuf::from("/gh/sessions"),
        except: PathBuf::from("/gh/sessions/%2Fws"),
    };
    assert!(entry.covers(Path::new("/gh/sessions")));
    assert!(entry.covers(Path::new("/gh/sessions/%2Fother/x")));
    assert!(entry.covers(Path::new("/gh/sessions/%2Fwsx")));
    assert!(!entry.covers(Path::new("/gh/sessions/%2Fws")));
    assert!(!entry.covers(Path::new("/gh/sessions/%2Fws/deep/file")));
    assert!(!entry.covers(Path::new("/gh/sessionsx/y")));
    assert!(!entry.covers(Path::new("/gh")));
}

/// A submodule's git directory (`.git/modules/<name>`) carries hooks and a config of
/// its own; the pattern covers a submodule added after the policy was built and a nested one.
#[test]
fn submodule_git_dirs_are_protected_by_pattern() {
    let ws = Path::new("/opt/ws-fixture/ws");
    let got = git_entries(ws, None, &[]).protected;
    for rel in [
        ".git/modules/lib/hooks/post-checkout",
        ".git/modules/lib/config",
        ".git/modules/lib/config.worktree",
        ".git/modules/lib/info/exclude",
        ".git/modules/lib/modules/nested/hooks/pre-commit",
        ".git/modules/lib/modules/nested/config",
        ".git/worktrees/feature/commondir",
        ".git/worktrees/feature/gitdir",
        ".git/worktrees/feature/config.worktree",
        ".git/hooks/pre-commit",
        ".git/config",
        ".git/config.worktree",
        ".git/info/attributes",
    ] {
        assert!(is_protected(&ws.join(rel), &got), "{rel} must be protected");
    }
    for rel in [
        ".git/index",
        ".git/HEAD",
        ".git/modules/lib/index",
        ".git/modules/lib/objects/ab/cd",
        ".git/modules/lib/hooksx",
        ".git/worktrees/feature/HEAD",
        ".git/worktrees/feature/index",
        "src/config",
    ] {
        assert!(!is_protected(&ws.join(rel), &got), "{rel} stays writable");
    }
}

/// A workspace whose path no glob can spell literally (a bracket class, a brace, a backslash)
/// gets its submodule entries enumerated, never a pattern the Seatbelt renderer would refuse.
#[test]
fn a_workspace_path_no_glob_can_spell_gets_enumerated_entries() {
    for name in ["app[v2]", "{{cookiecutter.slug}}", "back\\slash"] {
        let root = scratch("unspellable");
        let ws = root.join(name);
        std::fs::create_dir_all(ws.join(".git/modules/lib/hooks")).unwrap();
        std::fs::create_dir_all(ws.join(".git/worktrees/feature")).unwrap();
        let got = git_entries(&ws, None, &[]).protected;
        assert!(
            !got.iter()
                .any(|entry| matches!(entry, Protected::Glob { .. })),
            "{name}: {got:?}"
        );
        for rel in [
            ".git/modules/lib/hooks/pre-commit",
            ".git/modules/lib/config.worktree",
            ".git/worktrees/feature/commondir",
            ".git/worktrees/feature/gitdir",
            ".git/worktrees/feature/config.worktree",
        ] {
            assert!(
                is_protected(&ws.join(rel), &got),
                "{name}: {rel} in {got:?}"
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}

/// A linked worktree's `.git` is a file whose `gitdir:` line names the real git
/// directory (`<main>/.git/worktrees/<name>`), and that directory's `commondir` names the shared
/// one. The file itself, the per-worktree directory's `commondir` and `gitdir` pointers (git
/// follows both), both directories' hooks/config/config.worktree/info, a `core.hooksPath` tree in
/// the shared `config` and one in the per-worktree `config.worktree` (relative: under the
/// worktree, never the main checkout) are the floor; `HEAD`, `index` and the objects stay
/// writable.
#[test]
fn linked_worktree_git_file_gitdir_and_commondir_are_protected() {
    let root = scratch("worktree");
    let main = root.join("main");
    let common = main.join(".git");
    let per_worktree = common.join("worktrees").join("feature");
    let worktree = root.join("feature");
    let custom_hooks = root.join("githooks");
    std::fs::create_dir_all(per_worktree.join("hooks")).unwrap();
    std::fs::create_dir_all(common.join("hooks")).unwrap();
    std::fs::create_dir_all(&worktree).unwrap();
    std::fs::create_dir_all(&custom_hooks).unwrap();
    std::fs::write(
        worktree.join(".git"),
        format!("gitdir: {}\n", per_worktree.display()),
    )
    .unwrap();
    std::fs::write(per_worktree.join("commondir"), "../..\n").unwrap();
    std::fs::write(
        per_worktree.join("gitdir"),
        format!("{}\n", worktree.join(".git").display()),
    )
    .unwrap();
    std::fs::write(
        per_worktree.join("config.worktree"),
        "[core]\n\thooksPath = wt-hooks\n",
    )
    .unwrap();
    std::fs::write(
        common.join("config"),
        format!(
            "[core]\n\trepositoryformatversion = 0\n\thooksPath = {}\n",
            custom_hooks.display()
        ),
    )
    .unwrap();

    let got = git_entries(&worktree, None, &[]).protected;
    assert!(got.contains(&path(worktree.join(".git"))), "{got:?}");
    for dir in [&per_worktree, &common] {
        for entry in GIT_DIR_PROTECTED_ENTRIES {
            assert!(
                got.contains(&path(dir.join(entry))),
                "{} missing from {got:?}",
                dir.join(entry).display()
            );
        }
    }
    for pointer in ["commondir", "gitdir"] {
        assert!(
            got.contains(&path(per_worktree.join(pointer))),
            "{pointer} missing from {got:?}"
        );
    }
    assert!(got.contains(&path(custom_hooks.clone())), "{got:?}");
    assert!(got.contains(&path(worktree.join("wt-hooks"))), "{got:?}");
    assert!(!got.contains(&path(main.join("wt-hooks"))), "{got:?}");
    assert!(is_protected(&worktree.join(".git"), &got));
    assert!(is_protected(
        &per_worktree.join("hooks/post-checkout"),
        &got
    ));
    assert!(is_protected(&common.join("config"), &got));
    assert!(is_protected(&custom_hooks.join("pre-commit"), &got));
    assert!(!is_protected(&per_worktree.join("HEAD"), &got));
    assert!(!is_protected(&per_worktree.join("index"), &got));
    assert!(!is_protected(&common.join("objects/ab/cd"), &got));
    assert!(!is_protected(&worktree.join("src/main.rs"), &got));
    let _ = std::fs::remove_dir_all(&root);
}

/// A relative `core.hooksPath` is anchored at the workspace; a `.git` file whose `gitdir:` names
/// nothing that exists protects the file and nothing else. A `gitdir:` reached through a symlink
/// is followed as git follows it: the git directory's entries are protected where the kernel
/// sees them, and the link node inside the workspace is protected so no command can re-point it.
#[cfg(unix)]
#[test]
fn git_pointers_are_bounded_and_a_symlinked_gitdir_is_followed_with_its_link_pinned() {
    let root = scratch("pointers");
    let ws = root.join("ws");
    std::fs::create_dir_all(ws.join(".git")).unwrap();
    std::fs::write(
        ws.join(".git/config"),
        "[core]\n\thooksPath = \"tools/hooks\"\n",
    )
    .unwrap();
    let got = git_entries(&ws, None, &[]).protected;
    assert!(got.contains(&path(ws.join("tools/hooks"))), "{got:?}");

    let dangling = root.join("dangling");
    std::fs::create_dir_all(&dangling).unwrap();
    std::fs::write(dangling.join(".git"), "gitdir: /opt/ws-fixture/nowhere\n").unwrap();
    let got = git_entries(&dangling, None, &[]);
    assert_eq!(vec![path(dangling.join(".git"))], got.protected);
    assert!(got.unread.is_empty(), "{:?}", got.unread);

    let real_git = root.join("real-git");
    std::fs::create_dir_all(real_git.join("hooks")).unwrap();
    std::fs::write(
        real_git.join("config"),
        "[core]\n\thooksPath = linked-hooks\n",
    )
    .unwrap();
    let linked = root.join("linked");
    std::fs::create_dir_all(&linked).unwrap();
    std::os::unix::fs::symlink(&real_git, linked.join("gitlink")).unwrap();
    std::fs::write(linked.join(".git"), "gitdir: gitlink\n").unwrap();
    let got = git_entries(&linked, None, &[]).protected;
    for expected in [
        linked.join(".git"),
        real_git.join("hooks"),
        real_git.join("config"),
        linked.join("linked-hooks"),
    ] {
        assert!(
            got.contains(&path(expected.clone())),
            "{expected:?} in {got:?}"
        );
    }
    let gitlink = Protected::Node {
        path: linked.join("gitlink"),
    };
    assert!(got.contains(&gitlink), "{got:?}");
    assert!(is_protected(&linked.join("gitlink/hooks/pre-commit"), &got));
    assert!(!is_protected(&real_git.join("HEAD"), &got));
    assert!(!is_protected(&linked.join("src/main.rs"), &got));
    let _ = std::fs::remove_dir_all(&root);
}

/// The renderer carves an entry out of a root's allow when it reaches into the root: a path or
/// tree beneath it, a glob whose literal prefix lies beneath it. Nothing outside the root is
/// carved, and the root itself is not "inside" a protected path.
#[test]
fn reaches_into_selects_the_carve_outs_of_a_root() {
    let ws = Path::new("/opt/ws-fixture/ws");
    let home = Path::new("/opt/ws-fixture/u");
    let got = floor(&inputs(
        &ServedRoot::pin(ws),
        home.join(".grok").as_path(),
        home,
    ));
    let ws_carve: Vec<&Protected> = got.iter().filter(|e| e.reaches_into(ws)).collect();
    assert!(ws_carve.contains(&&path(ws.join(".git/hooks"))));
    assert!(ws_carve.contains(&&path(ws.join(".grok"))));
    assert!(ws_carve.contains(&&glob(format!("{}/.git/modules/**/hooks", ws.display()))));
    assert!(!ws_carve.iter().any(|e| e.covers(&home.join(".bashrc"))));
    let home_carve: Vec<&Protected> = got.iter().filter(|e| e.reaches_into(home)).collect();
    assert!(home_carve.contains(&&path(home.join(".bashrc"))));
    assert!(home_carve.contains(&&path(home.join(".ssh"))));
    assert!(
        home_carve
            .iter()
            .any(|e| matches!(e, Protected::TreeExcept { .. }))
    );
    assert!(
        !got.iter()
            .any(|e| e.reaches_into(Path::new("/opt/ws-fixture/elsewhere")))
    );
    assert!(
        !path(ws).reaches_into(&ws.join("src")),
        "a parent is not inside its child"
    );
}

/// A glob's matches can appear anywhere beneath its literal prefix (`**` spans a submodule added
/// later), so a root there holds the pattern: it is carved out of that root's allow and no grant
/// or card may name the root. A root above the prefix stays grantable with the entry carved out.
#[test]
fn a_root_at_or_beneath_a_globs_literal_prefix_is_carved_and_ungrantable() {
    let ws = Path::new("/opt/ws-fixture/ws");
    let hooks = glob(format!("{}/.git/modules/**/hooks", ws.display()));
    let floor = vec![hooks.clone()];
    for inside in [
        ".git/modules",
        ".git/modules/x",
        ".git/modules/x/hooks",
        ".git/modules/x/objects/ab",
    ] {
        let root = ws.join(inside);
        assert!(hooks.reaches_into(&root), "{inside}");
        assert!(is_ungrantable(&root, &floor), "{inside}");
    }
    for above in [ws.to_path_buf(), ws.join(".git")] {
        assert!(hooks.reaches_into(&above), "{above:?}");
        assert!(!is_ungrantable(&above, &floor), "{above:?}");
    }
    let beside = ws.join("src");
    assert!(!hooks.reaches_into(&beside));
    assert!(!is_ungrantable(&beside, &floor));
}

/// The anchor a rename could swap out: a path or tree as is, a glob up to its first
/// metacharacter.
#[test]
fn anchor_is_the_path_the_tree_or_the_globs_literal_prefix() {
    let ws = Path::new("/opt/ws-fixture/ws");
    assert_eq!(ws.join(".grok"), path(ws.join(".grok")).anchor());
    assert_eq!(
        ws.join(".git/modules"),
        glob(format!("{}/.git/modules/**/hooks", ws.display())).anchor()
    );
    let tree = Protected::TreeExcept {
        tree: ws.join("sessions"),
        except: ws.join("sessions/own"),
    };
    assert_eq!(ws.join("sessions"), tree.anchor());
}

/// `count` empty files directly under `dir`.
#[cfg(unix)]
fn fill(dir: &Path, count: usize) {
    std::fs::create_dir_all(dir).unwrap();
    for index in 0..count {
        std::fs::write(dir.join(format!("f{index}")), "").unwrap();
    }
}

/// A secret store too large to list (a browser profile) inside a write root is listed as far as
/// the bound and passed; any other protected tree that large still refuses.
#[cfg(unix)]
#[test]
fn a_secret_store_too_large_to_list_does_not_refuse() {
    let root = scratch("big-store");
    let home = root.join("home");
    fill(&home.join(".mozilla/profile"), HARD_LINK_SCAN_LIMIT + 1);
    fill(
        &home.join(".config/fish/functions"),
        HARD_LINK_SCAN_LIMIT + 1,
    );
    let roots = std::slice::from_ref(&home);
    let store = vec![path(home.join(".mozilla"))];
    assert_eq!(None, hard_linked_protected_file(&store, roots));
    let tree = vec![path(home.join(".config/fish/functions"))];
    assert_eq!(
        Some(Alias::Unlisted {
            tree: home.join(".config/fish/functions")
        }),
        hard_linked_protected_file(&tree, roots).map(|linked| linked.alias)
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// A second name alone refuses nothing: only one inside a write root that no floor entry covers
/// does, since a command can write the protected file through it. A name listed twice is still
/// one name, and a protected directory is never a candidate itself, whatever its link count.
#[cfg(unix)]
#[test]
fn a_protected_file_is_refused_only_for_a_link_a_write_root_reaches() {
    let root = scratch("nlink");
    let (ws, elsewhere) = (root.join("ws"), root.join("elsewhere"));
    std::fs::create_dir_all(ws.join(".git/hooks/sub")).unwrap();
    std::fs::create_dir_all(&elsewhere).unwrap();
    let config = ws.join(".git/config");
    std::fs::write(&config, "[core]\n").unwrap();
    let floor = vec![path(config.clone()), path(root.join("missing"))];
    let roots = std::slice::from_ref(&ws);
    assert_eq!(None, hard_linked_protected_file(&floor, roots));

    std::fs::hard_link(&config, elsewhere.join("config")).unwrap();
    assert_eq!(None, hard_linked_protected_file(&floor, roots));
    assert_eq!(None, hard_linked_protected_file(&floor, &[]));

    std::fs::hard_link(&config, ws.join("alias")).unwrap();
    let writable = Some(HardLinked {
        path: config.clone(),
        nlink: 3,
        alias: Alias::Writable(ws.join("alias")),
    });
    assert_eq!(writable, hard_linked_protected_file(&floor, roots));
    std::fs::remove_file(elsewhere.join("config")).unwrap();
    let twice = vec![path(config.clone()), path(config.clone())];
    assert_eq!(
        Some(Alias::Writable(ws.join("alias"))),
        hard_linked_protected_file(&twice, roots).map(|linked| linked.alias)
    );
    let hooks = vec![path(ws.join(".git/hooks"))];
    assert_eq!(None, hard_linked_protected_file(&hooks, roots));

    // one link named in two spellings, through a symlinked parent and resolved, is one name
    let dotfiles = root.join("dotfiles");
    std::fs::create_dir_all(&dotfiles).unwrap();
    std::os::unix::fs::symlink(&dotfiles, root.join("config-link")).unwrap();
    std::fs::write(dotfiles.join("gitconfig"), "[core]\n").unwrap();
    std::fs::hard_link(dotfiles.join("gitconfig"), ws.join("gitconfig")).unwrap();
    let spellings = vec![
        path(root.join("config-link/gitconfig")),
        path(dotfiles.join("gitconfig")),
    ];
    assert_eq!(
        Some(Alias::Writable(ws.join("gitconfig"))),
        hard_linked_protected_file(&spellings, roots).map(|linked| linked.alias)
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// Home files linked to each other (`~/.bashrc`, `~/.bash_profile`) are all protected names, so
/// no link is left for a command to write through and the workspace — too large to search — is
/// never walked. A third name outside the floor must be searched for: a workspace too large to
/// search refuses, a small one without it runs, and one holding it refuses.
#[cfg(unix)]
#[test]
fn home_files_linked_to_each_other_run_without_a_search() {
    let root = scratch("nlink-home");
    let (ws, home, small) = (root.join("ws"), root.join("home"), root.join("small"));
    fill(&ws.join("src"), HARD_LINK_SCAN_LIMIT + 1);
    fill(&small, 3);
    std::fs::create_dir_all(home.join("dotfiles")).unwrap();
    std::fs::write(home.join(".bashrc"), "# rc\n").unwrap();
    std::fs::hard_link(home.join(".bashrc"), home.join(".bash_profile")).unwrap();
    let floor = floor(&inputs(&ServedRoot::pin(&ws), &home.join(".grok"), &home));
    let roots = std::slice::from_ref(&ws);
    let searches = ROOT_SEARCHES.get();
    assert_eq!(None, hard_linked_protected_file(&floor, roots));
    assert_eq!(searches, ROOT_SEARCHES.get(), "no write root is walked");

    std::fs::hard_link(home.join(".bashrc"), home.join("dotfiles/bashrc")).unwrap();
    let unverified = hard_linked_protected_file(&floor, roots).expect("cannot rule it out");
    assert_eq!(searches + 1, ROOT_SEARCHES.get());
    assert_eq!(Alias::Unsearched { root: ws.clone() }, unverified.alias);
    assert_eq!(3, unverified.nlink);
    assert!(
        [home.join(".bashrc"), home.join(".bash_profile")].contains(&unverified.path),
        "{unverified:?}"
    );
    let small_roots = std::slice::from_ref(&small);
    assert_eq!(None, hard_linked_protected_file(&floor, small_roots));
    std::fs::hard_link(home.join(".bashrc"), small.join("bashrc")).unwrap();
    assert_eq!(
        Some(Alias::Writable(small.join("bashrc"))),
        hard_linked_protected_file(&floor, small_roots).map(|linked| linked.alias)
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// A hard link never crosses a device, so a write root on another one (`/proc`, far larger than
/// one walk) is not searched for the leftover link of a home file and refuses nothing.
#[cfg(target_os = "linux")]
#[test]
fn a_write_root_on_another_device_is_not_searched() {
    let root = scratch("nlink-device");
    let home = root.join("home");
    std::fs::create_dir_all(home.join("dotfiles")).unwrap();
    std::fs::write(home.join(".bashrc"), "# rc\n").unwrap();
    std::fs::hard_link(home.join(".bashrc"), home.join("dotfiles/bashrc")).unwrap();
    let floor = vec![path(home.join(".bashrc"))];
    let proc = PathBuf::from("/proc");
    let searches = ROOT_SEARCHES.get();
    assert_eq!(
        None,
        hard_linked_protected_file(&floor, std::slice::from_ref(&proc))
    );
    assert_eq!(searches, ROOT_SEARCHES.get(), "no write root is walked");
    let _ = std::fs::remove_dir_all(&root);
}

/// A directory a walk cannot list refuses when a name beneath it can be written: searchable but
/// unreadable (`chmod 311`), or unsearchable (`000`, `644`) yet owned by the user, who can open
/// it again. An unsearchable directory of another user holds nothing reachable and is passed
/// over, as is an entry gone meanwhile. As root every directory lists, so the alias itself is
/// found instead.
#[cfg(unix)]
#[test]
fn a_directory_the_walk_cannot_list_refuses_unless_nothing_beneath_it_is_reachable() {
    let root = scratch("unlisted");
    let (ws, home) = (root.join("ws"), root.join("home"));
    let hidden = ws.join("hidden");
    std::fs::create_dir_all(&hidden).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    let gitconfig = home.join(".gitconfig");
    std::fs::write(&gitconfig, "[core]\n").unwrap();
    std::fs::hard_link(&gitconfig, hidden.join("g")).unwrap();
    let floor = vec![path(gitconfig.clone())];
    let roots = std::slice::from_ref(&ws);
    let chmod = |dir: &Path, mode: u32| {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(mode)).unwrap();
    };
    let lists = |dir: &Path| std::fs::read_dir(dir).is_ok();
    let searchable = |dir: &Path| std::fs::symlink_metadata(dir.join(".")).is_ok();
    for mode in [0o311, 0o000, 0o644] {
        chmod(&hidden, mode);
        let expected = if lists(&hidden) && searchable(&hidden) {
            Alias::Writable(hidden.join("g"))
        } else {
            Alias::Unsearched {
                root: hidden.clone(),
            }
        };
        let got = hard_linked_protected_file(&floor, roots).map(|linked| linked.alias);
        assert_eq!(Some(expected), got, "mode {mode:o}");
    }
    chmod(&hidden, 0o000);
    if !lists(&hidden) {
        let denied = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        assert!(!unread_dir_refuses(&hidden, &denied, &[]), "another user's");
        chmod(&hidden, 0o311);
        assert!(unread_dir_refuses(&hidden, &denied, &[]), "searchable");
    }
    chmod(&hidden, 0o755);
    let vanished = std::io::Error::from(std::io::ErrorKind::NotFound);
    let owner = std::os::unix::fs::MetadataExt::uid(&std::fs::symlink_metadata(&hidden).unwrap());
    assert!(!unread_dir_refuses(&hidden, &vanished, &[owner]));
    std::fs::remove_file(hidden.join("g")).unwrap();

    let vscode = ws.join(".vscode/sub");
    let modules = ws.join(".git/modules/lib");
    for dir in [&vscode, &modules] {
        std::fs::create_dir_all(dir).unwrap();
        chmod(dir, 0o311);
    }
    let trees = [
        path(ws.join(".vscode")),
        glob(format!("{}/.git/modules/**/hooks", ws.display())),
    ];
    for (entry, unlisted) in trees.iter().zip([&vscode, &modules]) {
        let got = hard_linked_protected_file(std::slice::from_ref(entry), roots);
        let expected = (!lists(unlisted)).then(|| Alias::Unlisted {
            tree: unlisted.to_path_buf(),
        });
        assert_eq!(expected, got.map(|linked| linked.alias), "{entry:?}");
    }
    for dir in [&vscode, &modules] {
        chmod(dir, 0o755);
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// Every file under a protected directory of a write root is a protected name, nested ones too:
/// a hook kept as a hard link of a workspace script is refused, two hooks linked to each other
/// are not. A directory outside every write root is not listed (rustup's `~/.cargo/bin` links).
#[cfg(unix)]
#[test]
fn a_hard_linked_file_inside_a_protected_directory_of_a_write_root_is_refused() {
    let root = scratch("nlink-dir");
    let ws = root.join("ws");
    let hooks = ws.join(".git/hooks");
    std::fs::create_dir_all(hooks.join("nested")).unwrap();
    std::fs::create_dir_all(ws.join("scripts")).unwrap();
    std::fs::write(ws.join("scripts/pre-commit.sh"), "#!/bin/sh\n").unwrap();
    let floor = vec![path(hooks.clone())];
    let roots = std::slice::from_ref(&ws);
    assert_eq!(None, hard_linked_protected_file(&floor, roots));

    let hook = hooks.join("nested/pre-commit");
    std::fs::hard_link(ws.join("scripts/pre-commit.sh"), &hook).unwrap();
    let writable = Some(HardLinked {
        path: hook.clone(),
        nlink: 2,
        alias: Alias::Writable(ws.join("scripts/pre-commit.sh")),
    });
    assert_eq!(writable, hard_linked_protected_file(&floor, roots));
    std::fs::remove_file(ws.join("scripts/pre-commit.sh")).unwrap();
    std::fs::hard_link(&hook, hooks.join("pre-push")).unwrap();
    assert_eq!(None, hard_linked_protected_file(&floor, roots));

    let bin = root.join("home/.cargo/bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("rustup"), "").unwrap();
    std::fs::hard_link(bin.join("rustup"), bin.join("cargo")).unwrap();
    std::fs::hard_link(bin.join("rustup"), root.join("rustup-copy")).unwrap();
    assert_eq!(None, hard_linked_protected_file(&[path(bin)], roots));
    let _ = std::fs::remove_dir_all(&root);
}

/// A glob's present matches under a write root are protected names as well: a submodule hook
/// kept as a hard link of a workspace script is refused, nested behind another submodule too. A
/// git directory's object store is not walked, however large, and a link inside it is not a
/// match; a prefix outside every write root is not walked.
#[cfg(unix)]
#[test]
fn a_hard_linked_match_of_a_floor_glob_under_a_write_root_is_refused() {
    let root = scratch("nlink-glob");
    let ws = root.join("ws");
    let modules = ws.join(".git/modules");
    for dir in [
        "a/objects",
        "x/hooks",
        "x/objects/cd",
        "x/modules/nested/hooks",
    ] {
        std::fs::create_dir_all(modules.join(dir)).unwrap();
    }
    for git_dir in ["a", "x", "x/modules/nested"] {
        std::fs::write(modules.join(git_dir).join("HEAD"), "ref: refs/heads/main\n").unwrap();
    }
    for index in 0..=HARD_LINK_SCAN_LIMIT {
        std::fs::create_dir(modules.join(format!("a/objects/{index:x}"))).unwrap();
    }
    std::fs::write(ws.join("blob.bin"), "").unwrap();
    std::fs::hard_link(ws.join("blob.bin"), modules.join("x/objects/cd/ef")).unwrap();
    std::fs::write(ws.join("pre-commit.sh"), "#!/bin/sh\n").unwrap();
    std::fs::write(modules.join("x/hooks/pre-commit"), "#!/bin/sh\n").unwrap();
    let floor = vec![glob(format!("{}/**/hooks", modules.display()))];
    let roots = std::slice::from_ref(&ws);
    assert_eq!(None, hard_linked_protected_file(&floor, roots));

    let hook = modules.join("x/modules/nested/hooks/post-checkout");
    std::fs::hard_link(ws.join("pre-commit.sh"), &hook).unwrap();
    let writable = Some(HardLinked {
        path: hook.clone(),
        nlink: 2,
        alias: Alias::Writable(ws.join("pre-commit.sh")),
    });
    assert_eq!(writable, hard_linked_protected_file(&floor, roots));
    let elsewhere = [root.join("elsewhere")];
    assert_eq!(None, hard_linked_protected_file(&floor, &elsewhere));
    let _ = std::fs::remove_dir_all(&root);
}

/// A walk that cannot finish refuses: a modules tree with more directories than one walk queues,
/// a hard-linked hook inside it or not, and a protected directory holding more entries than one
/// walk reads.
#[cfg(unix)]
#[test]
fn a_protected_tree_too_large_to_list_is_refused() {
    let root = scratch("nlink-cap");
    let ws = root.join("ws");
    let modules = ws.join(".git/modules");
    for index in 0..=HARD_LINK_SCAN_LIMIT {
        std::fs::create_dir_all(modules.join(format!("d{index}"))).unwrap();
    }
    let floor = vec![glob(format!("{}/**/hooks", modules.display()))];
    let roots = std::slice::from_ref(&ws);
    let unlisted = Some(HardLinked {
        path: modules.clone(),
        nlink: 0,
        alias: Alias::Unlisted {
            tree: modules.clone(),
        },
    });
    assert_eq!(unlisted, hard_linked_protected_file(&floor, roots));
    std::fs::create_dir_all(modules.join("d0/hooks")).unwrap();
    std::fs::write(ws.join("pre-commit.sh"), "").unwrap();
    std::fs::hard_link(
        ws.join("pre-commit.sh"),
        modules.join("d0/hooks/pre-commit"),
    )
    .unwrap();
    assert_eq!(unlisted, hard_linked_protected_file(&floor, roots));

    let idea = ws.join(".idea");
    fill(&idea.join("libraries"), HARD_LINK_SCAN_LIMIT);
    assert_eq!(
        Some(Alias::Unlisted { tree: idea.clone() }),
        hard_linked_protected_file(&[path(idea)], roots).map(|linked| linked.alias)
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// A workspace of 100,000 entries whose protected files have no link left over runs with no
/// search: the write-root walker is never entered, and the only entries read are the protected
/// hooks directory's. Once a search is needed, the same workspace refuses as too large to search.
#[cfg(unix)]
#[test]
fn a_100k_entry_workspace_without_a_leftover_link_runs_with_no_scan() {
    let root = scratch("nlink-large");
    let (ws, home) = (root.join("ws"), root.join("home"));
    for dir in 0..100 {
        fill(&ws.join(format!("src/d{dir}")), 1000);
    }
    fill(&ws.join(".git/hooks"), 2);
    std::fs::write(ws.join(".git/config"), "[core]\n").unwrap();
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join(".bashrc"), "").unwrap();
    std::fs::hard_link(home.join(".bashrc"), home.join(".bash_profile")).unwrap();
    let floor = floor(&inputs(&ServedRoot::pin(&ws), &home.join(".grok"), &home));
    let roots = std::slice::from_ref(&ws);
    let (searches, read) = (ROOT_SEARCHES.get(), ENTRIES_READ.get());
    assert_eq!(None, hard_linked_protected_file(&floor, roots));
    assert_eq!(
        searches,
        ROOT_SEARCHES.get(),
        "the write root is never walked"
    );
    assert_eq!(read + 2, ENTRIES_READ.get(), "only the two hooks are read");

    std::fs::hard_link(ws.join(".git/config"), root.join("config.bak")).unwrap();
    assert_eq!(
        Some(Alias::Unsearched { root: ws.clone() }),
        hard_linked_protected_file(&floor, roots).map(|linked| linked.alias)
    );
    assert_eq!(searches + 1, ROOT_SEARCHES.get());
    let _ = std::fs::remove_dir_all(&root);
}

/// A file with a link left over is searched for on every command: an alias linked deep in the
/// workspace after a clean command is refused on the next, although the workspace root's own
/// times do not change.
#[cfg(unix)]
#[test]
fn an_alias_linked_deep_in_a_write_root_after_a_clean_command_is_refused_on_the_next() {
    let root = scratch("nlink-planted");
    let (ws, home) = (root.join("ws"), root.join("home"));
    std::fs::create_dir_all(ws.join("src/a/b/c")).unwrap();
    std::fs::create_dir_all(home.join("dotfiles")).unwrap();
    std::fs::write(home.join(".bashrc"), "# rc\n").unwrap();
    std::fs::hard_link(home.join(".bashrc"), home.join("dotfiles/bashrc")).unwrap();
    let floor = floor(&inputs(&ServedRoot::pin(&ws), &home.join(".grok"), &home));
    let roots = std::slice::from_ref(&ws);
    let times = |meta: std::fs::Metadata| (meta.mtime(), meta.mtime_nsec(), meta.ctime());
    let searches = ROOT_SEARCHES.get();
    assert_eq!(None, hard_linked_protected_file(&floor, roots));
    assert_eq!(searches + 1, ROOT_SEARCHES.get());
    let clean = times(std::fs::metadata(&ws).unwrap());

    std::fs::hard_link(home.join(".bashrc"), ws.join("src/a/b/c/bashrc")).unwrap();
    assert_eq!(clean, times(std::fs::metadata(&ws).unwrap()));
    let refused = Some(HardLinked {
        path: home.join(".bashrc"),
        nlink: 3,
        alias: Alias::Writable(ws.join("src/a/b/c/bashrc")),
    });
    assert_eq!(refused, hard_linked_protected_file(&floor, roots));
    assert_eq!(searches + 2, ROOT_SEARCHES.get(), "searched again");
    let _ = std::fs::remove_dir_all(&root);
}

/// Another folder's grant files are covered by the sessions tree alone until one has a second
/// link; then it is named, and its link inside a write root is refused as the own folder's is.
#[cfg(unix)]
#[test]
fn another_folders_grant_file_linked_into_a_write_root_is_refused() {
    let root = scratch("nlink-other-folder");
    let (ws, home) = (root.join("ws"), root.join("home"));
    let grok_home = home.join(".grok");
    std::fs::create_dir_all(&ws).unwrap();
    let served = ServedRoot::pin(&ws);
    let floor_now = || floor(&inputs(&served, &grok_home, &home));
    let own = own_session_dir(&inputs(&served, &grok_home, &home));
    let other =
        xai_grok_config::sessions_cwd_dir_in(&grok_home, &root.join("other").to_string_lossy());
    for dir in [&own, &other] {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(GLOBAL_GRANTS_FILENAME), "").unwrap();
    }
    let grants = other.join(GLOBAL_GRANTS_FILENAME);
    let permission = other.join("permission_cli.toml");
    std::fs::write(&permission, "").unwrap();
    let roots = std::slice::from_ref(&ws);
    assert!(!floor_now().contains(&path(grants.clone())), "not named");
    assert_eq!(None, hard_linked_protected_file(&floor_now(), roots));

    std::fs::hard_link(&grants, ws.join("rows.toml")).unwrap();
    let refused = Some(HardLinked {
        path: grants.clone(),
        nlink: 2,
        alias: Alias::Writable(ws.join("rows.toml")),
    });
    assert_eq!(refused, hard_linked_protected_file(&floor_now(), roots));
    std::fs::remove_file(ws.join("rows.toml")).unwrap();
    assert!(!floor_now().contains(&path(grants.clone())), "not named");
    assert_eq!(None, hard_linked_protected_file(&floor_now(), roots));

    std::fs::hard_link(&permission, ws.join("mode.toml")).unwrap();
    assert_eq!(
        Some(Alias::Writable(ws.join("mode.toml"))),
        hard_linked_protected_file(&floor_now(), roots).map(|linked| linked.alias)
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// No hard-link walk follows a symlink. A directory link inside a write root or a protected tree
/// to a tree too large to walk is one entry, never queued, so the check still runs clean; a link
/// to a protected file is not a second name of it; a protected name that is itself a link is not
/// counted as a name of its target, which the floor lists as its own entry and searches for.
#[cfg(unix)]
#[test]
fn symlinks_are_neither_walked_nor_aliases_in_the_hard_link_check() {
    use std::os::unix::fs::symlink;
    let root = scratch("nlink-symlink");
    let (ws, home, outside) = (root.join("ws"), root.join("home"), root.join("outside"));
    fill(&outside, HARD_LINK_SCAN_LIMIT + 1);
    std::fs::create_dir_all(ws.join(".git/hooks")).unwrap();
    std::fs::create_dir_all(home.join("dotfiles")).unwrap();
    let config = ws.join(".git/config");
    std::fs::write(&config, "[core]\n").unwrap();
    std::fs::hard_link(&config, root.join("config.bak")).unwrap();
    symlink(&outside, ws.join("vendor")).unwrap();
    symlink(&outside, ws.join(".git/hooks/lib")).unwrap();
    symlink(&config, ws.join("config-link")).unwrap();
    let floor = floor(&inputs(&ServedRoot::pin(&ws), &home.join(".grok"), &home));
    let roots = std::slice::from_ref(&ws);
    let (searches, read) = (ROOT_SEARCHES.get(), ENTRIES_READ.get());
    assert_eq!(None, hard_linked_protected_file(&floor, roots));
    assert_eq!(
        searches + 1,
        ROOT_SEARCHES.get(),
        "the leftover link is searched for"
    );
    assert!(
        ENTRIES_READ.get() - read < 16,
        "the linked tree is not read"
    );

    std::fs::remove_file(root.join("config.bak")).unwrap();
    let bashrc = home.join("dotfiles/bashrc");
    std::fs::write(&bashrc, "# rc\n").unwrap();
    std::fs::hard_link(&bashrc, root.join("bashrc.bak")).unwrap();
    symlink(&bashrc, home.join(".bashrc")).unwrap();
    let floor = super::floor(&inputs(&ServedRoot::pin(&ws), &home.join(".grok"), &home));
    assert!(floor.contains(&Protected::Path {
        path: bashrc.clone()
    }));
    let searches = ROOT_SEARCHES.get();
    assert_eq!(None, hard_linked_protected_file(&floor, roots));
    assert_eq!(
        searches + 1,
        ROOT_SEARCHES.get(),
        "the target's leftover link is searched for"
    );
    std::fs::hard_link(&bashrc, ws.join("bashrc.copy")).unwrap();
    assert_eq!(
        Some(Alias::Writable(ws.join("bashrc.copy"))),
        hard_linked_protected_file(&floor, roots).map(|linked| linked.alias)
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// The submodule enumeration a workspace no glob can spell falls back to lists real directories
/// only: a `modules` directory that is a symlink is not listed through, as the glob it stands
/// in for would not match through it.
#[cfg(unix)]
#[test]
fn an_enumerated_modules_directory_is_never_listed_through_a_symlink() {
    let root = scratch("unspellable-link");
    let (ws, elsewhere) = (root.join("app[v2]"), root.join("elsewhere"));
    std::fs::create_dir_all(ws.join(".git")).unwrap();
    std::fs::create_dir_all(elsewhere.join("lib/hooks")).unwrap();
    std::fs::create_dir_all(elsewhere.join("lib/modules/nested/hooks")).unwrap();
    std::os::unix::fs::symlink(&elsewhere, ws.join(".git/modules")).unwrap();
    let got = git_entries(&ws, None, &[]).protected;
    let through_link = ws.join(".git/modules");
    assert!(
        !got.iter()
            .filter_map(Protected::as_path)
            .any(|path| path.starts_with(&through_link) && path != through_link.as_path()),
        "{got:?}"
    );
    std::fs::remove_file(&through_link).unwrap();
    std::fs::create_dir_all(through_link.join("lib")).unwrap();
    std::os::unix::fs::symlink(
        elsewhere.join("lib/modules"),
        through_link.join("lib/modules"),
    )
    .unwrap();
    let got = git_entries(&ws, None, &[]).protected;
    assert!(
        is_protected(&through_link.join("lib/hooks/pre-commit"), &got),
        "{got:?}"
    );
    assert!(
        !is_protected(
            &through_link.join("lib/modules/nested/hooks/pre-commit"),
            &got
        ),
        "{got:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn protected_entries_round_trip_through_serde() {
    let entries = vec![
        path("/opt/ws-fixture/ws/.grok"),
        glob("/opt/ws-fixture/ws/.git/modules/**/hooks"),
        Protected::TreeExcept {
            tree: PathBuf::from("/gh/sessions"),
            except: PathBuf::from("/gh/sessions/%2Fws"),
        },
    ];
    let text = serde_json::to_string(&entries).unwrap();
    assert!(text.contains("\"kind\":\"tree_except\""), "{text}");
    let back: Vec<Protected> = serde_json::from_str(&text).unwrap();
    assert_eq!(entries, back);
}
