use std::sync::{Arc, Barrier};
use std::time::Duration;

use super::*;
use tempfile::TempDir;
use xai_grok_config_types::MemoryMode;

fn setup() -> (TempDir, V2CaptureStore) {
    let temp = TempDir::new().unwrap();
    let scope = temp.path().join("scope");
    crate::v2::ensure_scope_initialized(temp.path(), &scope, V2MemoryScope::Workspace).unwrap();
    let store = V2CaptureStore::open(&scope, V2MemoryScope::Workspace).unwrap();
    (temp, store)
}

fn range(from: u32, through: u32) -> CaptureRange {
    CaptureRange::try_new(from, through).unwrap()
}

fn claim(store: &V2CaptureStore, owner: &str, now: i64) -> CaptureLease {
    store
        .claim(&ClaimRequest {
            owner: owner.to_owned(),
            now,
            duration: Duration::from_secs(60),
        })
        .unwrap()
        .unwrap()
}

fn observation(statement: &str) -> ObservationDraft {
    ObservationDraft {
        observation_type: ObservationType::Project,
        topic_hint: Some("capture".to_owned()),
        statement: statement.to_owned(),
        keywords: vec!["durable".to_owned(), "sqlite".to_owned()],
        aliases: vec!["persistence".to_owned()],
        extraction_model: "grok-test".to_owned(),
        prompt_version: "capture-v1".to_owned(),
        created_at: 1_788_000_000,
        body: Some("Bounded supporting detail.".to_owned()),
    }
}

fn observations(statement: &str) -> CaptureOutcomeDraft {
    CaptureOutcomeDraft::Observations(vec![observation(statement)])
}

fn observation_pair(first: &str, second: &str) -> CaptureOutcomeDraft {
    CaptureOutcomeDraft::Observations(vec![observation(first), observation(second)])
}

#[test]
fn filename_is_deterministic_sanitized_and_session_distinct() {
    let (temp, store) = setup();
    let first = store.enqueue("../Team Session", range(4, 9)).unwrap();
    let first_lease = claim(&store, "worker-a", 10);
    let first_commit = store
        .commit(&first_lease, &observations("Deterministic alpha fact"), 11)
        .unwrap();
    assert_eq!(first.job_id, first_lease.job.job_id);
    assert_eq!(first.session_id, first_lease.job.session_id);
    assert_eq!(first.range, first_lease.job.range);
    assert_eq!(first_lease.job.attempt, 1);
    let first_relative = &first_commit.files[0];
    assert_eq!(first_relative.components().count(), 3);
    let filename = first_relative.file_name().unwrap().to_str().unwrap();
    assert!(filename.contains("__t000004-000009__n000.md"));
    assert!(!filename.contains(".."));
    assert!(!filename.contains('/'));

    store.enqueue("Team/Session", range(4, 9)).unwrap();
    let second_lease = claim(&store, "worker-b", 20);
    let second_commit = store
        .commit(&second_lease, &observations("Deterministic beta fact"), 21)
        .unwrap();
    assert_ne!(first_commit.files, second_commit.files);
    assert!(temp.path().join("scope").join(first_relative).is_file());
}

#[test]
fn schema_migrates_v1_and_reopen_is_idempotent() {
    let temp = TempDir::new().unwrap();
    let scope = temp.path().join("scope");
    std::fs::create_dir_all(scope.join("observations/_inbox")).unwrap();
    let state_path = scope.join("memory_state.sqlite");
    let connection = JournalMode::for_db_path(&state_path)
        .open(&state_path)
        .unwrap();
    connection
        .execute_batch(
            "CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO meta VALUES ('schema_version', '1');",
        )
        .unwrap();
    drop(connection);

    V2CaptureStore::open(&scope, V2MemoryScope::Global).unwrap();
    V2CaptureStore::open(&scope, V2MemoryScope::Global).unwrap();
    let connection = JournalMode::for_db_path(&state_path)
        .open_readonly(&state_path)
        .unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "3"
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'table' AND name LIKE 'capture_%'",
                [],
                |row| row.get::<_, u32>(0)
            )
            .unwrap(),
        4
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT value FROM meta WHERE key = 'capture_revision'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "0"
    );
}

