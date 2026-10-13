//! Batch Dream claims under the scope's shared consolidation lease, so it never
//! edits a scope alongside the default runner, and persists each commit before
//! touching topic bytes so an interrupted apply rolls forward.

use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use rusqlite::{OptionalExtension as _, TransactionBehavior, params};
use xai_sqlite_journal::JournalMode;

use crate::batch_dream_io::{hash_bytes, read_bounded};
use crate::{V2MemoryAccessPolicy, V2MemoryScope, V2PathClass};

pub use crate::batch_dream_catalog::{CATALOG_BUDGET_BYTES, CatalogTier, TopicCatalog};
pub use crate::batch_dream_control::BatchDreamControl;
pub use crate::batch_dream_plan::{
    BatchPlan, BatchResponse, MAX_PLAN_EDITS, MAX_PLAN_TEXT_BYTES, PlanCheck, PlanEdit,
    PlanOutcome, response_schema,
};
pub use crate::batch_dream_session::{
    BatchAction, BatchDreamSession, MAX_ACTIONS_PER_CALL, SearchCursor, SessionBudgets,
};

/// Largest note a batch accepts; matches the capture and write limits.
pub const MAX_NOTE_BYTES: usize = 16 * 1024;
/// Largest note-byte budget a single batch may request.
pub const MAX_BATCH_NOTE_BYTES: usize = 256 * 1024;
pub const MAX_BATCH_NOTES: usize = 20;
/// A note the model defers this many times is archived as unplaceable, so it
/// stops re-triggering runs that never place it.
pub const MAX_NOTE_DEFERRALS: i64 = 3;
pub const CAPACITY_DEFERRAL_REASON: &str = "batch too large";
pub const MAX_PATH_BYTES: usize = 240;
pub const MAX_CHANGES: usize = 32;
pub const MAX_SPLICES_PER_FILE: usize = 32;
pub const MAX_DEPENDENCIES: usize = 128;
/// Cap on new text across one commit, including created topics.
pub const MAX_COMMIT_TEXT_BYTES: usize = 256 * 1024;
pub const MAX_REASON_BYTES: usize = 512;
const MAX_OWNER_BYTES: usize = 128;
const MAX_EXCLUDED_NOTES: usize = 4_096;
const MAX_INBOX_ENTRIES: usize = 100_000;
pub(crate) const INBOX_DIR: &str = "observations/_inbox";
pub(crate) const TOPICS_DIR: &str = "topics";
pub(crate) const STAGE_PREFIX: &str = ".batch-dream-";

pub type Result<T> = std::result::Result<T, BatchDreamError>;

