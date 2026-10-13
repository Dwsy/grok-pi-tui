//! Apply publishes a stage only while the target still holds its base bytes, so
//! replay after a crash is idempotent and never clobbers an intervening edit.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use rusqlite::{OptionalExtension as _, TransactionBehavior, params};

use crate::batch_dream::{
    BatchCommit, BatchDreamError, BatchDreamStore, BatchLease, BatchReport,
    CAPACITY_DEFERRAL_REASON, FileChange, MAX_CHANGES, MAX_COMMIT_TEXT_BYTES, MAX_DEPENDENCIES,
    MAX_NOTE_BYTES, MAX_NOTE_DEFERRALS, MAX_REASON_BYTES, MAX_SPLICES_PER_FILE, NoteDisposition,
    Result, STAGE_PREFIX, TOPICS_DIR,
};
use crate::batch_dream_io::{hash_file, publish_copy, read_bounded, sync_directory, write_spliced};

/// Kept deferral reasons per note; older reasons fall off the front.
const MAX_REASONS_BYTES: i64 = 4 * MAX_REASON_BYTES as i64;
use crate::{MemoryIndex, MemoryStorage, V2ManifestBudget};

struct SettledNote {
    path: String,
    content_hash: String,
    disposition: String,
    reason: Option<String>,
}

struct PlannedChange {
    path: String,
    base_hash: Option<String>,
    post_hash: String,
    stage_path: PathBuf,
}

impl BatchDreamStore {
    /// Check, persist, and apply one batch commit; nothing is published unless
    /// the whole commit passes its checks.
    ///
    /// # Errors
    /// `Invalid`, `Conflict`, `StaleLease`, `Interrupted` before the plan is
    /// persisted, or storage errors. A conflict after persistence returns the
    /// notes to the queue when nothing was published, and otherwise blocks the
    /// batch until `retry_blocked`.
    pub fn commit(
        &self,
        lease: &BatchLease,
        commit: &BatchCommit,
        now: i64,
    ) -> Result<BatchReport> {
        self.persist_plan_as(lease, commit, now, "deferred")?;
        self.apply_or_block(&lease.operation_id, now)
    }

    /// Return every claimed note to the queue without changing topics.
    ///
    /// # Errors
    /// Returns `StaleLease` if the lease was lost, or storage errors.
    pub fn release(&self, lease: &BatchLease, reason: &str, now: i64) -> Result<BatchReport> {
        let commit = BatchCommit {
            outcomes: lease
                .notes
                .iter()
                .map(|note| crate::batch_dream::NoteOutcome {
                    path: note.path.clone(),
                    disposition: NoteDisposition::Deferred {
                        reason: reason.to_owned(),
                    },
                })
                .collect(),
            ..BatchCommit::default()
        };
        // A release is not the model's decision, so it does not count toward giving up.
        self.persist_plan_as(lease, &commit, now, "released")?;
        self.apply_or_block(&lease.operation_id, now)
    }