#[test]
fn network_mounted_capture_store_fails_before_creating_shared_state() {
    let temp = TempDir::new().unwrap();
    let scope = temp.path().join("scope");
    std::fs::create_dir(&scope).unwrap();

    let error = match V2CaptureStore::open_with_journal_mode(
        &scope,
        V2MemoryScope::Workspace,
        Some(JournalMode::Truncate),
    ) {
        Ok(_) => panic!("network-mounted capture unexpectedly opened"),
        Err(error) => error,
    };

    assert!(matches!(
        error,
        V2CaptureError::UnsupportedNetworkFilesystem { path } if path == scope
    ));
    assert!(!scope.join("observations").exists());
    assert!(!scope.join("memory_state.sqlite").exists());
    assert!(
        !JournalMode::Truncate
            .effective_db_path(&scope.join("memory_state.sqlite"))
            .exists()
    );
}

#[test]
fn duplicate_enqueue_is_one_job_and_requested_cursor_is_monotonic() {
    let (_temp, store) = setup();
    let first = store.enqueue("session", range(1, 4)).unwrap();
    let duplicate = store.enqueue("session", range(1, 4)).unwrap();
    store.enqueue("session", range(5, 8)).unwrap();
    store.enqueue("session", range(2, 3)).unwrap();
    assert_eq!(first, duplicate);
    assert_eq!(store.cursors("session").unwrap().requested, 8);
    let connection = store.open_state().unwrap();
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM capture_jobs", [], |row| row
                .get::<_, u32>(0))
            .unwrap(),
        3
    );
}

#[test]
fn observation_commit_converges_file_fts_manifest_and_cursors() {
    let (temp, store) = setup();
    store.enqueue("session", range(1, 5)).unwrap();
    let lease = claim(&store, "worker", 100);
    let result = store
        .commit(
            &lease,
            &observations("Quasar capture token is durable"),
            101,
        )
        .unwrap();
    let path = temp.path().join("scope").join(&result.files[0]);
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("schema_version: 2"));
    assert!(content.contains("type: project"));
    assert!(content.contains("job_id: \"cap_"));

    let storage = MemoryStorage::new_flat(temp.path(), &temp.path().join("scope"));
    let index = crate::MemoryIndex::open_or_create(
        &temp.path().join("scope/index.sqlite"),
        storage,
        xai_grok_config_types::MemoryIndexConfig::default(),
        1,
    )
    .unwrap();
    assert!(!index.search_fts("quasar durable", 10).unwrap().is_empty());
    let manifest = std::fs::read_to_string(temp.path().join("scope/MEMORY.md")).unwrap();
    assert!(manifest.contains("## Pending observations"));
    assert!(manifest.contains(result.files[0].to_str().unwrap()));
    assert_eq!(
        store.cursors("session").unwrap(),
        CaptureCursors {
            requested: 5,
            captured: 5,
            indexed: 5
        }
    );
}

#[test]
fn capture_reindex_preserves_existing_embedding_dimension_and_rows() {
    crate::init_sqlite_vec();
    let (temp, store) = setup();
    let scope = temp.path().join("scope");
    let index_path = scope.join("index.sqlite");
    let storage = MemoryStorage::new_flat(temp.path(), &scope);
    let index = crate::MemoryIndex::open_or_create(
        &index_path,
        storage,
        xai_grok_config_types::MemoryIndexConfig::default(),
        4,
    )
    .unwrap();
    if !index.vec_available() {
        return;
    }
    index
        .upsert_embedding("embedding-sentinel", &[0.0, 0.0, 0.0, 0.0])
        .unwrap();
    drop(index);

    store.enqueue("session", range(1, 2)).unwrap();
    store
        .commit(
            &claim(&store, "worker", 10),
            &observations("Capture must preserve vectors"),
            11,
        )
        .unwrap();

    let storage = MemoryStorage::new_flat(temp.path(), &scope);
    let index = crate::MemoryIndex::open_or_create(
        &index_path,
        storage,
        xai_grok_config_types::MemoryIndexConfig::default(),
        4,
    )
    .unwrap();
    assert_eq!(index.embedding_dimensions(), 4);
    assert!(
        index
            .vector_search(&[0.0, 0.0, 0.0, 0.0], 10)
            .unwrap()
            .iter()
            .any(|(chunk_id, _)| chunk_id == "embedding-sentinel")
    );
}

