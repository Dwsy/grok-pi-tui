//! Project-scoped cumulative usage for the slash command and model selectors.
//!
//! One file per project, keyed by a stable hash of the canonical project root and
//! stored under `$GROK_HOME/selector-usage/`. Nothing is written into the user's
//! repository, and every project keeps its own counters.
//!
//! Durability: the UI thread keeps an in-memory aggregate for ranking and hands
//! *deltas* to a background writer. The writer merges those deltas into the file
//! under a cross-process lock and replaces it atomically, so two terminals
//! working in the same project never overwrite each other's counts.

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::util::grok_home;

/// Per-project caps, so a long-lived project cannot grow the file without bound.
const MAX_COMMAND_ENTRIES: usize = 256;
const MAX_MODEL_ENTRIES: usize = 128;
/// Cross-process lock: retry window and how long a lock may sit before it is stale.
const LOCK_RETRIES: u32 = 100;
const LOCK_RETRY_DELAY: Duration = Duration::from_millis(20);
/// A held lock is only broken after this long; the critical section is a short
/// read-merge-write, so a live lock is never this old.
const STALE_LOCK_AGE: Duration = Duration::from_secs(300);
/// How many failed write batches are kept for a retry.
const MAX_PENDING_JOBS: usize = 16;
/// How often one batch is retried before it goes back to the queue.
const WRITE_RETRIES: u32 = 3;
const WRITE_RETRY_DELAY: Duration = Duration::from_millis(50);
/// How often the project's file is pulled back in, so another terminal's
/// choices become visible here too.
const REFRESH_INTERVAL: Duration = Duration::from_secs(2);

/// How often one key was used, and when it was last used.
/// The field order is the ranking order: frequency first, recency as tiebreak.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct UsageRank {
    pub count: u64,
    pub last_used: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct UsageFile {
    #[serde(default)]
    commands: HashMap<String, UsageRank>,
    #[serde(default)]
    models: HashMap<String, UsageRank>,
}

/// One pending increment to merge into the on-disk file.
#[derive(Debug, Clone)]
enum UsageDelta {
    Command { key: String, at: u64 },
    Model { key: String, at: u64 },
}

/// Cumulative usage for one project.
#[derive(Debug)]
pub struct ProjectUsage {
    /// Where this project's counters live; `None` when persistence is off.
    file_path: Option<PathBuf>,
    /// In-memory aggregate for ranking. Merged with the file at write time.
    data: UsageFile,
    /// When the file was last pulled in, so another terminal's choices show up
    /// without a disk read on every keystroke.
    last_refresh: Option<Instant>,
}

/// A worktree is a project, including linked worktrees (`.git` is a file).
/// Subdirectories and symlink spellings share the nearest repository root.
/// Outside Git, the canonical working directory is the project identity.
pub fn project_root(cwd: &Path) -> PathBuf {
    let absolute = if cwd.is_absolute() {
        cwd.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(cwd)
    };
    let canonical = fs::canonicalize(&absolute).unwrap_or(absolute);
    canonical
        .ancestors()
        .find(|dir| dir.join(".git").exists())
        .unwrap_or(&canonical)
        .to_path_buf()
}

/// Where a project's counters live: `$GROK_HOME/selector-usage/<project hash>.json`.
fn usage_file_path(root: &Path) -> PathBuf {
    let hash = blake3::hash(root.to_string_lossy().as_bytes()).to_hex();
    let name = format!("{}.json", &hash[..16]);
    grok_home().join("selector-usage").join(name)
}

/// Wall-clock microseconds, strictly increasing within this process.
///
/// Two choices recorded in the same microsecond still order by recency, so the
/// frequency-then-recency ranking stays deterministic.
fn now_micros() -> u64 {
    static LAST: AtomicU64 = AtomicU64::new(0);
    let wall = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_micros().min(u64::MAX as u128) as u64)
        .unwrap_or_default();
    loop {
        let last = LAST.load(Ordering::Relaxed);
        let next = wall.max(last.saturating_add(1));
        if LAST
            .compare_exchange_weak(last, next, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            return next;
        }
    }
}

