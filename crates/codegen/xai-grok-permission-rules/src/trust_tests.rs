use super::*;

#[test]
fn empty_store_trusts_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let store = TrustStore::load_from(tmp.path().join(TRUST_FILE_NAME));
    assert!(store.is_empty());
    assert!(!store.is_trusted(tmp.path()));
}

#[test]
fn default_path_in_maps_home_and_preserves_no_home() {
    // With a resolvable home the store sits at <home>/trusted_folders.toml.
    // `/home/alice/.grok` is not absolute on Windows; use a platform-absolute path.
    let home = std::env::temp_dir().join(".grok");
    assert!(
        home.is_absolute(),
        "positive case requires a platform-absolute home"
    );
    assert_eq!(
        TrustStore::default_path_in(Some(home.clone())),
        Some(home.join(TRUST_FILE_NAME))
    );

    // With NO resolvable home the path is `None`, never a synthesized fallback
    // This is the regression guard that keeps the store off the cwd-relative `./.grok` that grok_home() would invent
    // That is how a cloned repo's own `<repo>/.grok/trusted_folders.toml` could masquerade as the user-global store and self-trust the checkout
    assert_eq!(TrustStore::default_path_in(None), None);

    assert_eq!(
        TrustStore::default_path_in(Some(PathBuf::from(".grok"))),
        None
    );
    assert_eq!(
        TrustStore::default_path_in(Some(PathBuf::from("repo/.grok"))),
        None
    );
}

#[test]
fn default_path_follows_live_grok_home_not_once_lock() {
    let _lock = crate::ENV_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let pinned = tempfile::tempdir().unwrap();
    let _pinned_env = crate::TestEnvGuard::set("GROK_HOME", pinned.path());
    let _pin = xai_grok_config::grok_home();
    let home = tempfile::tempdir().unwrap();
    let _env = crate::TestEnvGuard::set("GROK_HOME", home.path());
    let fresh = TrustStore::default_path().expect("GROK_HOME set");
    assert_eq!(fresh, home.path().join(TRUST_FILE_NAME));
}

#[test]
fn default_path_sources_from_fresh_home_not_once_lock() {
    let _lock = crate::ENV_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    assert_eq!(
        TrustStore::default_path(),
        xai_dirs::resolve_grok_home()
            .filter(|h| h.is_absolute())
            .map(|h| h.join(TRUST_FILE_NAME))
    );
}

#[test]
fn no_home_store_trusts_nothing_and_persists_nothing() {
    // Simulate the no-home environment where `default_path()` is `None`: `load()` yields `empty()`, a store with no backing path
    // It must trust nothing and silently no-op on writes, never touching a cwd-relative `./.grok`
    let mut store = TrustStore::empty();
    assert!(store.is_empty());

    let key = Path::new("/some/abs/repo");
    assert!(!store.is_trusted(key), "no-home store trusts nothing");

    // set_trusted is a no-op that returns Ok and records nothing.
    store
        .set_trusted(key)
        .expect("no-home set_trusted is a no-op Ok");
    assert!(
        store.is_empty(),
        "no-home set_trusted must record nothing (in memory)"
    );
    assert!(
        !store.is_trusted(key),
        "still trusts nothing after the no-op write"
    );

    // set_untrusted likewise no-ops without panicking or recording.
    store
        .set_untrusted(key)
        .expect("no-home set_untrusted is a no-op Ok");
    assert!(store.is_empty());
}

#[test]
fn set_trusted_round_trips() {
    let tmp = tempfile::tempdir().unwrap();
    let store_path = tmp.path().join(TRUST_FILE_NAME);
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let key = canonicalize_or_owned(&repo);

    let mut store = TrustStore::load_from(store_path.clone());
    assert!(!store.is_trusted(&key));
    store.set_trusted(&key).unwrap();
    assert!(store.is_trusted(&key));

    // Reload from disk and verify persistence.
    let reloaded = TrustStore::load_from(store_path);
    assert!(reloaded.is_trusted(&key));
}