#[test]
fn noop_advances_cursors_without_files_or_index_changes() {
    let (temp, store) = setup();
    let index_path = temp.path().join("scope/index.sqlite");
    let before = JournalMode::for_db_path(&index_path)
        .open_readonly(&index_path)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM chunks", [], |row| {
            row.get::<_, u32>(0)
        })
        .unwrap();
    store.enqueue("session", range(1, 3)).unwrap();
    let result = store
        .commit(&claim(&store, "worker", 10), &CaptureOutcomeDraft::Noop, 11)
        .unwrap();
    assert!(result.files.is_empty());
    assert_eq!(
        std::fs::read_dir(temp.path().join("scope/observations/_inbox"))
            .unwrap()
            .count(),
        0
    );
    let after = JournalMode::for_db_path(&index_path)
        .open_readonly(&index_path)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM chunks", [], |row| {
            row.get::<_, u32>(0)
        })
        .unwrap();
    assert_eq!(before, after);
    assert_eq!(
        store.cursors("session").unwrap(),
        CaptureCursors {
            requested: 3,
            captured: 3,
            indexed: 3
        }
    );
}

#[test]
fn retry_same_bytes_is_idempotent_and_different_outcome_is_rejected() {
    let (_temp, store) = setup();
    store.enqueue("session", range(1, 2)).unwrap();
    let lease = claim(&store, "worker", 10);
    let outcome = observations("Retry-safe immutable fact");
    let first = store.commit(&lease, &outcome, 11).unwrap();
    let second = store.commit(&lease, &outcome, 12).unwrap();
    assert_eq!(first.outcome_hash, second.outcome_hash);
    assert!(second.was_already_committed);
    assert!(matches!(
        store.commit(&lease, &observations("Changed retry bytes"), 12),
        Err(V2CaptureError::Conflict(_))
    ));
}

#[test]
fn expired_lease_is_reclaimed_and_stale_owner_is_fenced() {
    let (_temp, store) = setup();
    store.enqueue("session", range(1, 2)).unwrap();
    let old = store
        .claim(&ClaimRequest {
            owner: "old".to_owned(),
            now: 10,
            duration: Duration::from_secs(5),
        })
        .unwrap()
        .unwrap();
    let new = store
        .claim(&ClaimRequest {
            owner: "new".to_owned(),
            now: 15,
            duration: Duration::from_secs(5),
        })
        .unwrap()
        .unwrap();
    assert_eq!(new.job.attempt, old.job.attempt + 1);
    assert!(matches!(
        store.commit(&old, &CaptureOutcomeDraft::Noop, 15),
        Err(V2CaptureError::StaleLease)
    ));
    store.commit(&new, &CaptureOutcomeDraft::Noop, 16).unwrap();
}

#[test]
fn reconcile_repairs_files_before_outcome_and_outcome_before_index() {
    let (temp, store) = setup();
    store.enqueue("files-first", range(1, 2)).unwrap();
    let files_lease = claim(&store, "worker-files", 10);
    let files_prepared = store
        .prepare_outcome(&files_lease.job, &observations("Files-first crash token"))
        .unwrap();
    for file in &files_prepared.files {
        persist_create_only(&temp.path().join("scope").join(&file.path), &file.bytes).unwrap();
    }
    store.reconcile().unwrap();
    assert_eq!(store.cursors("files-first").unwrap().indexed, 2);

    store.enqueue("outcome-first", range(1, 4)).unwrap();
    let outcome_lease = claim(&store, "worker-outcome", 20);
    let outcome_prepared = store
        .prepare_outcome(
            &outcome_lease.job,
            &observations("Outcome-before-index crash token"),
        )
        .unwrap();
    for file in &outcome_prepared.files {
        persist_create_only(&temp.path().join("scope").join(&file.path), &file.bytes).unwrap();
    }
    let mut connection = store.open_state().unwrap();
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    persist_outcome(&transaction, &outcome_lease.job, &outcome_prepared, 21).unwrap();
    transaction.commit().unwrap();
    assert_eq!(store.cursors("outcome-first").unwrap().indexed, 0);
    store.reconcile().unwrap();
    assert_eq!(store.cursors("outcome-first").unwrap().indexed, 4);

    let storage = MemoryStorage::new_flat(temp.path(), &temp.path().join("scope"));
    let index = crate::MemoryIndex::open_or_create(
        &temp.path().join("scope/index.sqlite"),
        storage,
        xai_grok_config_types::MemoryIndexConfig::default(),
        1,
    )
    .unwrap();
    assert!(!index.search_fts("files crash", 10).unwrap().is_empty());
    assert!(!index.search_fts("outcome crash", 10).unwrap().is_empty());
}

