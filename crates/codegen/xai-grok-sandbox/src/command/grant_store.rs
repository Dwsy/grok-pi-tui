//! The expiring allow-list on disk: `<grok_home>/sessions/<enc(root)>/sandbox_grants.toml` for
//! workspace rows, `<grok_home>/sandbox_grants.toml` for global rows, memory for session rows
//! keyed by the hub session that gave them (call rows never reach the store).
//!
//! A write re-reads the file and replaces it under one advisory lock (a `.lock` sidecar, `flock`),
//! and memory takes the new rows only once the file is written, so two writers of one file (a
//! second daemon, another folder sharing the global file) cannot lose each other's rows or bring
//! back a revoked one. A file that cannot be read or parsed loads with no rows and is never
//! overwritten. Nothing the store keeps is followed: a symlink, a FIFO or another user's file in
//! a rows file's or the lock's place, or a directory between the grok home and them that is a
//! symlink or not the daemon user's, is refused ([`read_rows_text`], [`rows_dir`]).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::command::canonical::{ServedRoot, canonical_path, is_within};
use crate::command::git_config::GitConfigEnv;
use crate::command::grants::{Clock, Grant, GrantDecision, GrantId, GrantScope, GrantSubject};
use crate::command::policy::{PolicyError, symlink_free_spelling};
use crate::command::protected::{
    self, FileOwner, GLOBAL_GRANTS_FILENAME, HeldDir, Protected, lock_in_turn,
};
use crate::command::violation::propose::is_too_broad_anywhere;