#[test]
fn persist_overwrites_existing_and_round_trips_both() {
    let tmp = tempfile::tempdir().unwrap();
    let store_path = tmp.path().join(TRUST_FILE_NAME);
    let repo_a = tmp.path().join("repo-a");
    let repo_b = tmp.path().join("repo-b");
    std::fs::create_dir_all(&repo_a).unwrap();
    std::fs::create_dir_all(&repo_b).unwrap();
    let key_a = canonicalize_or_owned(&repo_a);
    let key_b = canonicalize_or_owned(&repo_b);

    let mut store = TrustStore::load_from(store_path.clone());
    store.set_trusted(&key_a).unwrap();
    // The second persist runs over an already-existing destination file.
    store.set_trusted(&key_b).unwrap();

    // Both decisions survive the overwrite, after reloading from disk.
    let reloaded = TrustStore::load_from(store_path.clone());
    assert!(reloaded.is_trusted(&key_a));
    assert!(reloaded.is_trusted(&key_b));

    // The owner-only guarantee still holds after the overwrite, independent of umask (NamedTempFile creates 0600 on Unix regardless of umask)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&store_path).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "trust store must stay 0600 after overwrite"
        );
    }
}

#[test]
fn trust_cascades_to_subdirectories() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let child = repo.join("crates").join("inner");
    std::fs::create_dir_all(&child).unwrap();
    git2::Repository::init(&repo).unwrap();
    let repo_key = canonicalize_or_owned(&repo);
    let child_key = canonicalize_or_owned(&child);

    let mut store = TrustStore::load_from(tmp.path().join(TRUST_FILE_NAME));
    store.set_trusted(&repo_key).unwrap();

    assert!(store.is_trusted(&child_key));
    // A sibling outside the trusted root is NOT trusted.
    let sibling = canonicalize_or_owned(tmp.path()).join("other-repo");
    assert!(!store.is_trusted(&sibling));
    // The cascade is component-wise (`Path::starts_with`), so `…/repo` must not trust `…/repo-sibling` or `…/repository`
    let prefix_sibling = canonicalize_or_owned(tmp.path()).join("repo-sibling");
    assert!(
        !store.is_trusted(&prefix_sibling),
        "string-prefix sibling must NOT be trusted (cascade is component-wise)"
    );
}

#[test]
fn parent_grant_does_not_cover_nested_git_root() {
    let tmp = tempfile::tempdir().unwrap();
    let parent = tmp.path().join("work");
    let nested = parent.join("evil");
    std::fs::create_dir_all(&nested).unwrap();
    git2::Repository::init(&nested).unwrap();
    let parent_key = canonicalize_or_owned(&parent);
    let nested_key = canonicalize_or_owned(&nested);

    let mut store = TrustStore::load_from(tmp.path().join(TRUST_FILE_NAME));
    store.set_trusted(&parent_key).unwrap();

    assert!(
        store.is_trusted(&parent_key),
        "the granted folder stays trusted"
    );
    assert!(
        !store.is_trusted(&nested_key),
        "a nested git root must not inherit a parent grant"
    );
}

#[test]
fn git_parent_grant_does_not_cover_nested_git_root() {
    let tmp = tempfile::tempdir().unwrap();
    let parent = tmp.path().join("work");
    let nested = parent.join("evil");
    let sibling = parent.join("src");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::create_dir_all(&sibling).unwrap();
    git2::Repository::init(&parent).unwrap();
    git2::Repository::init(&nested).unwrap();
    let parent_key = canonicalize_or_owned(&parent);
    let nested_key = canonicalize_or_owned(&nested);
    let sibling_key = canonicalize_or_owned(&sibling);

    let mut store = TrustStore::load_from(tmp.path().join(TRUST_FILE_NAME));
    store.set_trusted(&parent_key).unwrap();

    assert!(
        store.is_trusted(&parent_key),
        "the granted folder stays trusted"
    );
    assert!(
        store.is_trusted(&sibling_key),
        "a same-repo subdirectory is still covered"
    );
    assert!(
        !store.is_trusted(&nested_key),
        "a nested git root must not inherit a parent grant"
    );
    assert!(
        !store.is_trusted(&nested_key.join("src")),
        "a descendant of the nested git root must not inherit a parent grant"
    );
}

