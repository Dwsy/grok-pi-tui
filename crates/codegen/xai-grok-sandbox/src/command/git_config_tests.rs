use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::{
    GIT_CONFIG_GLOBAL_ENV, GIT_CONFIG_INCLUDE_DEPTH, GIT_DIRS_LIMIT, GIT_METADATA_READ_LIMIT,
    GitConfigEnv, GitEntries, GitMetadataUnread, XDG_CONFIG_HOME_ENV,
};
use crate::command::protected::{Protected, is_protected, is_ungrantable};

fn path(p: impl Into<PathBuf>) -> Protected {
    Protected::Path { path: p.into() }
}

fn node(p: impl Into<PathBuf>) -> Protected {
    Protected::Node { path: p.into() }
}

/// [`super::git_entries_in`] with git's default places for the global config.
fn entries(ws: &Path, user_home: Option<&Path>, write_roots: &[PathBuf]) -> GitEntries {
    super::git_entries_in(ws, user_home, write_roots, &GitConfigEnv::default())
}

/// The floor entries of [`entries`], for the tests that assert nothing was unread.
fn git_entries(ws: &Path, user_home: Option<&Path>, write_roots: &[PathBuf]) -> Vec<Protected> {
    let got = entries(ws, user_home, write_roots);
    assert!(got.unread.is_empty(), "{:?}", got.unread);
    got.protected
}

fn env(config_global: Option<&str>, xdg_config_home: Option<&str>) -> GitConfigEnv {
    GitConfigEnv {
        config_global: config_global.map(OsString::from),
        xdg_config_home: xdg_config_home.map(OsString::from),
    }
}

/// A `[core] hooksPath = <hooks>` config at `path`, its parent created.
fn write_hooks_config(path: &Path, hooks: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, format!("[core]\n\thooksPath = {hooks}\n")).unwrap();
}

fn scratch(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "xai-sandbox-git-config-{}-{tag}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    dunce::canonicalize(&root).unwrap()
}