    pub(crate) fn persist_plan_as(
        &self,
        lease: &BatchLease,
        commit: &BatchCommit,
        now: i64,
        requeue_disposition: &str,
    ) -> Result<()> {
        self.control.check()?;
        let connection = self.connection()?;
        self.validate_lease(&connection, lease, now)?;
        validate_outcomes(lease, commit)?;
        validate_budgets(commit)?;
        let mut stages = Vec::with_capacity(commit.changes.len());
        let mut planned = Vec::with_capacity(commit.changes.len());
        let mut changed_bases = BTreeMap::new();
        for change in &commit.changes {
            let target = self.validate_topic_path(&connection, change.path())?;
            let mut stage = tempfile::Builder::new()
                .prefix(STAGE_PREFIX)
                .tempfile_in(self.scope_dir.join(TOPICS_DIR))?;
            let base_hash = match change {
                FileChange::Splice {
                    path,
                    base_hash,
                    splices,
                } => {
                    if hash_file(&target, &self.control)? != *base_hash {
                        return Err(BatchDreamError::Conflict(format!("{path} changed")));
                    }
                    write_spliced(&target, stage.as_file_mut(), splices, &self.control)?;
                    if hash_file(&target, &self.control)? != *base_hash {
                        return Err(BatchDreamError::Conflict(format!(
                            "{path} changed while staging"
                        )));
                    }
                    Some(base_hash.clone())
                }
                FileChange::Create { path, content } => {
                    if target.try_exists()? {
                        return Err(BatchDreamError::Conflict(format!("{path} already exists")));
                    }
                    stage.write_all(content.as_bytes())?;
                    stage.as_file().sync_all()?;
                    None
                }
            };
            let post_hash = hash_file(stage.path(), &self.control)?;
            if base_hash.as_ref() == Some(&post_hash) {
                return Err(BatchDreamError::Invalid(format!(
                    "change to {} leaves the file unchanged",
                    change.path()
                )));
            }
            changed_bases.insert(change.path().to_owned(), base_hash.clone());
            planned.push(PlannedChange {
                path: change.path().to_owned(),
                base_hash,
                post_hash,
                stage_path: stage.path().to_path_buf(),
            });
            stages.push(stage);
        }
        for dependency in &commit.dependencies {
            let target = self.validate_topic_path(&connection, &dependency.path)?;
            let expected = match changed_bases.get(&dependency.path) {
                Some(base) => base.as_ref(),
                None => Some(&dependency.content_hash),
            };
            if expected != Some(&dependency.content_hash)
                || !target.try_exists()?
                || hash_file(&target, &self.control)? != dependency.content_hash
            {
                return Err(BatchDreamError::Conflict(format!(
                    "{} changed after it was read",
                    dependency.path
                )));
            }
        }
        sync_directory(&self.scope_dir.join(TOPICS_DIR))?;
        self.control.check()?;
        drop(connection);

        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        self.validate_lease(&transaction, lease, now)?;
        for change in &planned {
            transaction.execute(
                "INSERT INTO batch_dream_changes(operation_id, path, base_hash, post_hash, stage_path)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    lease.operation_id,
                    change.path,
                    change.base_hash,
                    change.post_hash,
                    change.stage_path.to_string_lossy()
                ],
            )?;
        }
        for dependency in &commit.dependencies {
            transaction.execute(
                "INSERT OR REPLACE INTO batch_dream_dependencies(operation_id, path, content_hash)
                 VALUES (?1, ?2, ?3)",
                params![lease.operation_id, dependency.path, dependency.content_hash],
            )?;
        }
        for outcome in &commit.outcomes {
            let (disposition, reason) = match &outcome.disposition {
                NoteDisposition::Applied => ("applied", None),
                NoteDisposition::NoChange => ("no_change", None),
                NoteDisposition::Deferred { reason }
                    if commit.capacity_deferrals_uncounted
                        && reason.trim().eq_ignore_ascii_case(CAPACITY_DEFERRAL_REASON) =>
                {
                    ("released", Some(reason.as_str()))
                }
                NoteDisposition::Deferred { reason } => {
                    (requeue_disposition, Some(reason.as_str()))
                }
            };
            transaction.execute(
                "UPDATE batch_dream_batch_notes SET disposition = ?3, reason = ?4
                 WHERE operation_id = ?1 AND path = ?2",
                params![lease.operation_id, outcome.path, disposition, reason],
            )?;
        }
        transaction.execute(
            "UPDATE batch_dream_batches SET status = 'planned' WHERE operation_id = ?1",
            params![lease.operation_id],
        )?;
        // The default runner only adopts plans with a plan hash, so it never replays this one.
        transaction.execute(
            "UPDATE consolidation_operations SET status = 'topics_written' WHERE operation_id = ?1",
            params![lease.operation_id],
        )?;
        transaction.commit()?;
        for stage in stages {
            stage
                .keep()
                .map_err(|error| BatchDreamError::Io(error.error))?;
        }
        Ok(())
    }

    /// Publish a persisted plan, archive its settled notes, and release the lease.
    pub(crate) fn apply_planned(&self, operation_id: &str, now: i64) -> Result<BatchReport> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (owner, generation): (String, u64) = transaction
            .query_row(
                "SELECT b.owner, b.generation FROM batch_dream_batches b
                 JOIN consolidation_lock l ON l.singleton = 1
                 WHERE b.operation_id = ?1 AND b.status = 'planned'
                   AND l.owner = b.owner AND l.generation = b.generation AND l.expires_at > ?2",
                params![operation_id, now],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .ok_or(BatchDreamError::StaleLease)?;
        let changes = read_changes(&transaction, operation_id)?;
        let posts: BTreeMap<&str, &str> = changes
            .iter()
            .map(|change| (change.path.as_str(), change.post_hash.as_str()))
            .collect();
        let dependencies = read_pairs(
            &transaction,
            "SELECT path, content_hash FROM batch_dream_dependencies WHERE operation_id = ?1",
            operation_id,
        )?;
        let unchecked = crate::batch_dream_control::BatchDreamControl::default();
        for (path, expected) in &dependencies {
            let target = self.validate_topic_path(&transaction, path)?;
            let actual = hash_file(&target, &unchecked)?;
            if actual != *expected && posts.get(path.as_str()) != Some(&actual.as_str()) {
                return Err(BatchDreamError::Conflict(format!("{path} changed")));
            }
        }
        let mut pending = Vec::new();
        for change in &changes {
            let target = self.validate_topic_path(&transaction, &change.path)?;
            let actual = if target.try_exists()? {
                Some(hash_file(&target, &unchecked)?)
            } else {
                None
            };
            if actual.as_ref() == Some(&change.post_hash) {
                continue;
            }
            if actual != change.base_hash {
                return Err(BatchDreamError::Conflict(format!(
                    "{} changed",
                    change.path
                )));
            }
            if change.stage_path.parent() != Some(self.scope_dir.join(TOPICS_DIR).as_path())
                || std::fs::symlink_metadata(&change.stage_path)?
                    .file_type()
                    .is_symlink()
                || hash_file(&change.stage_path, &unchecked)? != change.post_hash
            {
                return Err(BatchDreamError::Conflict(
                    "staged bytes changed before publication".to_owned(),
                ));
            }
            pending.push((change, target));
        }
        for (change, target) in pending {
            if change.base_hash.is_none() {
                // A hard link fails instead of replacing a topic created meanwhile.
                std::fs::hard_link(&change.stage_path, &target)?;
                sync_directory(&self.scope_dir.join(TOPICS_DIR))?;
            } else {
                publish_copy(&change.stage_path, &target)?;
            }
        }

        let notes = read_notes(&transaction, operation_id)?;
        let archive_dir = self.scope_dir.join("archive").join(operation_id);
        let mut report = BatchReport {
            operation_id: operation_id.to_owned(),
            changed_topics: changes.iter().map(|change| change.path.clone()).collect(),
            ..BatchReport::default()
        };
        for SettledNote {
            path,
            content_hash,
            disposition,
            reason,
        } in &notes
        {
            match disposition.as_str() {
                "released" => {
                    requeue(&transaction, path, reason.as_deref(), false)?;
                    report.deferred.push(path.clone());
                    continue;
                }
                "deferred" => {
                    let deferrals = requeue(&transaction, path, reason.as_deref(), true)?;
                    if deferrals < MAX_NOTE_DEFERRALS {
                        report.deferred.push(path.clone());
                        continue;
                    }
                    let reasons: Option<String> = transaction.query_row(
                        "SELECT reason FROM batch_dream_queue WHERE path = ?1",
                        params![path],
                        |row| row.get(0),
                    )?;
                    tracing::warn!(
                        target: crate::MEMORY_LOG_TARGET,
                        path = %path,
                        deferrals,
                        reasons = %reasons.unwrap_or_default(),
                        "batch Dream archived a note it could not place"
                    );
                    transaction.execute(
                        "UPDATE batch_dream_batch_notes SET disposition = 'unplaceable'
                         WHERE operation_id = ?1 AND path = ?2",
                        params![operation_id, path],
                    )?;
                    report.unplaceable.push(path.clone());
                }
                "applied" => report.applied.push(path.clone()),
                "no_change" => report.no_change.push(path.clone()),
                other => {
                    return Err(BatchDreamError::Invalid(format!(
                        "note {path} has no recorded outcome ({other})"
                    )));
                }
            }
            let file_name = Path::new(path)
                .file_name()
                .ok_or_else(|| BatchDreamError::Invalid(format!("note {path} has no name")))?;
            let archived_relative =
                format!("archive/{operation_id}/{}", file_name.to_string_lossy());
            std::fs::create_dir_all(&archive_dir)?;
            copy_note_idempotently(
                &self.scope_dir.join(path),
                &self.scope_dir.join(&archived_relative),
                content_hash,
            )?;
            transaction.execute(
                "INSERT INTO consolidation_archives(source_path, operation_id, archive_path, content_hash)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(source_path) DO UPDATE SET operation_id = excluded.operation_id,
                    archive_path = excluded.archive_path, content_hash = excluded.content_hash",
                params![path, operation_id, archived_relative, content_hash],
            )?;
            transaction.execute(
                "INSERT INTO v2_archive_retention(source_path, archived_at) VALUES (?1, ?2)
                 ON CONFLICT(source_path) DO UPDATE SET archived_at = excluded.archived_at",
                params![path, now],
            )?;
            transaction.execute(
                "DELETE FROM batch_dream_queue WHERE path = ?1",
                params![path],
            )?;
        }
        if archive_dir.try_exists()? {
            sync_directory(&archive_dir)?;
        }
        transaction.execute(
            "UPDATE batch_dream_batches SET status = 'completed', completed_at = ?2,
                cleanup_pending = 1 WHERE operation_id = ?1",
            params![operation_id, now],
        )?;
        transaction.execute(
            "UPDATE consolidation_operations SET status = 'completed', completed_at = ?2
             WHERE operation_id = ?1",
            params![operation_id, now],
        )?;
        transaction.execute(
            "UPDATE consolidation_lock SET owner = NULL, expires_at = NULL
             WHERE singleton = 1 AND owner = ?1 AND generation = ?2",
            params![owner, generation],
        )?;
        crate::v2::bump_manifest_revision_in_transaction(&self.scope_dir, &transaction)?;
        transaction.commit()?;
        self.cleanup_completed(operation_id)?;
        Ok(report)
    }

    /// Turn a planned batch none of whose targets holds its post bytes into a
    /// release, so applying it returns every note to the queue uncounted.
    /// `false` if any target does.
    pub(crate) fn release_unpublished(&self, operation_id: &str, reason: &str) -> Result<bool> {
        let unchecked = crate::batch_dream_control::BatchDreamControl::default();
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changes = read_changes(&transaction, operation_id)?;
        for change in &changes {
            let target = self.scope_dir.join(&change.path);
            if target.try_exists()? && hash_file(&target, &unchecked)? == change.post_hash {
                return Ok(false);
            }
        }
        let released = transaction.execute(
            "UPDATE batch_dream_batch_notes SET disposition = 'released', reason = ?2
             WHERE operation_id = ?1
               AND EXISTS(SELECT 1 FROM batch_dream_batches
                          WHERE operation_id = ?1 AND status = 'planned')",
            params![operation_id, reason],
        )?;
        if released == 0 {
            return Ok(false);
        }
        transaction.execute(
            "DELETE FROM batch_dream_changes WHERE operation_id = ?1",
            params![operation_id],
        )?;
        transaction.execute(
            "DELETE FROM batch_dream_dependencies WHERE operation_id = ?1",
            params![operation_id],
        )?;
        transaction.commit()?;
        Ok(true)
    }

    /// Idempotent, so recovery can repeat it after a crash past the SQL commit.
    pub(crate) fn cleanup_completed(&self, operation_id: &str) -> Result<()> {
        let connection = self.connection()?;
        let archived = read_pairs(
            &connection,
            "SELECT n.path, n.content_hash FROM batch_dream_batch_notes n
             WHERE n.operation_id = ?1 AND n.disposition IN ('applied','no_change','unplaceable')",
            operation_id,
        )?;
        for (path, content_hash) in &archived {
            let source = self.scope_dir.join(path);
            match read_bounded(&source, MAX_NOTE_BYTES as u64) {
                Ok(bytes) if crate::batch_dream_io::hash_bytes(&bytes) == *content_hash => {
                    std::fs::remove_file(&source)?;
                }
                Ok(_) | Err(BatchDreamError::Invalid(_)) => {
                    tracing::warn!(
                        target: crate::MEMORY_LOG_TARGET,
                        %path,
                        "batch Dream kept an archived note that changed before cleanup"
                    );
                }
                Err(BatchDreamError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                }
                Err(error) => return Err(error),
            }
        }
        sync_directory(&self.scope_dir.join(crate::batch_dream::INBOX_DIR))?;
        let changes = read_changes(&connection, operation_id)?;
        for change in &changes {
            if change.stage_path.parent() != Some(self.scope_dir.join(TOPICS_DIR).as_path()) {
                return Err(BatchDreamError::Invalid("unsafe stage path".to_owned()));
            }
            match std::fs::remove_file(&change.stage_path) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                    return Err(BatchDreamError::Io(error));
                }
                _ => {}
            }
        }
        sync_directory(&self.scope_dir.join(TOPICS_DIR))?;
        self.converge_index(&changes, &archived)?;
        crate::v2::regenerate_scope_manifest(
            &self.scope_dir,
            self.scope,
            V2ManifestBudget::configured(),
        )?;
        connection.execute(
            "UPDATE batch_dream_batches SET cleanup_pending = 0 WHERE operation_id = ?1",
            params![operation_id],
        )?;
        Ok(())
    }

    fn converge_index(
        &self,
        changes: &[PlannedChange],
        archived: &[(String, String)],
    ) -> Result<()> {
        let index_error = |error: rusqlite::Error| BatchDreamError::Database(error);
        let storage = MemoryStorage::new_flat(&self.scope_dir, &self.scope_dir);
        let mut index = MemoryIndex::open_or_create(
            &self.scope_dir.join("index.sqlite"),
            storage,
            xai_grok_config_types::MemoryIndexConfig::default(),
            1,
        )
        .map_err(index_error)?;
        let source = match self.scope {
            crate::V2MemoryScope::Global => "global",
            crate::V2MemoryScope::Workspace => "workspace",
        };
        for change in changes {
            index
                .reindex_file(&self.scope_dir.join(&change.path), source)
                .map_err(index_error)?;
        }
        for (path, _) in archived {
            index
                .delete_path(&self.scope_dir.join(path))
                .map_err(index_error)?;
        }
        Ok(())
    }
}

