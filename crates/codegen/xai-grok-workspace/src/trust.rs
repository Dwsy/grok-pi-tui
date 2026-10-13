use std::path::Path;
use std::sync::Once;

pub(crate) use xai_grok_permission_rules::trust::{Recorded, TrustPersistError, trust_store_home};
pub use xai_grok_permission_rules::trust::{
    TRUST_FILE_NAME, TrustStore, is_home_dir, is_unsafe_trust_root, workspace_key,
};

/// One-time migration of legacy project-hook trust grants into the unified folder-trust store. Idempotent and guarded to run at most once per process.
/// The legacy file is then renamed to `*.migrated` so it is read only once.
pub fn migrate_legacy_hook_trust() {
    // Local/dev builds do NO trust-store I/O: skip the load and the legacy-file rename
    if crate::folder_trust::folder_trust_inert() {
        return;
    }
    static MIGRATED: Once = Once::new();
    MIGRATED.call_once(|| {
        // Same fresh home as the store; do not follow user_grok_home()'s OnceLock.
        let Some(home) = xai_dirs::resolve_grok_home() else {
            return;
        };
        let legacy_file = home.join(xai_grok_config::TRUSTED_HOOK_PROJECTS_FILENAME);
        let mut store = TrustStore::load();
        let migrated = migrate_legacy_hook_trust_in(&legacy_file, &mut store);
        if migrated > 0 {
            tracing::info!(
                migrated,
                "migrated legacy hook-trust grants into folder-trust"
            );
        }
    });
}