#[test]
fn slow_convergence_does_not_block_capture_queue_writes() {
    let (temp, store) = setup();
    let scope = temp.path().join("scope");
    store.enqueue("captured", range(1, 2)).unwrap();
    let lease = claim(&store, "capture-worker", 10);
    let prepared = store
        .prepare_outcome(&lease.job, &observations("Slow convergence fact"))
        .unwrap();
    for file in &prepared.files {
        persist_create_only(&scope.join(&file.path), &file.bytes).unwrap();
    }
    let mut connection = store.open_state().unwrap();
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    persist_outcome(&transaction, &lease.job, &prepared, 11).unwrap();
    transaction.commit().unwrap();

    let queue_store = V2CaptureStore::open(&scope, V2MemoryScope::Workspace).unwrap();
    let (convergence_started_tx, convergence_started_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    let reconcile = std::thread::spawn(move || {
        store.reconcile_at_with(12, || {
            convergence_started_tx.send(()).unwrap();
            resume_rx.recv().unwrap();
            Ok(())
        })
    });
    convergence_started_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap();

    let (enqueue_finished_tx, enqueue_finished_rx) = std::sync::mpsc::channel();
    let enqueue = std::thread::spawn(move || {
        let result = queue_store.enqueue("queued", range(3, 4));
        enqueue_finished_tx.send(result.is_ok()).unwrap();
    });
    assert!(
        enqueue_finished_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
    );

    resume_tx.send(()).unwrap();
    reconcile.join().unwrap().unwrap();
    enqueue.join().unwrap();
}

#[test]
fn stale_reconcile_cannot_replace_newer_cross_handle_manifest() {
    let (temp, store) = setup();
    let scope = temp.path().join("scope");
    store.enqueue("session", range(1, 2)).unwrap();
    let first_lease = claim(&store, "first-worker", 10);
    let first = store
        .prepare_outcome(&first_lease.job, &observations("First snapshot fact"))
        .unwrap();
    for file in &first.files {
        persist_create_only(&scope.join(&file.path), &file.bytes).unwrap();
    }
    let mut connection = store.open_state().unwrap();
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    persist_outcome(&transaction, &first_lease.job, &first, 11).unwrap();
    transaction.commit().unwrap();

    store.enqueue("session", range(3, 4)).unwrap();
    let second_lease = claim(&store, "second-worker", 20);
    let newer = V2CaptureStore::open(&scope, V2MemoryScope::Workspace).unwrap();
    let observer = V2CaptureStore::open(&scope, V2MemoryScope::Workspace).unwrap();
    let (older_rendered_tx, older_rendered_rx) = std::sync::mpsc::channel();
    let (resume_older_tx, resume_older_rx) = std::sync::mpsc::channel();
    let older = std::thread::spawn(move || {
        store.reconcile_at_with(21, || {
            older_rendered_tx.send(()).unwrap();
            resume_older_rx.recv().unwrap();
            Ok(())
        })
    });
    older_rendered_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap();

    let second_result = newer
        .commit(&second_lease, &observations("Concurrent snapshot fact"), 21)
        .unwrap();
    let newer_manifest = std::fs::read_to_string(scope.join("MEMORY.md")).unwrap();
    assert!(newer_manifest.contains(first.files[0].path.to_str().unwrap()));
    assert!(newer_manifest.contains(second_result.files[0].to_str().unwrap()));

    resume_older_tx.send(()).unwrap();
    older.join().unwrap().unwrap();
    let final_manifest = std::fs::read_to_string(scope.join("MEMORY.md")).unwrap();
    assert_eq!(final_manifest, newer_manifest);
    assert_eq!(observer.cursors("session").unwrap().indexed, 4);
}

#[test]
fn orphan_adoption_skips_live_lease_and_adopts_only_expired_work() {
    let (temp, store) = setup();
    let scope = temp.path().join("scope");
    store.enqueue("abandoned", range(1, 2)).unwrap();
    store.enqueue("active", range(1, 3)).unwrap();
    let abandoned = store
        .claim(&ClaimRequest {
            owner: "abandoned-worker".to_owned(),
            now: 10,
            duration: Duration::from_secs(1),
        })
        .unwrap()
        .unwrap();
    let active = store
        .claim(&ClaimRequest {
            owner: "active-worker".to_owned(),
            now: 10,
            duration: Duration::from_secs(60),
        })
        .unwrap()
        .unwrap();
    let abandoned_files = store
        .prepare_outcome(&abandoned.job, &observations("Abandoned orphan fact"))
        .unwrap();
    let active_files = store
        .prepare_outcome(&active.job, &observations("Old active orphan fact"))
        .unwrap();
    for file in abandoned_files.files.iter().chain(&active_files.files) {
        persist_create_only(&scope.join(&file.path), &file.bytes).unwrap();
    }

    store.reconcile_at_with(12, || Ok(())).unwrap();
    let connection = store.open_state().unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM capture_outcomes WHERE job_id = ?1",
                params![abandoned.job.job_id],
                |row| row.get::<_, u32>(0)
            )
            .unwrap(),
        1
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM capture_outcomes WHERE job_id = ?1",
                params![active.job.job_id],
                |row| row.get::<_, u32>(0)
            )
            .unwrap(),
        0
    );
    drop(connection);

    let result = store
        .commit(&active, &observations("New active retry fact"), 13)
        .unwrap();
    assert!(
        std::fs::read_to_string(scope.join(&result.files[0]))
            .unwrap()
            .contains("New active retry fact")
    );
}