fn validate_outcomes(lease: &BatchLease, commit: &BatchCommit) -> Result<()> {
    let claimed: BTreeSet<&str> = lease.notes.iter().map(|note| note.path.as_str()).collect();
    let mut settled = BTreeSet::new();
    let mut has_applied = false;
    for outcome in &commit.outcomes {
        if !claimed.contains(outcome.path.as_str()) || !settled.insert(outcome.path.as_str()) {
            return Err(BatchDreamError::Invalid(format!(
                "outcome for {} is unclaimed or repeated",
                outcome.path
            )));
        }
        match &outcome.disposition {
            NoteDisposition::Applied => has_applied = true,
            NoteDisposition::NoChange => {}
            NoteDisposition::Deferred { reason } => {
                if reason.trim().is_empty() || reason.len() > MAX_REASON_BYTES {
                    return Err(BatchDreamError::Invalid(format!(
                        "deferral reason must be 1..={MAX_REASON_BYTES} bytes"
                    )));
                }
            }
        }
    }
    if settled.len() != claimed.len() {
        return Err(BatchDreamError::Invalid(
            "every claimed note needs exactly one outcome".to_owned(),
        ));
    }
    if has_applied == commit.changes.is_empty() {
        return Err(BatchDreamError::Invalid(
            "topic changes require an applied note, and an applied note requires a change"
                .to_owned(),
        ));
    }
    Ok(())
}

