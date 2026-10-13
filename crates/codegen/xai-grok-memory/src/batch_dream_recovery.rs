//! A plan that conflicts before any target holds its intended bytes is dropped
//! and its notes go back to the queue. Any other failed plan is blocked with
//! its lease released, so unrelated notes keep flowing until the files are
//! repaired and `retry_blocked` is called.

use rusqlite::{OptionalExtension as _, TransactionBehavior, params};

use crate::batch_dream::{
    BatchDreamError, BatchDreamStore, BatchReport, Result, STAGE_PREFIX, TOPICS_DIR,
};

const RECOVERY_LEASE_SECS: i64 = 60;
const MAX_ERROR_BYTES: usize = 512;

impl BatchDreamStore {
    pub(crate) fn apply_or_block(&self, operation_id: &str, now: i64) -> Result<BatchReport> {
        let error = match self.apply_planned(operation_id, now) {
            Ok(report) => return Ok(report),
            Err(error) => error,
        };
        if matches!(
            error,
            BatchDreamError::Conflict(_)
                | BatchDreamError::Access(_)
                | BatchDreamError::Invalid(_)
                | BatchDreamError::Io(_)
        ) {
            let mut message = error.to_string();
            message.truncate(message.floor_char_boundary(MAX_ERROR_BYTES));
            if matches!(error, BatchDreamError::Conflict(_)) {
                match self.release_unpublished(operation_id, &message) {
                    Ok(true) => {
                        tracing::warn!(target: crate::MEMORY_LOG_TARGET, %operation_id, %error, "batch Dream plan failed before publishing; notes returned to the queue");
                        return self.apply_planned(operation_id, now);
                    }
                    Ok(false) => {}
                    Err(release) => {
                        tracing::warn!(target: crate::MEMORY_LOG_TARGET, %operation_id, error = %release, "batch Dream could not release an unpublished plan");
                    }
                }
            }
            let mut connection = self.connection()?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let blocked = transaction.execute(
                "UPDATE batch_dream_batches SET status = 'blocked', last_error = ?2
                 WHERE operation_id = ?1 AND status = 'planned'",
                params![operation_id, message],
            )?;
            if blocked == 1 {
                transaction.execute(
                    "UPDATE consolidation_operations SET status = 'failed', last_error = ?2
                     WHERE operation_id = ?1",
                    params![operation_id, message],
                )?;
                transaction.execute(
                    "UPDATE consolidation_lock SET owner = NULL, expires_at = NULL
                     WHERE singleton = 1 AND generation =
                        (SELECT generation FROM batch_dream_batches WHERE operation_id = ?1)",
                    params![operation_id],
                )?;
            }
            transaction.commit()?;
        }
        Err(error)
    }