#[test]
fn malformed_orphan_does_not_block_adoption_for_other_jobs() {
    let (temp, store) = setup();
    let scope = temp.path().join("scope");
    store.enqueue("malformed", range(1, 2)).unwrap();
    store.enqueue("recoverable", range(1, 3)).unwrap();
    let malformed = store
        .claim(&ClaimRequest {
            owner: "malformed-worker".to_owned(),
            now: 10,
            duration: Duration::from_secs(1),
        })
        .unwrap()
        .unwrap();
    let recoverable = store
        .claim(&ClaimRequest {
            owner: "recoverable-worker".to_owned(),
            now: 10,
            duration: Duration::from_secs(1),
        })
        .unwrap()
        .unwrap();
    let malformed_files = store
        .prepare_outcome(&malformed.job, &observations("Malformed orphan"))
        .unwrap();
    std::fs::write(
        scope.join(&malformed_files.files[0].path),
        b"not valid observation metadata",
    )
    .unwrap();
    let recoverable_files = store
        .prepare_outcome(&recoverable.job, &observations("Recoverable orphan"))
        .unwrap();
    for file in &recoverable_files.files {
        persist_create_only(&scope.join(&file.path), &file.bytes).unwrap();
    }

    store.reconcile_at_with(12, || Ok(())).unwrap();
    let connection = store.open_state().unwrap();
    let outcome_count = |job_id: &str| {
        connection
            .query_row(
                "SELECT COUNT(*) FROM capture_outcomes WHERE job_id = ?1",
                params![job_id],
                |row| row.get::<_, u32>(0),
            )
            .unwrap()
    };
    assert_eq!(outcome_count(&malformed.job.job_id), 0);
    assert_eq!(outcome_count(&recoverable.job.job_id), 1);
}