fn validate_budgets(commit: &BatchCommit) -> Result<()> {
    if commit.changes.len() > MAX_CHANGES || commit.dependencies.len() > MAX_DEPENDENCIES {
        return Err(BatchDreamError::Invalid(format!(
            "commit allows {MAX_CHANGES} changes and {MAX_DEPENDENCIES} dependencies"
        )));
    }
    let mut paths = BTreeSet::new();
    let mut text_bytes = 0usize;
    for change in &commit.changes {
        if !paths.insert(change.path()) {
            return Err(BatchDreamError::Invalid(format!(
                "{} is changed more than once",
                change.path()
            )));
        }
        match change {
            FileChange::Splice { splices, .. } => {
                if splices.is_empty() || splices.len() > MAX_SPLICES_PER_FILE {
                    return Err(BatchDreamError::Invalid(format!(
                        "a file change needs 1..={MAX_SPLICES_PER_FILE} splices"
                    )));
                }
                let mut previous: Option<(u64, u64)> = None;
                for splice in splices {
                    let overlaps = previous.is_some_and(|(start, end)| {
                        splice.start < end || (splice.start == start && start == end)
                    });
                    if splice.end < splice.start || overlaps {
                        return Err(BatchDreamError::Invalid(format!(
                            "splices in {} overlap or are unsorted",
                            change.path()
                        )));
                    }
                    previous = Some((splice.start, splice.end));
                    text_bytes = text_bytes.saturating_add(splice.text.len());
                }
            }
            FileChange::Create { content, .. } => {
                if content.trim().is_empty() {
                    return Err(BatchDreamError::Invalid(format!(
                        "{} would be created empty",
                        change.path()
                    )));
                }
                text_bytes = text_bytes.saturating_add(content.len());
            }
        }
    }
    if text_bytes > MAX_COMMIT_TEXT_BYTES {
        return Err(BatchDreamError::Invalid(format!(
            "commit text exceeds {MAX_COMMIT_TEXT_BYTES} bytes"
        )));
    }
    Ok(())
}