#[derive(Debug, thiserror::Error)]
pub enum GrantError {
    #[error("grant subject is protected and can never be allowed: {path}")]
    Protected { path: PathBuf },
    #[error("grant subject path is not absolute: {path}")]
    NotAbsolute { path: PathBuf },
    #[error("grant subject is too broad to allow: {path}")]
    TooBroad { path: PathBuf },
    /// The subject's spelling has a symlink below its top-level component, or a component that
    /// cannot be inspected: the folder the user saw is not the one the row would open.
    #[error(transparent)]
    Root(PolicyError),
    #[error("no grant with id {id}")]
    NotFound { id: GrantId },
    #[error("a call-scoped grant lives with the call, not in the store")]
    CallScoped,
    #[error("a session grant is keyed by its hub session: use add_session")]
    SessionScoped,
    #[error("add_session takes a session grant, not {scope:?}")]
    NotSessionScoped { scope: GrantScope },
    #[error("grant file could not be serialized")]
    Serialize(#[from] toml::ser::Error),
    #[error("grant file at {path} could not be read; it is left as is")]
    Unreadable {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("grant file at {path} could not be parsed; it is left as is")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("io error at {context}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },
}

/// The `[[grant]]` array the file holds; unknown keys are ignored so a newer writer's file loads.
#[derive(Debug, Default, Serialize, Deserialize)]
struct GrantFile {
    #[serde(default, rename = "grant")]
    grants: Vec<Grant>,
}

/// `(mtime, len, inode)` of a file at the last read. The atomic writer replaces the inode on
/// every persist, so a same-tick rewrite of equal length on a coarse-mtime filesystem still
/// changes the signature.
type FileSignature = Option<(SystemTime, u64, u64)>;

/// A file's last commit. `signature`, the file's at the read or write committed, is locked by
/// every commit across the file I/O it reflects; `rows`, what was read or written (`None` for a
/// file that exists but cannot be read or parsed), is replaced under that lock but never held
/// across I/O, so a reader waits on no disk, even behind a commit whose caller went away.
#[derive(Clone)]
struct Commit {
    signature: Arc<Mutex<FileSignature>>,
    rows: Arc<Mutex<Option<Vec<Grant>>>>,
}

impl Commit {
    /// Replaces the committed state; `held` is `self.signature`, locked across the I/O.
    fn store(&self, held: &mut FileSignature, signature: FileSignature, rows: Option<Vec<Grant>>) {
        *lock_value(&self.rows) = rows;
        *held = signature;
    }
}

/// One rows file and its [`Commit`], which changes only on a blocking thread, together with the
/// read or write it reflects: a caller dropped while one runs (a revoke whose request went away)
/// cannot leave memory behind the disk.
#[derive(Clone)]
struct PersistedRows {
    path: PathBuf,
    /// The grok home the file lies under: every directory from it down to the file's is the
    /// daemon user's own ([`rows_dir`]).
    grok_home: PathBuf,
    /// How long an edit waits for the file's lock ([`GRANT_LOCK_WAIT`]).
    lock_wait: Duration,
    commit: Commit,
}

impl PersistedRows {
    async fn load(path: PathBuf, grok_home: PathBuf, rules: &RowRules, now: i64) -> PersistedRows {
        let signature = signature_of(&path).await;
        let rows = read_rows(&path, &grok_home, rules, now).await;
        PersistedRows {
            path,
            grok_home,
            lock_wait: GRANT_LOCK_WAIT,
            commit: Commit {
                signature: Arc::new(Mutex::new(signature)),
                rows: Arc::new(Mutex::new(rows)),
            },
        }
    }

    /// The unexpired rows as last committed, `None` while the file cannot be read.
    fn live(&self, now: i64) -> Option<Vec<Grant>> {
        let rows = lock_value(&self.commit.rows);
        let live = rows.as_ref()?.iter().filter(|g| g.is_live(now)).cloned();
        Some(live.collect())
    }

    /// One read-modify-write of the file under its lock ([`edit_rows_locked`]): the rows on disk
    /// are read afresh, `edit` changes them (returning whether it did), the expired go, the file
    /// is replaced and memory takes the new rows, all in the blocking task, which runs to the end
    /// even when this future is dropped. A failed write leaves memory equal to disk. Returns
    /// whether anything was written.
    async fn edit(
        &self,
        rules: &RowRules,
        now: i64,
        store: Store,
        edit: impl FnOnce(&mut Vec<Grant>) -> bool + Send + 'static,
    ) -> Result<bool, GrantError> {
        let (persisted, rules) = (self.clone(), rules.clone());
        let context = self.path.display().to_string();
        tokio::task::spawn_blocking(move || edit_rows_locked(&persisted, &rules, now, store, edit))
            .await
            .map_err(|e| GrantError::Io {
                context,
                source: std::io::Error::other(e),
            })?
    }

    /// Re-reads the file when its signature moved since the last commit, on a blocking thread
    /// holding the signature lock, so no edit's commit can land between the read and its store.
    async fn reload_if_changed(&self, rules: &RowRules, now: i64) -> bool {
        let (path, rules) = (self.path.clone(), rules.clone());
        let grok_home = self.grok_home.clone();
        let commit = self.commit.clone();
        let reload = tokio::task::spawn_blocking(move || {
            let mut held = lock_value(&commit.signature);
            let signature = signature_from(std::fs::symlink_metadata(&path));
            if signature == *held {
                return false;
            }
            let text = read_rows_text(&path, &grok_home);
            let rows = rows_or_none(&path, parse_rows(&path, text, &rules, now));
            commit.store(&mut held, signature, rows);
            true
        });
        match reload.await {
            Ok(changed) => changed,
            Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
            Err(_) => false,
        }
    }
}

/// Whether an edit may bring the file into being: an add does; a revoke finds nothing to remove in
/// a file that does not exist, and so creates no directory, lock or file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Store {
    Create,
    ExistingOnly,
}

/// What a row is checked against on add and at load ([`refused_subject`]): the floor, and the
/// user's home in its canonical spelling, which no row may grant whole.
#[derive(Clone)]
struct RowRules {
    protected: Vec<Protected>,
    user_home: Option<PathBuf>,
}

/// A poisoned lock still guards a whole value: a commit replaces each in one assignment.
fn lock_value<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A "for this conversation" row and the hub session that gave it; gone with the session.
struct SessionRow {
    session_id: String,
    grant: Grant,
}

pub struct GrantStore {
    workspace_root: PathBuf,
    rules: RowRules,
    workspace: PersistedRows,
    global: PersistedRows,
    session: Vec<SessionRow>,
    clock: Arc<dyn Clock>,
}

impl GrantStore {
    /// Loads the workspace and global files under `xai_grok_config::grok_home()`.
    /// `control_socket_dir` is the daemon's endpoint directory, part of the floor a loaded row is
    /// checked against; the floor reads the global git config where the daemon's environment
    /// puts it ([`GitConfigEnv::from_host`]).
    pub async fn open(
        workspace_root: &Path,
        control_socket_dir: &Path,
        clock: Arc<dyn Clock>,
    ) -> GrantStore {
        let grok_home = xai_grok_config::grok_home();
        let user_home = xai_dirs::home_dir();
        // The floor reads git metadata and walks directories: blocking work, off the runtime
        let (root, home, user, socket_dir) = (
            workspace_root.to_path_buf(),
            grok_home.clone(),
            user_home.clone(),
            control_socket_dir.to_path_buf(),
        );
        let build = move || {
            protected::floor(&protected::ProtectedInputs {
                workspace_root: &ServedRoot::pin(&root),
                grok_home: &home,
                user_home: user.as_deref(),
                control_socket_dir: &socket_dir,
                git_env: &GitConfigEnv::from_host(),
            })
        };
        let protected = match tokio::task::spawn_blocking(build.clone()).await {
            Ok(protected) => protected,
            Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
            // Cancelled by the runtime's shutdown: the floor is still owed, so build it here
            Err(_) => build(),
        };
        GrantStore::open_in(
            &grok_home,
            workspace_root,
            protected,
            user_home.as_deref(),
            clock,
        )
        .await
    }

    /// [`GrantStore::open`] with every path injected: `protected` is the floor a row is checked
    /// against, `user_home` the home no row may grant whole.
    pub async fn open_in(
        grok_home: &Path,
        workspace_root: &Path,
        protected: Vec<Protected>,
        user_home: Option<&Path>,
        clock: Arc<dyn Clock>,
    ) -> GrantStore {
        let workspace_path =
            xai_grok_config::sessions_cwd_dir_in(grok_home, &workspace_root.to_string_lossy())
                .join(GLOBAL_GRANTS_FILENAME);
        let global_path = grok_home.join(GLOBAL_GRANTS_FILENAME);
        let now = clock.now_unix();
        let rules = RowRules {
            protected,
            user_home: user_home.map(canonical_path),
        };
        GrantStore {
            workspace_root: workspace_root.to_path_buf(),
            workspace: PersistedRows::load(workspace_path, grok_home.to_path_buf(), &rules, now)
                .await,
            global: PersistedRows::load(global_path, grok_home.to_path_buf(), &rules, now).await,
            rules,
            session: Vec::new(),
            clock,
        }
    }

    pub fn workspace_file(&self) -> &Path {
        &self.workspace.path
    }

    pub fn global_file(&self) -> &Path {
        &self.global.path
    }

    /// Test seam: the lock wait of both files, for a test that holds a lock still and waits it out.
    #[cfg(test)]
    fn with_lock_wait(mut self, wait: Duration) -> GrantStore {
        self.workspace.lock_wait = wait;
        self.global.lock_wait = wait;
        self
    }

    /// Unexpired rows of every decision, session ∪ workspace ∪ global, across every session
    /// (listing).
    pub fn live(&self) -> Vec<Grant> {
        let now = self.clock.now_unix();
        let mut live: Vec<Grant> = self
            .session
            .iter()
            .map(|row| &row.grant)
            .filter(|g| g.is_live(now))
            .cloned()
            .collect();
        live.extend(self.workspace.live(now).unwrap_or_default());
        live.extend(self.global.live(now).unwrap_or_default());
        live
    }

    /// Live allow rows not covered by a live deny row, across every session: listing only. It
    /// drops the deny rows that cut a kept allow, so a policy built from it would leave that
    /// overlap open. A policy for one call is built from [`allows_not_denied`] over
    /// [`GrantStore::live_shared`] and that call's own session's rows
    /// ([`GrantStore::live_session_rows`]), never from this.
    pub fn live_allows(&self) -> Vec<Grant> {
        let mut rows = allows_not_denied(&self.live());
        rows.retain(|g| g.decision == GrantDecision::Allow);
        rows
    }

    /// Unexpired workspace and global rows of every decision: what every hub session in the
    /// folder shares. While either file cannot be read, the deny rows it holds are unknown and
    /// could cover an allow in the other, so only deny rows are shared until it loads.
    pub fn live_shared(&self) -> Vec<Grant> {
        let now = self.clock.now_unix();
        let (workspace, global) = (self.workspace.live(now), self.global.live(now));
        let readable = workspace.is_some() && global.is_some();
        let mut live: Vec<Grant> = workspace.into_iter().chain(global).flatten().collect();
        if !readable {
            live.retain(|g| g.decision == GrantDecision::Deny);
        }
        live
    }

    /// Unexpired session rows, each with the hub session that gave it: a "for this
    /// conversation" row applies to that session's calls only.
    pub fn live_session_rows(&self) -> Vec<(String, Grant)> {
        let now = self.clock.now_unix();
        self.session
            .iter()
            .filter(|row| row.grant.is_live(now))
            .map(|row| (row.session_id.clone(), row.grant.clone()))
            .collect()
    }

    /// Records a persisted grant (workspace or global scope). A workspace row is keyed on this
    /// store's root regardless of the `root` the scope names. The file is re-read under its lock
    /// before the row is appended and the row enters memory only after the file is written.
    ///
    /// # Errors
    /// [`GrantError::CallScoped`] for a call-scoped grant (it belongs to the call, never to the
    /// store); [`GrantError::SessionScoped`] for a session grant (see [`GrantStore::add_session`]);
    /// [`GrantError::Protected`] / [`GrantError::NotAbsolute`] for a subject in the floor or a
    /// relative path; [`GrantError::Io`] /
    /// [`GrantError::Serialize`] when the file cannot be written, [`GrantError::Unreadable`] /
    /// [`GrantError::Parse`] when the file on disk cannot be read (the row is then not recorded).
    pub async fn add(&mut self, mut grant: Grant) -> Result<GrantId, GrantError> {
        self.check_subject(&grant.subject)?;
        grant.subject = canonical_subject(grant.subject);
        let id = grant.id.clone();
        let now = self.clock.now_unix();
        // As at load: a row dated ahead of the clock would outlive its TTL by the difference
        grant.granted_at = grant.granted_at.min(now);
        match &grant.scope {
            GrantScope::Call => return Err(GrantError::CallScoped),
            GrantScope::Session => return Err(GrantError::SessionScoped),
            GrantScope::Workspace { .. } => {
                self.workspace
                    .edit(&self.rules, now, Store::Create, move |rows| {
                        rows.push(grant);
                        true
                    })
                    .await?;
            }
            GrantScope::Global => {
                self.global
                    .edit(&self.rules, now, Store::Create, move |rows| {
                        rows.push(grant);
                        true
                    })
                    .await?;
            }
        }
        Ok(id)
    }

    /// Records a session grant ("for this conversation") in memory, keyed by the hub session that
    /// gave it: [`GrantStore::clear_session`] drops it with that session.
    ///
    /// # Errors
    /// [`GrantError::NotSessionScoped`] for any other scope; [`GrantError::Protected`] for a
    /// subject in the floor.
    pub fn add_session(
        &mut self,
        session_id: &str,
        mut grant: Grant,
    ) -> Result<GrantId, GrantError> {
        if grant.scope != GrantScope::Session {
            return Err(GrantError::NotSessionScoped { scope: grant.scope });
        }
        self.check_subject(&grant.subject)?;
        grant.subject = canonical_subject(grant.subject);
        grant.granted_at = grant.granted_at.min(self.clock.now_unix());
        let id = grant.id.clone();
        self.session.push(SessionRow {
            session_id: session_id.to_owned(),
            grant,
        });
        Ok(id)
    }

    /// The floor check `add` and `add_session` apply, for a caller that keeps a row outside the
    /// store (a call-scoped grant consumed by one replay). The floor and the breadth cap meet
    /// the subject's path in its canonical spelling too; the symlink check meets it as given.
    ///
    /// # Errors
    /// [`GrantError::Protected`] when the subject names a protected path;
    /// [`GrantError::NotAbsolute`] when it names a relative one; [`GrantError::Root`] when it
    /// passes through a symlink; [`GrantError::TooBroad`] when it resolves to a folder no card
    /// may offer.
    pub fn check_subject(&self, subject: &GrantSubject) -> Result<(), GrantError> {
        refused_subject(subject, &self.rules).map_or(Ok(()), Err)
    }

    /// Removes one row wherever it lives. Each file is re-read under its lock before the row is
    /// looked up, so a row another writer added meanwhile survives and a row it already removed
    /// is `NotFound`.
    ///
    /// # Errors
    /// [`GrantError::NotFound`] when no live or expired row has `id` in a readable file; a persist
    /// error otherwise (the row then stays).
    pub async fn revoke(&mut self, id: &GrantId) -> Result<(), GrantError> {
        let before = self.session.len();
        self.session.retain(|row| row.grant.id != *id);
        if self.session.len() != before {
            return Ok(());
        }
        let now = self.clock.now_unix();
        for file in [&self.workspace, &self.global] {
            let id = id.clone();
            match file
                .edit(&self.rules, now, Store::ExistingOnly, move |rows| {
                    remove_by_id(rows, &id)
                })
                .await
            {
                Ok(true) => return Ok(()),
                Ok(false) => {}
                // A file that loads as empty holds no row this store ever listed
                Err(error @ (GrantError::Unreadable { .. } | GrantError::Parse { .. })) => {
                    tracing::warn!(%error, "revoke skipped an unreadable sandbox grants file");
                }
                Err(error) => return Err(error),
            }
        }
        Err(GrantError::NotFound { id: id.clone() })
    }

    /// Re-reads a file whose signature changed since the last read (the desktop's revoke, another
    /// daemon's grant). Returns whether anything was reloaded.
    pub async fn reload_if_changed(&mut self) -> bool {
        let now = self.clock.now_unix();
        let mut changed = false;
        for file in [&self.workspace, &self.global] {
            changed |= file.reload_if_changed(&self.rules, now).await;
        }
        changed
    }

    /// A hub session ended: its "for this conversation" rows are gone. Returns how many were
    /// dropped. Rows of other sessions and the persisted files are untouched.
    pub fn clear_session(&mut self, session_id: &str) -> usize {
        let before = self.session.len();
        self.session.retain(|row| row.session_id != session_id);
        before - self.session.len()
    }

    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }
}

fn remove_by_id(rows: &mut Vec<Grant>, id: &GrantId) -> bool {
    let before = rows.len();
    rows.retain(|g| g.id != *id);
    rows.len() != before
}

/// Why a subject can never be a row: its path is relative (no policy can apply it, so one such
/// row would refuse every command), lies in the floor or at or beneath a glob entry's literal
/// prefix ([`protected::is_ungrantable`]), passes through a symlink below its top-level
/// component as spelled, or resolves to a folder too broad for any card
/// ([`is_too_broad_anywhere`]). The store takes rows from the wire, not only from its own cards.
fn refused_subject(subject: &GrantSubject, rules: &RowRules) -> Option<GrantError> {
    match subject {
        GrantSubject::FsWriteRoot { root } | GrantSubject::FsRead { root } => {
            if !root.is_absolute() {
                Some(GrantError::NotAbsolute { path: root.clone() })
            } else if protected::is_ungrantable(root, &rules.protected) {
                Some(GrantError::Protected { path: root.clone() })
            } else if let Err(error) = symlink_free_spelling(root) {
                Some(GrantError::Root(error))
            } else {
                let canonical = canonical_path(root);
                is_too_broad_anywhere(&canonical, rules.user_home.as_deref())
                    .then_some(GrantError::TooBroad { path: canonical })
            }
        }
        GrantSubject::NetHost { .. } | GrantSubject::BuildCaches => None,
    }
}

/// A path subject in its canonical spelling (canonicalised once at the grant
/// boundary); the other subjects unchanged.
pub fn canonical_subject(subject: GrantSubject) -> GrantSubject {
    match subject {
        GrantSubject::FsWriteRoot { root } => GrantSubject::FsWriteRoot {
            root: canonical_path(&root),
        },
        GrantSubject::FsRead { root } => GrantSubject::FsRead {
            root: canonical_path(&root),
        },
        other => other,
    }
}

/// Whether a deny subject shadows an allow subject: same kind and, for a path, either tree holds
/// the other (case-insensitively on macOS); for a host, the deny's pattern covers the allow's.
fn subject_covers(deny: &GrantSubject, allow: &GrantSubject) -> bool {
    match (deny, allow) {
        // The policy has no per-grant exclusion: a broader allow would keep the denied tree open
        (GrantSubject::FsWriteRoot { root: d }, GrantSubject::FsWriteRoot { root: a })
        | (GrantSubject::FsRead { root: d }, GrantSubject::FsRead { root: a }) => {
            is_within(a, d) || is_within(d, a)
        }
        (
            GrantSubject::NetHost {
                host: d,
                port: d_port,
            },
            GrantSubject::NetHost {
                host: a,
                port: a_port,
            },
        ) => a.is_covered_by(d) && (d_port.is_none() || d_port == a_port),
        (GrantSubject::BuildCaches, GrantSubject::BuildCaches) => true,
        _ => false,
    }
}

/// Whether a deny subject overlaps an allow subject it does not cover, so the deny travels with
/// the allow ([`allows_not_denied`]): host patterns either way with ports that meet, where the
/// decider refuses the overlap and the rest of the allow holds; a path write root against the
/// build-cache family either way, which only the policy can judge (it knows the trees).
fn subject_cuts(deny: &GrantSubject, allow: &GrantSubject) -> bool {
    match (deny, allow) {
        (
            GrantSubject::NetHost {
                host: d,
                port: d_port,
            },
            GrantSubject::NetHost {
                host: a,
                port: a_port,
            },
        ) => {
            (a.is_covered_by(d) || d.is_covered_by(a))
                && (d_port.is_none() || a_port.is_none() || d_port == a_port)
        }
        (GrantSubject::FsWriteRoot { .. }, GrantSubject::BuildCaches)
        | (GrantSubject::BuildCaches, GrantSubject::FsWriteRoot { .. }) => true,
        _ => false,
    }
}

async fn signature_of(path: &Path) -> FileSignature {
    signature_from(tokio::fs::symlink_metadata(path).await)
}

/// Of the rows file itself, never a symlink's target: the read refuses one, so a link retargeted
/// behind it changes nothing it could load.
fn signature_from(meta: std::io::Result<std::fs::Metadata>) -> FileSignature {
    let meta = meta.ok()?;
    Some((meta.modified().ok()?, meta.len(), inode_of(&meta)))
}

#[cfg(unix)]
fn inode_of(meta: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt as _;
    meta.ino()
}

#[cfg(not(unix))]
fn inode_of(_meta: &std::fs::Metadata) -> u64 {
    0
}

/// Largest grants file read. Far above any real allow-list; a file past it is refused like an
/// unreadable one, so a corrupt or hostile file cannot make the daemon allocate without bound.
const MAX_GRANTS_FILE_BYTES: u64 = 1024 * 1024;

/// Rows from one file for memory ([`rows_or_none`]).
async fn read_rows(
    path: &Path,
    grok_home: &Path,
    rules: &RowRules,
    now: i64,
) -> Option<Vec<Grant>> {
    let (owned, home) = (path.to_path_buf(), grok_home.to_path_buf());
    let text = match tokio::task::spawn_blocking(move || read_rows_text(&owned, &home)).await {
        Ok(text) => text,
        Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
        Err(error) => Err(std::io::Error::other(error)),
    };
    rows_or_none(path, parse_rows(path, text, rules, now))
}

/// A file [`parse_rows`] refuses is logged and holds no rows, `None` (fail closed: no grant is
/// worse than a forged one, and [`GrantStore::live_shared`] then shares no allow at all).
fn rows_or_none(path: &Path, rows: Result<Vec<Grant>, GrantError>) -> Option<Vec<Grant>> {
    rows.inspect_err(|error| {
        tracing::warn!(path = %path.display(), error = %error, "failed loading sandbox grants; ignoring file");
    })
    .ok()
}

/// A rows file's text, blocking, up to [`MAX_GRANTS_FILE_BYTES`], as a file the daemon keeps: a
/// symlink a command left in its place while the folder was `off`, into a folder it may still
/// write once the folder enforces, is refused rather than read, as is a FIFO or another user's
/// file. It is read through its directories' handles ([`rows_dir`]).
fn read_rows_text(path: &Path, grok_home: &Path) -> std::io::Result<String> {
    rows_dir(grok_home, path, false)?.read(
        file_name(path),
        MAX_GRANTS_FILE_BYTES,
        FileOwner::Daemon,
    )
}

/// The roots a command of any folder served from `grok_home` may have been let write, as the
/// grok home records them, canonical: the sessions tree (each folder's command directory lies in
/// it), every folder a session directory there names, and every write root in the global grants
/// file and each folder's, expired rows, deny rows and row kinds this version does not know
/// included. Only real directories under `sessions` are folders' session directories: a
/// symlinked one is refused when its folder is served, so its grants never applied.
///
/// # Errors
/// The sessions tree cannot be listed, or a grants file is there but cannot be read or is not
/// TOML: what it granted is unknown.
pub fn recorded_write_roots(grok_home: &Path) -> std::io::Result<Vec<PathBuf>> {
    let sessions = grok_home.join("sessions");
    let mut roots = vec![canonical_path(&sessions)];
    let mut files = vec![grok_home.join(GLOBAL_GRANTS_FILENAME)];
    match std::fs::read_dir(&sessions) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry?;
                if !entry.file_type()?.is_dir() {
                    continue;
                }
                let dir = entry.path();
                if let Some(cwd) = xai_grok_config::decode_cwd_from_dirname(&dir) {
                    roots.push(canonical_path(Path::new(&cwd)));
                }
                files.push(dir.join(GLOBAL_GRANTS_FILENAME));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    for file in files {
        let text = match read_rows_text(&file, grok_home) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        let table: toml::Table = toml::from_str(&text).map_err(|error| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{} is not TOML: {}", file.display(), error.message()),
            )
        })?;
        let rows = table.get("grant").and_then(toml::Value::as_array);
        roots.extend(
            rows.into_iter()
                .flatten()
                .filter_map(|row| row.get("subject"))
                .filter(|subject| {
                    subject.get("kind").and_then(toml::Value::as_str) == Some("fs_write_root")
                })
                .filter_map(|subject| subject.get("root").and_then(toml::Value::as_str))
                .map(|root| canonical_path(Path::new(root))),
        );
    }
    Ok(roots)
}