#[test]
fn retry_removes_malformed_files_in_owned_namespace() {
    let (temp, store) = setup();
    store.enqueue("session", range(1, 2)).unwrap();
    let first_lease = claim(&store, "first-worker", 10);
    let prepared = store
        .prepare_outcome(&first_lease.job, &observations("First attempt"))
        .unwrap();
    let malformed_path = temp
        .path()
        .join("scope")
        .join(&prepared.files[0].path)
        .with_file_name(
            prepared.files[0]
                .path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .replace("__n000.md", "__ninvalid.md"),
        );
    std::fs::write(&malformed_path, b"malformed").unwrap();

    let retry_lease = claim(&store, "retry-worker", 70);
    store
        .commit(&retry_lease, &observations("Retry succeeds"), 71)
        .unwrap();
    assert!(!malformed_path.exists());
}

#[test]
fn leased_retry_replaces_partial_same_job_files_with_different_count() {
    let (temp, store) = setup();
    store.enqueue("session", range(1, 2)).unwrap();
    let first_lease = claim(&store, "first-worker", 10);
    let first_outcome = observation_pair("Old first fact", "Old second fact");
    let first_prepared = store
        .prepare_outcome(&first_lease.job, &first_outcome)
        .unwrap();
    let first_path = temp
        .path()
        .join("scope")
        .join(&first_prepared.files[0].path);
    let second_path = temp
        .path()
        .join("scope")
        .join(&first_prepared.files[1].path);
    persist_create_only(&first_path, &first_prepared.files[0].bytes).unwrap();
    persist_create_only(&second_path, &first_prepared.files[1].bytes).unwrap();

    let retry_lease = claim(&store, "retry-worker", 70);
    let result = store
        .commit(&retry_lease, &observations("New first fact"), 71)
        .unwrap();

    assert_eq!(result.files.len(), 1);
    assert!(
        std::fs::read_to_string(first_path)
            .unwrap()
            .contains("New first fact")
    );
    assert!(!second_path.exists());
}

#[test]
fn leased_noop_retry_removes_partial_same_job_files() {
    let (temp, store) = setup();
    store.enqueue("session", range(1, 2)).unwrap();
    let first_lease = claim(&store, "first-worker", 10);
    let prepared = store
        .prepare_outcome(&first_lease.job, &observations("Abandoned fact"))
        .unwrap();
    let path = temp.path().join("scope").join(&prepared.files[0].path);
    persist_create_only(&path, &prepared.files[0].bytes).unwrap();

    let retry_lease = claim(&store, "retry-worker", 70);
    let result = store
        .commit(&retry_lease, &CaptureOutcomeDraft::Noop, 71)
        .unwrap();

    assert!(result.files.is_empty());
    assert!(!path.exists());
}

#[test]
fn leased_retry_accepts_partial_same_job_files_with_identical_bytes() {
    let (temp, store) = setup();
    store.enqueue("session", range(1, 2)).unwrap();
    let first_lease = claim(&store, "first-worker", 10);
    let outcome = observation_pair("Stable first fact", "Stable second fact");
    let prepared = store.prepare_outcome(&first_lease.job, &outcome).unwrap();
    persist_create_only(
        &temp.path().join("scope").join(&prepared.files[0].path),
        &prepared.files[0].bytes,
    )
    .unwrap();

    let retry_lease = claim(&store, "retry-worker", 70);
    let result = store.commit(&retry_lease, &outcome, 71).unwrap();
    assert_eq!(result.files.len(), 2);
}

#[test]
fn leased_retry_replaces_tampered_file_at_owned_path() {
    let (temp, store) = setup();
    store.enqueue("session", range(1, 2)).unwrap();
    let first_lease = claim(&store, "first-worker", 10);
    let outcome = observations("Original fact");
    let prepared = store.prepare_outcome(&first_lease.job, &outcome).unwrap();
    let path = temp.path().join("scope").join(&prepared.files[0].path);
    let foreign = String::from_utf8(prepared.files[0].bytes.clone())
        .unwrap()
        .replace(
            &format!("job_id: \"{}\"", first_lease.job.job_id),
            "job_id: \"cap_foreign\"",
        )
        .into_bytes();
    persist_create_only(&path, &foreign).unwrap();

    let retry_lease = claim(&store, "retry-worker", 70);
    store
        .commit(&retry_lease, &observations("Retry fact"), 71)
        .unwrap();
    assert!(
        std::fs::read_to_string(path)
            .unwrap()
            .contains("Retry fact")
    );
}