#[test]
fn parent_grant_does_not_cover_nongit_descendant() {
    let tmp = tempfile::tempdir().unwrap();
    let parent = tmp.path().join("work");
    let child = parent.join("tarball");
    std::fs::create_dir_all(&child).unwrap();
    let parent_key = canonicalize_or_owned(&parent);
    let child_key = canonicalize_or_owned(&child);

    // Plant a dummy .git because libgit2 discover ignores GIT_CEILING_DIRECTORIES.
    std::fs::write(tmp.path().join(".git"), "gitdir: /nonexistent\n").unwrap();

    let mut store = TrustStore::load_from(tmp.path().join(TRUST_FILE_NAME));
    store.set_trusted(&parent_key).unwrap();

    assert!(
        store.is_trusted(&parent_key),
        "the granted folder stays trusted"
    );
    assert!(
        !store.is_trusted(&child_key),
        "a non-git descendant must not inherit a parent grant"
    );
}

#[test]
fn most_specific_decision_wins_over_ancestor_cascade() {
    // An explicit child untrust must override a trusted ancestor (the bug where an untrust was undone by the cascade on the next reload)
    // The longest-prefix match decides, so siblings of the untrusted child stay trusted via the ancestor
    let tmp = tempfile::tempdir().unwrap();
    let parent = tmp.path().join("parent");
    let child = parent.join("child");
    let other = parent.join("other");
    std::fs::create_dir_all(&child).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    git2::Repository::init(&parent).unwrap();
    let parent_key = canonicalize_or_owned(&parent);
    let child_key = canonicalize_or_owned(&child);
    let other_key = canonicalize_or_owned(&other);

    let mut store = TrustStore::load_from(tmp.path().join(TRUST_FILE_NAME));
    store.set_trusted(&parent_key).unwrap();
    store.set_untrusted(&child_key).unwrap();

    assert!(store.is_trusted(&parent_key), "the ancestor stays trusted");
    assert!(
        !store.is_trusted(&child_key),
        "an explicit child untrust overrides the trusted ancestor"
    );
    assert!(
        !store.is_trusted(&child_key.join("nested")),
        "the untrust cascades to the child's own subdirectories"
    );
    assert!(
        store.is_trusted(&other_key),
        "a sibling without its own decision is still trusted via the ancestor"
    );

    // The most-specific-wins decision survives a reload from disk.
    let reloaded = TrustStore::load_from(tmp.path().join(TRUST_FILE_NAME));
    assert!(!reloaded.is_trusted(&child_key));
    assert!(reloaded.is_trusted(&other_key));
}

#[test]
fn most_specific_trust_wins_over_untrusted_ancestor() {
    // The symmetric half of most-specific-wins: with an UNTRUSTED ancestor and a nearer TRUSTED child, the child IS trusted
    // That trust cascades to the child's own subdirectories
    let tmp = tempfile::tempdir().unwrap();
    let parent = tmp.path().join("parent");
    let child = parent.join("child");
    std::fs::create_dir_all(&child).unwrap();
    git2::Repository::init(&parent).unwrap();
    let parent_key = canonicalize_or_owned(&parent);
    let child_key = canonicalize_or_owned(&child);

    let mut store = TrustStore::load_from(tmp.path().join(TRUST_FILE_NAME));
    store.set_untrusted(&parent_key).unwrap();
    store.set_trusted(&child_key).unwrap();

    assert!(
        !store.is_trusted(&parent_key),
        "the ancestor stays untrusted"
    );
    assert!(
        store.is_trusted(&child_key),
        "a nearer explicit trust overrides the untrusted ancestor"
    );
    assert!(
        store.is_trusted(&child_key.join("nested")),
        "the child's trust cascades to its own subdirectories"
    );

    // The decision survives a reload from disk.
    let reloaded = TrustStore::load_from(tmp.path().join(TRUST_FILE_NAME));
    assert!(!reloaded.is_trusted(&parent_key));
    assert!(reloaded.is_trusted(&child_key));
}

#[cfg(unix)]
#[test]
fn persisted_file_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let store_path = tmp.path().join(TRUST_FILE_NAME);
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();

    let mut store = TrustStore::load_from(store_path.clone());
    store.set_trusted(&canonicalize_or_owned(&repo)).unwrap();

    let mode = std::fs::metadata(&store_path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600, "trust store must be 0600");
}