#[derive(Debug, thiserror::Error)]
pub enum BatchDreamError {
    #[error("batch Dream was interrupted before durable publication")]
    Interrupted,
    #[error("invalid batch Dream request: {0}")]
    Invalid(String),
    #[error("another memory consolidation is running")]
    Busy,
    #[error("batch Dream lease is stale")]
    StaleLease,
    #[error("batch Dream conflict: {0}")]
    Conflict(String),
    #[error("batch Dream database failed: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("batch Dream filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("batch Dream access denied: {0}")]
    Access(#[from] crate::V2AccessError),
    #[error("batch Dream manifest or index update failed: {0}")]
    Storage(#[from] crate::V2StorageError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchClaimRequest<'a> {
    pub owner: &'a str,
    pub now: i64,
    pub duration: Duration,
    /// Note bytes the batch may hold; at least one note is always claimed.
    pub max_note_bytes: usize,
    /// Scope-relative note paths this run already attempted.
    pub excluded: &'a [String],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimedNote {
    pub path: String,
    pub content_hash: String,
    /// Capture commit time, or file modification time for manual notes.
    pub created_at: i64,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchLease {
    pub operation_id: String,
    pub owner: String,
    pub generation: u64,
    pub expires_at: i64,
    /// Claimed notes in time order.
    pub notes: Vec<ClaimedNote>,
}

/// Replace bytes `start..end` of the base file with `text`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Splice {
    pub start: u64,
    pub end: u64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileChange {
    Splice {
        path: String,
        base_hash: String,
        splices: Vec<Splice>,
    },
    Create {
        path: String,
        content: String,
    },
}

impl FileChange {
    #[must_use]
    pub fn path(&self) -> &str {
        match self {
            FileChange::Splice { path, .. } | FileChange::Create { path, .. } => path,
        }
    }
}

/// A topic version an outcome relied on without editing it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDependency {
    pub path: String,
    pub content_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoteDisposition {
    Applied,
    NoChange,
    Deferred { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteOutcome {
    pub path: String,
    pub disposition: NoteDisposition,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BatchCommit {
    pub changes: Vec<FileChange>,
    pub dependencies: Vec<FileDependency>,
    pub outcomes: Vec<NoteOutcome>,
    /// Set by the runner when a reply was cut off and the batch holds more than one note; only then
    /// is a deferral with [`CAPACITY_DEFERRAL_REASON`] requeued without counting toward giving up.
    pub capacity_deferrals_uncounted: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BatchReport {
    pub operation_id: String,
    pub applied: Vec<String>,
    pub no_change: Vec<String>,
    pub deferred: Vec<String>,
    /// Notes archived after [`MAX_NOTE_DEFERRALS`] deferrals.
    pub unplaceable: Vec<String>,
    pub changed_topics: Vec<String>,
}

/// Scope-local batch Dream storage handle.
#[derive(Debug)]
pub struct BatchDreamStore {
    pub(crate) scope_dir: PathBuf,
    pub(crate) scope: V2MemoryScope,
    pub(crate) access: V2MemoryAccessPolicy,
    pub(crate) control: BatchDreamControl,
}

impl BatchDreamStore {
    /// Open an initialized scope and additively migrate batch Dream tables.
    ///
    /// # Errors
    /// Returns `Invalid` for a scope that is not the configured root or has a
    /// symlinked component, and access, I/O, or database errors otherwise.
    pub fn open(
        scope_dir: impl AsRef<Path>,
        scope: V2MemoryScope,
        global_dir: &Path,
        workspace_dir: &Path,
    ) -> Result<BatchDreamStore> {
        let scope_dir = scope_dir.as_ref().to_path_buf();
        let access = V2MemoryAccessPolicy::new(global_dir, workspace_dir)?;
        let expected = match scope {
            V2MemoryScope::Global => global_dir,
            V2MemoryScope::Workspace => workspace_dir,
        };
        if dunce::canonicalize(&scope_dir)? != dunce::canonicalize(expected)? {
            return Err(BatchDreamError::Invalid(
                "scope does not match its configured root".to_owned(),
            ));
        }
        for relative in ["", "memory_state.sqlite", TOPICS_DIR, INBOX_DIR, "archive"] {
            let path = scope_dir.join(relative);
            if std::fs::symlink_metadata(&path)?.file_type().is_symlink() {
                return Err(BatchDreamError::Invalid(format!(
                    "symlinked scope component: {}",
                    path.display()
                )));
            }
        }
        let store = BatchDreamStore {
            scope_dir,
            scope,
            access,
            control: BatchDreamControl::default(),
        };
        let mut connection = store.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        crate::v2_maintenance::migrate_v2_hardening(&transaction)
            .map_err(|error| BatchDreamError::Invalid(error.to_string()))?;
        transaction.execute_batch(
            "CREATE TABLE IF NOT EXISTS batch_dream_queue (
                path TEXT PRIMARY KEY,
                content_hash TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                queue_order INTEGER NOT NULL,
                attempts INTEGER NOT NULL DEFAULT 0,
                deferrals INTEGER NOT NULL DEFAULT 0,
                status TEXT NOT NULL CHECK(status IN ('pending','claimed','invalid')),
                reason TEXT
            );
            CREATE TABLE IF NOT EXISTS batch_dream_batches (
                operation_id TEXT PRIMARY KEY,
                owner TEXT NOT NULL,
                generation INTEGER NOT NULL,
                expires_at INTEGER NOT NULL,
                status TEXT NOT NULL
                    CHECK(status IN ('claimed','planned','blocked','completed','released')),
                created_at INTEGER NOT NULL,
                completed_at INTEGER,
                cleanup_pending INTEGER NOT NULL DEFAULT 0,
                last_error TEXT
            );
            CREATE TABLE IF NOT EXISTS batch_dream_batch_notes (
                operation_id TEXT NOT NULL,
                path TEXT NOT NULL,
                content_hash TEXT NOT NULL,
                disposition TEXT CHECK(disposition IN ('applied','no_change','deferred','released','unplaceable')),
                reason TEXT,
                PRIMARY KEY(operation_id, path)
            );
            CREATE TABLE IF NOT EXISTS batch_dream_changes (
                operation_id TEXT NOT NULL,
                path TEXT NOT NULL,
                base_hash TEXT,
                post_hash TEXT NOT NULL,
                stage_path TEXT NOT NULL,
                PRIMARY KEY(operation_id, path)
            );
            CREATE TABLE IF NOT EXISTS batch_dream_dependencies (
                operation_id TEXT NOT NULL,
                path TEXT NOT NULL,
                content_hash TEXT NOT NULL,
                PRIMARY KEY(operation_id, path)
            );",
        )?;
        transaction.commit()?;
        Ok(store)
    }

    #[must_use]
    pub fn with_control(mut self, control: BatchDreamControl) -> BatchDreamStore {
        self.control = control;
        self
    }

    #[must_use]
    pub fn scope_dir(&self) -> &Path {
        &self.scope_dir
    }

    /// Deferrals recorded for a queued note, or `None` when the note is not queued.
    ///
    /// # Errors
    /// Returns database errors.
    pub fn queued_deferrals(&self, path: &str) -> Result<Option<i64>> {
        Ok(self
            .connection()?
            .query_row(
                "SELECT deferrals FROM batch_dream_queue WHERE path = ?1",
                rusqlite::params![path],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub(crate) fn connection(&self) -> Result<rusqlite::Connection> {
        let path = self.scope_dir.join("memory_state.sqlite");
        Ok(JournalMode::for_db_path(&path).open(&path)?)
    }

    /// Claim the oldest pending notes that fit the request's byte budget;
    /// deferred notes queue behind new ones so a repeatedly deferring note never starves them.
    ///
    /// # Errors
    /// `Busy` while another consolidation holds the lease, `Conflict` while a
    /// default-runner plan awaits recovery, `Invalid` for a malformed request, or storage errors.
    pub fn claim(&self, request: &BatchClaimRequest<'_>) -> Result<Option<BatchLease>> {
        self.control.check()?;
        let expires_at = validate_claim_request(request)?;
        self.recover(request.now)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (generation, lock_expiry) = transaction.query_row(
            "SELECT generation, expires_at FROM consolidation_lock WHERE singleton = 1",
            [],
            |row| Ok((row.get::<_, u64>(0)?, row.get::<_, Option<i64>>(1)?)),
        )?;
        if lock_expiry.is_some_and(|expiry| expiry > request.now) {
            return Err(BatchDreamError::Busy);
        }
        let has_default_plan: bool = transaction.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM consolidation_operations
                WHERE plan_hash IS NOT NULL
                  AND status IN ('planned','topics_written','archived','failed')
             )",
            [],
            |row| row.get(0),
        )?;
        if has_default_plan {
            return Err(BatchDreamError::Conflict(
                "a default Dream plan must be recovered by the default runner".to_owned(),
            ));
        }
        self.sync_queue(&transaction)?;
        let notes = self.select_notes(&transaction, request)?;
        if notes.is_empty() {
            transaction.commit()?;
            return Ok(None);
        }
        let generation = generation
            .checked_add(1)
            .ok_or_else(|| BatchDreamError::Invalid("lease generation overflows".to_owned()))?;
        let lease = BatchLease {
            operation_id: format!("batch_dream_{generation}"),
            owner: request.owner.to_owned(),
            generation,
            expires_at,
            notes,
        };
        transaction.execute(
            "UPDATE consolidation_lock SET owner = ?1, generation = ?2, expires_at = ?3
             WHERE singleton = 1",
            params![lease.owner, generation, expires_at],
        )?;
        transaction.execute(
            "INSERT INTO consolidation_operations(operation_id, status, owner, generation, created_at)
             VALUES (?1, 'claimed', ?2, ?3, ?4)",
            params![lease.operation_id, lease.owner, generation, request.now],
        )?;
        transaction.execute(
            "INSERT INTO batch_dream_batches(operation_id, owner, generation, expires_at, status, created_at)
             VALUES (?1, ?2, ?3, ?4, 'claimed', ?5)",
            params![lease.operation_id, lease.owner, generation, expires_at, request.now],
        )?;
        for note in &lease.notes {
            transaction.execute(
                "INSERT INTO consolidation_claim_items(operation_id, source_path, content_hash)
                 VALUES (?1, ?2, ?3)",
                params![lease.operation_id, note.path, note.content_hash],
            )?;
            transaction.execute(
                "INSERT INTO batch_dream_batch_notes(operation_id, path, content_hash)
                 VALUES (?1, ?2, ?3)",
                params![lease.operation_id, note.path, note.content_hash],
            )?;
            transaction.execute(
                "UPDATE batch_dream_queue SET status = 'claimed', attempts = attempts + 1
                 WHERE path = ?1",
                params![note.path],
            )?;
        }
        transaction.commit()?;
        Ok(Some(lease))
    }

    /// Record every eligible inbox note in the queue, oldest first.
    fn sync_queue(&self, transaction: &rusqlite::Transaction<'_>) -> Result<()> {
        let has_capture_files: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master
             WHERE type = 'table' AND name = 'capture_observation_files')",
            [],
            |row| row.get(0),
        )?;
        let inbox = self.scope_dir.join(INBOX_DIR);
        for (index, entry) in std::fs::read_dir(&inbox)?.enumerate() {
            self.control.check()?;
            if index >= MAX_INBOX_ENTRIES {
                return Err(BatchDreamError::Invalid(format!(
                    "inbox holds more than {MAX_INBOX_ENTRIES} entries"
                )));
            }
            let entry = entry?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let relative = format!("{INBOX_DIR}/{name}");
            if !entry.file_type()?.is_file()
                || validate_note_path(&relative).is_err()
                || is_excluded_note(transaction, &relative)?
            {
                continue;
            }
            let capture = if has_capture_files {
                transaction
                    .query_row(
                        "SELECT f.content_hash, o.committed_at
                         FROM capture_observation_files f JOIN capture_outcomes o USING(job_id)
                         WHERE REPLACE(f.path, char(92), '/') = ?1",
                        params![relative],
                        |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                    )
                    .optional()?
            } else {
                None
            };
            // Capture publishes the file before its outcome row commits.
            if capture.is_none() && crate::v2_consolidation::is_capture_note_name(&name) {
                continue;
            }
            let bytes = match read_bounded(&entry.path(), MAX_NOTE_BYTES as u64) {
                Ok(bytes) => bytes,
                Err(BatchDreamError::Invalid(reason)) => {
                    mark_invalid(transaction, &relative, &reason)?;
                    continue;
                }
                Err(error) => return Err(error),
            };
            let content_hash = hash_bytes(&bytes);
            let created_at = match &capture {
                Some((expected, _)) if *expected != content_hash => continue,
                Some((_, committed_at)) => *committed_at,
                None => modified_unix_seconds(&entry.metadata()?),
            };
            transaction.execute(
                "INSERT INTO batch_dream_queue(path, content_hash, created_at, queue_order, status)
                 VALUES (?1, ?2, ?3, 0, 'pending')
                 ON CONFLICT(path) DO UPDATE SET
                    status = CASE
                        WHEN batch_dream_queue.status = 'claimed' THEN 'claimed'
                        WHEN batch_dream_queue.content_hash != excluded.content_hash THEN 'pending'
                        ELSE batch_dream_queue.status END,
                    content_hash = CASE
                        WHEN batch_dream_queue.status = 'claimed' THEN batch_dream_queue.content_hash
                        ELSE excluded.content_hash END",
                params![relative, content_hash, created_at],
            )?;
        }
        Ok(())
    }

    fn select_notes(
        &self,
        transaction: &rusqlite::Transaction<'_>,
        request: &BatchClaimRequest<'_>,
    ) -> Result<Vec<ClaimedNote>> {
        transaction.execute_batch(
            "CREATE TEMP TABLE IF NOT EXISTS batch_dream_skip(path TEXT PRIMARY KEY);
             DELETE FROM batch_dream_skip;",
        )?;
        for path in request.excluded {
            transaction.execute(
                "INSERT OR IGNORE INTO batch_dream_skip(path) VALUES (?1)",
                params![path],
            )?;
        }
        let budget = request.max_note_bytes.min(MAX_BATCH_NOTE_BYTES);
        let mut used = 0usize;
        let mut notes = Vec::new();
        while notes.len() < MAX_BATCH_NOTES {
            self.control.check()?;
            let Some((path, content_hash, created_at)) = transaction
                .query_row(
                    "SELECT path, content_hash, created_at FROM batch_dream_queue q
                     WHERE status = 'pending'
                       AND NOT EXISTS(SELECT 1 FROM batch_dream_skip s WHERE s.path = q.path)
                     ORDER BY queue_order, created_at, path LIMIT 1",
                    [],
                    |row| Ok((row.get::<_, String>(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()?
            else {
                break;
            };
            transaction.execute(
                "INSERT INTO batch_dream_skip(path) VALUES (?1)",
                params![path],
            )?;
            if is_excluded_note(transaction, &path)? {
                transaction.execute(
                    "DELETE FROM batch_dream_queue WHERE path = ?1",
                    params![path],
                )?;
                continue;
            }
            let absolute = self.scope_dir.join(&path);
            let bytes = match read_bounded(&absolute, MAX_NOTE_BYTES as u64) {
                Ok(bytes) => bytes,
                Err(BatchDreamError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                    transaction.execute(
                        "DELETE FROM batch_dream_queue WHERE path = ?1",
                        params![path],
                    )?;
                    continue;
                }
                Err(BatchDreamError::Invalid(reason)) => {
                    mark_invalid(transaction, &path, &reason)?;
                    continue;
                }
                Err(error) => return Err(error),
            };
            let Ok(content) = String::from_utf8(bytes) else {
                mark_invalid(transaction, &path, "note is not UTF-8")?;
                continue;
            };
            if hash_bytes(content.as_bytes()) != content_hash {
                continue;
            }
            if !notes.is_empty() && used.saturating_add(content.len()) > budget {
                break;
            }
            used = used.saturating_add(content.len());
            notes.push(ClaimedNote {
                path,
                content_hash,
                created_at,
                content,
            });
        }
        notes.sort_by(|left, right| {
            (left.created_at, &left.path).cmp(&(right.created_at, &right.path))
        });
        Ok(notes)
    }

    pub(crate) fn validate_lease(
        &self,
        connection: &rusqlite::Connection,
        lease: &BatchLease,
        now: i64,
    ) -> Result<()> {
        let is_valid: bool = connection.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM consolidation_lock l
                JOIN batch_dream_batches b ON b.operation_id = ?1
                WHERE l.singleton = 1 AND l.owner = ?2 AND l.generation = ?3
                  AND l.expires_at > ?4 AND b.generation = l.generation
                  AND b.status = 'claimed'
             )",
            params![lease.operation_id, lease.owner, lease.generation, now],
            |row| row.get(0),
        )?;
        if is_valid {
            Ok(())
        } else {
            Err(BatchDreamError::StaleLease)
        }
    }

    /// Resolve a direct topic path, rejecting protected, excluded, or linked files.
    pub(crate) fn validate_topic_path(
        &self,
        connection: &rusqlite::Connection,
        relative: &str,
    ) -> Result<PathBuf> {
        let path = Path::new(relative);
        if relative.len() > MAX_PATH_BYTES
            || path.parent() != Some(Path::new(TOPICS_DIR))
            || path.extension().and_then(|value| value.to_str()) != Some("md")
            || path
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
            || path
                .file_name()
                .and_then(|name| name.to_str())
                .is_none_or(|name| name.starts_with('.'))
        {
            return Err(BatchDreamError::Invalid(format!(
                "not a direct topic path: {relative}"
            )));
        }
        let absolute = self.scope_dir.join(path);
        if self.access.classify_path(&absolute)? != V2PathClass::Topic(self.scope) {
            return Err(BatchDreamError::Invalid(format!(
                "path is outside this scope's topics: {relative}"
            )));
        }
        match std::fs::symlink_metadata(&absolute) {
            Ok(metadata) if !metadata.is_file() => {
                return Err(BatchDreamError::Invalid(format!(
                    "topic is not a regular file: {relative}"
                )));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(BatchDreamError::Io(error)),
        }
        let is_excluded: bool = connection.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM memory_v2_tombstones WHERE REPLACE(relative_path, char(92), '/') = ?1
                UNION ALL
                SELECT 1 FROM memory_v2_quarantined_paths
                WHERE REPLACE(relative_path, char(92), '/') = ?1
             )",
            params![relative],
            |row| row.get(0),
        )?;
        if is_excluded {
            return Err(BatchDreamError::Conflict(format!(
                "topic path is durably excluded: {relative}"
            )));
        }
        Ok(absolute)
    }
}

fn validate_claim_request(request: &BatchClaimRequest<'_>) -> Result<i64> {
    if request.owner.is_empty()
        || request.owner.len() > MAX_OWNER_BYTES
        || request.owner.chars().any(char::is_control)
        || request.now < 0
        || request.max_note_bytes == 0
    {
        return Err(BatchDreamError::Invalid(
            "claim needs an owner, a timestamp, and a note budget".to_owned(),
        ));
    }
    if request.excluded.len() > MAX_EXCLUDED_NOTES
        || request
            .excluded
            .iter()
            .any(|path| path.len() > MAX_PATH_BYTES)
    {
        return Err(BatchDreamError::Invalid(format!(
            "claim may exclude at most {MAX_EXCLUDED_NOTES} note paths"
        )));
    }
    let duration = i64::try_from(request.duration.as_secs())
        .ok()
        .filter(|duration| *duration > 0)
        .ok_or_else(|| BatchDreamError::Invalid("invalid lease duration".to_owned()))?;
    request
        .now
        .checked_add(duration)
        .ok_or_else(|| BatchDreamError::Invalid("lease expiry overflows".to_owned()))
}

pub(crate) fn validate_note_path(relative: &str) -> Result<()> {
    let path = Path::new(relative);
    if relative.len() > MAX_PATH_BYTES
        || path.parent() != Some(Path::new(INBOX_DIR))
        || path.extension().and_then(|value| value.to_str()) != Some("md")
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(BatchDreamError::Invalid(format!(
            "not a direct inbox note path: {relative}"
        )));
    }
    Ok(())
}

fn is_excluded_note(connection: &rusqlite::Connection, relative: &str) -> Result<bool> {
    Ok(connection.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM consolidation_archives WHERE REPLACE(source_path, char(92), '/') = ?1
            UNION ALL
            SELECT 1 FROM memory_v2_tombstones WHERE REPLACE(relative_path, char(92), '/') = ?1
            UNION ALL
            SELECT 1 FROM memory_v2_hidden_observations
            WHERE REPLACE(relative_path, char(92), '/') = ?1
            UNION ALL
            SELECT 1 FROM memory_v2_quarantined_paths
            WHERE REPLACE(relative_path, char(92), '/') = ?1
         )",
        params![relative],
        |row| row.get(0),
    )?)
}

fn mark_invalid(connection: &rusqlite::Connection, relative: &str, reason: &str) -> Result<()> {
    tracing::warn!(target: crate::MEMORY_LOG_TARGET, path = %relative, %reason, "batch Dream skipped a note");
    connection.execute(
        "INSERT INTO batch_dream_queue(path, content_hash, created_at, queue_order, status, reason)
         VALUES (?1, '', 0, 0, 'invalid', ?2)
         ON CONFLICT(path) DO UPDATE SET status = 'invalid', reason = excluded.reason",
        params![relative, reason],
    )?;
    Ok(())
}

fn modified_unix_seconds(metadata: &std::fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|elapsed| i64::try_from(elapsed.as_secs()).ok())
        .unwrap_or(0)
}

#[cfg(test)]
#[path = "batch_dream_tests.rs"]
mod tests;