#[test]
fn recovery_directory_and_database_scans_fail_at_named_caps() {
    let (temp, store) = setup();
    let inbox = temp.path().join("scope/observations/_inbox");
    for index in 0..=MAX_INBOX_ENTRIES {
        std::fs::write(inbox.join(format!("unrelated-{index}.tmp")), []).unwrap();
    }
    assert!(matches!(
        store.reconcile(),
        Err(V2CaptureError::RecoveryDirectoryLimit { limit, .. })
            if limit == MAX_INBOX_ENTRIES
    ));

    let (_temp, store) = setup();
    store.ensure_session("session").unwrap();
    let mut connection = store.open_state().unwrap();
    let transaction = connection.transaction().unwrap();
    for index in 0..=MAX_RECOVERY_JOBS {
        transaction
            .execute(
                "INSERT INTO capture_jobs(
                    job_id, session_id, from_turn, through_turn, status, attempt
                 ) VALUES (?1, 'session', ?2, ?2, 'pending', 0)",
                params![format!("job-{index}"), index],
            )
            .unwrap();
    }
    transaction.commit().unwrap();
    assert!(matches!(
        store.reconcile(),
        Err(V2CaptureError::RecoveryRowLimit { rows, limit })
            if rows == "uncommitted capture jobs" && limit == MAX_RECOVERY_JOBS
    ));
}

#[test]
fn committed_observation_scan_has_a_named_row_cap() {
    let (_temp, store) = setup();
    store.ensure_session("session").unwrap();
    let mut connection = store.open_state().unwrap();
    let transaction = connection.transaction().unwrap();
    for index in 0..=MAX_COMMITTED_OBSERVATION_ROWS {
        let job_id = format!("job-{index}");
        transaction
            .execute(
                "INSERT INTO capture_jobs(
                    job_id, session_id, from_turn, through_turn, status, attempt
                 ) VALUES (?1, 'session', ?2, ?2, 'completed', 1)",
                params![job_id, index],
            )
            .unwrap();
        transaction
            .execute(
                "INSERT INTO capture_outcomes(job_id, outcome_kind, outcome_hash, committed_at)
                 VALUES (?1, 'observations', 'hash', 0)",
                params![job_id],
            )
            .unwrap();
        transaction
            .execute(
                "INSERT INTO capture_observation_files(job_id, ordinal, path, content_hash)
                 VALUES (?1, 0, ?2, 'hash')",
                params![job_id, format!("observations/_inbox/{index}.md")],
            )
            .unwrap();
    }
    transaction.commit().unwrap();
    assert!(matches!(
        store.reconcile(),
        Err(V2CaptureError::RecoveryRowLimit { rows, limit })
            if rows == "committed observation files"
                && limit == MAX_COMMITTED_OBSERVATION_ROWS
    ));
}

#[test]
fn convergence_errors_preserve_typed_sources() {
    use std::error::Error as _;

    let (temp, store) = setup();
    std::fs::remove_file(temp.path().join("scope/MEMORY.md")).unwrap();
    std::fs::create_dir(temp.path().join("scope/MEMORY.md")).unwrap();
    let error = store.reconcile().unwrap_err();
    assert!(matches!(&error, V2CaptureError::Manifest(_)));
    assert!(error.source().is_some());

    let (temp, store) = setup();
    store.enqueue("session", range(1, 2)).unwrap();
    let lease = claim(&store, "worker", 10);
    let prepared = store
        .prepare_outcome(&lease.job, &observations("Index source fact"))
        .unwrap();
    for file in &prepared.files {
        persist_create_only(&temp.path().join("scope").join(&file.path), &file.bytes).unwrap();
    }
    let mut connection = store.open_state().unwrap();
    let transaction = connection.transaction().unwrap();
    persist_outcome(&transaction, &lease.job, &prepared, 11).unwrap();
    transaction.commit().unwrap();
    std::fs::remove_file(temp.path().join("scope/index.sqlite")).unwrap();
    std::fs::create_dir(temp.path().join("scope/index.sqlite")).unwrap();
    let error = store.reconcile().unwrap_err();
    assert!(matches!(&error, V2CaptureError::Index { .. }));
    assert!(error.source().is_some());
}