impl ProjectUsage {
    /// Load a project's counters. `persist_enabled` false keeps everything in
    /// memory (unit tests and the no-disk seam).
    pub fn load(root: &Path, persist_enabled: bool) -> Self {
        let file_path = usage_file_path(root);
        let data = if persist_enabled {
            read_usage_file(&file_path).unwrap_or_default()
        } else {
            UsageFile::default()
        };
        Self {
            file_path: persist_enabled.then_some(file_path),
            data,
            last_refresh: None,
        }
    }

    /// An empty, non-persistent store (callers that have no project context).
    pub fn empty() -> Self {
        Self {
            file_path: None,
            data: UsageFile::default(),
            last_refresh: None,
        }
    }

    /// Pull the project's file back in, at most once per [`REFRESH_INTERVAL`].
    ///
    /// Another terminal working in the same project writes to the same file;
    /// this keeps both rankings in step without a disk read per keystroke.
    pub fn refresh_if_stale(&mut self) {
        let Some(path) = self.file_path.clone() else {
            return;
        };
        let due = self
            .last_refresh
            .is_none_or(|at| at.elapsed() >= REFRESH_INTERVAL);
        if !due {
            return;
        }
        self.last_refresh = Some(Instant::now());
        if let Some(file) = read_usage_file(&path) {
            merge_ranks(&mut self.data, file);
        }
    }

    #[cfg(test)]
    pub(super) fn seed_command(&mut self, command: &str, rank: UsageRank) {
        self.data.commands.insert(command.into(), rank);
    }

    pub fn command_rank(&self, canonical: &str) -> UsageRank {
        self.data
            .commands
            .get(canonical.trim_start_matches('/'))
            .copied()
            .unwrap_or_default()
    }

    pub fn model_rank(&self, id: &str) -> UsageRank {
        self.data.models.get(id).copied().unwrap_or_default()
    }

    /// Count one real command submission.
    pub fn record_command(&mut self, canonical: &str) {
        let key = canonical.trim().trim_start_matches('/');
        if key.is_empty() {
            return;
        }
        let at = now_micros();
        Self::increment(self.data.commands.entry(key.to_string()).or_default(), at);
        trim_map(&mut self.data.commands, MAX_COMMAND_ENTRIES);
        self.enqueue(UsageDelta::Command {
            key: key.to_string(),
            at,
        });
    }

    /// Count one real model choice.
    pub fn record_model(&mut self, id: &str) {
        if id.is_empty() {
            return;
        }
        let at = now_micros();
        Self::increment(self.data.models.entry(id.to_string()).or_default(), at);
        trim_map(&mut self.data.models, MAX_MODEL_ENTRIES);
        self.enqueue(UsageDelta::Model {
            key: id.to_string(),
            at,
        });
    }

    fn increment(rank: &mut UsageRank, at: u64) {
        rank.count = rank.count.saturating_add(1);
        rank.last_used = rank.last_used.max(at);
    }

    /// Hand the increment to the background writer. The UI never blocks on disk.
    fn enqueue(&self, delta: UsageDelta) {
        let Some(path) = self.file_path.clone() else {
            return;
        };
        if !enqueue_write(path, vec![delta]) {
            tracing::warn!("selector usage: writer unavailable; usage not persisted");
        }
    }
}

/// Keep the least used entries when a project exceeds the cap.
fn trim_map(map: &mut HashMap<String, UsageRank>, cap: usize) {
    if map.len() <= cap {
        return;
    }
    let mut entries: Vec<(String, UsageRank)> =
        map.iter().map(|(key, rank)| (key.clone(), *rank)).collect();
    // Weakest first: fewest uses, then least recent.
    entries.sort_by(|left, right| left.1.cmp(&right.1));
    let drop_count = entries.len() - cap;
    for (key, _) in entries.into_iter().take(drop_count) {
        map.remove(&key);
    }
}

fn read_usage_file(path: &Path) -> Option<UsageFile> {
    let bytes = fs::read(path).ok()?;
    match serde_json::from_slice::<UsageFile>(&bytes) {
        Ok(file) => Some(file),
        Err(error) => {
            tracing::warn!(%error, "selector usage: unreadable file; starting a fresh counter");
            None
        }
    }
}

/// Fold a file snapshot into the in-memory view.
///
/// Counts only ever grow, so the larger of the two wins; an increment that has
/// not reached the file yet stays visible in memory.
fn merge_ranks(data: &mut UsageFile, other: UsageFile) {
    merge_rank_map(&mut data.commands, other.commands);
    merge_rank_map(&mut data.models, other.models);
}