fn read_changes(
    connection: &rusqlite::Connection,
    operation_id: &str,
) -> Result<Vec<PlannedChange>> {
    let mut statement = connection.prepare(
        "SELECT path, base_hash, post_hash, stage_path FROM batch_dream_changes
         WHERE operation_id = ?1 ORDER BY path",
    )?;
    let changes = statement
        .query_map(params![operation_id], |row| {
            Ok(PlannedChange {
                path: row.get(0)?,
                base_hash: row.get(1)?,
                post_hash: row.get(2)?,
                stage_path: PathBuf::from(row.get::<_, String>(3)?),
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(changes)
}

fn read_pairs(
    connection: &rusqlite::Connection,
    sql: &str,
    operation_id: &str,
) -> Result<Vec<(String, String)>> {
    let mut statement = connection.prepare(sql)?;
    let pairs = statement
        .query_map(params![operation_id], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(pairs)
}

/// Return a note to the back of the queue; a model deferral also counts toward
/// [`MAX_NOTE_DEFERRALS`] and keeps its reasons, newest last.
fn requeue(
    connection: &rusqlite::Connection,
    path: &str,
    reason: Option<&str>,
    counts: bool,
) -> Result<i64> {
    let reason = reason.unwrap_or("");
    if counts {
        connection.execute(
            "UPDATE batch_dream_queue SET status = 'pending', deferrals = deferrals + 1,
                reason = substr(COALESCE(reason || char(10), '') || ?2, -?3, ?3),
                queue_order = (SELECT MAX(queue_order) + 1 FROM batch_dream_queue)
             WHERE path = ?1",
            params![path, reason, MAX_REASONS_BYTES],
        )?;
    } else {
        connection.execute(
            "UPDATE batch_dream_queue SET status = 'pending',
                reason = CASE WHEN reason IS NULL OR reason = '' THEN ?2 ELSE reason END,
                queue_order = (SELECT MAX(queue_order) + 1 FROM batch_dream_queue)
             WHERE path = ?1",
            params![path, reason],
        )?;
    }
    Ok(connection.query_row(
        "SELECT deferrals FROM batch_dream_queue WHERE path = ?1",
        params![path],
        |row| row.get(0),
    )?)
}

fn read_notes(connection: &rusqlite::Connection, operation_id: &str) -> Result<Vec<SettledNote>> {
    let mut statement = connection.prepare(
        "SELECT path, content_hash, COALESCE(disposition, ''), reason
         FROM batch_dream_batch_notes WHERE operation_id = ?1 ORDER BY path",
    )?;
    let notes = statement
        .query_map(params![operation_id], |row| {
            Ok(SettledNote {
                path: row.get(0)?,
                content_hash: row.get(1)?,
                disposition: row.get(2)?,
                reason: row.get(3)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(notes)
}

fn copy_note_idempotently(source: &Path, destination: &Path, expected_hash: &str) -> Result<()> {
    if destination.try_exists()? {
        let archived = read_bounded(destination, MAX_NOTE_BYTES as u64)?;
        if crate::batch_dream_io::hash_bytes(&archived) != expected_hash {
            return Err(BatchDreamError::Conflict(format!(
                "archive destination differs: {}",
                destination.display()
            )));
        }
        return Ok(());
    }
    let bytes = read_bounded(source, MAX_NOTE_BYTES as u64)?;
    if crate::batch_dream_io::hash_bytes(&bytes) != expected_hash {
        return Err(BatchDreamError::Conflict(format!(
            "claimed note changed: {}",
            source.display()
        )));
    }
    let parent = destination
        .parent()
        .ok_or_else(|| BatchDreamError::Invalid("archive path has no parent".to_owned()))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(&bytes)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist_noclobber(destination)
        .map_err(|error| BatchDreamError::Io(error.error))?;
    Ok(())
}