#[test]
fn home_dir_is_not_persisted() {
    // Serialize with the test in this file that mutates $HOME: its temp $HOME window could otherwise flip is_home_dir mid-test
    // This test mutates no env itself
    let _lock = crate::ENV_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let store_path = tmp.path().join(TRUST_FILE_NAME);
    let Some(home) = xai_dirs::home_dir() else {
        return; // no home dir in this environment; nothing to assert
    };

    let mut store = TrustStore::load_from(store_path.clone());
    store.set_trusted(&home).unwrap();

    // Nothing was persisted, and the store still holds no folders.
    assert!(store.is_empty(), "home dir must not be recorded");
    assert!(
        !store_path.exists(),
        "no trust file should be written for the home dir"
    );
}

#[test]
fn workspace_key_falls_back_to_cwd_outside_repo() {
    // A freshly created temp dir is not inside a git repo in CI sandboxes; the key should be the canonicalized dir itself
    let tmp = tempfile::tempdir().unwrap();
    let sub = tmp.path().join("plain");
    std::fs::create_dir_all(&sub).unwrap();
    let key = workspace_key(&sub);
    assert!(key.is_absolute());
    // Only pin the fallback when the temp dir is outside any git repo (a dev/CI checkout may place $TMPDIR inside the source repository)
    if git2::Repository::discover(&sub).is_err() {
        assert_eq!(key, canonicalize_or_owned(&sub));
    }
}

#[test]
fn workspace_key_ignores_home_git_repo_for_subdir() {
    // Home-is-a-git-repo (dotfiles in $HOME): the git up-walk finds home as the repo root, but a subdir must key on the SUBDIR, not $HOME
    // Pin HOME and USERPROFILE: xai_dirs::home_dir reads USERPROFILE on Windows
    let _lock = crate::ENV_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    let _home_guard = crate::TestEnvGuard::set("HOME", home.path());
    let _userprofile_guard = crate::TestEnvGuard::set("USERPROFILE", home.path());
    git2::Repository::init(home.path()).unwrap();
    let civ = home.path().join("Documents").join("civ");
    std::fs::create_dir_all(&civ).unwrap();

    let key = workspace_key(&civ);
    assert_eq!(
        key,
        canonicalize_or_owned(&civ),
        "a subdir under a home git repo must key on the subdir, not $HOME"
    );
    assert!(
        !is_home_dir(&key),
        "the workspace key must never resolve to the home dir"
    );
}

#[test]
fn empty_key_is_not_trusted() {
    // Fail closed: a degenerate `[folders.""] trusted = true` must not trust anything
    // The empty path is a prefix of every path, so honoring it would trust the whole filesystem (fail open)
    let tmp = tempfile::tempdir().unwrap();
    let store_path = tmp.path().join(TRUST_FILE_NAME);
    std::fs::write(&store_path, "[folders.\"\"]\ntrusted = true\n").unwrap();

    let store = TrustStore::load_from(store_path);
    // The record loads, so this exercises the read-side guard (not a parse drop).
    assert!(!store.is_empty(), "empty-key record should still load");
    assert!(
        !store.is_trusted(Path::new("/some/arbitrary/path")),
        "an empty key must not trust the filesystem"
    );
}

#[test]
fn malformed_store_fails_soft_to_empty() {
    // Corrupt TOML must fail closed: empty store, trust nothing, no panic.
    let tmp = tempfile::tempdir().unwrap();
    let store_path = tmp.path().join(TRUST_FILE_NAME);
    std::fs::write(&store_path, "this is not = valid toml [[[").unwrap();

    let store = TrustStore::load_from(store_path);
    assert!(store.is_empty(), "malformed store must load as empty");
    assert!(!store.is_trusted(Path::new("/any/path")));
}

#[test]
fn root_key_is_not_trusted() {
    // Fail closed: a `[folders."/"]` record must not trust every absolute path
    // The root is a prefix of all of them via the cascade, so it is ignored on read even if it reaches the file by hand-edit / migration
    let tmp = tempfile::tempdir().unwrap();
    let store_path = tmp.path().join(TRUST_FILE_NAME);
    std::fs::write(&store_path, "[folders.\"/\"]\ntrusted = true\n").unwrap();

    let store = TrustStore::load_from(store_path);
    assert!(!store.is_empty(), "root-key record should still load");
    assert!(
        !store.is_trusted(Path::new("/any/abs/path")),
        "filesystem root must never be honored as a trust key"
    );
}

