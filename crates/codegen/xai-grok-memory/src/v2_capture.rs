//! Durable capture queue and immutable observation persistence for memory v2.
//!
//! The state database is the cross-process serialization point. Observation
//! files are published before their outcome row, so a crash can leave orphan
//! files; each file carries its job id and expected file count, allowing
//! reconciliation to adopt only a complete, hash-consistent set. Outcomes are
//! committed before indexing and indexed-cursor advancement, so reconciliation
//! can deterministically replay that second crash window.

use std::collections::BTreeMap;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{OptionalExtension as _, TransactionBehavior, params};
use xai_sqlite_journal::{JournalMode, is_network_fs};

use crate::storage::MemoryStorage;
use crate::v2::{V2ManifestBudget, V2MemoryScope, persist_scope_manifest, render_scope_manifest};

const STATE_SCHEMA_VERSION: &str = "3";
const MAX_SESSION_BYTES: usize = 256;
const MAX_OWNER_BYTES: usize = 128;
const MAX_MODEL_BYTES: usize = 128;
const MAX_PROMPT_VERSION_BYTES: usize = 64;
const MAX_TOPIC_BYTES: usize = 128;
const MAX_STATEMENT_BYTES: usize = 1_024;
const MAX_BODY_BYTES: usize = 8 * 1_024;
const MAX_KEYWORDS: usize = 16;
const MAX_ALIASES: usize = 16;
const MAX_TERM_BYTES: usize = 64;
const MAX_OBSERVATIONS: usize = 128;
const MAX_FAILURE_BYTES: usize = 512;
const MAX_OBSERVATION_FILE_BYTES: u64 = 16 * 1_024;
const MAX_RECOVERY_JOBS: usize = 4_096;
const MAX_INBOX_ENTRIES: usize = 4_096;
const MAX_COMMITTED_OBSERVATION_ROWS: usize = 4_096;
const MAX_REINDEX_ATTEMPTS: usize = 3;

pub type Result<T> = std::result::Result<T, V2CaptureError>;