fn merge_rank_map(target: &mut HashMap<String, UsageRank>, other: HashMap<String, UsageRank>) {
    for (key, rank) in other {
        let entry = target.entry(key).or_default();
        entry.count = entry.count.max(rank.count);
        entry.last_used = entry.last_used.max(rank.last_used);
    }
}

// ---------------------------------------------------------------------------
// Background writer
// ---------------------------------------------------------------------------

type WriteJob = (PathBuf, Vec<UsageDelta>);

/// Send one delta batch to the writer thread. Returns `false` when no writer
/// could be started, so the caller can report that the delta stays unpersisted.
fn enqueue_write(path: PathBuf, deltas: Vec<UsageDelta>) -> bool {
    static WRITER: OnceLock<Option<Sender<WriteJob>>> = OnceLock::new();
    let tx = WRITER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<WriteJob>();
        match std::thread::Builder::new()
            .name("selector-usage-writer".to_string())
            .spawn(move || writer_loop(rx))
        {
            Ok(_) => Some(tx),
            Err(error) => {
                tracing::debug!(%error, "selector usage: writer thread spawn failed");
                None
            }
        }
    });
    match tx {
        Some(tx) => tx.send((path, deltas)).is_ok(),
        None => false,
    }
}

fn writer_loop(rx: mpsc::Receiver<WriteJob>) {
    let mut pending: VecDeque<WriteJob> = VecDeque::new();
    while let Ok(job) = rx.recv() {
        pending.push_back(job);
        // A failed write keeps its delta and is retried before the next batch.
        while let Some((path, deltas)) = pending.pop_front() {
            match write_with_retry(&path, &deltas) {
                Ok(()) => {}
                Err(error) => {
                    tracing::warn!(%error, path = %path.display(), "selector usage: write failed");
                    if pending.len() < MAX_PENDING_JOBS {
                        pending.push_front((path, deltas));
                        break;
                    }
                    // The queue is full of failures; drop this batch loudly
                    // instead of growing without bound.
                    tracing::error!(
                        path = %path.display(),
                        "selector usage: dropping pending usage increments"
                    );
                }
            }
        }
    }
}

/// Try a batch a few times before giving it back to the queue.
fn write_with_retry(path: &Path, deltas: &[UsageDelta]) -> io::Result<()> {
    let mut last_error = None;
    for attempt in 0..WRITE_RETRIES {
        match merge_deltas(path, deltas) {
            Ok(()) => return Ok(()),
            Err(error) => {
                last_error = Some(error);
                std::thread::sleep(WRITE_RETRY_DELAY * (attempt + 1));
            }
        }
    }
    Err(last_error.unwrap_or_else(|| io::Error::other("usage write failed")))
}

/// Merge increments into the project file under a cross-process lock.
fn merge_deltas(path: &Path, deltas: &[UsageDelta]) -> io::Result<()> {
    let _lock = FileLock::acquire(path)?;
    let mut file = read_usage_file(path).unwrap_or_default();
    for delta in deltas {
        let (map, key, at) = match delta {
            UsageDelta::Command { key, at } => (&mut file.commands, key, *at),
            UsageDelta::Model { key, at } => (&mut file.models, key, *at),
        };
        let entry = map.entry(key.clone()).or_default();
        entry.count = entry.count.saturating_add(1);
        // The record time travels with the delta, so another process merging
        // later cannot make an older choice look newer.
        entry.last_used = entry.last_used.max(at);
    }
    trim_map(&mut file.commands, MAX_COMMAND_ENTRIES);
    trim_map(&mut file.models, MAX_MODEL_ENTRIES);
    write_atomic(path, &file)
}