/// `core.hooksPath` is resolved as git resolves it — `~/…` under the user's home, relative under
/// the working tree — and a global one in `~/.gitconfig` or `~/.config/git/config` is protected
/// too; a `~` value with no home to resolve it names nothing rather than a folder in the workspace.
#[test]
fn hooks_path_resolves_home_relative_values_and_the_global_config() {
    let root = scratch("hooks-path-home");
    let ws = root.join("ws");
    let home = root.join("home");
    std::fs::create_dir_all(ws.join(".git")).unwrap();
    std::fs::create_dir_all(home.join(".config/git")).unwrap();
    std::fs::write(
        ws.join(".git/config"),
        "[core]\n\thooksPath = ~/repo-hooks\n",
    )
    .unwrap();
    std::fs::write(
        home.join(".gitconfig"),
        "[core]\n\thooksPath = ~/.githooks\n",
    )
    .unwrap();
    std::fs::write(
        home.join(".config/git/config"),
        "[core]\n\thooksPath = tools/shared-hooks\n",
    )
    .unwrap();
    let got = git_entries(&ws, Some(&home), &[]);
    for expected in [
        home.join("repo-hooks"),
        home.join(".githooks"),
        ws.join("tools/shared-hooks"),
    ] {
        assert!(
            got.contains(&path(expected.clone())),
            "{expected:?} in {got:?}"
        );
    }
    assert!(
        !got.iter().any(|entry| entry.covers(&ws.join("~"))),
        "{got:?}"
    );
    let got = git_entries(&ws, None, &[]);
    assert!(
        !got.iter().any(|entry| entry.covers(&ws.join("~"))),
        "{got:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// Git looks `~user/…` up in the user database, which is not read here: both homes it names on
/// the usual layout are protected (the user's own and the sibling `<home>/../user`), never a
/// `~user` folder in the working tree, and nothing without a home. `~/…` stays under the home
/// and a relative value under the working tree.
#[test]
fn hooks_path_under_a_named_user_protects_both_homes_it_can_name() {
    let root = scratch("hooks-path-user");
    let ws = root.join("ws");
    let home = root.join("users/me");
    std::fs::create_dir_all(ws.join(".git")).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    let floor_for = |value: &str, user_home: Option<&std::path::Path>| {
        let config = format!("[core]\n\thooksPath = {value}\n");
        std::fs::write(ws.join(".git/config"), config).unwrap();
        git_entries(&ws, user_home, &[])
    };

    let got = floor_for("~alice/hooks", Some(&home));
    for expected in [home.join("hooks"), root.join("users/alice/hooks")] {
        assert!(
            got.contains(&path(expected.clone())),
            "{expected:?} in {got:?}"
        );
    }
    let in_ws = ws.join("~alice/hooks");
    assert!(!got.iter().any(|entry| entry.covers(&in_ws)), "{got:?}");
    let got = floor_for("~alice/hooks", None);
    assert!(!got.iter().any(|entry| entry.covers(&in_ws)), "{got:?}");

    let got = floor_for("~/hooks", Some(&home));
    assert!(got.contains(&path(home.join("hooks"))), "{got:?}");
    assert!(!got.contains(&path(root.join("users/hooks"))), "{got:?}");
    let got = floor_for("tools/hooks", Some(&home));
    assert!(got.contains(&path(ws.join("tools/hooks"))), "{got:?}");
    assert!(!got.contains(&path(home.join("tools/hooks"))), "{got:?}");
    let _ = std::fs::remove_dir_all(&root);
}

/// Git applies the last `core.hooksPath` it reads: across repeated keys and several `[core]`
/// sections, a key on its header's line, a trailing comment. An empty value unsets and a later
/// one sets it again; a `[core "sub"]` value is another key.
#[test]
fn hooks_path_is_the_last_assignment_git_reads() {
    let root = scratch("hooks-path-last");
    let ws = root.join("ws");
    std::fs::create_dir_all(ws.join(".git")).unwrap();
    let floor_for = |config: &str| {
        std::fs::write(ws.join(".git/config"), config).unwrap();
        git_entries(&ws, None, &[])
    };

    let got = floor_for(concat!(
        "[core]\n\thooksPath = first-hooks\n\thooksPath = second-hooks\n",
        "[user]\n\tname = someone\n",
        "[core] hooksPath = \"last hooks\" ; trailing\n",
        "[core \"sub\"]\n\thooksPath = sub-hooks\n",
    ));
    assert!(got.contains(&path(ws.join("last hooks"))), "{got:?}");
    for stale in ["first-hooks", "second-hooks", "sub-hooks"] {
        assert!(!got.contains(&path(ws.join(stale))), "{stale} in {got:?}");
    }

    let got = floor_for("[core]\n\thooksPath =\n\thooksPath = back-hooks\n");
    assert!(got.contains(&path(ws.join("back-hooks"))), "{got:?}");

    let got = floor_for("[core]\n\thooksPath = gone-hooks\n[core]\n\thooksPath = \"\"\n");
    assert!(!got.contains(&path(ws.join("gone-hooks"))), "{got:?}");
    let _ = std::fs::remove_dir_all(&root);
}

/// `[include]` files are read where they appear, relative to the including file or under the
/// home: one after the repository's value wins, a repository value after it wins back. A
/// missing include is skipped yet protected, so no command can create it.
#[test]
fn hooks_path_follows_includes_in_place() {
    let root = scratch("hooks-path-include");
    let ws = root.join("ws");
    let home = root.join("home");
    std::fs::create_dir_all(ws.join(".git")).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        ws.join("team.gitconfig"),
        "[core]\n\thooksPath = team-hooks\n",
    )
    .unwrap();
    std::fs::write(
        home.join("personal.gitconfig"),
        "[core]\n\thooksPath = ~/personal-hooks\n",
    )
    .unwrap();
    let floor_for = |config: &str| {
        std::fs::write(ws.join(".git/config"), config).unwrap();
        git_entries(&ws, Some(&home), &[])
    };

    let got =
        floor_for("[core]\n\thooksPath = repo-hooks\n[include]\n\tpath = ../team.gitconfig\n");
    assert!(got.contains(&path(ws.join("team-hooks"))), "{got:?}");
    assert!(!got.contains(&path(ws.join("repo-hooks"))), "{got:?}");
    assert!(got.contains(&path(ws.join("team.gitconfig"))), "{got:?}");

    let got =
        floor_for("[include]\n\tpath = ../team.gitconfig\n[core]\n\thooksPath = repo-hooks\n");
    assert!(got.contains(&path(ws.join("repo-hooks"))), "{got:?}");
    assert!(!got.contains(&path(ws.join("team-hooks"))), "{got:?}");

    let got = floor_for("[include]\n\tpath = ~/personal.gitconfig\n\tpath = missing.gitconfig\n");
    assert!(got.contains(&path(home.join("personal-hooks"))), "{got:?}");
    assert!(
        got.contains(&path(home.join("personal.gitconfig"))),
        "{got:?}"
    );
    assert!(
        got.contains(&path(ws.join(".git/missing.gitconfig"))),
        "{got:?}"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// Git dies past ten nested includes and so runs no hook; a chain that deep is followed to the
/// bound and the file past it is reported unread, never skipped in silence: a self-include, and
/// a chain one file longer than git follows. A chain git follows whole is read whole.
#[test]
fn an_include_chain_past_the_bound_is_unread_not_skipped() {
    let root = scratch("hooks-path-include-depth");
    let ws = root.join("ws");
    std::fs::create_dir_all(ws.join(".git")).unwrap();
    std::fs::write(
        ws.join(".git/config"),
        "[core]\n\thooksPath = loop-hooks\n[include]\n\tpath = config\n",
    )
    .unwrap();
    let got = entries(&ws, None, &[]);
    assert!(
        got.protected.contains(&path(ws.join("loop-hooks"))),
        "{got:?}"
    );
    assert_eq!(
        vec![GitMetadataUnread::IncludesUnfollowed {
            path: ws.join(".git/config"),
            depth: GIT_CONFIG_INCLUDE_DEPTH,
            files: super::GIT_CONFIG_FILES_LIMIT,
        }],
        got.unread
    );

    let chain = |links: usize| {
        for level in 0..links {
            std::fs::write(
                ws.join(format!(".git/level{level}")),
                format!("[include]\n\tpath = level{}\n", level + 1),
            )
            .unwrap();
        }
        std::fs::write(
            ws.join(format!(".git/level{links}")),
            "[core]\n\thooksPath = deep-hooks\n",
        )
        .unwrap();
        std::fs::write(ws.join(".git/config"), "[include]\n\tpath = level0\n").unwrap();
        entries(&ws, None, &[])
    };
    let followed = chain(GIT_CONFIG_INCLUDE_DEPTH - 1);
    assert!(
        followed.protected.contains(&path(ws.join("deep-hooks"))),
        "{followed:?}"
    );
    assert!(followed.unread.is_empty(), "{:?}", followed.unread);
    let past = chain(GIT_CONFIG_INCLUDE_DEPTH);
    assert!(
        !past.protected.contains(&path(ws.join("deep-hooks"))),
        "{past:?}"
    );
    assert!(
        matches!(
            past.unread.as_slice(),
            [GitMetadataUnread::IncludesUnfollowed { path, .. }]
                if *path == ws.join(format!(".git/level{GIT_CONFIG_INCLUDE_DEPTH}"))
        ),
        "{:?}",
        past.unread
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// An include git follows but the scan cannot resolve as git would is reported unread, never
/// skipped in silence: git finds `~bob/…` in the user database, so a `hooksPath` there may name a
/// write root. Likewise `~` with no home, `%(prefix)/…`, an empty value, no including directory.
#[test]
fn an_include_that_cannot_be_resolved_is_unread_not_skipped() {
    let root = scratch("hooks-path-include-unresolved");
    let ws = root.join("ws");
    let home = root.join("home");
    let config = ws.join(".git/config");
    std::fs::create_dir_all(ws.join(".git")).unwrap();
    let planted = ws.join("planted-hooks");
    write_hooks_config(&home.join("bob/team"), &planted.display().to_string());
    for (text, value, user_home, reason) in [
        (
            "[include]\n\tpath = ~bob/team\n",
            "~bob/team",
            Some(&home),
            "is under another user's home",
        ),
        (
            "[includeIf \"gitdir:/\"]\n\tpath = ~bob/team\n",
            "~bob/team",
            Some(&home),
            "is under another user's home",
        ),
        (
            "[include]\n\tpath = ~/bob/team\n",
            "~/bob/team",
            None,
            "is under a home directory that is not known",
        ),
        (
            "[include]\n\tpath = %(prefix)/etc/team\n",
            "%(prefix)/etc/team",
            Some(&home),
            "is under git's install prefix",
        ),
        ("[include]\n\tpath =\n", "", Some(&home), "is empty"),
    ] {
        std::fs::write(&config, text).unwrap();
        let got = entries(
            &ws,
            user_home.map(PathBuf::as_path),
            std::slice::from_ref(&ws),
        );
        assert_eq!(
            vec![GitMetadataUnread::Unreadable {
                path: config.clone(),
                reason: format!("its include {value:?} {reason}, not resolved here"),
            }],
            got.unread,
            "{text}"
        );
        assert!(!got.protected.contains(&path(&planted)), "{text}: {got:?}");
    }
    assert_eq!(
        Err("is relative to no directory"),
        super::include_path("team", Path::new("/"), None)
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// An `[includeIf]` condition is not evaluated: the `hooksPath` it would set is protected beside
/// the value in effect without it, whichever one git ends up using.
#[test]
fn a_conditional_include_protects_its_hooks_path_beside_the_effective_one() {
    let root = scratch("hooks-path-include-if");
    let ws = root.join("ws");
    std::fs::create_dir_all(ws.join(".git")).unwrap();
    std::fs::write(
        ws.join(".git/work.gitconfig"),
        "[core]\n\thooksPath = work-hooks\n",
    )
    .unwrap();
    std::fs::write(
        ws.join(".git/config"),
        "[core]\n\thooksPath = repo-hooks\n[includeIf \"gitdir:/opt/ws-fixture/other/\"]\n\tpath = work.gitconfig\n",
    )
    .unwrap();
    let got = git_entries(&ws, None, &[]);
    for tree in ["repo-hooks", "work-hooks"] {
        assert!(
            got.contains(&path(ws.join(tree))),
            "{tree} missing from {got:?}"
        );
    }
    assert!(
        got.contains(&path(ws.join(".git/work.gitconfig"))),
        "{got:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// Git's older dotted header splits at its first `.`, lowercased, before any quoted part:
/// `[IncludeIf.GitDir]` and `[includeIf.or "gitdir:…"]` are `includeIf`s whose files are read
/// and protected, `[core.x]` is another section, `[ "x"]` a header git accepts too.
#[test]
fn a_dotted_header_splits_at_its_first_dot() {
    let root = scratch("dotted-header");
    let ws = root.join("ws");
    write_hooks_config(&ws.join(".git/work.gitconfig"), "work-hooks");
    write_hooks_config(&ws.join(".git/team.gitconfig"), "team-hooks");
    std::fs::write(
        ws.join(".git/config"),
        concat!(
            "[IncludeIf.GitDir]\n\tpath = work.gitconfig\n",
            "[includeIf.or \"gitdir:/opt/ws-fixture/\"]\n\tpath = team.gitconfig\n",
            "[ \"x\"]\n\tname = v\n",
            "[core.x]\n\thooksPath = sub-hooks\n",
            "[core]\n\thooksPath = repo-hooks\n",
        ),
    )
    .unwrap();
    let got = git_entries(&ws, None, &[]);
    for expected in [
        "repo-hooks",
        "work-hooks",
        "team-hooks",
        ".git/work.gitconfig",
        ".git/team.gitconfig",
    ] {
        assert!(
            got.contains(&path(ws.join(expected))),
            "{expected} missing from {got:?}"
        );
    }
    assert!(!got.contains(&path(ws.join("sub-hooks"))), "{got:?}");
    let _ = std::fs::remove_dir_all(&root);
}

/// A line git rejects (`[includeIf.gitdir:/path]`: `:` is no name character) makes git run no
/// command. The file is unread from that line, never cut short in silence, so a `hooksPath` after
/// it cannot pass the floor unseen; one before it is still read, and an include is judged alike.
#[test]
fn a_config_line_git_rejects_leaves_the_file_unread() {
    let root = scratch("rejected-line");
    let ws = root.join("ws");
    let config = ws.join(".git/config");
    let team = ws.join(".git/team.gitconfig");
    std::fs::create_dir_all(ws.join(".git")).unwrap();
    let rejected = |path: &Path| GitMetadataUnread::Unreadable {
        path: path.to_path_buf(),
        reason: "its line 3 does not parse as git config, so nothing from there on is read"
            .to_owned(),
    };

    std::fs::write(
        &config,
        concat!(
            "[core]\n\thooksPath = early-hooks\n",
            "[includeIf.gitdir:/path]\n\tpath = work.gitconfig\n",
            "[core]\n\thooksPath = late-hooks\n",
        ),
    )
    .unwrap();
    let got = entries(&ws, None, &[]);
    assert_eq!(vec![rejected(&config)], got.unread);
    assert!(
        got.protected.contains(&path(ws.join("early-hooks"))),
        "{got:?}"
    );

    std::fs::write(&config, "[include]\n\tpath = team.gitconfig\n").unwrap();
    std::fs::write(
        &team,
        "[user]\n\tname = me\n[foo_bar]\n[core]\n\thooksPath = team-hooks\n",
    )
    .unwrap();
    let got = entries(&ws, None, &[]);
    assert_eq!(vec![rejected(&team)], got.unread);
    assert!(got.protected.contains(&path(team.clone())), "{got:?}");
    let _ = std::fs::remove_dir_all(&root);
}

/// Every path the git layout or a config names follows the `core.hooksPath` rule: at or above
/// the workspace, the home or a write root, only what git reads or runs beneath it is protected.
/// A `.git` file pointing above the workspace can make the git directory's `hooks` (its hooks
/// stay), `info` (its files stay) or `config` (nothing stays) the workspace itself; an include of
/// `..`, `~` or `/` names a directory, which git cannot read; `~user` names the own home and the
/// sibling one, here holding the workspace; a link leading to the home is narrowed as the home.
#[test]
fn every_git_path_at_or_above_a_write_root_is_narrowed() {
    let root = scratch("over-roots");
    let home = root.join("users/me");
    std::fs::create_dir_all(&home).unwrap();
    let covered = |got: &[Protected], probe: &Path| got.iter().any(|entry| entry.covers(probe));
    let with_config = |ws: &Path, config: &str, write_roots: &[PathBuf]| {
        std::fs::create_dir_all(ws.join(".git")).unwrap();
        std::fs::write(ws.join(".git/config"), config).unwrap();
        git_entries(ws, Some(&home), write_roots)
    };

    for (entry, kept) in [
        ("hooks", Some("pre-commit")),
        ("info", Some("exclude")),
        ("config", None),
    ] {
        let ws = root.join("gitdir").join(entry);
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::write(ws.join(".git"), "gitdir: ..\n").unwrap();
        let got = git_entries(&ws, Some(&home), &[]);
        assert!(got.contains(&path(ws.join(".git"))), "{entry}: {got:?}");
        assert!(!covered(&got, &ws.join("src/main.rs")), "{entry}: {got:?}");
        assert!(
            kept.is_none_or(|kept| got.contains(&path(ws.join(kept)))),
            "{entry}: {got:?}"
        );
    }

    let ws = root.join("ws");
    for include in ["..", "~", "/"] {
        let got = with_config(&ws, &format!("[include]\n\tpath = {include}\n"), &[]);
        for probe in [ws.join("src/main.rs"), home.join("proj")] {
            assert!(!covered(&got, &probe), "{include}: {got:?}");
        }
    }

    let sibling = root.join("users/alice");
    let in_sibling = sibling.join("ws");
    let got = with_config(&in_sibling, "[core]\n\thooksPath = ~alice\n", &[]);
    for probe in [in_sibling.join("src/main.rs"), home.join("proj")] {
        assert!(!covered(&got, &probe), "{got:?}");
    }
    for hooks in [&home, &sibling] {
        assert!(got.contains(&path(hooks.join("pre-commit"))), "{got:?}");
    }

    let hooks_over_cache = "[core]\n\thooksPath = ../cache\n";
    let cache = root.join("cache/registry");
    let got = with_config(&ws, hooks_over_cache, &[]);
    assert!(got.contains(&path(root.join("cache"))), "{got:?}");
    let got = with_config(&ws, hooks_over_cache, std::slice::from_ref(&cache));
    assert!(!covered(&got, &cache.join("index")), "{got:?}");
    assert!(
        got.contains(&path(root.join("cache/pre-commit"))),
        "{got:?}"
    );

    #[cfg(unix)]
    {
        let linked_home = root.join("linked-me");
        std::os::unix::fs::symlink(&home, &linked_home).unwrap();
        let got = git_entries(&ws, Some(&linked_home), &[]);
        assert!(
            got.contains(&path(linked_home.join(".gitconfig"))),
            "{got:?}"
        );
        assert!(!covered(&got, &linked_home.join("proj")), "{got:?}");
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// Git reads configs through symlinks (a stow'd `~/.gitconfig`, a linked `~/.config/git`): each
/// file read is protected at its target beside the link, each link on the way (in a target too)
/// as a node, a hooks path or worktree as spelled, and a link to a device protects no device.
#[cfg(unix)]
#[test]
fn configs_are_read_through_symlinks_and_protected_where_they_lead() {
    let root = scratch("through-symlinks");
    let ws = root.join("ws");
    let home = root.join("home");
    let dotfiles = home.join("dotfiles");
    std::fs::create_dir_all(ws.join(".git")).unwrap();
    std::fs::create_dir_all(ws.join("shared")).unwrap();
    std::fs::create_dir_all(ws.join("real-conf")).unwrap();
    std::fs::create_dir_all(ws.join("scripts/tools/hooks")).unwrap();
    std::fs::create_dir_all(ws.join("wt-real")).unwrap();
    std::os::unix::fs::symlink("scripts/tools", ws.join("tools")).unwrap();
    std::os::unix::fs::symlink("tools/hooks", ws.join("team-hooks")).unwrap();
    std::os::unix::fs::symlink("wt-real", ws.join("wt-link")).unwrap();
    std::os::unix::fs::symlink("..", ws.join("up")).unwrap();
    std::fs::create_dir_all(dotfiles.join("git")).unwrap();
    std::fs::create_dir_all(home.join(".config")).unwrap();
    std::fs::write(
        dotfiles.join("gitconfig"),
        "[user]\n\tname = me\n[include]\n\tpath = ~/.config/git/work\n",
    )
    .unwrap();
    std::fs::write(
        dotfiles.join("git/work"),
        "[core]\n\thooksPath = ~/dotfiles/work-hooks\n",
    )
    .unwrap();
    std::os::unix::fs::symlink("dotfiles", home.join("stow")).unwrap();
    std::os::unix::fs::symlink("stow/gitconfig", home.join(".gitconfig")).unwrap();
    std::os::unix::fs::symlink(dotfiles.join("git"), home.join(".config/git")).unwrap();
    std::fs::write(
        ws.join("shared/config"),
        "[core]\n\thooksPath = shared-hooks\n\tworktree = ../wt-link\n[include]\n\tpath = ../conf/team\n",
    )
    .unwrap();
    std::fs::write(
        ws.join("real-conf/team"),
        "[core]\n\thooksPath = team-hooks\n[include]\n\tpath = ../up/outer\n",
    )
    .unwrap();
    std::os::unix::fs::symlink(ws.join("shared/config"), ws.join(".git/config")).unwrap();
    std::os::unix::fs::symlink(ws.join("real-conf"), ws.join("conf")).unwrap();

    let got = git_entries(&ws, Some(&home), &[]);
    for expected in [
        dotfiles.join("work-hooks"),
        dotfiles.join("gitconfig"),
        dotfiles.join("git/work"),
        home.join(".config/git/work"),
        ws.join("team-hooks"),
        ws.join("scripts/tools/hooks"),
        ws.join("wt-link/team-hooks"),
        ws.join("wt-real/team-hooks"),
        ws.join(".git/config"),
        ws.join("shared/config"),
        ws.join("real-conf/team"),
    ] {
        assert!(
            got.contains(&path(expected.clone())),
            "{expected:?} in {got:?}"
        );
    }
    for link in [
        home.join(".config/git"),
        home.join(".gitconfig"),
        home.join("stow"),
        ws.join("up"),
        ws.join("team-hooks"),
        ws.join("tools"),
        ws.join("wt-link"),
        ws.join(".git/config"),
        ws.join("conf"),
    ] {
        assert!(got.contains(&node(link.clone())), "{link:?} in {got:?}");
    }
    for through in [home.join("stow/notes"), ws.join("tools/build.sh")] {
        assert!(!is_protected(&through, &got), "{through:?} in {got:?}");
    }
    assert!(!got.contains(&path(ws.join("shared-hooks"))), "{got:?}");
    assert!(is_protected(&home.join(".gitconfig"), &got));
    assert!(is_protected(&ws.join("conf/team"), &got));
    assert!(!is_protected(&ws.join("real-conf"), &got));
    assert!(!is_protected(&ws.join("shared"), &got));

    std::fs::remove_file(home.join(".gitconfig")).unwrap();
    std::os::unix::fs::symlink("/dev/null", home.join(".gitconfig")).unwrap();
    let got = git_entries(&ws, Some(&home), &[]);
    assert!(!got.contains(&path("/dev/null")), "{got:?}");
    assert!(!got.contains(&path(dotfiles.join("work-hooks"))), "{got:?}");
    let _ = std::fs::remove_dir_all(&root);
}

/// A link met in another link's target is a node, inside a write root too (`~/.gitconfig ->
/// proj2/link/gitconfig` through `proj2/link -> real`), and a link loop stops at the hop limit.
#[cfg(unix)]
#[test]
fn a_link_in_a_links_target_is_a_node_and_a_loop_stops() {
    let root = scratch("link-chain");
    let (ws, home) = (root.join("ws"), root.join("home"));
    std::fs::create_dir_all(ws.join(".git")).unwrap();
    std::fs::create_dir_all(home.join("proj2/real")).unwrap();
    std::fs::write(home.join("proj2/real/gitconfig"), "[user]\n\tname = me\n").unwrap();
    std::os::unix::fs::symlink("real", home.join("proj2/link")).unwrap();
    std::os::unix::fs::symlink("proj2/link/gitconfig", home.join(".gitconfig")).unwrap();
    let got = git_entries(&ws, Some(&home), &[home.join("proj2")]);
    assert!(got.contains(&node(home.join("proj2/link"))), "{got:?}");
    assert!(
        got.contains(&path(home.join("proj2/real/gitconfig"))),
        "{got:?}"
    );
    assert!(
        !is_protected(&home.join("proj2/link/notes"), &got),
        "{got:?}"
    );

    std::fs::remove_file(home.join(".gitconfig")).unwrap();
    std::os::unix::fs::symlink("loop-a", home.join(".gitconfig")).unwrap();
    std::os::unix::fs::symlink("loop-b", home.join("loop-a")).unwrap();
    std::os::unix::fs::symlink("loop-a", home.join("loop-b")).unwrap();
    let got = entries(&ws, Some(&home), &[]).protected;
    for link in [".gitconfig", "loop-a", "loop-b"] {
        assert!(got.contains(&node(home.join(link))), "{link} in {got:?}");
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// A linked worktree at `ws` of the repository at `main`: `ws/.git` names the worktree's git
/// directory as `gitdir` spells it, and that directory's `commondir` holds `commondir`.
#[cfg(unix)]
fn linked_worktree(ws: &Path, main: &Path, gitdir: &Path, commondir: &str) {
    let git_dir = main.join(".git/worktrees/wt");
    std::fs::create_dir_all(&git_dir).unwrap();
    std::fs::create_dir_all(main.join("src")).unwrap();
    std::fs::create_dir_all(ws).unwrap();
    std::fs::write(git_dir.join("commondir"), format!("{commondir}\n")).unwrap();
    std::fs::write(ws.join(".git"), format!("gitdir: {}\n", gitdir.display())).unwrap();
}

/// `link`, met on a pointer's path, is a node: the checkout it leads to, spelled through it as
/// `checkout`, stays grantable and writable — the worktree's `index` too — while the link
/// itself and the checkout's git entries stay protected.
#[cfg(unix)]
fn assert_pointer_link_is_a_node(got: &[Protected], link: &Path, checkout: &Path) {
    assert!(!is_ungrantable(&checkout.join("src"), got), "{got:?}");
    assert!(!is_protected(&checkout.join("src/lib.rs"), got), "{got:?}");
    let index = checkout.join(".git/worktrees/wt/index");
    assert!(!is_protected(&index, got), "{got:?}");
    assert!(got.contains(&node(link)), "{link:?} in {got:?}");
    assert!(is_protected(link, got), "{got:?}");
    let hook = checkout.join(".git/hooks/pre-commit");
    assert!(is_protected(&hook, got), "{got:?}");
}

/// A stow-style link on the `gitdir:` path of a `.git` file (`links/main -> ../main`) is a node,
/// never the tree it leads to.
#[cfg(unix)]
#[test]
fn link_on_the_gitdir_pointer_path_is_a_node_not_a_tree() {
    let root = scratch("gitdir-link");
    let (ws, main, link) = (root.join("ws"), root.join("main"), root.join("links/main"));
    linked_worktree(&ws, &main, &link.join(".git/worktrees/wt"), "../..");
    std::fs::create_dir_all(root.join("links")).unwrap();
    std::os::unix::fs::symlink("../main", &link).unwrap();
    let got = git_entries(&ws, None, &[]);
    assert_pointer_link_is_a_node(&got, &link, &link);
    let _ = std::fs::remove_dir_all(&root);
}

/// A stow-style link on the path a `commondir` names is a node, never the tree it leads to.
#[cfg(unix)]
#[test]
fn link_on_the_commondir_pointer_path_is_a_node_not_a_tree() {
    let root = scratch("commondir-link");
    let (ws, main, link) = (root.join("ws"), root.join("main"), root.join("links/main"));
    let commondir = link.join(".git");
    let gitdir = main.join(".git/worktrees/wt");
    linked_worktree(&ws, &main, &gitdir, &commondir.display().to_string());
    std::fs::create_dir_all(root.join("links")).unwrap();
    std::os::unix::fs::symlink("../main", &link).unwrap();
    let got = git_entries(&ws, None, &[]);
    assert_pointer_link_is_a_node(&got, &link, &link);
    let _ = std::fs::remove_dir_all(&root);
}

/// A pointer link outside every root that leads to one (`links/home -> ../home`, the main
/// checkout under the home) is still a node: [`super::narrowed`] would keep nothing of a tree.
#[cfg(unix)]
#[test]
fn pointer_link_leading_to_a_root_is_still_a_node() {
    let root = scratch("pointer-link-to-root");
    let (ws, home, link) = (root.join("ws"), root.join("home"), root.join("links/home"));
    let main = home.join("main");
    linked_worktree(&ws, &main, &link.join("main/.git/worktrees/wt"), "../..");
    std::fs::create_dir_all(root.join("links")).unwrap();
    std::os::unix::fs::symlink("../home", &link).unwrap();
    let got = git_entries(&ws, Some(&home), &[]);
    assert_pointer_link_is_a_node(&got, &link, &link.join("main"));
    let _ = std::fs::remove_dir_all(&root);
}

/// A config is decoded as git decodes it — bytes that are not UTF-8 in a comment or another
/// value change nothing — and read whole up to the limit; one past it is unread, never a prefix
/// whose `core.hooksPath` is missed, and a `core.hooksPath` whose own bytes are not UTF-8 names
/// a path that cannot be spelled, so it is unread too.
#[test]
fn a_config_is_read_lossily_up_to_the_limit_and_is_unread_past_it() {
    let root = scratch("read-limit");
    let ws = root.join("ws");
    std::fs::create_dir_all(ws.join(".git")).unwrap();
    let config = ws.join(".git/config");
    let padded = |padding: usize| {
        let mut bytes = b"[user]\n\tname = \"caf\xe9\" # \xff\xfe\n".to_vec();
        bytes.extend(std::iter::repeat_n(b'#', padding));
        bytes.extend(b"\n[core]\n\thooksPath = late-hooks\n");
        bytes
    };
    let limit = usize::try_from(GIT_METADATA_READ_LIMIT).unwrap();

    // A real config with a long remote and alias history runs past 64 KiB; the cap is 1 MiB
    for padding in [600 * 1024, limit - 4096] {
        std::fs::write(&config, padded(padding)).unwrap();
        let got = entries(&ws, None, &[]);
        assert!(got.unread.is_empty(), "{padding}: {:?}", got.unread);
        assert!(
            got.protected.contains(&path(ws.join("late-hooks"))),
            "{padding}: {got:?}"
        );
    }

    std::fs::write(&config, padded(limit)).unwrap();
    let got = entries(&ws, None, &[]);
    assert_eq!(
        vec![GitMetadataUnread::TooLarge {
            path: config.clone(),
            limit: GIT_METADATA_READ_LIMIT,
        }],
        got.unread
    );
    assert!(
        !got.protected.contains(&path(ws.join("late-hooks"))),
        "{got:?}"
    );
    assert!(got.protected.contains(&path(config.clone())), "{got:?}");

    std::fs::write(&config, b"[core]\n\thooksPath = h\xf6\xf6ks\n").unwrap();
    let got = entries(&ws, None, &[]);
    assert_eq!(
        vec![GitMetadataUnread::NotUtf8 {
            path: config.clone(),
        }],
        got.unread
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// `config.worktree` (read by git under `extensions.worktreeConfig`, after `config`) is a
/// protected entry of every git directory and is read for `core.hooksPath` like `config`.
#[test]
fn config_worktree_is_protected_and_read_for_the_hooks_path() {
    let root = scratch("config-worktree");
    let ws = root.join("ws");
    std::fs::create_dir_all(ws.join(".git")).unwrap();
    std::fs::write(
        ws.join(".git/config"),
        "[core]\n\thooksPath = main-hooks\n[extensions]\n\tworktreeConfig = true\n",
    )
    .unwrap();
    std::fs::write(
        ws.join(".git/config.worktree"),
        "[core]\n\thooksPath = wt-hooks\n",
    )
    .unwrap();
    let got = git_entries(&ws, None, &[]);
    for expected in [
        ws.join(".git/config.worktree"),
        ws.join("main-hooks"),
        ws.join("wt-hooks"),
    ] {
        assert!(
            got.contains(&path(expected.clone())),
            "{expected:?} in {got:?}"
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// A submodule's `core.hooksPath` is read from its git directory under `.git/modules` — whether
/// the entries there are protected by pattern or enumerated — and a relative one is resolved
/// against the submodule's working tree (its `core.worktree`) beside the workspace, never
/// against the workspace alone: `.git/modules/lib/config` naming `sub-hooks` protects
/// `<ws>/lib/sub-hooks`. A nested submodule's is read the same way.
#[test]
fn a_submodule_hooks_path_is_resolved_against_its_own_worktree() {
    for name in ["ws", "app[v2]"] {
        let root = scratch("submodule-hooks");
        let ws = root.join(name);
        let lib = ws.join(".git/modules/lib");
        let nested = lib.join("modules/nested");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(lib.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(
            lib.join("config"),
            "[core]\n\tworktree = ../../../lib\n\thooksPath = sub-hooks\n",
        )
        .unwrap();
        std::fs::write(nested.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(
            nested.join("config"),
            "[core]\n\tworktree = ../../../../../lib/nested\n\thooksPath = /opt/ws-fixture/abs-hooks\n",
        )
        .unwrap();
        let got = git_entries(&ws, None, &[]);
        for expected in [
            ws.join("lib/sub-hooks"),
            PathBuf::from("/opt/ws-fixture/abs-hooks"),
        ] {
            assert!(
                got.contains(&path(expected.clone())),
                "{name}: {expected:?} in {got:?}"
            );
        }
        assert!(
            is_protected(&nested.join("hooks/pre-commit"), &got),
            "{name}: {got:?}"
        );
        assert!(!is_protected(&ws.join("lib/src/main.rs"), &got), "{name}");
        let _ = std::fs::remove_dir_all(&root);
    }
}

/// From the main checkout each linked worktree's `gitdir` pointer anchors a relative
/// `core.hooksPath` in the shared `config` and its own `config.worktree` (never the main
/// checkout's), as spelled: through a link, the link and where it leads are protected.
#[test]
fn linked_worktrees_anchor_the_shared_and_their_own_hooks_paths() {
    let root = scratch("worktrees");
    let ws = root.join("ws");
    let feature = root.join("feature");
    let per_worktree = ws.join(".git/worktrees/feature");
    std::fs::create_dir_all(&per_worktree).unwrap();
    std::fs::create_dir_all(&feature).unwrap();
    std::fs::write(
        ws.join(".git/config"),
        "[core]\n\thooksPath = shared-hooks\n",
    )
    .unwrap();
    std::fs::write(
        per_worktree.join("gitdir"),
        format!("{}\n", feature.join(".git").display()),
    )
    .unwrap();
    std::fs::write(
        per_worktree.join("config.worktree"),
        "[core]\n\thooksPath = wt-hooks\n",
    )
    .unwrap();
    let got = git_entries(&ws, None, &[]);
    for expected in [
        ws.join("shared-hooks"),
        feature.join("shared-hooks"),
        feature.join("wt-hooks"),
    ] {
        assert!(
            got.contains(&path(expected.clone())),
            "{expected:?} in {got:?}"
        );
    }
    assert!(!got.contains(&path(ws.join("wt-hooks"))), "{got:?}");

    #[cfg(unix)]
    {
        let link = root.join("feature-link");
        std::os::unix::fs::symlink(&feature, &link).unwrap();
        let gitdir = format!("{}\n", link.join(".git").display());
        std::fs::write(per_worktree.join("gitdir"), gitdir).unwrap();
        let got = git_entries(&ws, None, &[]);
        for expected in [link.join("wt-hooks"), feature.join("wt-hooks")] {
            assert!(got.contains(&path(expected.clone())), "{expected:?}");
        }
        assert!(got.contains(&node(link)), "{got:?}");
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// A FIFO in a config's place cannot stall the daemon: the file is opened once, non-blocking,
/// and judged on the open handle, so the scan returns.
#[cfg(unix)]
#[test]
fn a_fifo_in_a_configs_place_does_not_stall_the_scan() {
    let root = scratch("fifo");
    let ws = root.join("ws");
    std::fs::create_dir_all(ws.join(".git")).unwrap();
    let fifo = std::ffi::CString::new(ws.join(".git/config").into_os_string().into_encoded_bytes())
        .unwrap();
    // SAFETY: `fifo` is a valid NUL-terminated path and `mkfifo` reads nothing else.
    assert_eq!(0, unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) });
    let (done, finished) = std::sync::mpsc::channel();
    let scanned = ws.clone();
    std::thread::spawn(move || {
        let _ = done.send(entries(&scanned, None, &[]));
    });
    let got = finished
        .recv_timeout(std::time::Duration::from_secs(20))
        .expect("the scan must return with a FIFO in the config's place");
    assert!(
        got.protected.contains(&path(ws.join(".git/config"))),
        "{got:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// The `modules` tree is listed for every submodule config to read; a command can create
/// directories there, so the walk stops at its bound and reports the tree unread rather than
/// reading without end or skipping the rest in silence.
#[test]
fn a_modules_tree_past_the_bound_is_unread() {
    let root = scratch("modules-bound");
    let ws = root.join("ws");
    let modules = ws.join(".git/modules");
    for index in 0..GIT_DIRS_LIMIT {
        std::fs::create_dir_all(modules.join(format!("m{index}"))).unwrap();
    }
    let got = entries(&ws, None, &[]);
    assert!(got.unread.is_empty(), "{:?}", got.unread);
    std::fs::create_dir_all(modules.join("one-more")).unwrap();
    let got = entries(&ws, None, &[]);
    assert_eq!(
        vec![GitMetadataUnread::GitDirsUnlisted {
            tree: modules.clone(),
            limit: GIT_DIRS_LIMIT,
        }],
        got.unread
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// Git looks for its global config where the environment says: `GIT_CONFIG_GLOBAL` alone when
/// set (neither home file is read then), else `$XDG_CONFIG_HOME/git/config` in place of
/// `~/.config/git/config`, and `~/.gitconfig`. The file git reads is read for its hooks path and
/// protected — as named and where a link leads — and the files it does not read are not.
#[test]
fn the_global_config_is_where_the_environment_puts_it() {
    let root = scratch("global-env");
    let ws = root.join("ws");
    let home = root.join("home");
    let elsewhere = root.join("elsewhere");
    std::fs::create_dir_all(ws.join(".git")).unwrap();
    write_hooks_config(&home.join(".gitconfig"), "~/home-hooks");
    write_hooks_config(&home.join(".config/git/config"), "~/xdg-default-hooks");
    write_hooks_config(&elsewhere.join("xdg/git/config"), "~/xdg-hooks");
    write_hooks_config(&elsewhere.join("global.gitconfig"), "~/global-hooks");
    let entries = |env: &GitConfigEnv| {
        let got = super::git_entries_in(&ws, Some(&home), &[], env);
        assert!(got.unread.is_empty(), "{:?}", got.unread);
        got.protected
    };
    let hooks = |name: &str| path(home.join(name));

    // No environment: both home files, in git's default places
    let got = entries(&GitConfigEnv::default());
    for expected in [
        hooks("home-hooks"),
        hooks("xdg-default-hooks"),
        path(home.join(".gitconfig")),
        path(home.join(".config/git/config")),
    ] {
        assert!(got.contains(&expected), "{expected:?} in {got:?}");
    }
    assert!(!got.contains(&hooks("xdg-hooks")), "{got:?}");
    assert!(!got.contains(&hooks("global-hooks")), "{got:?}");

    // XDG_CONFIG_HOME moves the XDG file; ~/.gitconfig stays
    let xdg = elsewhere.join("xdg");
    let got = entries(&env(None, xdg.to_str()));
    for expected in [
        hooks("home-hooks"),
        hooks("xdg-hooks"),
        path(home.join(".gitconfig")),
        path(xdg.join("git/config")),
    ] {
        assert!(got.contains(&expected), "{expected:?} in {got:?}");
    }
    assert!(!got.contains(&hooks("xdg-default-hooks")), "{got:?}");
    // An empty XDG_CONFIG_HOME is unset to git
    let got = entries(&env(None, Some("")));
    assert!(got.contains(&hooks("xdg-default-hooks")), "{got:?}");
    assert!(!got.contains(&hooks("xdg-hooks")), "{got:?}");

    // GIT_CONFIG_GLOBAL replaces both home files, whatever XDG_CONFIG_HOME says
    let global = elsewhere.join("global.gitconfig");
    let got = entries(&env(global.to_str(), xdg.to_str()));
    assert!(got.contains(&hooks("global-hooks")), "{got:?}");
    assert!(got.contains(&path(global.clone())), "{got:?}");
    for not_read in ["home-hooks", "xdg-hooks", "xdg-default-hooks"] {
        assert!(!got.contains(&hooks(not_read)), "{not_read} in {got:?}");
    }
    assert!(!got.contains(&path(xdg.join("git/config"))), "{got:?}");
    // An empty GIT_CONFIG_GLOBAL is a global level git skips
    let got = entries(&env(Some(""), None));
    for not_read in ["home-hooks", "xdg-default-hooks", "global-hooks"] {
        assert!(!got.contains(&hooks(not_read)), "{not_read} in {got:?}");
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// `GIT_CONFIG_GLOBAL=/dev/null` is git's "no global config": read (empty), and the device is
/// not made a floor entry, or every `> /dev/null` would be refused. A relative value in either
/// variable names a file git reads from each command's working directory, which the floor
/// cannot know: unread, so enforce refuses rather than guesses.
#[cfg(unix)]
#[test]
fn a_device_as_the_global_config_is_read_not_protected_and_a_relative_one_is_unread() {
    let root = scratch("global-env-edge");
    let ws = root.join("ws");
    let home = root.join("home");
    std::fs::create_dir_all(ws.join(".git")).unwrap();
    write_hooks_config(&home.join(".gitconfig"), "~/home-hooks");
    let got = super::git_entries_in(&ws, Some(&home), &[], &env(Some("/dev/null"), None));
    assert!(got.unread.is_empty(), "{:?}", got.unread);
    assert!(!got.protected.contains(&path("/dev/null")), "{got:?}");
    assert!(
        !got.protected.contains(&path(home.join("home-hooks"))),
        "~/.gitconfig is not read under GIT_CONFIG_GLOBAL: {got:?}"
    );

    for (config_global, xdg, name) in [
        (Some("rel/gitconfig"), None, GIT_CONFIG_GLOBAL_ENV),
        (None, Some("rel/xdg"), XDG_CONFIG_HOME_ENV),
    ] {
        let got = super::git_entries_in(&ws, Some(&home), &[], &env(config_global, xdg));
        let [
            GitMetadataUnread::Unreadable {
                path: unread,
                reason,
            },
        ] = got.unread.as_slice()
        else {
            panic!("{name}: {:?}", got.unread);
        };
        assert_eq!(
            Path::new(config_global.or(xdg).unwrap()),
            unread.as_path(),
            "{name}"
        );
        assert!(reason.contains(name), "{name}: {reason}");
        assert!(reason.contains("relative"), "{name}: {reason}");
    }
    // A relative XDG_CONFIG_HOME leaves ~/.gitconfig read; a relative GIT_CONFIG_GLOBAL replaces it
    let got = super::git_entries_in(&ws, Some(&home), &[], &env(None, Some("rel/xdg")));
    assert!(
        got.protected.contains(&path(home.join("home-hooks"))),
        "{got:?}"
    );
    let got = super::git_entries_in(&ws, Some(&home), &[], &env(Some("rel/gitconfig"), None));
    assert!(
        !got.protected.contains(&path(home.join("home-hooks"))),
        "{got:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// The daemon's environment is what [`GitConfigEnv::from_host`] reads, variable for variable —
/// each field from the variable of its name, an unset one `None`.
#[test]
fn the_host_env_is_read_variable_for_variable() {
    assert_eq!("GIT_CONFIG_GLOBAL", GIT_CONFIG_GLOBAL_ENV);
    assert_eq!("XDG_CONFIG_HOME", XDG_CONFIG_HOME_ENV);
    let named = GitConfigEnv::from_lookup(|name| Some(OsString::from(format!("/from/{name}"))));
    assert_eq!(
        env(
            Some("/from/GIT_CONFIG_GLOBAL"),
            Some("/from/XDG_CONFIG_HOME")
        ),
        named
    );
    let only_xdg = GitConfigEnv::from_lookup(|name| {
        (name == XDG_CONFIG_HOME_ENV).then(|| OsString::from("/xdg"))
    });
    assert_eq!(env(None, Some("/xdg")), only_xdg);
    assert_eq!(GitConfigEnv::default(), GitConfigEnv::from_lookup(|_| None));
    let host = GitConfigEnv::from_host();
    assert_eq!(std::env::var_os(GIT_CONFIG_GLOBAL_ENV), host.config_global);
    assert_eq!(std::env::var_os(XDG_CONFIG_HOME_ENV), host.xdg_config_home);
}
