//! Folder-trust store ("do you trust this folder?").
//!
//! Persists per-folder trust decisions to `~/.grok/trusted_folders.toml`.
//! This is the durable backing store for the VS-Code-style folder-trust gate that decides whether repo-local MCP / LSP servers may spawn.
//! Those servers run arbitrary commands from repo-controlled config files.
//!
//! TOML shape:
//! ```toml
//! [folders."/abs/repo/root"]
//! trusted = true
//! decided_at = 1780000000
//! ```
//!
//! A recorded grant covers that workspace key and descendants that still resolve to the same git root ([`workspace_key`]).
//! A nearer recorded decision wins.
//! Other workspace keys under the path, including nested git roots, are not covered.
//! The persisted file is written atomically with owner-only (`0600`) permissions.
//!
//! The store is rooted at a fresh [`xai_dirs::resolve_grok_home`], never `grok_home()` or a cwd-relative `./.grok`.
//! Home is `None` when `$GROK_HOME` and the user home are unset, or when the resolved home is relative.
//! In that no-home environment [`TrustStore::load`] yields an empty store that trusts nothing and persists nothing.
//! So a cloned repo can never ship a `./.grok/trusted_folders.toml` that self-trusts its own checkout (fail closed).

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Filename of the folder-trust store under `~/.grok/`.
pub const TRUST_FILE_NAME: &str = xai_grok_config::TRUSTED_FOLDERS_FILENAME;

/// A single folder's trust record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FolderTrust {
    /// Whether the folder (and same-repo descendants) is trusted.
    pub trusted: bool,
    /// Unix timestamp (seconds) of when the decision was recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_at: Option<i64>,
}

/// On-disk document shape for `trusted_folders.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct TrustDocument {
    #[serde(default)]
    folders: BTreeMap<String, FolderTrust>,
}

/// A failed read is not represented here.
enum StoreRead {
    /// File absent. Safe to create.
    Missing,
    /// Read and parsed (including empty/whitespace as an empty map).
    Document(TrustDocument),
}

impl StoreRead {
    fn into_document(self) -> TrustDocument {
        match self {
            Self::Missing => TrustDocument::default(),
            Self::Document(doc) => doc,
        }
    }
}

#[derive(Debug)]
pub enum TrustPersistError {
    /// Read or parse failed; nothing published.
    Unreadable(io::Error),
    /// Document was read (or missing) but the publish failed.
    Publish(io::Error),
}

impl TrustPersistError {
    pub fn as_io(&self) -> &io::Error {
        match self {
            Self::Unreadable(e) | Self::Publish(e) => e,
        }
    }
}