#[test]
fn tied_conflicting_aliases_fail_closed() {
    // Two equal-depth, non-canonical aliases of the SAME folder carry CONFLICTING decisions `Path::components()` normalizes the trailing slash so `/a/b` and `/a/b/` tie on depth, yet they
    // load as distinct map keys The tie branch ANDs the tied records, so any untrusted tied alias forces a fail-closed `false` REGARDLESS of map order Asserting BOTH orderings pins this:
    // a last-wins revert (return the LAST equal-depth record) returns `true` for ordering (b) below A single pinned ordering would pass under both the AND-loop and the buggy last-wins form
    let fails_closed = |trusted_ab: bool, trusted_ab_slash: bool| {
        let tmp = tempfile::tempdir().unwrap();
        let store_path = tmp.path().join(TRUST_FILE_NAME);
        std::fs::write(
            &store_path,
            format!(
                "[folders.'/a/b']\ntrusted = {trusted_ab}\n\
                     [folders.'/a/b/']\ntrusted = {trusted_ab_slash}\n"
            ),
        )
        .unwrap();
        let store = TrustStore::load_from(store_path);
        assert_eq!(store.len(), 2, "both alias records should load distinctly");
        // `/a/b/c` does not exist, so `canonicalize_or_owned` is a no-op; both aliases prefix it and tie on depth.
        !store.is_trusted(Path::new("/a/b/c"))
    };

    // (a) untrusted alias sorts LAST (`/a/b/`): caught by a revert to the original `any(trusted)` form, but NOT by a last-wins revert
    assert!(
        fails_closed(true, false),
        "tie with `/a/b` trusted + `/a/b/` untrusted must fail closed"
    );
    // (b) untrusted alias sorts FIRST (`/a/b`): a last-wins revert would return the last record (`/a/b/`, trusted)
    //     THIS ordering is what catches a last-wins regression; the AND-loop still yields false
    assert!(
        fails_closed(false, true),
        "tie with `/a/b` untrusted + `/a/b/` trusted must STILL fail closed"
    );
}

#[test]
fn home_key_on_disk_is_not_honored() {
    // Serialize with the test in this file that mutates $HOME: its temp $HOME window could otherwise flip is_home_dir mid-test
    // This test mutates no env itself
    let _lock = crate::ENV_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    // A hand-edited / migrated `[folders."<home>"]` record must not trust repos under $HOME; the read side ignores it, matching set_trusted
    let Some(home) = xai_dirs::home_dir() else {
        return; // no home dir in this environment; nothing to assert
    };
    let canonical_home = canonicalize_or_owned(&home);
    let tmp = tempfile::tempdir().unwrap();
    let store_path = tmp.path().join(TRUST_FILE_NAME);
    // TOML literal-string key avoids escaping issues on any platform.
    let body = format!(
        "[folders.'{}']\ntrusted = true\n",
        canonical_home.to_string_lossy()
    );
    std::fs::write(&store_path, body).unwrap();

    let store = TrustStore::load_from(store_path);
    let sub = canonical_home.join("some").join("sub");
    assert!(
        !store.is_trusted(&sub),
        "a home-dir key on disk must not be honored"
    );
}

#[cfg(unix)]
#[test]
fn set_trusted_canonicalizes_key() {
    use std::os::unix::fs::symlink;
    let tmp = tempfile::tempdir().unwrap();
    let store_path = tmp.path().join(TRUST_FILE_NAME);
    let real = tmp.path().join("real-repo");
    std::fs::create_dir_all(&real).unwrap();
    let link = tmp.path().join("link-repo");
    symlink(&real, &link).unwrap();

    // Trust via the symlink alias.
    let mut store = TrustStore::load_from(store_path);
    store.set_trusted(&link).unwrap();

    let canonical_real = canonicalize_or_owned(&real);
    assert!(
        store.is_trusted(&canonical_real),
        "set_trusted must store the canonical path so canonical lookups match"
    );
}