    /// Allow a blocked plan to apply again once every target holds its base or post bytes.
    ///
    /// # Errors
    /// `Invalid` unless the batch is blocked, or a database error.
    pub fn retry_blocked(&self, operation_id: &str) -> Result<()> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = transaction.execute(
            "UPDATE batch_dream_batches SET status = 'planned', expires_at = 0, last_error = NULL
             WHERE operation_id = ?1 AND status = 'blocked'",
            params![operation_id],
        )?;
        if changed != 1 {
            return Err(BatchDreamError::Invalid(format!(
                "batch {operation_id} is not blocked"
            )));
        }
        transaction.execute(
            "UPDATE consolidation_operations SET status = 'topics_written', last_error = NULL
             WHERE operation_id = ?1",
            params![operation_id],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Finish or release batches whose lease expired; live leases are never touched.
    ///
    /// # Errors
    /// Storage errors only; conflicting plans are blocked rather than returned.
    pub fn recover(&self, now: i64) -> Result<usize> {
        let mut recovered = 0;
        loop {
            self.control.check()?;
            let mut connection = self.connection()?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let is_active: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM consolidation_lock
                 WHERE singleton = 1 AND expires_at > ?1)",
                params![now],
                |row| row.get(0),
            )?;
            let expired = if is_active {
                None
            } else {
                transaction
                    .query_row(
                        "SELECT operation_id, owner, status FROM batch_dream_batches
                         WHERE status IN ('claimed','planned') AND expires_at <= ?1
                         ORDER BY generation LIMIT 1",
                        params![now],
                        |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, String>(2)?,
                            ))
                        },
                    )
                    .optional()?
            };
            let Some((operation_id, owner, status)) = expired else {
                transaction.commit()?;
                break;
            };
            if status == "claimed" {
                transaction.execute(
                    "UPDATE batch_dream_queue SET status = 'pending', reason = 'interrupted',
                        queue_order = (SELECT MAX(queue_order) + 1 FROM batch_dream_queue)
                     WHERE status = 'claimed' AND path IN
                        (SELECT path FROM batch_dream_batch_notes WHERE operation_id = ?1)",
                    params![operation_id],
                )?;
                transaction.execute(
                    "UPDATE batch_dream_batches SET status = 'released', completed_at = ?2
                     WHERE operation_id = ?1",
                    params![operation_id, now],
                )?;
                transaction.execute(
                    "UPDATE consolidation_operations SET status = 'failed', completed_at = ?2,
                        last_error = 'lease expired before a plan was persisted'
                     WHERE operation_id = ?1",
                    params![operation_id, now],
                )?;
                transaction.execute(
                    "UPDATE consolidation_lock SET owner = NULL, expires_at = NULL WHERE singleton = 1",
                    [],
                )?;
                transaction.commit()?;
            } else {
                let generation = transaction
                    .query_row(
                        "SELECT generation FROM consolidation_lock WHERE singleton = 1",
                        [],
                        |row| row.get::<_, u64>(0),
                    )?
                    .checked_add(1)
                    .ok_or_else(|| {
                        BatchDreamError::Invalid("lease generation overflows".to_owned())
                    })?;
                let expires_at = now.saturating_add(RECOVERY_LEASE_SECS);
                transaction.execute(
                    "UPDATE consolidation_lock SET owner = ?1, generation = ?2, expires_at = ?3
                     WHERE singleton = 1",
                    params![owner, generation, expires_at],
                )?;
                transaction.execute(
                    "UPDATE batch_dream_batches SET generation = ?2, expires_at = ?3
                     WHERE operation_id = ?1",
                    params![operation_id, generation, expires_at],
                )?;
                transaction.execute(
                    "UPDATE consolidation_operations SET generation = ?2 WHERE operation_id = ?1",
                    params![operation_id, generation],
                )?;
                transaction.commit()?;
                match self.apply_or_block(&operation_id, now) {
                    Ok(_)
                    | Err(
                        BatchDreamError::Conflict(_)
                        | BatchDreamError::Access(_)
                        | BatchDreamError::Invalid(_)
                        | BatchDreamError::Io(_),
                    ) => {}
                    Err(error) => return Err(error),
                }
            }
            recovered += 1;
        }
        let pending_cleanup: Vec<String> = {
            let connection = self.connection()?;
            let mut statement = connection.prepare(
                "SELECT operation_id FROM batch_dream_batches
                 WHERE status = 'completed' AND cleanup_pending = 1 ORDER BY generation",
            )?;
            statement
                .query_map([], |row| row.get(0))?
                .collect::<std::result::Result<_, _>>()?
        };
        for operation_id in pending_cleanup {
            self.cleanup_completed(&operation_id)?;
        }
        self.remove_orphan_stages(now)?;
        Ok(recovered)
    }

    fn remove_orphan_stages(&self, now: i64) -> Result<()> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let is_active: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM consolidation_lock WHERE singleton = 1 AND expires_at > ?1)",
            params![now],
            |row| row.get(0),
        )?;
        if !is_active {
            for entry in std::fs::read_dir(self.scope_dir.join(TOPICS_DIR))? {
                self.control.check()?;
                let entry = entry?;
                if !entry.file_type()?.is_file()
                    || !entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(STAGE_PREFIX)
                {
                    continue;
                }
                let is_retained: bool = transaction.query_row(
                    "SELECT EXISTS(SELECT 1 FROM batch_dream_changes c
                     JOIN batch_dream_batches b USING(operation_id)
                     WHERE c.stage_path = ?1 AND b.status IN ('planned','blocked'))",
                    params![entry.path().to_string_lossy()],
                    |row| row.get(0),
                )?;
                if !is_retained {
                    std::fs::remove_file(entry.path())?;
                }
            }
        }
        transaction.commit()?;
        Ok(())
    }
}