impl From<TrustPersistError> for io::Error {
    fn from(err: TrustPersistError) -> Self {
        match err {
            TrustPersistError::Unreadable(e) | TrustPersistError::Publish(e) => e,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recorded {
    Durable,
    /// Unsafe root or no backing path: nothing written.
    Skipped,
}

/// Persisted set of trusted folders. `path` is `None` only in a no-home environment (see [`TrustStore::load`]): such a store holds no folders, trusts nothing, and persists nothing.
#[derive(Debug, Clone)]
pub struct TrustStore {
    doc: TrustDocument,
    /// Backing file, or `None` when no user home resolves; such a store trusts nothing and persists nothing.
    /// Never a cwd-relative path.
    path: Option<PathBuf>,
    /// False when a backing file could not be read or parsed. Not an empty document.
    disk_readable: bool,
}

impl TrustStore {
    /// Load the trust store from a fresh user-home resolve (never the `grok_home()` OnceLock). When no user home resolves (see the module-level fail-closed note) the path is `None` and this returns an [`Self::empty`] store.
    pub fn load() -> Self {
        match Self::default_path() {
            Some(path) => Self::load_from(path),
            None => Self::empty(),
        }
    }

    /// Load from a custom path (for tests).
    pub fn load_from(path: PathBuf) -> Self {
        match Self::read_doc_strict(&path) {
            Ok(read) => Self {
                doc: read.into_document(),
                path: Some(path),
                disk_readable: true,
            },
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "folder trust: failed to read trust store; leaving file untouched"
                );
                Self {
                    doc: TrustDocument::default(),
                    path: Some(path),
                    disk_readable: false,
                }
            }
        }
    }

    /// An empty store with no backing path: trusts nothing and persists nothing.
    /// Used for the no-home environment where [`Self::default_path`] resolves to `None`.
    /// `disk_readable` is true so a missing home is not confused with a corrupt file.
    pub fn empty() -> Self {
        Self {
            doc: TrustDocument::default(),
            path: None,
            disk_readable: true,
        }
    }

    pub fn disk_readable(&self) -> bool {
        self.disk_readable
    }

    /// Whether a backing file path exists (false for the no-home empty store).
    pub fn has_store_path(&self) -> bool {
        self.path.is_some()
    }

    /// `<user_grok_home>/trusted_folders.toml`, or `None` when no absolute home resolves.
    pub fn default_path() -> Option<PathBuf> {
        Self::default_path_in(trust_store_home())
    }

    /// Relative home is `None`: joining it would write a cwd-relative store.
    fn default_path_in(user_grok_home: Option<PathBuf>) -> Option<PathBuf> {
        let home = user_grok_home?;
        if !home.is_absolute() {
            return None;
        }
        Some(home.join(TRUST_FILE_NAME))
    }

    /// Whether `key` is trusted, per the MOST-SPECIFIC recorded decision that applies to this workspace. A descendant with its own workspace key is not covered.
    /// The query key is canonicalized here, so callers need not pre-canonicalize (symmetric with [`Self::set_trusted`]).
    pub fn is_trusted(&self, key: &Path) -> bool {
        // An unreadable store is not an empty allow-list; fail closed.
        if !self.disk_readable {
            return false;
        }
        let query = canonicalize_or_owned(key);
        let query_id = workspace_id(&query);
        // Longest covering match decides. Canonical keys are unique; a hand-edited store can still tie on non-canonical aliases
        // On a tie every tied record must be trusted, so a contradictory edit fails closed
        let mut best_depth: Option<usize> = None;
        let mut trusted = false;
        for (folder, record) in &self.doc.folders {
            let folder = Path::new(folder);
            if is_unsafe_trust_root(folder) || !query.starts_with(folder) {
                continue;
            }
            if workspace_id(folder) != query_id {
                continue;
            }
            let depth = folder.components().count();
            match best_depth {
                Some(d) if depth < d => {}
                Some(d) if depth == d => trusted &= record.trusted,
                _ => {
                    best_depth = Some(depth);
                    trusted = record.trusted;
                }
            }
        }
        trusted
    }

    /// Record `workspace_key` as **trusted** and persist to disk. Such a path therefore fails closed (it won't match on lookup) rather than over-trusting.
    pub fn set_trusted(&mut self, workspace_key: &Path) -> io::Result<()> {
        self.record_decision(workspace_key, true)
    }

    /// Record `workspace_key` as **untrusted** ("Never" / explicitly declined) and persist to disk. to avoid re-prompting).
    pub fn set_untrusted(&mut self, workspace_key: &Path) -> io::Result<()> {
        self.record_decision(workspace_key, false)
    }

    /// Number of recorded folders (for diagnostics / tests).
    pub fn len(&self) -> usize {
        self.doc.folders.len()
    }

    /// Whether the store has no recorded folders.
    pub fn is_empty(&self) -> bool {
        self.doc.folders.is_empty()
    }

    /// Whether `workspace_key` has an EXACT recorded decision (trusted OR untrusted); the cascade does not apply.
    /// Used by the legacy-hook-trust migration to avoid overriding a folder the user has already decided on.
    pub fn has_decision(&self, workspace_key: &Path) -> bool {
        let canonical = canonicalize_or_owned(workspace_key);
        self.doc
            .folders
            .contains_key(canonical.to_string_lossy().as_ref())
    }

    // ── Internal ──────────────────────────────────────────────────────

    /// Shared write path for [`Self::set_trusted`] / [`Self::set_untrusted`]. With no backing path (no-home environment) it likewise warns and returns `Ok(())`, so it never writes a cwd-relative file.
    /// Otherwise it performs a locked read-modify-write-commit: 1.
    fn record_decision(&mut self, workspace_key: &Path, trusted: bool) -> io::Result<()> {
        self.record_decision_strict(workspace_key, trusted)?;
        Ok(())
    }

    /// Locked RMW. A failed re-read is `Err` and does not persist. Memory commits only after a durable write.
    pub fn record_decision_strict(
        &mut self,
        workspace_key: &Path,
        trusted: bool,
    ) -> Result<Recorded, TrustPersistError> {
        let canonical = canonicalize_or_owned(workspace_key);
        if is_unsafe_trust_root(&canonical) {
            tracing::warn!(
                path = %canonical.display(),
                trusted,
                "folder trust: refusing to record an over-broad root (home, filesystem root, or non-absolute path); nothing recorded"
            );
            return Ok(Recorded::Skipped);
        }

        // No backing file (no-home env): record nothing. Callers must not treat this as a durable grant.
        let Some(path) = self.path.clone() else {
            tracing::warn!(
                path = %canonical.display(),
                trusted,
                "folder trust: no user grok home resolved; trust decision not recorded"
            );
            return Ok(Recorded::Skipped);
        };

        // Confirm a readable document (or missing file) before setup errors count as publish.
        if !self.disk_readable {
            match Self::read_doc_strict(&path) {
                Ok(_) => {}
                Err(e) => return Err(TrustPersistError::Unreadable(e)),
            }
        }

        // The lock file lives beside the store, so ensure the dir exists first.
        let parent = path.parent().ok_or_else(|| {
            TrustPersistError::Publish(io::Error::new(
                io::ErrorKind::InvalidInput,
                "trust store path has no parent",
            ))
        })?;
        if let Err(e) = std::fs::create_dir_all(parent) {
            return Err(TrustPersistError::Publish(e));
        }

        // Serialize cross-process writers for the whole read-modify-write so a concurrent peer's records are preserved, not clobbered
        let _lock = match ExclusiveLock::acquire(&path.with_extension("toml.lock")) {
            Ok(lock) => lock,
            Err(e) => return Err(TrustPersistError::Publish(e)),
        };

        // Re-read under the lock. A failed read is not an empty document and must not be written back.
        let mut doc = match Self::read_doc_strict(&path) {
            Ok(read) => read.into_document(),
            Err(e) => return Err(TrustPersistError::Unreadable(e)),
        };
        doc.folders.insert(
            canonical.to_string_lossy().to_string(),
            FolderTrust {
                trusted,
                decided_at: now_unix(),
            },
        );

        // Commit to memory only after a successful durable write, so a failure leaves the in-memory store unchanged
        if let Err(e) = Self::persist_doc(&path, &doc) {
            return Err(TrustPersistError::Publish(e));
        }
        self.doc = doc;
        self.disk_readable = true;
        Ok(Recorded::Durable)
    }

    /// Strict read: a genuine absent entry (`symlink_metadata` `NotFound` / `NotADirectory`) and a successfully read empty/whitespace file are empty documents.
    /// A symlink (even dangling) is never `Missing`: follow-time `NotFound` from `read_to_string` must not let persist replace the link.
    fn read_doc_strict(path: &Path) -> Result<StoreRead, io::Error> {
        // Probe without following so a dangling symlink is an existing entry, not Missing.
        let link_meta = match std::fs::symlink_metadata(path) {
            Ok(meta) => meta,
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                ) =>
            {
                return Ok(StoreRead::Missing);
            }
            Err(e) => return Err(e),
        };
        let is_symlink = link_meta.file_type().is_symlink();

        let contents = match std::fs::read_to_string(path) {
            Ok(c) if c.trim().is_empty() => {
                return Ok(StoreRead::Document(TrustDocument::default()));
            }
            Ok(c) => c,
            Err(e)
                if !is_symlink
                    && matches!(
                        e.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                    ) =>
            {
                return Ok(StoreRead::Missing);
            }
            Err(e) => return Err(e),
        };
        match toml::from_str::<TrustDocument>(&contents) {
            Ok(doc) => Ok(StoreRead::Document(doc)),
            Err(e) => Err(io::Error::new(io::ErrorKind::InvalidData, e)),
        }
    }