#[test]
fn set_untrusted_records_explicit_deny() {
    let tmp = tempfile::tempdir().unwrap();
    let store_path = tmp.path().join(TRUST_FILE_NAME);
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let key = canonicalize_or_owned(&repo);

    let mut store = TrustStore::load_from(store_path.clone());
    store.set_untrusted(&key).unwrap();
    assert!(!store.is_trusted(&key), "an explicit deny is not trusted");
    assert!(!store.is_empty(), "the deny decision is recorded");

    // Reload from disk: the deny record persisted.
    let reloaded = TrustStore::load_from(store_path);
    assert!(!reloaded.is_trusted(&key));
    assert!(!reloaded.is_empty(), "deny record survives reload");
}

#[test]
fn trust_decision_flips() {
    let tmp = tempfile::tempdir().unwrap();
    let store_path = tmp.path().join(TRUST_FILE_NAME);
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let key = canonicalize_or_owned(&repo);

    let mut store = TrustStore::load_from(store_path.clone());
    store.set_trusted(&key).unwrap();
    assert!(store.is_trusted(&key));
    store.set_untrusted(&key).unwrap();
    assert!(!store.is_trusted(&key), "untrust flips the stored bool");
    store.set_trusted(&key).unwrap();
    assert!(store.is_trusted(&key), "re-trust flips it back");
    // The insert overwrites: one record per folder, no duplicates
    assert_eq!(store.len(), 1);

    let reloaded = TrustStore::load_from(store_path);
    assert!(reloaded.is_trusted(&key));
    assert_eq!(reloaded.len(), 1);
}

#[cfg(unix)]
#[test]
fn is_trusted_canonicalizes_query() {
    use std::os::unix::fs::symlink;
    let tmp = tempfile::tempdir().unwrap();
    let store_path = tmp.path().join(TRUST_FILE_NAME);
    let real = tmp.path().join("real-repo");
    std::fs::create_dir_all(&real).unwrap();
    let link = tmp.path().join("link-repo");
    symlink(&real, &link).unwrap();

    let mut store = TrustStore::load_from(store_path);
    store.set_trusted(&canonicalize_or_owned(&real)).unwrap();

    // A query via the symlink alias resolves to the trusted real dir.
    assert!(
        store.is_trusted(&link),
        "is_trusted must canonicalize the query so a symlink alias matches"
    );
}

#[test]
fn concurrent_writers_do_not_clobber() {
    let tmp = tempfile::tempdir().unwrap();
    let store_path = tmp.path().join(TRUST_FILE_NAME);
    let repo_a = tmp.path().join("repo-a");
    let repo_b = tmp.path().join("repo-b");
    std::fs::create_dir_all(&repo_a).unwrap();
    std::fs::create_dir_all(&repo_b).unwrap();
    let key_a = canonicalize_or_owned(&repo_a);
    let key_b = canonicalize_or_owned(&repo_b);

    // Two instances loaded while the file is empty: both start with an empty in-memory doc, mimicking two processes that raced the initial load
    let mut s1 = TrustStore::load_from(store_path.clone());
    let mut s2 = TrustStore::load_from(store_path.clone());
    s1.set_trusted(&key_a).unwrap();
    s2.set_trusted(&key_b).unwrap();

    // The locked re-read-merge means s2's write did not clobber s1's.
    let reloaded = TrustStore::load_from(store_path);
    assert!(
        reloaded.is_trusted(&key_a),
        "A must survive a concurrent write"
    );
    assert!(
        reloaded.is_trusted(&key_b),
        "B must survive a concurrent write"
    );
}

#[cfg(unix)]
#[test]
fn persist_failure_leaves_memory_unchanged() {
    // Make the destination path itself a DIRECTORY so the final atomic rename in persist fails (renaming a file over a directory)
    // This is robust even when tests run as root (a chmod 0o500 dir would be bypassed by root)
    // It exercises the invariant: on a write error the in-memory doc is left unchanged
    let tmp = tempfile::tempdir().unwrap();
    let store_path = tmp.path().join(TRUST_FILE_NAME);
    std::fs::create_dir_all(&store_path).unwrap(); // store path is a dir, not a file
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let key = canonicalize_or_owned(&repo);

    let mut store = TrustStore::load_from(store_path);
    let result = store.set_trusted(&key);
    assert!(
        result.is_err(),
        "persist over a directory destination must fail"
    );
    assert!(
        !store.is_trusted(&key),
        "memory must be unchanged on persist failure"
    );
}