#[derive(Debug, thiserror::Error)]
pub enum V2CaptureError {
    #[error("invalid capture input: {0}")]
    Invalid(String),
    #[error("capture conflict: {0}")]
    Conflict(String),
    #[error("capture lease is stale or not owned by this worker")]
    StaleLease,
    #[error(
        "durable capture is unavailable on network-mounted scope {path}: shared observation files require a shared coordination database"
    )]
    UnsupportedNetworkFilesystem { path: PathBuf },
    #[error("capture clock is before the Unix epoch")]
    Clock(#[source] std::time::SystemTimeError),
    #[error("capture state database failed")]
    Database(#[from] rusqlite::Error),
    #[error("capture filesystem operation failed at {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("capture index convergence failed at {path}")]
    Index {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },
    #[error("capture manifest convergence failed")]
    Manifest(#[source] crate::v2::V2StorageError),
    #[error("capture recovery directory {path} exceeds the {limit}-entry safety limit")]
    RecoveryDirectoryLimit { path: PathBuf, limit: usize },
    #[error("capture recovery query for {rows} exceeds the {limit}-row safety limit")]
    RecoveryRowLimit { rows: &'static str, limit: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureRange {
    from_turn: u32,
    through_turn: u32,
}

impl CaptureRange {
    pub fn try_new(from_turn: u32, through_turn: u32) -> Result<Self> {
        if from_turn > through_turn {
            return Err(V2CaptureError::Invalid(
                "capture range starts after it ends".to_owned(),
            ));
        }
        if through_turn > 999_999 {
            return Err(V2CaptureError::Invalid(
                "capture turn exceeds the six-digit durable path limit".to_owned(),
            ));
        }
        Ok(Self {
            from_turn,
            through_turn,
        })
    }

    pub fn from_turn(self) -> u32 {
        self.from_turn
    }

    pub fn through_turn(self) -> u32 {
        self.through_turn
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObservationType {
    User,
    Feedback,
    Project,
    Reference,
}

impl ObservationType {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "user" => Ok(Self::User),
            "feedback" => Ok(Self::Feedback),
            "project" => Ok(Self::Project),
            "reference" => Ok(Self::Reference),
            _ => Err(V2CaptureError::Invalid(format!(
                "unknown observation type {value:?}"
            ))),
        }
    }

    /// xai-codegen-lint: allow(manual_strum)
    fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Feedback => "feedback",
            Self::Project => "project",
            Self::Reference => "reference",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservationDraft {
    pub observation_type: ObservationType,
    pub topic_hint: Option<String>,
    pub statement: String,
    pub keywords: Vec<String>,
    pub aliases: Vec<String>,
    pub extraction_model: String,
    pub prompt_version: String,
    pub created_at: i64,
    pub body: Option<String>,
}

impl ObservationDraft {
    fn validate(&self) -> Result<()> {
        validate_text("statement", &self.statement, 1, MAX_STATEMENT_BYTES)?;
        validate_text(
            "extraction model",
            &self.extraction_model,
            1,
            MAX_MODEL_BYTES,
        )?;
        validate_text(
            "prompt version",
            &self.prompt_version,
            1,
            MAX_PROMPT_VERSION_BYTES,
        )?;
        if !(0..=253_402_300_799).contains(&self.created_at) {
            return Err(V2CaptureError::Invalid(
                "created timestamp is outside the supported range".to_owned(),
            ));
        }
        if let Some(topic) = &self.topic_hint {
            validate_text("topic hint", topic, 1, MAX_TOPIC_BYTES)?;
        }
        if let Some(body) = &self.body
            && (body.is_empty()
                || body.len() > MAX_BODY_BYTES
                || body.chars().any(|character| {
                    character.is_control() && !matches!(character, '\n' | '\r' | '\t')
                }))
        {
            return Err(V2CaptureError::Invalid(format!(
                "observation body must contain 1..={MAX_BODY_BYTES} safe bytes"
            )));
        }
        validate_terms("keywords", &self.keywords, MAX_KEYWORDS)?;
        validate_terms("aliases", &self.aliases, MAX_ALIASES)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureOutcomeDraft {
    Observations(Vec<ObservationDraft>),
    Noop,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureJob {
    pub job_id: String,
    pub session_id: String,
    pub range: CaptureRange,
    pub attempt: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureLease {
    pub job: CaptureJob,
    pub owner: String,
    pub expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimRequest {
    pub owner: String,
    pub now: i64,
    pub duration: Duration,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CaptureCursors {
    pub requested: u32,
    pub captured: u32,
    pub indexed: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitResult {
    pub outcome_hash: String,
    pub files: Vec<PathBuf>,
    pub was_already_committed: bool,
}

pub struct V2CaptureStore {
    scope_dir: PathBuf,
    scope: V2MemoryScope,
    state_path: PathBuf,
}

impl V2CaptureStore {
    /// Open and additively migrate one initialized v2 scope.
    pub fn open(scope_dir: impl AsRef<Path>, scope: V2MemoryScope) -> Result<Self> {
        Self::open_with_journal_mode(scope_dir, scope, None)
    }

    fn open_with_journal_mode(
        scope_dir: impl AsRef<Path>,
        scope: V2MemoryScope,
        journal_mode: Option<JournalMode>,
    ) -> Result<Self> {
        let scope_dir = scope_dir.as_ref().to_path_buf();
        reject_symlink(&scope_dir)?;
        let state_path = scope_dir.join("memory_state.sqlite");
        let journal_mode = journal_mode.unwrap_or_else(|| JournalMode::for_db_path(&state_path));
        ensure_shared_coordination(&scope_dir, journal_mode)?;
        reject_symlink(&scope_dir.join("observations"))?;
        reject_symlink(&scope_dir.join("observations/_inbox"))?;
        std::fs::create_dir_all(scope_dir.join("observations/_inbox"))
            .map_err(|source| io_error(scope_dir.join("observations/_inbox"), source))?;
        for path in [
            scope_dir.join("observations"),
            scope_dir.join("observations/_inbox"),
            scope_dir.join("memory_state.sqlite"),
            scope_dir.join("index.sqlite"),
            scope_dir.join("MEMORY.md"),
        ] {
            reject_symlink(&path)?;
        }
        let store = Self {
            scope_dir,
            scope,
            state_path,
        };
        store.migrate()?;
        Ok(store)
    }

    pub fn ensure_session(&self, session_id: &str) -> Result<()> {
        let safe_session = safe_session_name(session_id)?;
        let connection = self.open_state()?;
        connection.execute(
            "INSERT OR IGNORE INTO capture_sessions(
                session_id, safe_session, requested_cursor, captured_cursor, indexed_cursor
             ) VALUES (?1, ?2, 0, 0, 0)",
            params![session_id, safe_session],
        )?;
        Ok(())
    }

    /// Enqueue a deterministic range. Duplicate requests return the same job.
    pub fn enqueue(&self, session_id: &str, range: CaptureRange) -> Result<CaptureJob> {
        let safe_session = safe_session_name(session_id)?;
        let job_id = deterministic_job_id(session_id, range);
        let mut connection = self.open_state()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "INSERT OR IGNORE INTO capture_sessions(
                session_id, safe_session, requested_cursor, captured_cursor, indexed_cursor
             ) VALUES (?1, ?2, 0, 0, 0)",
            params![session_id, safe_session],
        )?;
        transaction.execute(
            "UPDATE capture_sessions
             SET requested_cursor = MAX(requested_cursor, ?2)
             WHERE session_id = ?1",
            params![session_id, range.through_turn],
        )?;
        transaction.execute(
            "INSERT OR IGNORE INTO capture_jobs(
                job_id, session_id, from_turn, through_turn, status, attempt
             ) VALUES (?1, ?2, ?3, ?4, 'pending', 0)",
            params![job_id, session_id, range.from_turn, range.through_turn],
        )?;
        let job = read_job(&transaction, &job_id)?;
        transaction.commit()?;
        Ok(job)
    }

    /// Claim pending/failed work, reclaiming an expired running lease.
    pub fn claim(&self, request: &ClaimRequest) -> Result<Option<CaptureLease>> {
        validate_text("lease owner", &request.owner, 1, MAX_OWNER_BYTES)?;
        validate_runtime_timestamp(request.now)?;
        let lease_seconds = i64::try_from(request.duration.as_secs())
            .map_err(|_| V2CaptureError::Invalid("lease duration is too large".to_owned()))?;
        if lease_seconds == 0 {
            return Err(V2CaptureError::Invalid(
                "lease duration must be at least one second".to_owned(),
            ));
        }
        let expires_at = request
            .now
            .checked_add(lease_seconds)
            .ok_or_else(|| V2CaptureError::Invalid("lease expiry overflows".to_owned()))?;
        let mut connection = self.open_state()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let job_id = transaction
            .query_row(
                "SELECT job_id FROM capture_jobs
                 WHERE status IN ('pending', 'failed')
                    OR (status = 'running' AND lease_expires_at <= ?1)
                 ORDER BY rowid LIMIT 1",
                params![request.now],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let Some(job_id) = job_id else {
            transaction.commit()?;
            return Ok(None);
        };
        transaction.execute(
            "UPDATE capture_jobs SET status = 'running', lease_owner = ?2,
                lease_expires_at = ?3, attempt = attempt + 1, last_error = NULL
             WHERE job_id = ?1",
            params![job_id, request.owner, expires_at],
        )?;
        let job = read_job(&transaction, &job_id)?;
        transaction.commit()?;
        Ok(Some(CaptureLease {
            job,
            owner: request.owner.clone(),
            expires_at,
        }))
    }

    /// Commit an outcome and converge files, lexical index, manifest, and cursors.
    pub fn commit(
        &self,
        lease: &CaptureLease,
        outcome: &CaptureOutcomeDraft,
        now: i64,
    ) -> Result<CommitResult> {
        validate_runtime_timestamp(now)?;
        reject_symlink(&self.scope_dir)?;
        reject_symlink(&self.scope_dir.join("observations"))?;
        reject_symlink(&self.scope_dir.join("observations/_inbox"))?;
        let prepared = self.prepare_outcome(&lease.job, outcome)?;
        let mut connection = self.open_state()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;

        if let Some(existing_hash) = transaction
            .query_row(
                "SELECT outcome_hash FROM capture_outcomes WHERE job_id = ?1",
                params![lease.job.job_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
        {
            if existing_hash != prepared.hash {
                return Err(V2CaptureError::Conflict(
                    "job was already committed with a different outcome".to_owned(),
                ));
            }
            transaction.commit()?;
            self.reconcile()?;
            return Ok(CommitResult {
                outcome_hash: existing_hash,
                files: prepared
                    .files
                    .iter()
                    .map(|file| file.path.clone())
                    .collect(),
                was_already_committed: true,
            });
        }
        validate_lease(&transaction, lease, now)?;
        self.remove_retry_orphans(&lease.job)?;
        for file in &prepared.files {
            persist_create_only(&self.scope_dir.join(&file.path), &file.bytes)?;
        }
        persist_outcome(&transaction, &lease.job, &prepared, now)?;
        transaction.commit()?;
        self.reconcile()?;
        Ok(CommitResult {
            outcome_hash: prepared.hash,
            files: prepared.files.into_iter().map(|file| file.path).collect(),
            was_already_committed: false,
        })
    }

    pub fn fail_retryable(&self, lease: &CaptureLease, now: i64, error: &str) -> Result<()> {
        validate_text("capture failure", error, 1, MAX_FAILURE_BYTES)?;
        validate_runtime_timestamp(now)?;
        let mut connection = self.open_state()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        validate_lease(&transaction, lease, now)?;
        transaction.execute(
            "UPDATE capture_jobs SET status = 'failed', lease_owner = NULL,
                lease_expires_at = NULL, last_error = ?2 WHERE job_id = ?1",
            params![lease.job.job_id, error],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Repair complete orphan file sets and replay indexing/manifest/cursors.
    pub fn reconcile(&self) -> Result<()> {
        let now = i64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(V2CaptureError::Clock)?
                .as_secs(),
        )
        .map_err(|_| V2CaptureError::Invalid("capture clock exceeds i64".to_owned()))?;
        self.reconcile_at_with(now, || Ok(()))
    }

    fn reconcile_at_with(
        &self,
        now: i64,
        before_manifest_publish: impl FnOnce() -> Result<()>,
    ) -> Result<()> {
        validate_runtime_timestamp(now)?;
        reject_symlink(&self.scope_dir)?;
        reject_symlink(&self.scope_dir.join("observations"))?;
        reject_symlink(&self.scope_dir.join("observations/_inbox"))?;
        reject_symlink(&self.scope_dir.join("index.sqlite"))?;
        reject_symlink(&self.scope_dir.join("MEMORY.md"))?;
        let mut connection = self.open_state()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        self.adopt_complete_orphans(&transaction, now)?;

        let mut statement = transaction.prepare(
            "SELECT f.job_id, f.ordinal, f.path, f.content_hash
             FROM capture_observation_files f
             JOIN capture_outcomes o ON o.job_id = f.job_id
             ORDER BY f.job_id, f.ordinal
             LIMIT ?1",
        )?;
        let rows = statement
            .query_map(params![MAX_COMMITTED_OBSERVATION_ROWS + 1], |row| {
                Ok(CommittedObservationFile {
                    path: row.get(2)?,
                    content_hash: row.get(3)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(statement);
        if rows.len() > MAX_COMMITTED_OBSERVATION_ROWS {
            return Err(V2CaptureError::RecoveryRowLimit {
                rows: "committed observation files",
                limit: MAX_COMMITTED_OBSERVATION_ROWS,
            });
        }

        let mut statement = transaction.prepare(
            "SELECT session_id, captured_cursor
             FROM capture_sessions
             WHERE indexed_cursor < captured_cursor
             ORDER BY session_id
             LIMIT ?1",
        )?;
        let cursors = statement
            .query_map(params![MAX_RECOVERY_JOBS + 1], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(statement);
        if cursors.len() > MAX_RECOVERY_JOBS {
            return Err(V2CaptureError::RecoveryRowLimit {
                rows: "capture sessions pending indexing",
                limit: MAX_RECOVERY_JOBS,
            });
        }
        let revision = read_capture_revision(&transaction)?;
        transaction.commit()?;

        self.index_committed_rows(&rows)?;
        let manifest =
            render_scope_manifest(&self.scope_dir, self.scope, V2ManifestBudget::default())
                .map_err(V2CaptureError::Manifest)?;
        before_manifest_publish()?;

        let mut connection = self.open_state()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        // Rendering and indexing intentionally happen without the queue write
        // lock. Fence only the cheap atomic publish: a newer outcome must keep
        // the manifest it generated instead of being replaced by this snapshot.
        if read_capture_revision(&transaction)? != revision {
            transaction.commit()?;
            return Ok(());
        }
        persist_scope_manifest(&self.scope_dir, &manifest).map_err(V2CaptureError::Manifest)?;
        for (session_id, captured_cursor) in cursors {
            transaction.execute(
                "UPDATE capture_sessions
                 SET indexed_cursor = MAX(indexed_cursor, ?2)
                 WHERE session_id = ?1",
                params![session_id, captured_cursor],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    fn index_committed_rows(&self, rows: &[CommittedObservationFile]) -> Result<()> {
        for row in rows {
            let path = self.scope_dir.join(&row.path);
            let bytes = read_regular_bounded(&path)?;
            if content_hash(&bytes) != row.content_hash {
                return Err(V2CaptureError::Conflict(format!(
                    "observation file {} differs from its committed hash",
                    path.display()
                )));
            }
        }

        if !rows.is_empty() {
            let storage = MemoryStorage::new_flat(&self.scope_dir, &self.scope_dir);
            let index_path = self.scope_dir.join("index.sqlite");
            let mut index = crate::MemoryIndex::open_or_create_preserving_dimensions(
                &index_path,
                storage,
                xai_grok_config_types::MemoryIndexConfig::default(),
                1,
            )
            .map_err(|source| V2CaptureError::Index {
                path: index_path.clone(),
                source,
            })?;
            let source = match self.scope {
                V2MemoryScope::Global => "global",
                V2MemoryScope::Workspace => "workspace",
            };
            for row in rows {
                let path = self.scope_dir.join(&row.path);
                reindex_with_race_retry(&mut index, &path, source).map_err(|source| {
                    V2CaptureError::Index {
                        path: path.clone(),
                        source,
                    }
                })?;
            }
        }
        Ok(())
    }

    pub fn cursors(&self, session_id: &str) -> Result<CaptureCursors> {
        validate_session_id(session_id)?;
        self.open_state()?
            .query_row(
                "SELECT requested_cursor, captured_cursor, indexed_cursor
                 FROM capture_sessions WHERE session_id = ?1",
                params![session_id],
                |row| {
                    Ok(CaptureCursors {
                        requested: row.get(0)?,
                        captured: row.get(1)?,
                        indexed: row.get(2)?,
                    })
                },
            )
            .optional()?
            .ok_or_else(|| V2CaptureError::Invalid("unknown capture session".to_owned()))
    }

    fn migrate(&self) -> Result<()> {
        let mut connection = self.open_state()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute_batch(
            "CREATE TABLE IF NOT EXISTS meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS capture_sessions (
                session_id TEXT PRIMARY KEY,
                safe_session TEXT NOT NULL UNIQUE,
                requested_cursor INTEGER NOT NULL DEFAULT 0 CHECK(requested_cursor >= 0),
                captured_cursor INTEGER NOT NULL DEFAULT 0 CHECK(captured_cursor >= 0),
                indexed_cursor INTEGER NOT NULL DEFAULT 0 CHECK(indexed_cursor >= 0)
            );
            CREATE TABLE IF NOT EXISTS capture_jobs (
                job_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL REFERENCES capture_sessions(session_id),
                from_turn INTEGER NOT NULL,
                through_turn INTEGER NOT NULL,
                status TEXT NOT NULL CHECK(status IN ('pending','running','completed','failed')),
                lease_owner TEXT,
                lease_expires_at INTEGER,
                attempt INTEGER NOT NULL DEFAULT 0,
                last_error TEXT,
                UNIQUE(session_id, from_turn, through_turn)
            );
            CREATE TABLE IF NOT EXISTS capture_outcomes (
                job_id TEXT PRIMARY KEY REFERENCES capture_jobs(job_id),
                outcome_kind TEXT NOT NULL CHECK(outcome_kind IN ('observations','noop')),
                outcome_hash TEXT NOT NULL,
                committed_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS capture_observation_files (
                job_id TEXT NOT NULL REFERENCES capture_outcomes(job_id),
                ordinal INTEGER NOT NULL,
                path TEXT NOT NULL UNIQUE,
                content_hash TEXT NOT NULL,
                PRIMARY KEY(job_id, ordinal)
            );",
        )?;
        transaction.execute(
            "INSERT OR IGNORE INTO meta(key, value) VALUES ('capture_revision', '0')",
            [],
        )?;
        transaction.execute(
            "INSERT INTO meta(key, value) VALUES ('schema_version', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![STATE_SCHEMA_VERSION],
        )?;
        transaction.commit()?;
        Ok(())
    }

    fn open_state(&self) -> Result<rusqlite::Connection> {
        reject_symlink(&self.state_path)?;
        let journal_mode = JournalMode::for_db_path(&self.state_path);
        ensure_shared_coordination(&self.scope_dir, journal_mode)?;
        journal_mode
            .open(&self.state_path)
            .map_err(V2CaptureError::Database)
    }

    fn prepare_outcome(
        &self,
        job: &CaptureJob,
        outcome: &CaptureOutcomeDraft,
    ) -> Result<PreparedOutcome> {
        let safe_session = safe_session_name(&job.session_id)?;
        let files = match outcome {
            CaptureOutcomeDraft::Noop => Vec::new(),
            CaptureOutcomeDraft::Observations(observations) => {
                if observations.is_empty() || observations.len() > MAX_OBSERVATIONS {
                    return Err(V2CaptureError::Invalid(format!(
                        "observations must contain 1..={MAX_OBSERVATIONS} entries"
                    )));
                }
                observations
                    .iter()
                    .enumerate()
                    .map(|(ordinal, observation)| {
                        observation.validate()?;
                        let filename = format!(
                            "{safe_session}__t{:06}-{:06}__n{ordinal:03}.md",
                            job.range.from_turn, job.range.through_turn
                        );
                        let path = PathBuf::from("observations/_inbox").join(filename);
                        let bytes =
                            render_observation(observation, job, ordinal, observations.len())?
                                .into_bytes();
                        Ok(PreparedFile {
                            path,
                            hash: content_hash(&bytes),
                            bytes,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?
            }
        };
        let kind = if files.is_empty() {
            "noop"
        } else {
            "observations"
        };
        let mut hash_input = format!("v2:{kind}:{}", job.job_id);
        for (ordinal, file) in files.iter().enumerate() {
            hash_input.push_str(&format!(
                "\n{ordinal}:{}:{}",
                file.path.display(),
                file.hash
            ));
        }
        Ok(PreparedOutcome {
            kind,
            hash: content_hash(hash_input.as_bytes()),
            files,
        })
    }

    fn adopt_complete_orphans(
        &self,
        transaction: &rusqlite::Transaction<'_>,
        now: i64,
    ) -> Result<()> {
        let mut jobs_statement = transaction.prepare(
            "SELECT j.job_id, j.session_id, j.from_turn, j.through_turn, j.attempt, s.safe_session
             FROM capture_jobs j JOIN capture_sessions s USING(session_id)
             LEFT JOIN capture_outcomes o USING(job_id)
             WHERE o.job_id IS NULL
               AND (
                   j.status != 'running'
                   OR j.lease_owner IS NULL
                   OR j.lease_expires_at IS NULL
                   OR j.lease_expires_at <= ?1
               )
             ORDER BY j.rowid
             LIMIT ?2",
        )?;
        let jobs = jobs_statement
            .query_map(params![now, MAX_RECOVERY_JOBS + 1], |row| {
                Ok((
                    CaptureJob {
                        job_id: row.get(0)?,
                        session_id: row.get(1)?,
                        range: CaptureRange {
                            from_turn: row.get(2)?,
                            through_turn: row.get(3)?,
                        },
                        attempt: row.get(4)?,
                    },
                    row.get::<_, String>(5)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(jobs_statement);
        if jobs.len() > MAX_RECOVERY_JOBS {
            return Err(V2CaptureError::RecoveryRowLimit {
                rows: "uncommitted capture jobs",
                limit: MAX_RECOVERY_JOBS,
            });
        }

        let inbox = self.scope_dir.join("observations/_inbox");
        let entries = read_dir_bounded(&inbox, MAX_INBOX_ENTRIES)?;
        for (job, safe_session) in jobs {
            let prefix = format!(
                "{safe_session}__t{:06}-{:06}__n",
                job.range.from_turn, job.range.through_turn
            );
            let mut found = BTreeMap::new();
            let mut expected_count = None;
            let mut consistent = true;
            for entry in &entries {
                if !entry.is_file
                    || !entry.name.starts_with(&prefix)
                    || !entry.name.ends_with(".md")
                {
                    continue;
                }
                let Ok(ordinal) = recovery_ordinal(&entry.name, &prefix) else {
                    continue;
                };
                let Ok(bytes) = read_regular_bounded(&entry.path) else {
                    continue;
                };
                let Ok(metadata) = recovery_metadata(&bytes) else {
                    continue;
                };
                if metadata.job_id != job.job_id
                    || metadata.session_id != job.session_id
                    || metadata.range != job.range
                    || metadata.ordinal != ordinal
                {
                    continue;
                }
                if expected_count.is_some_and(|old| old != metadata.count)
                    || found.contains_key(&ordinal)
                {
                    consistent = false;
                    break;
                }
                expected_count = Some(metadata.count);
                found.insert(
                    ordinal,
                    PreparedFile {
                        path: PathBuf::from("observations/_inbox").join(&entry.name),
                        hash: content_hash(&bytes),
                        bytes,
                    },
                );
            }
            if !consistent {
                continue;
            }
            let Some(count) = expected_count else {
                continue;
            };
            if count == 0
                || count > MAX_OBSERVATIONS
                || found.len() != count
                || !(0..count).all(|ordinal| found.contains_key(&ordinal))
            {
                continue;
            }
            let files: Vec<_> = found.into_values().collect();
            let mut hash_input = format!("v2:observations:{}", job.job_id);
            for (ordinal, file) in files.iter().enumerate() {
                hash_input.push_str(&format!(
                    "\n{ordinal}:{}:{}",
                    file.path.display(),
                    file.hash
                ));
            }
            let prepared = PreparedOutcome {
                kind: "observations",
                hash: content_hash(hash_input.as_bytes()),
                files,
            };
            persist_outcome(transaction, &job, &prepared, 0)?;
        }
        Ok(())
    }

    fn remove_retry_orphans(&self, job: &CaptureJob) -> Result<()> {
        let safe_session = safe_session_name(&job.session_id)?;
        let prefix = format!(
            "{safe_session}__t{:06}-{:06}__n",
            job.range.from_turn, job.range.through_turn
        );
        let inbox = self.scope_dir.join("observations/_inbox");
        for entry in read_dir_bounded(&inbox, MAX_INBOX_ENTRIES)? {
            if entry.is_file && entry.name.starts_with(&prefix) && entry.name.ends_with(".md") {
                std::fs::remove_file(&entry.path).map_err(|source| io_error(entry.path, source))?;
            }
        }
        Ok(())
    }
}

struct PreparedOutcome {
    kind: &'static str,
    hash: String,
    files: Vec<PreparedFile>,
}

struct PreparedFile {
    path: PathBuf,
    hash: String,
    bytes: Vec<u8>,
}

struct CommittedObservationFile {
    path: String,
    content_hash: String,
}

struct RecoveryMetadata {
    job_id: String,
    session_id: String,
    range: CaptureRange,
    ordinal: usize,
    count: usize,
}

struct RecoveryDirectoryEntry {
    path: PathBuf,
    name: String,
    is_file: bool,
}

fn reindex_with_race_retry(
    index: &mut crate::MemoryIndex,
    path: &Path,
    source: &str,
) -> std::result::Result<(), rusqlite::Error> {
    for attempt in 1..=MAX_REINDEX_ATTEMPTS {
        match index.reindex_file(path, source) {
            Ok(_) => return Ok(()),
            Err(error)
                if attempt < MAX_REINDEX_ATTEMPTS
                    && matches!(
                        &error,
                        rusqlite::Error::SqliteFailure(failure, _)
                            if failure.code == rusqlite::ErrorCode::ConstraintViolation
                    ) =>
            {
                // A peer may have inserted from the same stale index snapshot.
                // Retry after its commit is visible; the attempt bound still
                // fails closed on genuine index corruption.
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("the bounded reindex loop always returns")
}

fn ensure_shared_coordination(scope_dir: &Path, journal_mode: JournalMode) -> Result<()> {
    if is_network_fs(scope_dir) || journal_mode == JournalMode::Truncate {
        return Err(V2CaptureError::UnsupportedNetworkFilesystem {
            path: scope_dir.to_path_buf(),
        });
    }
    Ok(())
}

fn validate_session_id(session_id: &str) -> Result<()> {
    validate_text("session id", session_id, 1, MAX_SESSION_BYTES)
}

fn safe_session_name(session_id: &str) -> Result<String> {
    validate_session_id(session_id)?;
    let mut slug = String::with_capacity(48);
    let mut previous_dash = false;
    for character in session_id.chars() {
        let mapped = if character.is_ascii_alphanumeric() {
            character.to_ascii_lowercase()
        } else if matches!(character, '-' | '_') {
            character
        } else {
            '-'
        };
        if mapped == '-' {
            if !previous_dash && !slug.is_empty() {
                slug.push(mapped);
            }
            previous_dash = true;
        } else {
            slug.push(mapped);
            previous_dash = false;
        }
        if slug.len() >= 36 {
            break;
        }
    }
    let slug = slug.trim_matches('-');
    let slug = if slug.is_empty() { "session" } else { slug };
    let digest = blake3::hash(session_id.as_bytes()).to_hex();
    Ok(format!("{slug}-{digest}"))
}

fn deterministic_job_id(session_id: &str, range: CaptureRange) -> String {
    let input = format!(
        "capture-v2\0{session_id}\0{}\0{}",
        range.from_turn, range.through_turn
    );
    format!("cap_{}", &blake3::hash(input.as_bytes()).to_hex()[..24])
}

fn validate_text(name: &str, value: &str, min: usize, max: usize) -> Result<()> {
    if value.len() < min || value.len() > max {
        return Err(V2CaptureError::Invalid(format!(
            "{name} must contain {min}..={max} bytes"
        )));
    }
    if value.chars().any(char::is_control) {
        return Err(V2CaptureError::Invalid(format!(
            "{name} contains control characters"
        )));
    }
    Ok(())
}

fn validate_runtime_timestamp(value: i64) -> Result<()> {
    if value < 0 {
        Err(V2CaptureError::Invalid(
            "capture timestamp cannot be negative".to_owned(),
        ))
    } else {
        Ok(())
    }
}

fn validate_terms(name: &str, values: &[String], max_count: usize) -> Result<()> {
    if values.len() > max_count {
        return Err(V2CaptureError::Invalid(format!(
            "{name} exceeds the {max_count}-entry limit"
        )));
    }
    for value in values {
        validate_text(name, value, 1, MAX_TERM_BYTES)?;
    }
    Ok(())
}

fn render_observation(
    observation: &ObservationDraft,
    job: &CaptureJob,
    ordinal: usize,
    count: usize,
) -> Result<String> {
    let json = |value: &str| {
        serde_json::to_string(value)
            .map_err(|error| V2CaptureError::Invalid(format!("cannot encode observation: {error}")))
    };
    let topic = observation
        .topic_hint
        .as_deref()
        .map(&json)
        .transpose()?
        .unwrap_or_else(|| "null".to_owned());
    let keywords = serde_json::to_string(&observation.keywords)
        .map_err(|error| V2CaptureError::Invalid(error.to_string()))?;
    let aliases = serde_json::to_string(&observation.aliases)
        .map_err(|error| V2CaptureError::Invalid(error.to_string()))?;
    let mut rendered = format!(
        "---\nschema_version: 2\ntype: {}\ntopic_hint: {topic}\nkeywords: {keywords}\n\
         aliases: {aliases}\nsession_id: {}\nfrom_turn: {}\nthrough_turn: {}\n\
         extraction_model: {}\nprompt_version: {}\ncreated_at: {}\njob_id: {}\n\
         observation_ordinal: {ordinal}\nobservation_count: {count}\n---\n\n# {}\n",
        observation.observation_type.as_str(),
        json(&job.session_id)?,
        job.range.from_turn,
        job.range.through_turn,
        json(&observation.extraction_model)?,
        json(&observation.prompt_version)?,
        observation.created_at,
        json(&job.job_id)?,
        observation.statement
    );
    if let Some(body) = &observation.body {
        rendered.push('\n');
        rendered.push_str(body);
        rendered.push('\n');
    }
    Ok(rendered)
}

fn content_hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

fn read_job(connection: &rusqlite::Connection, job_id: &str) -> Result<CaptureJob> {
    connection
        .query_row(
            "SELECT job_id, session_id, from_turn, through_turn, attempt
             FROM capture_jobs WHERE job_id = ?1",
            params![job_id],
            |row| {
                Ok(CaptureJob {
                    job_id: row.get(0)?,
                    session_id: row.get(1)?,
                    range: CaptureRange {
                        from_turn: row.get(2)?,
                        through_turn: row.get(3)?,
                    },
                    attempt: row.get(4)?,
                })
            },
        )
        .map_err(V2CaptureError::Database)
}

fn validate_lease(
    transaction: &rusqlite::Transaction<'_>,
    lease: &CaptureLease,
    now: i64,
) -> Result<()> {
    let matches = transaction.query_row(
        "SELECT COUNT(*) FROM capture_jobs
         WHERE job_id = ?1 AND status = 'running' AND lease_owner = ?2
           AND lease_expires_at > ?3 AND attempt = ?4 AND session_id = ?5
           AND from_turn = ?6 AND through_turn = ?7",
        params![
            lease.job.job_id,
            lease.owner,
            now,
            lease.job.attempt,
            lease.job.session_id,
            lease.job.range.from_turn,
            lease.job.range.through_turn
        ],
        |row| row.get::<_, u32>(0),
    )?;
    if matches == 1 {
        Ok(())
    } else {
        Err(V2CaptureError::StaleLease)
    }
}

fn persist_outcome(
    transaction: &rusqlite::Transaction<'_>,
    job: &CaptureJob,
    prepared: &PreparedOutcome,
    committed_at: i64,
) -> Result<()> {
    transaction.execute(
        "INSERT INTO capture_outcomes(job_id, outcome_kind, outcome_hash, committed_at)
         VALUES (?1, ?2, ?3, ?4)",
        params![job.job_id, prepared.kind, prepared.hash, committed_at],
    )?;
    for (ordinal, file) in prepared.files.iter().enumerate() {
        let path = file.path.to_str().ok_or_else(|| {
            V2CaptureError::Invalid("observation path is not valid UTF-8".to_owned())
        })?;
        transaction.execute(
            "INSERT INTO capture_observation_files(job_id, ordinal, path, content_hash)
             VALUES (?1, ?2, ?3, ?4)",
            params![job.job_id, ordinal, path, file.hash],
        )?;
    }
    transaction.execute(
        "UPDATE capture_jobs SET status = 'completed', lease_owner = NULL,
            lease_expires_at = NULL, last_error = NULL WHERE job_id = ?1",
        params![job.job_id],
    )?;
    advance_captured_cursor(transaction, &job.session_id)?;
    transaction.execute(
        "UPDATE meta
         SET value = CAST(CAST(value AS INTEGER) + 1 AS TEXT)
         WHERE key = 'capture_revision'",
        [],
    )?;
    Ok(())
}

fn read_capture_revision(connection: &rusqlite::Connection) -> Result<i64> {
    let revision = connection.query_row(
        "SELECT CAST(value AS INTEGER) FROM meta WHERE key = 'capture_revision'",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    if revision < 0 {
        return Err(V2CaptureError::Conflict(
            "capture revision cannot be negative".to_owned(),
        ));
    }
    Ok(revision)
}

fn advance_captured_cursor(
    transaction: &rusqlite::Transaction<'_>,
    session_id: &str,
) -> Result<()> {
    let mut cursor = transaction.query_row(
        "SELECT captured_cursor FROM capture_sessions WHERE session_id = ?1",
        params![session_id],
        |row| row.get::<_, u32>(0),
    )?;
    loop {
        let next = transaction
            .query_row(
                "SELECT MAX(through_turn) FROM capture_jobs
                 WHERE session_id = ?1 AND status = 'completed' AND from_turn <= ?2",
                params![session_id, cursor.saturating_add(1)],
                |row| row.get::<_, Option<u32>>(0),
            )?
            .unwrap_or(cursor);
        if next <= cursor {
            break;
        }
        cursor = next;
    }
    transaction.execute(
        "UPDATE capture_sessions SET captured_cursor = MAX(captured_cursor, ?2)
         WHERE session_id = ?1",
        params![session_id, cursor],
    )?;
    Ok(())
}

fn read_dir_bounded(path: &Path, limit: usize) -> Result<Vec<RecoveryDirectoryEntry>> {
    let entries = std::fs::read_dir(path).map_err(|source| io_error(path, source))?;
    let mut bounded = Vec::new();
    for (index, entry) in entries.enumerate() {
        if index == limit {
            return Err(V2CaptureError::RecoveryDirectoryLimit {
                path: path.to_path_buf(),
                limit,
            });
        }
        let entry = entry.map_err(|source| io_error(path, source))?;
        let file_type = entry
            .file_type()
            .map_err(|source| io_error(entry.path(), source))?;
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        bounded.push(RecoveryDirectoryEntry {
            path: entry.path(),
            name,
            is_file: file_type.is_file(),
        });
    }
    Ok(bounded)
}

fn recovery_ordinal(name: &str, prefix: &str) -> Result<usize> {
    name[prefix.len()..name.len() - 3]
        .parse::<usize>()
        .map_err(|_| {
            V2CaptureError::Conflict(format!("malformed deterministic observation path {name}"))
        })
}

fn persist_create_only(path: &Path, bytes: &[u8]) -> Result<()> {
    reject_symlink(path)?;
    if path.exists() {
        let existing = read_regular_bounded(path)?;
        return if existing == bytes {
            Ok(())
        } else {
            Err(V2CaptureError::Conflict(format!(
                "create-only observation {} already has different bytes",
                path.display()
            )))
        };
    }
    let parent = path.parent().ok_or_else(|| {
        V2CaptureError::Invalid("observation path does not have a parent".to_owned())
    })?;
    reject_symlink(parent)?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(parent).map_err(|source| io_error(path, source))?;
    temporary
        .write_all(bytes)
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|source| io_error(path, source))?;
    match temporary.persist_noclobber(path) {
        Ok(_) => Ok(()),
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            let existing = read_regular_bounded(path)?;
            if existing == bytes {
                Ok(())
            } else {
                Err(V2CaptureError::Conflict(format!(
                    "create-only observation {} raced with different bytes",
                    path.display()
                )))
            }
        }
        Err(error) => Err(io_error(path, error.error)),
    }
}

fn read_regular_bounded(path: &Path) -> Result<Vec<u8>> {
    reject_symlink(path)?;
    let metadata = std::fs::symlink_metadata(path).map_err(|source| io_error(path, source))?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_OBSERVATION_FILE_BYTES {
        return Err(V2CaptureError::Invalid(format!(
            "{} is not a bounded regular observation file",
            path.display()
        )));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    std::fs::File::open(path)
        .and_then(|mut file| {
            std::io::Read::by_ref(&mut file)
                .take(MAX_OBSERVATION_FILE_BYTES + 1)
                .read_to_end(&mut bytes)
        })
        .map_err(|source| io_error(path, source))?;
    if bytes.len() as u64 > MAX_OBSERVATION_FILE_BYTES {
        return Err(V2CaptureError::Invalid(
            "observation file exceeds its read limit".to_owned(),
        ));
    }
    Ok(bytes)
}

fn recovery_metadata(bytes: &[u8]) -> Result<RecoveryMetadata> {
    let content = std::str::from_utf8(bytes)
        .map_err(|_| V2CaptureError::Conflict("observation is not valid UTF-8".to_owned()))?;
    let field = |name: &str| {
        content
            .lines()
            .find_map(|line| line.strip_prefix(&format!("{name}: ")))
            .ok_or_else(|| V2CaptureError::Conflict(format!("observation omits {name}")))
    };
    if field("schema_version")? != "2" {
        return Err(V2CaptureError::Conflict(
            "observation has an unsupported schema version".to_owned(),
        ));
    }
    ObservationType::parse(field("type")?)
        .map_err(|error| V2CaptureError::Conflict(error.to_string()))?;
    let parse_json = |name: &str| {
        serde_json::from_str::<String>(field(name)?)
            .map_err(|_| V2CaptureError::Conflict(format!("observation has invalid {name}")))
    };
    let parse_number = |name: &str| {
        field(name)?
            .parse::<u32>()
            .map_err(|_| V2CaptureError::Conflict(format!("observation has invalid {name}")))
    };
    let job_id = parse_json("job_id")?;
    let session_id = parse_json("session_id")?;
    let range = CaptureRange::try_new(parse_number("from_turn")?, parse_number("through_turn")?)
        .map_err(|error| V2CaptureError::Conflict(error.to_string()))?;
    let ordinal = field("observation_ordinal")?
        .parse::<usize>()
        .map_err(|_| V2CaptureError::Conflict("observation has invalid ordinal".to_owned()))?;
    let count = field("observation_count")?
        .parse::<usize>()
        .map_err(|_| V2CaptureError::Conflict("observation has invalid count".to_owned()))?;
    Ok(RecoveryMetadata {
        job_id,
        session_id,
        range,
        ordinal,
        count,
    })
}

fn reject_symlink(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(V2CaptureError::Invalid(format!(
            "symbolic links are not allowed at {}",
            path.display()
        ))),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(io_error(path, source)),
    }
}

fn io_error(path: impl AsRef<Path>, source: std::io::Error) -> V2CaptureError {
    V2CaptureError::Io {
        path: path.as_ref().to_path_buf(),
        source,
    }
}

#[cfg(test)]
#[path = "v2_capture_tests.rs"]
mod tests;