/// Replace `path` with `file` through a private temp file in the same directory.
fn write_atomic(path: &Path, file: &UsageFile) -> io::Result<()> {
    let Some(parent) = path.parent() else {
        return Err(io::Error::other("usage file has no parent directory"));
    };
    fs::create_dir_all(parent)?;
    let bytes = serde_json::to_vec(file)
        .map_err(|error| io::Error::other(format!("serialize usage file: {error}")))?;
    let tmp = unique_temp_path(path);
    let write_result = (|| -> io::Result<()> {
        let mut handle = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        handle.write_all(&bytes)?;
        handle.sync_all()?;
        drop(handle);
        fs::rename(&tmp, path)
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    write_result
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A temp name only this process uses, so two writers never share a file.
fn unique_temp_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "selector-usage".to_string());
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp_name = format!("{name}.tmp-{}-{counter}", std::process::id());
    path.with_file_name(temp_name)
}

/// Cross-process mutex for one usage file, built from an atomic `create_dir`.
/// The lock directory carries this holder's token, so a lock that was broken as
/// stale and taken over by another process is never removed by its old owner.
struct FileLock {
    path: PathBuf,
    token: String,
}

impl FileLock {
    fn acquire(target: &Path) -> io::Result<Self> {
        let lock_path = target.with_extension("lock");
        let token = format!(
            "{}-{}",
            std::process::id(),
            NEXT_LOCK_ID.fetch_add(1, Ordering::Relaxed)
        );
        for attempt in 0..LOCK_RETRIES {
            match fs::create_dir(&lock_path) {
                Ok(()) => {
                    if let Err(error) = fs::write(lock_path.join("owner"), &token) {
                        // Leave no half-owned lock behind.
                        let _ = fs::remove_dir_all(&lock_path);
                        return Err(error);
                    }
                    return Ok(Self {
                        path: lock_path,
                        token,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    if lock_is_stale(&lock_path) {
                        let _ = fs::remove_dir_all(&lock_path);
                        continue;
                    }
                    if attempt + 1 < LOCK_RETRIES {
                        std::thread::sleep(LOCK_RETRY_DELAY);
                        continue;
                    }
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!("timed out acquiring usage lock {}", lock_path.display()),
                    ));
                }
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("timed out acquiring usage lock {}", lock_path.display()),
        ))
    }

    /// Whether this process still owns the lock directory.
    fn still_ours(&self) -> bool {
        fs::read_to_string(self.path.join("owner"))
            .map(|owner| owner.trim() == self.token)
            .unwrap_or(false)
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        // Only remove the lock while it is still ours.
        if self.still_ours() {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

/// Distinguishes lock holders inside this process.
static NEXT_LOCK_ID: AtomicU64 = AtomicU64::new(0);

fn lock_is_stale(path: &Path) -> bool {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age > STALE_LOCK_AGE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn command_delta(key: &str, at: u64) -> UsageDelta {
        UsageDelta::Command {
            key: key.into(),
            at,
        }
    }

    #[test]
    fn frequency_precedes_recency_and_namespaces_are_separate() {
        let mut usage = ProjectUsage::load(Path::new("."), false);
        usage.record_command("model");
        usage.record_command("/model");
        usage.record_command("help");
        usage.record_model("anthropic::same-id");
        usage.record_model("openrouter::same-id");
        usage.record_model("openrouter::same-id");
        assert!(usage.command_rank("model") > usage.command_rank("help"));
        assert!(usage.model_rank("openrouter::same-id") > usage.model_rank("anthropic::same-id"));
        assert_eq!(usage.command_rank("anthropic::same-id").count, 0);
        usage.record_command("help");
        assert!(usage.command_rank("help") > usage.command_rank("model"));
    }

    #[test]
    fn worktree_identity_unifies_subdirectories_and_separates_projects() {
        let temp = temp_root();
        let repo = temp.path().join("repo");
        let other = temp.path().join("other");
        fs::create_dir_all(repo.join("src/nested")).unwrap();
        fs::write(repo.join(".git"), "gitdir: elsewhere").unwrap();
        fs::create_dir_all(&other).unwrap();
        assert_eq!(project_root(&repo), project_root(&repo.join("src/nested")));
        assert_ne!(project_root(&repo), project_root(&other));
        #[cfg(unix)]
        {
            let alias = temp.path().join("alias");
            std::os::unix::fs::symlink(&repo, &alias).unwrap();
            assert_eq!(project_root(&repo), project_root(&alias));
        }
    }

    #[test]
    fn projects_get_separate_files_inside_the_user_home() {
        let temp = temp_root();
        let repo = temp.path().join("repo");
        let other = temp.path().join("other");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(&other).unwrap();
        let repo_usage = ProjectUsage::load(&repo, true);
        let other_usage = ProjectUsage::load(&other, true);
        assert_ne!(repo_usage.file_path, other_usage.file_path);
        assert!(
            repo_usage
                .file_path
                .is_some_and(|path| path.starts_with(grok_home()))
        );
        // The user's repository stays untouched.
        assert!(!repo.join("selector-usage.json").exists());
    }

    #[test]
    fn separate_writers_do_not_overwrite_each_others_counts() {
        let temp = temp_root();
        let path = temp.path().join("selector-usage.json");
        // Two independent processes would each hold their own aggregate; the
        // merge has to add both increments instead of letting the last writer win.
        merge_deltas(&path, &[command_delta("model", 1)]).unwrap();
        merge_deltas(&path, &[command_delta("model", 2)]).unwrap();
        merge_deltas(&path, &[command_delta("help", 3)]).unwrap();
        merge_deltas(
            &path,
            &[UsageDelta::Model {
                key: "p::m".into(),
                at: 4,
            }],
        )
        .unwrap();
        let file = read_usage_file(&path).unwrap();
        assert_eq!(file.commands.get("model").unwrap().count, 2);
        assert_eq!(file.commands.get("help").unwrap().count, 1);
        assert_eq!(file.models.get("p::m").unwrap().count, 1);
    }

    #[test]
    fn merge_keeps_other_entries() {
        let temp = temp_root();
        let path = temp.path().join("selector-usage.json");
        merge_deltas(&path, &[command_delta("a", 1)]).unwrap();
        merge_deltas(&path, &[command_delta("b", 2)]).unwrap();
        let file = read_usage_file(&path).unwrap();
        assert!(file.commands.contains_key("a"));
        assert!(file.commands.contains_key("b"));
    }

    #[test]
    fn a_broken_file_is_replaced_instead_of_blocking_the_counter() {
        let temp = temp_root();
        let path = temp.path().join("selector-usage.json");
        fs::write(&path, b"broken").unwrap();
        merge_deltas(&path, &[command_delta("model", 1)]).unwrap();
        let file = read_usage_file(&path).unwrap();
        assert_eq!(file.commands.get("model").unwrap().count, 1);
    }

    #[test]
    fn record_times_are_strictly_increasing_so_ties_stay_deterministic() {
        let first = now_micros();
        let second = now_micros();
        assert!(second > first, "recency tiebreak needs distinct stamps");
    }

    #[test]
    fn merging_keeps_the_earlier_record_time_when_a_later_merge_arrives() {
        let temp = temp_root();
        let path = temp.path().join("selector-usage.json");
        merge_deltas(
            &path,
            &[UsageDelta::Command {
                key: "model".into(),
                at: 500,
            }],
        )
        .unwrap();
        merge_deltas(
            &path,
            &[UsageDelta::Command {
                key: "model".into(),
                at: 400,
            }],
        )
        .unwrap();
        let file = read_usage_file(&path).unwrap();
        assert_eq!(file.commands.get("model").unwrap().count, 2);
        assert_eq!(file.commands.get("model").unwrap().last_used, 500);
    }

    #[test]
    fn refresh_picks_up_another_writers_counts() {
        let temp = temp_root();
        let path = temp.path().join("selector-usage.json");
        let mut usage = ProjectUsage::load(temp.path(), false);
        usage.file_path = Some(path.clone());
        usage.last_refresh = None;
        merge_deltas(&path, &[command_delta("model", 5)]).unwrap();
        usage.refresh_if_stale();
        assert_eq!(usage.command_rank("model").count, 1);
        // A later refresh keeps the larger count instead of shrinking it.
        merge_deltas(&path, &[command_delta("model", 6)]).unwrap();
        usage.last_refresh = None;
        usage.refresh_if_stale();
        assert_eq!(usage.command_rank("model").count, 2);
    }

    #[test]
    fn atomic_write_leaves_no_temp_files_behind() {
        let temp = temp_root();
        let path = temp.path().join("selector-usage.json");
        merge_deltas(&path, &[command_delta("model", 1)]).unwrap();
        let leftovers: Vec<_> = fs::read_dir(temp.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| name.contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "unexpected temp files: {leftovers:?}");
    }
}