/// [`migrate_legacy_hook_trust`] with explicit paths, so the migration is testable without the process-global grok-home cache.
/// Returns the number of grants seeded into `store`.
fn migrate_legacy_hook_trust_in(legacy_file: &Path, store: &mut TrustStore) -> usize {
    // A read error must NOT be mistaken for "no grants": bail without renaming
    // A transient/permission failure then can't permanently consume the legacy file
    let projects = match xai_grok_hooks::trust::list_trusted_projects_with_file(legacy_file) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(
                path = %legacy_file.display(),
                error = %e,
                "failed to read legacy hook-trust file; leaving it in place for a future run"
            );
            return 0;
        }
    };
    // A failed load is not "no decision": do not seed from an empty stand-in or consume the legacy file.
    if !store.has_store_path() || !store.disk_readable() {
        tracing::warn!(
            path = %legacy_file.display(),
            "leaving legacy hook-trust file in place; folder-trust store has no readable document"
        );
        return 0;
    }
    let mut migrated = 0;
    let mut had_seed_error = false;
    for project in &projects {
        // Never override an existing decision: a folder the user has since trusted or untrusted keeps that decision
        // A re-run after a rename failure then can't silently re-trust a folder the user untrusted in between
        if store.has_decision(project) {
            continue;
        }
        match store.record_decision_strict(project, true) {
            Ok(Recorded::Durable) => migrated += 1,
            Ok(Recorded::Skipped) => {
                // Silent Ok (unsafe root / no path) is not a durable insert; do not consume the legacy file.
                had_seed_error = true;
            }
            Err(e) => {
                tracing::warn!(
                    path = %project.display(),
                    error = %e.as_io(),
                    "failed to migrate a legacy hook-trust grant"
                );
                had_seed_error = true;
            }
        }
    }
    // Rename so the legacy file is consumed exactly once, reached only after a SUCCESSFUL read AND with every grant seeded
    // A seeding write error leaves the file in place for a future run (mirrors the read-error bail)
    // Idempotent: skipped when already migrated/absent
    if had_seed_error {
        tracing::warn!(
            path = %legacy_file.display(),
            "leaving legacy hook-trust file in place after a seeding error; a future run will retry"
        );
    } else if legacy_file.exists() {
        let migrated_file = legacy_file.with_extension("migrated");
        if let Err(e) = std::fs::rename(legacy_file, &migrated_file) {
            tracing::warn!(
                path = %legacy_file.display(),
                error = %e,
                "failed to rename legacy hook-trust file after migration"
            );
        }
    }
    migrated
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use xai_grok_permission_rules::trust::canonicalize_or_owned;

    #[test]
    fn migrate_legacy_hook_trust_seeds_store_and_renames_file() {
        let tmp = tempfile::tempdir().unwrap();
        let store_path = tmp.path().join(TRUST_FILE_NAME);
        // A real project dir so set_trusted's canonicalize succeeds.
        let project = tmp.path().join("repo");
        std::fs::create_dir_all(&project).unwrap();
        let project_key = canonicalize_or_owned(&project);

        // Legacy file with one canonical project path.
        let legacy = tmp.path().join("trusted-hook-projects");
        std::fs::write(&legacy, format!("{}\n", project_key.display())).unwrap();

        let mut store = TrustStore::load_from(store_path.clone());
        let migrated = migrate_legacy_hook_trust_in(&legacy, &mut store);
        assert_eq!(migrated, 1);
        assert!(store.is_trusted(&project_key), "migrated grant is trusted");

        // Legacy file renamed so it is read only once.
        assert!(!legacy.exists(), "legacy file consumed");
        assert!(
            legacy.with_extension("migrated").exists(),
            "renamed to .migrated"
        );

        // The grant persisted to disk.
        let reloaded = TrustStore::load_from(store_path);
        assert!(reloaded.is_trusted(&project_key));
    }

    #[test]
    fn migrate_legacy_hook_trust_does_not_override_existing_decision() {
        let tmp = tempfile::tempdir().unwrap();
        let store_path = tmp.path().join(TRUST_FILE_NAME);
        let project = tmp.path().join("repo");
        std::fs::create_dir_all(&project).unwrap();
        let project_key = canonicalize_or_owned(&project);

        let legacy = tmp.path().join("trusted-hook-projects");
        std::fs::write(&legacy, format!("{}\n", project_key.display())).unwrap();

        // The user has ALREADY untrusted this folder in the unified store.
        let mut store = TrustStore::load_from(store_path);
        store.set_untrusted(&project_key).unwrap();

        let migrated = migrate_legacy_hook_trust_in(&legacy, &mut store);
        assert_eq!(migrated, 0, "an already-decided folder is not re-seeded");
        assert!(
            !store.is_trusted(&project_key),
            "the user's untrust decision is preserved, not overridden by migration"
        );
        // The legacy file is still consumed (renamed) so it is read only once.
        assert!(!legacy.exists());
        assert!(legacy.with_extension("migrated").exists());
    }

    #[test]
    fn migrate_legacy_hook_trust_does_not_rename_without_backing_store() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("repo");
        std::fs::create_dir_all(&project).unwrap();
        let legacy = tmp.path().join("trusted-hook-projects");
        std::fs::write(
            &legacy,
            format!(
                "{}
",
                canonicalize_or_owned(&project).display()
            ),
        )
        .unwrap();
        let mut store = TrustStore::empty();
        assert_eq!(migrate_legacy_hook_trust_in(&legacy, &mut store), 0);
        assert!(
            legacy.exists(),
            "no-home store must not consume the legacy file"
        );
    }

    #[test]
    fn migrate_legacy_hook_trust_does_not_rename_on_unreadable_store() {
        let tmp = tempfile::tempdir().unwrap();
        let store_path = tmp.path().join(TRUST_FILE_NAME);
        std::fs::write(&store_path, b"[[[not toml").unwrap();
        let project = tmp.path().join("repo");
        std::fs::create_dir_all(&project).unwrap();
        let legacy = tmp.path().join("trusted-hook-projects");
        std::fs::write(
            &legacy,
            format!(
                "{}
",
                canonicalize_or_owned(&project).display()
            ),
        )
        .unwrap();
        let mut store = TrustStore::load_from(store_path);
        assert!(!store.disk_readable());
        assert_eq!(migrate_legacy_hook_trust_in(&legacy, &mut store), 0);
        assert!(
            legacy.exists(),
            "unread store must not consume the legacy file"
        );
    }

    #[test]
    fn migrate_legacy_hook_trust_is_noop_when_file_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let store_path = tmp.path().join(TRUST_FILE_NAME);
        let legacy = tmp.path().join("trusted-hook-projects"); // never created

        let mut store = TrustStore::load_from(store_path);
        let migrated = migrate_legacy_hook_trust_in(&legacy, &mut store);
        assert_eq!(migrated, 0);
        assert!(store.is_empty(), "nothing recorded");
        assert!(
            !legacy.with_extension("migrated").exists(),
            "no rename without source"
        );
    }

    #[test]
    fn migrate_legacy_hook_trust_leaves_unreadable_file_in_place() {
        // A legacy file that EXISTS but can't be read must not be consumed
        // A transient read error would otherwise rename it and permanently drop every grant
        // Use a directory at the legacy path: it `exists()` but `read_to_string` errors (non-NotFound), portably simulating the failure
        let tmp = tempfile::tempdir().unwrap();
        let store_path = tmp.path().join(TRUST_FILE_NAME);
        let legacy = tmp.path().join("trusted-hook-projects");
        std::fs::create_dir_all(&legacy).unwrap();

        let mut store = TrustStore::load_from(store_path);
        let migrated = migrate_legacy_hook_trust_in(&legacy, &mut store);
        assert_eq!(migrated, 0, "an unreadable legacy file seeds nothing");
        assert!(store.is_empty(), "nothing recorded on a read failure");
        assert!(legacy.exists(), "unreadable legacy file is left in place");
        assert!(
            !legacy.with_extension("migrated").exists(),
            "unreadable legacy file must not be consumed/renamed"
        );
    }

    #[test]
    fn migrate_legacy_hook_trust_leaves_file_in_place_on_seed_write_error() {
        // A seeding WRITE failure (e.g. a full disk) must not consume the legacy file either: leave it un-renamed so a future run retries the grants.
        // Force set_trusted to error by making the store path a DIRECTORY so its atomic persist rename fails
        // (Same trick as `persist_failure_leaves_memory_unchanged`, robust even when run as root.)
        let tmp = tempfile::tempdir().unwrap();
        let store_path = tmp.path().join(TRUST_FILE_NAME);
        std::fs::create_dir_all(&store_path).unwrap(); // store path is a dir, not a file
        let project = tmp.path().join("repo");
        std::fs::create_dir_all(&project).unwrap();
        let project_key = canonicalize_or_owned(&project);

        let legacy = tmp.path().join("trusted-hook-projects");
        std::fs::write(&legacy, format!("{}\n", project_key.display())).unwrap();

        let mut store = TrustStore::load_from(store_path);
        let migrated = migrate_legacy_hook_trust_in(&legacy, &mut store);
        assert_eq!(migrated, 0, "a seeding write error seeds nothing");
        assert!(
            legacy.exists(),
            "legacy file is left in place on a seeding write error"
        );
        assert!(
            !legacy.with_extension("migrated").exists(),
            "a seeding write error must not consume/rename the legacy file"
        );
    }

    // ── workspace_key registry collapse (grok-managed worktrees) ─────────

    // The crate-shared env lock and env guards travel as ONE value
    // Struct field order (see lib.rs) restores the env before the lock releases, no matter how the caller binds the fixture's return
    use crate::LockedTestEnv;

    /// Point `GROK_HOME` at an isolated tempdir and register one grok-managed worktree at `<home>/worktrees/repo/<name>`.
    /// The worktree dir is a PLAIN directory (NOT a git linked worktree), so only the registry can collapse it.
    fn register_grok_worktree(
        temp: &tempfile::TempDir,
        name: &str,
        source_repo: &Path,
        creation_mode: &str,
    ) -> (LockedTestEnv, PathBuf) {
        use xai_fast_worktree::{WorktreeDb, WorktreeKind, WorktreeRecord, WorktreeStatus};

        // Canonicalize so macOS's `/var` (a symlink to `/private/var`) agrees between the stored record path and the canonicalized lookup query
        let root = dunce::canonicalize(temp.path()).unwrap();
        let home = root.join("grok-home");
        let wt = home.join("worktrees").join("repo").join(name);
        std::fs::create_dir_all(&wt).unwrap();

        // Acquire the lock, then set the env under it (LockedTestEnv restores the env before releasing the lock on drop)
        let env = LockedTestEnv::lock().set("GROK_HOME", &home);

        let db = WorktreeDb::open(&home).unwrap();
        let record = WorktreeRecord {
            id: name.to_string(),
            path: wt.clone(),
            source_repo: source_repo.to_path_buf(),
            repo_name: "repo".to_string(),
            kind: WorktreeKind::Session,
            creation_mode: creation_mode.to_string(),
            git_ref: None,
            head_commit: None,
            session_id: None,
            creator_pid: None,
            created_at: 100,
            last_accessed_at: None,
            status: WorktreeStatus::Alive,
            metadata: None,
        };
        db.register(&record).unwrap();
        (env, wt)
    }

    #[test]
    fn workspace_key_collapses_standalone_grok_worktree_onto_source_repo() {
        // A standalone worktree is a full clone with its OWN `.git`, so git topology can't link it to its source The registry (worktrees.db) must collapse
        // it onto the recorded source repo so trust is shared The worktree dir is a plain dir (no git), proving the REGISTRY path (not git topology) does
        // the collapse `source_repo` is a real git repo (as in production), so the git-root normalization is deterministic regardless of where `$TMPDIR` lives
        let temp = tempfile::TempDir::new().unwrap();
        let root = dunce::canonicalize(temp.path()).unwrap();
        let source_repo = root.join("source-repo");
        std::fs::create_dir_all(&source_repo).unwrap();
        git2::Repository::init(&source_repo).unwrap();

        let (_env, wt) = register_grok_worktree(&temp, "wt", &source_repo, "standalone");

        let expected = canonicalize_or_owned(&source_repo);
        assert_eq!(
            workspace_key(&wt),
            expected,
            "a standalone grok worktree must collapse onto its recorded source repo"
        );
        // A cwd nested below the worktree root collapses onto the same key (the registry walk ascends to the registered worktree)
        let nested = wt.join("crates").join("inner");
        std::fs::create_dir_all(&nested).unwrap();
        assert_eq!(
            workspace_key(&nested),
            expected,
            "a nested cwd in the worktree collapses onto the same source repo"
        );
    }

    #[test]
    fn workspace_key_collapses_worktree_onto_source_repo_git_root() {
        // The registry records `source_repo` as the launch cwd, which may be a SUBDIR of the repo
        // workspace_key must key on the repo's git ROOT, not on `<repo>/sub`
        // A worktree launched from a subdir then shares ONE key with the source and linked worktrees (which key on the root)
        let temp = tempfile::TempDir::new().unwrap();
        let root = dunce::canonicalize(temp.path()).unwrap();
        let repo = root.join("realrepo");
        std::fs::create_dir_all(&repo).unwrap();
        git2::Repository::init(&repo).unwrap();
        let subdir = repo.join("crates").join("sub");
        std::fs::create_dir_all(&subdir).unwrap();

        let (_env, wt) = register_grok_worktree(&temp, "wt", &subdir, "standalone");

        assert_eq!(
            workspace_key(&wt),
            canonicalize_or_owned(&repo),
            "source_repo recorded as a subdir must collapse onto the repo git root"
        );
    }

    #[test]
    fn workspace_key_ignores_registry_for_cwd_outside_worktrees_dir() {
        // A populated registry must NOT collapse a cwd OUTSIDE `<grok_home>/worktrees` The reader's cwd gate skips the registry there, so the key falls back to
        // git/cwd Non-vacuous: the registry IS populated with a real git source repo that WOULD be returned for a worktree cwd `outside` is its OWN git repo (under
        // grok HOME but not under its `worktrees/`), so the fallback is deterministic (no conditional skip) We assert the key is `outside`'s own root, never the source repo
        let temp = tempfile::TempDir::new().unwrap();
        let root = dunce::canonicalize(temp.path()).unwrap();
        let source_repo = root.join("source-repo");
        std::fs::create_dir_all(&source_repo).unwrap();
        git2::Repository::init(&source_repo).unwrap();

        let (_env, _wt) = register_grok_worktree(&temp, "wt", &source_repo, "standalone");

        // Under grok HOME but NOT under `<home>/worktrees`, and its own git repo.
        let outside = root.join("grok-home").join("not-worktrees").join("proj");
        std::fs::create_dir_all(&outside).unwrap();
        git2::Repository::init(&outside).unwrap();

        let key = workspace_key(&outside);
        assert_eq!(
            key,
            canonicalize_or_owned(&outside),
            "a cwd outside <grok_home>/worktrees keys on its own repo root"
        );
        assert_ne!(
            key,
            canonicalize_or_owned(&source_repo),
            "it must not collapse onto the populated registry's source repo"
        );
    }
}