    /// Write `doc` to `path` atomically (unique temp, fsync, rename) with owner-only (`0600`) permissions. Uses a unique temp file in the destination directory so concurrent writers never share a temp path.
    /// `persist` performs an atomic replace, including over an existing destination on Windows.
    fn persist_doc(path: &Path, doc: &TrustDocument) -> io::Result<()> {
        use std::io::Write;

        let parent = path.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "trust store path has no parent",
            )
        })?;
        std::fs::create_dir_all(parent)?;

        let body = toml::to_string_pretty(doc)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        // Unique temp in the same directory (atomic rename requires same FS).
        let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
        tmp.write_all(body.as_bytes())?;
        // Durably flush to disk before publishing so a crash can't leave a zero-length or stale store behind
        // (`File::flush` is a no-op for durability; `sync_all` is what guarantees the bytes hit disk.)
        tmp.as_file().sync_all()?;
        // Atomic publish.
        tmp.persist(path).map_err(|e| e.error)?;
        Ok(())
    }
}

/// Compute the trust **workspace key** for a working directory. The key is the canonicalized git repository root when `cwd` is inside a repo (trust applies to the whole repo), otherwise the canonicalized `cwd`.
/// A grok-managed worktree first collapses onto its recorded source repo's git ROOT (via the `~/.grok/worktrees.db` registry), so every `grok -w` worktree shares one trust key regardless of creation mode (including standalone clones that git can't link back to their source) and regardless of the subdir `grok -w` was launched from (the recorded source repo may be a repo subdir).
pub fn workspace_key(cwd: &Path) -> PathBuf {
    let key = git_derived_workspace_key(cwd);
    if is_unsafe_trust_root(&key) {
        return canonicalize_or_owned(cwd);
    }
    key
}