#[test]
fn independent_handles_for_two_sessions_preserve_files_and_manifest() {
    let (temp, first) = setup();
    let scope = temp.path().join("scope");
    let second = V2CaptureStore::open(&scope, V2MemoryScope::Workspace).unwrap();
    first.enqueue("session-a", range(1, 3)).unwrap();
    first.enqueue("session-b", range(1, 3)).unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let handles: Vec<_> = [
        (first, "worker-a", "Concurrent alpha token"),
        (second, "worker-b", "Concurrent beta token"),
    ]
    .into_iter()
    .map(|(store, owner, statement)| {
        let barrier = barrier.clone();
        std::thread::spawn(move || {
            barrier.wait();
            let lease = claim(&store, owner, 10);
            store.commit(&lease, &observations(statement), 11).unwrap()
        })
    })
    .collect();
    let results: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect();
    assert_ne!(results[0].files, results[1].files);
    assert!(scope.join(&results[0].files[0]).is_file());
    assert!(scope.join(&results[1].files[0]).is_file());
    let manifest = std::fs::read_to_string(scope.join("MEMORY.md")).unwrap();
    assert!(manifest.contains(results[0].files[0].to_str().unwrap()));
    assert!(manifest.contains(results[1].files[0].to_str().unwrap()));
}

#[test]
fn invalid_ranges_bounds_controls_and_symlinks_fail_closed() {
    assert!(CaptureRange::try_new(2, 1).is_err());
    assert!(CaptureRange::try_new(0, 1_000_000).is_err());
    assert!(ObservationType::parse("instruction").is_err());
    let (_temp, store) = setup();
    assert!(store.enqueue("bad\nsession", range(1, 2)).is_err());
    store.enqueue("session", range(1, 2)).unwrap();
    let lease = claim(&store, "worker", 10);
    let mut invalid = observation("valid");
    invalid.statement = "x".repeat(MAX_STATEMENT_BYTES + 1);
    assert!(matches!(
        store.commit(
            &lease,
            &CaptureOutcomeDraft::Observations(vec![invalid]),
            11
        ),
        Err(V2CaptureError::Invalid(_))
    ));
    let mut invalid = observation("valid");
    invalid.keywords = (0..=MAX_KEYWORDS).map(|index| index.to_string()).collect();
    assert!(
        store
            .commit(
                &lease,
                &CaptureOutcomeDraft::Observations(vec![invalid]),
                11
            )
            .is_err()
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().unwrap();
        let scope = temp.path().join("scope");
        std::fs::create_dir_all(scope.join("observations")).unwrap();
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        symlink(&outside, scope.join("observations/_inbox")).unwrap();
        assert!(V2CaptureStore::open(&scope, V2MemoryScope::Workspace).is_err());
        assert_eq!(std::fs::read_dir(outside).unwrap().count(), 0);
    }
}

#[test]
fn v2_sources_are_evergreen_while_legacy_sessions_still_decay() {
    let temp = TempDir::new().unwrap();
    let v2 = MemoryStorage::new_for_mode(
        Path::new("/home/test/project"),
        Some(temp.path()),
        MemoryMode::V2,
    );
    assert_eq!(
        v2.classify_source(&v2.workspace_dir().join("topics/project.md")),
        "workspace"
    );
    assert_eq!(
        v2.classify_source(&v2.workspace_dir().join("observations/_inbox/capture.md")),
        "workspace"
    );
    assert_eq!(
        v2.classify_source(&v2.global_dir().join("observations/_inbox/capture.md")),
        "global"
    );

    let legacy = MemoryStorage::with_paths(
        temp.path().join("legacy"),
        temp.path().join("legacy/workspace"),
    );
    assert_eq!(
        legacy.classify_source(&legacy.workspace_dir().join("sessions/log.md")),
        "session"
    );
    assert_eq!(
        legacy.classify_source(&legacy.workspace_dir().join("topic.md")),
        "session"
    );
    assert_eq!(
        legacy.classify_source(&legacy.workspace_dir().join("MEMORY.md")),
        "workspace"
    );
}