#[test]
fn corrupt_store_is_not_rewritten_by_set_trusted() {
    let tmp = tempfile::tempdir().unwrap();
    let store_path = tmp.path().join(TRUST_FILE_NAME);
    let good = br#"
[folders."/tmp/keep"]
trusted = true
"#;
    std::fs::write(&store_path, good).unwrap();
    let corrupt = b"this is not toml [[[";
    std::fs::write(&store_path, corrupt).unwrap();
    let before = std::fs::read(&store_path).unwrap();

    let mut store = TrustStore::load_from(store_path.clone());
    assert!(!store.disk_readable());
    assert!(!store.is_trusted(Path::new("/tmp/keep")));
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let err = store.set_trusted(&repo);
    assert!(err.is_err(), "grant over an unreadable store must fail");
    let after = std::fs::read(&store_path).unwrap();
    assert_eq!(
        before, after,
        "failed parse must not shrink or replace the file"
    );
    assert!(!TrustStore::load_from(store_path).is_trusted(&repo));
}

#[test]
fn missing_file_grant_creates_only_the_new_key() {
    let tmp = tempfile::tempdir().unwrap();
    let store_path = tmp.path().join(TRUST_FILE_NAME);
    assert!(!store_path.exists());
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let key = canonicalize_or_owned(&repo);

    let mut store = TrustStore::load_from(store_path.clone());
    assert!(store.disk_readable());
    store.set_trusted(&key).unwrap();
    let reloaded = TrustStore::load_from(store_path);
    assert!(reloaded.is_trusted(&key));
    assert_eq!(reloaded.len(), 1);
}

#[test]
fn whitespace_file_is_empty_document_and_mergeable() {
    let tmp = tempfile::tempdir().unwrap();
    let store_path = tmp.path().join(TRUST_FILE_NAME);
    std::fs::write(&store_path, "  \n\t  ").unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let key = canonicalize_or_owned(&repo);

    let mut store = TrustStore::load_from(store_path.clone());
    assert!(store.disk_readable());
    assert!(store.is_empty());
    store.set_trusted(&key).unwrap();
    assert!(TrustStore::load_from(store_path).is_trusted(&key));
}

#[test]
fn not_a_directory_parent_is_missing_not_unreadable() {
    let tmp = tempfile::tempdir().unwrap();
    let parent_file = tmp.path().join("not-a-dir");
    std::fs::write(&parent_file, b"x").unwrap();
    let store_path = parent_file.join(TRUST_FILE_NAME);
    let store = TrustStore::load_from(store_path);
    assert!(
        store.disk_readable(),
        "NotADirectory (ENOTDIR) is Missing, not Unreadable"
    );
    assert!(store.is_empty());

    let dir_dest = tmp.path().join("store-is-a-dir");
    std::fs::create_dir_all(&dir_dest).unwrap();
    let eisdir = TrustStore::load_from(dir_dest);
    assert!(
        !eisdir.disk_readable(),
        "IsADirectory (EISDIR) stays Unreadable"
    );
}

#[test]
fn load_on_corrupt_file_does_not_truncate() {
    let tmp = tempfile::tempdir().unwrap();
    let store_path = tmp.path().join(TRUST_FILE_NAME);
    let bytes = b"not = [valid";
    std::fs::write(&store_path, bytes).unwrap();
    let store = TrustStore::load_from(store_path.clone());
    assert!(!store.disk_readable());
    assert_eq!(std::fs::read(&store_path).unwrap(), bytes);
}