/// The workspace key derived from git topology, before [`workspace_key`] rejects an over-broad root in favor of the cwd.
fn git_derived_workspace_key(cwd: &Path) -> PathBuf {
    // A grok-managed worktree (any creation mode, incl. standalone clones git can't link) collapses onto its recorded source repo so trust is shared.
    if let Some(source_repo) = crate::worktree::source_repo_for_cwd(&cwd.to_string_lossy()) {
        // Key on the source repo's git ROOT so every worktree of one repo shares ONE key regardless of the subdir grok -w was launched from
        // This matches the git-topology branch below
        // Fall back to the recorded path when the source repo is gone (a standalone worktree whose source was deleted still works)
        let root = git2::Repository::discover(&source_repo)
            .ok()
            .and_then(|r| r.workdir().map(canonicalize_or_owned));
        return root.unwrap_or_else(|| canonicalize_or_owned(&source_repo));
    }
    if let Ok(repo) = git2::Repository::discover(cwd) {
        // Share one trust key across a repo's worktrees instead of re-prompting per worktree.
        if repo.is_worktree()
            && let Ok(main) = git2::Repository::open(repo.commondir())
            && let Some(main_workdir) = main.workdir()
            && canonicalize_or_owned(&main_workdir.join(".git"))
                == canonicalize_or_owned(repo.commondir())
        {
            return canonicalize_or_owned(main_workdir);
        }
        if let Some(workdir) = repo.workdir() {
            return canonicalize_or_owned(workdir);
        }
    }
    canonicalize_or_owned(cwd)
}

/// Whether `path` resolves to the user's home directory.
pub fn is_home_dir(path: &Path) -> bool {
    let Some(home) = xai_dirs::home_dir() else {
        return false;
    };
    canonicalize_or_owned(path) == canonicalize_or_owned(&home)
}

/// Whether `key` is too broad to ever be a safe trust root: refused on write and ignored on read (fail closed). Also consumed by [`crate::folder_trust`] as the "key can never be recorded" signal.
/// Such a key can't be durably gated, so it resolves Trusted instead of prompting on a decision that could never persist.
pub fn is_unsafe_trust_root(key: &Path) -> bool {
    !key.is_absolute() || key.parent().is_none() || is_home_dir(key)
}

/// `workspace_key` of the nearest existing ancestor (git2 discover fails on a missing path).
fn workspace_id(path: &Path) -> PathBuf {
    workspace_key(path.ancestors().find(|p| p.exists()).unwrap_or(path))
}

/// Fresh `$GROK_HOME` or `<home>/.grok`. Does not call `grok_home()` and does not create directories.
pub fn trust_store_home() -> Option<PathBuf> {
    xai_dirs::resolve_grok_home()
}

pub fn canonicalize_or_owned(path: &Path) -> PathBuf {
    dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn now_unix() -> Option<i64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs() as i64)
}

/// RAII exclusive advisory lock on a sidecar lock file, released on drop. Serializes concurrent `TrustStore` writers (multiple processes / instances sharing `~/.grok/`) across the whole read-modify-write so updates merge instead of clobbering each other.
/// The lock is advisory; only writers that take it (i.e.
struct ExclusiveLock {
    file: std::fs::File,
}

impl ExclusiveLock {
    fn acquire(lock_path: &Path) -> io::Result<Self> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)?;
        fs2::FileExt::lock_exclusive(&file)?;
        Ok(Self { file })
    }
}

impl Drop for ExclusiveLock {
    fn drop(&mut self) {
        // Best-effort unlock; the OS also releases the flock when `file` closes.
        let _ = fs2::FileExt::unlock(&self.file);
    }
}

#[cfg(test)]
#[path = "trust_tests.rs"]
mod tests;