/// The directory a rows file sits in, held from the grok home down ([`HeldDir`]): each directory on
/// the way must be the daemon user's and no symlink, which an unconfined command could have left
/// pointing into a folder it may still write once the folder enforces.
fn rows_dir(grok_home: &Path, path: &Path, create: bool) -> std::io::Result<HeldDir> {
    HeldDir::open(
        grok_home,
        path.parent().unwrap_or(path),
        FileOwner::Daemon,
        create,
    )
}

fn file_name(path: &Path) -> &std::ffi::OsStr {
    path.file_name().unwrap_or_default()
}

/// Rows from one file's text; a missing file is empty. An unreadable, refused, oversized or
/// unparsable file (including one with a subject kind this version does not know) is an error.
/// Rows whose subject is protected or a relative path are dropped with one warning; a
/// `granted_at` in the future is clamped to `now` so a TTL row cannot outlive its duration.
fn parse_rows(
    path: &Path,
    text: std::io::Result<String>,
    rules: &RowRules,
    now: i64,
) -> Result<Vec<Grant>, GrantError> {
    let io = |source: std::io::Error| GrantError::Unreadable {
        path: path.to_path_buf(),
        source,
    };
    let text = match text {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(io(e)),
    };
    let file: GrantFile = toml::from_str(&text).map_err(|source| GrantError::Parse {
        path: path.to_path_buf(),
        source,
    })?;
    // A row with a subject this version does not know (a `cmd_prefix` from an older file) fails
    // the parse above: the file loads as empty and no write replaces it
    // Checked as spelled on disk, so a root since swapped for a symlink is dropped, not followed
    let (kept, dropped): (Vec<Grant>, Vec<Grant>) = file
        .grants
        .into_iter()
        .partition(|g| refused_subject(&g.subject, rules).is_none());
    let kept: Vec<Grant> = kept
        .into_iter()
        .map(|mut g| {
            g.subject = canonical_subject(g.subject);
            g
        })
        .collect();
    if !dropped.is_empty() {
        tracing::warn!(
            path = %path.display(),
            dropped = dropped.len(),
            "dropped sandbox grants whose subject is protected, not absolute, symlinked or too broad"
        );
    }
    Ok(kept
        .into_iter()
        .map(|mut g| {
            if g.granted_at > now {
                tracing::warn!(id = %g.id, granted_at = g.granted_at, now, "sandbox grant dated in the future; clamping to now");
                g.granted_at = now;
            }
            g
        })
        .collect())
}

