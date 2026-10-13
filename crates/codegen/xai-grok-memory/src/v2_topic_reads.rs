//! Per-topic read ledger that ranks the compact memory index by use.
//!
//! Only the tool-facing `MemoryV2Access::record_read` counts a read, so Dream,
//! forget, and carry-over reads never skew the ranking. Counting runs for every
//! v2 user so the ledger is warm when `compact_index_enabled` turns on. The
//! ledger is advisory: a failed count or a failed lookup degrades to path order
//! and never fails the read, the render, or the browse listing.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

use rusqlite::{OpenFlags, params};
use xai_sqlite_journal::JournalMode;

use crate::v2::{Result, V2StorageError};

/// Declared by `initialize_state_db` so the read path never runs DDL.
pub(crate) const TOPIC_READS_TABLE_SQL: &str = "CREATE TABLE IF NOT EXISTS memory_v2_topic_reads (
    relative_path TEXT PRIMARY KEY,
    read_count INTEGER NOT NULL,
    last_read_at INTEGER NOT NULL
);";

/// A model `read_file` must not stall behind Dream or capture holding the
/// scope write lock; losing one count is cheaper than a slow read.
const COUNT_BUSY_TIMEOUT: Duration = Duration::from_millis(100);

/// Order two topics the way the compact index does: most read first, then by
/// path. Browse listings use the same rule so the modal matches the next render.
pub(crate) fn compare_topics_by_use(left: (&str, u64), right: (&str, u64)) -> Ordering {
    right.1.cmp(&left.1).then_with(|| left.0.cmp(right.0))
}

/// Order two inbox observations newest first. Observation files are written
/// once, so modification time is when the note entered the inbox; the path is
/// a deterministic tiebreak.
pub(crate) fn compare_observations_newest_first(
    left: (Option<SystemTime>, &str),
    right: (Option<SystemTime>, &str),
) -> Ordering {
    right.0.cmp(&left.0).then_with(|| right.1.cmp(left.1))
}

/// Count one ordinary model read of `relative_path` in the scope at `scope_dir`.
///
/// Opens without `CREATE`, so a read racing `/memory clear` can never leave a
/// bare database behind; a database that is missing, symlinked, or not
/// openable is skipped.
///
/// # Errors
///
/// Returns [`V2StorageError::Database`] if the upsert fails, including when
/// the scope's write lock is held longer than [`COUNT_BUSY_TIMEOUT`].
pub(crate) fn record_topic_read(scope_dir: &Path, relative_path: &str) -> Result<()> {
    let state_path = scope_dir.join("memory_state.sqlite");
    let journal_mode = JournalMode::for_db_path(&state_path);
    let effective_path = journal_mode.effective_db_path(&state_path);
    match std::fs::symlink_metadata(&effective_path) {
        Ok(metadata) if metadata.file_type().is_symlink() => return Ok(()),
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(V2StorageError::Io {
                operation: "inspect v2 state read ledger",
                path: state_path,
                source,
            });
        }
    }
    let database_error = |source| V2StorageError::Database {
        database: "state",
        path: state_path.clone(),
        source,
    };
    let connection = match rusqlite::Connection::open_with_flags(
        &effective_path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) {
        Ok(connection) => connection,
        Err(rusqlite::Error::SqliteFailure(error, _))
            if error.code == rusqlite::ErrorCode::CannotOpen =>
        {
            return Ok(());
        }
        Err(source) => return Err(database_error(source)),
    };
    journal_mode
        .apply_with_retry_until(&connection, Instant::now() + COUNT_BUSY_TIMEOUT)
        .and_then(|()| connection.busy_timeout(COUNT_BUSY_TIMEOUT))
        .map_err(database_error)?;
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    connection
        .execute(
            "INSERT INTO memory_v2_topic_reads(relative_path, read_count, last_read_at)
             VALUES (?1, 1, ?2)
             ON CONFLICT(relative_path) DO UPDATE
             SET read_count = read_count + 1, last_read_at = excluded.last_read_at",
            params![relative_path.replace('\\', "/"), now],
        )
        .map(|_| ())
        .map_err(database_error)
}

/// Read counts per topic path from an already open state connection. A
/// missing ledger table means nothing has been read yet.
pub(crate) fn read_counts_from_state(
    connection: &rusqlite::Connection,
) -> rusqlite::Result<BTreeMap<String, u64>> {
    let table_exists = connection.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM sqlite_master
            WHERE type = 'table' AND name = 'memory_v2_topic_reads'
         )",
        [],
        |row| row.get::<_, bool>(0),
    )?;
    if !table_exists {
        return Ok(BTreeMap::new());
    }
    let mut statement =
        connection.prepare("SELECT relative_path, read_count FROM memory_v2_topic_reads")?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    rows.map(|row| row.map(|(path, count)| (path, u64::try_from(count).unwrap_or(0))))
        .collect()
}