#[test]
fn workspace_key_collapses_linked_worktrees_onto_main_checkout() {
    // Every linked `grok -w` worktree of a repo must share ONE trust key: its main checkout's root
    // Build a real repo and two linked worktrees and assert each collapses onto the main checkout (trusted once, not re-prompted per worktree)
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join("main");
    std::fs::create_dir_all(&main).unwrap();
    let repo = git2::Repository::init(&main).unwrap();

    // Worktree creation requires a valid HEAD, so make an initial commit.
    let sig = git2::Signature::now("t", "t@t").unwrap();
    let tree = {
        let mut idx = repo.index().unwrap();
        let oid = idx.write_tree().unwrap();
        repo.find_tree(oid).unwrap()
    };
    repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
        .unwrap();

    // Linked worktrees OUTSIDE the main dir (git2 creates the paths).
    let wt1 = dir.path().join("wt1");
    let wt2 = dir.path().join("wt2");
    repo.worktree("wt1", &wt1, None).unwrap();
    repo.worktree("wt2", &wt2, None).unwrap();

    let main_key = workspace_key(&main);
    // Parity: the main checkout keys off its own workdir.
    assert_eq!(main_key, canonicalize_or_owned(&main));
    assert_eq!(
        workspace_key(&wt1),
        main_key,
        "worktree must collapse onto main checkout"
    );
    assert_eq!(
        workspace_key(&wt2),
        main_key,
        "second worktree must share the same key"
    );
}

#[test]
fn workspace_key_bare_repo_worktree_does_not_widen_to_parent() {
    // A bare repo's `commondir()` is the bare dir itself, so a naive `commondir().parent()` would key off the dir CONTAINING the repo
    // That would trust every sibling via the subdirectory cascade
    // The key must instead fall back to the worktree's OWN dir (narrow, never widened)
    let dir = tempfile::tempdir().unwrap();
    let bare = dir.path().join("repo.git");
    let repo = git2::Repository::init_bare(&bare).unwrap();

    // Worktree creation needs a valid HEAD; build an empty commit (bare repo has no index, so use a treebuilder for the empty tree)
    let sig = git2::Signature::now("t", "t@t").unwrap();
    let tree_oid = repo.treebuilder(None).unwrap().write().unwrap();
    let tree = repo.find_tree(tree_oid).unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
        .unwrap();

    let wt = dir.path().join("wt");
    repo.worktree("wt", &wt, None).unwrap();

    let key = workspace_key(&wt);
    assert_ne!(
        key,
        canonicalize_or_owned(dir.path()),
        "bare-repo worktree key must not widen to the parent dir"
    );
    assert_eq!(
        key,
        canonicalize_or_owned(&wt),
        "bare-repo worktree falls back to its own dir (narrow, safe)"
    );
}

#[test]
fn workspace_key_separate_gitdir_worktree_does_not_widen() {
    // `git init --separate-git-dir` leaves `core.worktree` unset The common gitdir's INFERRED workdir is then
    // the PARENT of the relocated gitdir, not the checkout The layout guard (`<workdir>/.git` must equal the
    // common gitdir) rejects that The key falls back to the worktree's own dir, never widening to the gitdir's parent
    let dir = tempfile::tempdir().unwrap();
    let checkout = dir.path().join("checkout");
    let gitdir = dir.path().join("gitstore");
    std::fs::create_dir_all(&checkout).unwrap();
    let run = |args: &[&str], cwd: &std::path::Path| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };
    // If git isn't usable for this layout, skip rather than report a false failure
    if !run(
        &[
            "init",
            "--separate-git-dir",
            gitdir.to_str().unwrap(),
            checkout.to_str().unwrap(),
        ],
        dir.path(),
    ) || !run(&["commit", "--allow-empty", "-m", "init"], &checkout)
    {
        return;
    }
    let wt = dir.path().join("wt");
    if !run(&["worktree", "add", wt.to_str().unwrap()], &checkout) {
        return;
    }

    // Only assert once the worktree is a linked worktree whose common gitdir is the relocated separate gitdir (the layout this test targets)
    let Ok(repo) = git2::Repository::discover(&wt) else {
        return;
    };
    if !repo.is_worktree() {
        return;
    }

    let key = workspace_key(&wt);
    // The invariant: the key is NOT a broad ancestor of the checkout.
    assert_ne!(
        key,
        canonicalize_or_owned(dir.path()),
        "separate-gitdir worktree key must not widen to the gitdir's parent"
    );
    assert_eq!(
        key,
        canonicalize_or_owned(&wt),
        "separate-gitdir worktree falls back to its own dir (narrow, safe)"
    );
}