/// The rows a policy is built from: the allow rows among `live` that no deny row among them
/// covers ([`subject_covers`]), then the deny rows that overlap a kept allow without covering it
/// ([`subject_cuts`]), so the deny wins for the overlap and the rest of the allow holds. A port
/// deny under a host-wide allow is the decider's to apply (host allowed, minus that port); a path
/// deny against the build-cache family, either way, is closed by [`SandboxPolicy::build`].
///
/// [`SandboxPolicy::build`]: crate::command::policy::SandboxPolicy::build
pub fn allows_not_denied(live: &[Grant]) -> Vec<Grant> {
    let (denies, allows): (Vec<&Grant>, Vec<&Grant>) =
        live.iter().partition(|g| g.decision == GrantDecision::Deny);
    let kept: Vec<&Grant> = allows
        .into_iter()
        .filter(|a| {
            !denies
                .iter()
                .any(|d| subject_covers(&d.subject, &a.subject))
        })
        .collect();
    let cutting: Vec<&Grant> = denies
        .into_iter()
        .filter(|d| kept.iter().any(|a| subject_cuts(&d.subject, &a.subject)))
        .collect();
    kept.into_iter().chain(cutting).cloned().collect()
}

/// How long a grant-file edit waits for the file's lock. Commands cannot open the sidecar
/// ([`protected::GRANTS_LOCK_FILENAME`] is in the floor and read-denied), but any other process
/// can hold it: an edit fails rather than stalling the store and every tool call behind it.
const GRANT_LOCK_WAIT: Duration = Duration::from_secs(5);

/// The advisory lock every writer of `path` takes: a sidecar beside the file, because the file
/// itself is replaced by rename on every write and a lock on the old inode would not travel.
fn lock_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(std::ffi::OsStr::to_os_string)
        .unwrap_or_default();
    name.push(".lock");
    path.with_file_name(name)
}

/// The read-modify-write of one rows file under its lock, on a blocking thread: lock, read the
/// rows on disk, `edit` them, drop the expired, replace the file atomically and owner-only (the
/// file holds what the user allowed, so a torn or world-readable copy must never exist;
/// [`HeldDir::replace`]), and commit the new rows and signature to its [`Commit`], whose signature
/// lock is held across the write so no reload stores the old file's rows over the new ones.
/// `false` when `edit` changed nothing (nothing is written; memory still takes the rows read). A
/// file on disk that cannot be read or parsed is an error before `edit` runs, so its rows are
/// never replaced (a symlink in its place included: it is neither followed nor replaced). Under
/// [`Store::ExistingOnly`] a missing file is `false` before anything is created. The directories
/// are walked and created by handle ([`rows_dir`]), and the lock, the read and the replacement go
/// through the last one, so a directory swapped for a symlink after the walk redirects none.
///
/// `flock` is per open file description, so two stores in one daemon contend on it exactly as
/// two daemons do.
///
/// The lock is taken in turn ([`lock_in_turn`]): another store of this daemon's edits following
/// back to back, each re-taking the lock the moment the last released it, let this one in at the
/// next release; another daemon's contend on the poll, within the same wait.
fn edit_rows_locked(
    persisted: &PersistedRows,
    rules: &RowRules,
    now: i64,
    store: Store,
    edit: impl FnOnce(&mut Vec<Grant>) -> bool,
) -> Result<bool, GrantError> {
    let (path, commit) = (persisted.path.as_path(), &persisted.commit);
    let io = |source: std::io::Error| GrantError::Io {
        context: path.display().to_string(),
        source,
    };
    if store == Store::ExistingOnly
        && std::fs::symlink_metadata(path)
            .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
    {
        return Ok(false);
    }
    let dir = rows_dir(&persisted.grok_home, path, true).map_err(io)?;
    let lock = dir.open_lock(file_name(&lock_path(path))).map_err(io)?;
    lock_in_turn(&lock, persisted.lock_wait).map_err(io)?;
    let result = (|| {
        let signature = signature_from(std::fs::symlink_metadata(path));
        let text = dir.read(file_name(path), MAX_GRANTS_FILE_BYTES, FileOwner::Daemon);
        let mut rows = parse_rows(path, text, rules, now)?;
        if !edit(&mut rows) {
            // A row another writer removed leaves memory with the rest; the signature predates
            // the read, so a write racing it is reloaded later
            commit.store(&mut lock_value(&commit.signature), signature, Some(rows));
            return Ok(false);
        }
        rows.retain(|g| g.is_live(now));
        let file = GrantFile { grants: rows };
        let contents = toml::to_string_pretty(&file)?;
        let mut held = lock_value(&commit.signature);
        dir.replace(file_name(path), &contents, FileOwner::Daemon)
            .map_err(io)?;
        let signature = signature_from(std::fs::symlink_metadata(path));
        commit.store(&mut held, signature, Some(file.grants));
        Ok(true)
    })();
    // The OS releases the lock with the descriptor either way
    let _ = fs2::FileExt::unlock(&lock);
    result
}

#[cfg(test)]
#[path = "grant_store_tests.rs"]
mod tests;
