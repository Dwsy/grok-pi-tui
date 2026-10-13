//! The daemon side of the per-command shell sandbox: one [`WorkspaceSandbox`] per served folder
//! implements the tools crate's [`SandboxLaunch`] seam (policy, mode, backend, wrapping) and keeps
//! what the result path needs to decode a denial and replay the command once a grant is given.
//!
//! The tools crate stays policy-blind: every shell spawn site calls `prepare`, the hub's result
//! path calls [`WorkspaceSandbox::finish`], and `permission::sandbox_gate` settles what `finish`
//! decoded. Nothing here spawns a process. A held network connection is posted mid-command by the
//! folder's proxy decider (`network.rs`) through its sink and settled with the call's bound owner;
//! nothing here releases holds after the run.

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};

use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use xai_grok_config_types::RemoteSettings;
use xai_grok_sandbox::SandboxProfile;
pub use xai_grok_sandbox::command::CallId;
pub use xai_grok_sandbox::command::GrantError;
pub use xai_grok_sandbox::command::SandboxMode;
use xai_grok_sandbox::command::backend::{SandboxBackend, SandboxCommandError};
pub use xai_grok_sandbox::command::grants::GrantId;
use xai_grok_sandbox::command::grants::{Clock, Grant, GrantSubject, SystemClock};
use xai_grok_sandbox::command::mode::{ResolvedSandboxMode, SANDBOX_MODE_ENV};
use xai_grok_sandbox::command::policy::{PolicyError, WritableLocations, daemon_tmp_dirs};
use xai_grok_sandbox::command::violation::{CommandExit, DecodeInput, ProposalBounds};
use xai_grok_sandbox::command::{
    BackendName, GitConfigEnv, GrantStore, HostProbe, ObserveSummary, RefusedUnderGrant,
    SandboxPolicy, ServedRoot, Violation, decode, detect_backend, refused_under_grant,
};
use xai_grok_telemetry::events::{SandboxCommandOutcome, SandboxSettlement};

use crate::permission::PermissionHookTransport;
use crate::sandbox_mode::{
    SandboxModeInputs, SandboxModeWriteError, resolve_sandbox_mode_in, set_workspace_mode_in,
};

mod calls;
mod grants;
mod launch;
pub mod metrics;
mod network;
pub(crate) mod result_path;

pub use calls::CallOwner;
#[cfg(test)]
use calls::MAX_OPEN_CALLS;
use calls::{CallRefused, CallTable};
pub use grants::{global_grants, grants_to_json, observe_summary_to_json, revoke_global_grant};
pub use network::{NetworkInfo, NetworkStartError};

/// The daemon's control-socket directory must be in the built policy's protected floor, whatever
/// the profile, the grants or a cwd inside it say. `is_protected` asks with both spellings: on
/// macOS `/var/folders/…` is an alias of `/private/var/…`.
fn assert_control_socket_protected(
    policy: &SandboxPolicy,
    control_socket_dir: &Path,
) -> Result<(), WorkspaceSandboxError> {
    if policy.is_protected(control_socket_dir) {
        return Ok(());
    }
    Err(WorkspaceSandboxError::ControlSocketUnprotected {
        dir: control_socket_dir.to_path_buf(),
    })
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

#[cfg(all(test, unix))]
#[path = "real_wiring_tests.rs"]
mod real_wiring_tests;

#[cfg(all(test, unix))]
#[path = "network_tests.rs"]
mod network_tests;

/// The model-visible refusal under `enforce` on a host with no backend.
pub const ENFORCE_UNAVAILABLE_TEXT: &str = "the command sandbox is set to enforce but this host has no per-command sandbox backend; \
     no shell command can run; ask the folder's owner to set `[sandbox] mode = \"observe\"` (or GROK_SANDBOX_MODE=observe) to run commands unsandboxed.";

/// The model-visible refusal under `enforce` while the folder's session directory is not safe;
/// the reason follows in parentheses.
pub const SESSION_DIR_UNSAFE_TEXT: &str = "the command sandbox is set to enforce but this folder's session directory could not be made a private directory of this user, \
     so no shell command can run; ask the folder's owner to fix that path, or to set `[sandbox] mode = \"observe\"` to run commands unsandboxed";

/// The model-visible refusal of a command while the sandbox tracks as many calls as it keeps and
/// every one of them may still be running.
pub const CALL_TABLE_FULL_TEXT: &str = "the command was not run: the sandbox already tracks as many commands as it can, \
     and each of them may still be running (background jobs included). Stop some of them or let them finish, then run the command again";

/// The model-visible refusal of a tool's write to a mode layer while the folder enforces.
pub(crate) const MODE_LAYER_WRITE_TEXT: &str = "the file was not changed: it is, or may resolve to, one of this folder's sandbox mode files \
     (workspaced.toml), which only the user may change while the sandbox enforces";

/// Why the sandbox could not do what was asked. The `SandboxLaunch`
/// seam and the control verbs render it with `Display`.
#[derive(Debug, thiserror::Error)]
pub enum WorkspaceSandboxError {
    #[error("sandbox policy cannot be built: {0}")]
    Policy(#[from] PolicyError),
    /// The store refused or could not persist a grant; nothing is in effect.
    #[error(transparent)]
    Grant(#[from] GrantError),
    #[error("{ENFORCE_UNAVAILABLE_TEXT}")]
    EnforceUnavailable,
    #[error("sandbox refused to prepare the command: {0}")]
    Wrap(#[from] SandboxCommandError),
    /// The built policy does not protect the daemon's own control-socket directory, so a command
    /// could reach the endpoint that grants it more. Never expected; no command runs under it.
    #[error(
        "sandbox policy does not protect the daemon's control-socket directory {dir}; no command runs under it"
    )]
    ControlSocketUnprotected { dir: PathBuf },
    /// A grant whose path is not UTF-8: the grants file and the Settings list hold UTF-8 only,
    /// so it is recorded nowhere, in memory or on disk.
    #[error("sandbox grant not recorded: its path {path} is not valid UTF-8")]
    SubjectNotUtf8 { path: String },
    /// A call-scoped grant for a call whose result is already in: no spawn of it would carry the
    /// grant, so it is not reported as given.
    #[error("sandbox grant not recorded: the command it was for has already finished")]
    CallFinished,
    /// An allow a live deny row covers: deny rows win, so it would be recorded and never apply.
    #[error(
        "sandbox grant not recorded: an \"Always reject\" row covers it; revoke that row in Settings first"
    )]
    DeniedByRow,
    /// Under `enforce`, the folder's own session directory could not be created or is not a
    /// real, owner-only directory, so the floor that keeps commands out of it has a gap.
    #[error("{SESSION_DIR_UNSAFE_TEXT} ({reason})")]
    SessionDirUnsafe { reason: String },
    /// The call table is full and every call in it may still be running: a new command is refused
    /// rather than one of them evicted, which would lose what holds its process to its mode. A
    /// call that spawns nothing runs unbound, so a background job can still be stopped.
    #[error("{CALL_TABLE_FULL_TEXT}")]
    CallTableFull,
    /// Under `enforce`, a spawn after the mode was switched on and before the folder engaged:
    /// nothing is ready to wrap it, so it does not run.
    #[error(
        "the command sandbox was just switched to enforce and is still starting for this folder; run the command again"
    )]
    NotEngaged,
    /// The call's final result is already in: a spawn for it is late and does not run, since
    /// nothing could ask about it or take its result.
    #[error("sandbox refused to prepare the command: its call has already finished")]
    CallReleased,
    /// The folder's proxy runs but minted no credential for this spawn: the command does not
    /// run, since a proxy pointer with no credential would let its requests pass as nobody's.
    #[error("sandbox refused to prepare the command: no proxy credential for it ({reason})")]
    NoCallCredential { reason: String },
}

impl From<CallRefused> for WorkspaceSandboxError {
    fn from(refused: CallRefused) -> WorkspaceSandboxError {
        match refused {
            CallRefused::TableFull => WorkspaceSandboxError::CallTableFull,
            CallRefused::Released => WorkspaceSandboxError::CallReleased,
        }
    }
}

/// What [`WorkspaceSandbox::finish`] learned about a finished command.
#[derive(Clone, Debug, Default)]
pub enum Finished {
    /// The sandbox did not stop the command, or it was never prepared here.
    #[default]
    Ran,
    /// The sandbox stopped the command: the denial the gate settles.
    Violation(FinishedViolation),
    /// The replayed run was stopped on the very target its grant allows: nothing to card, but
    /// the tool result names the backend's limitation, never the raw OS error.
    RefusedUnderGrant(RefusedUnderGrant),
}

impl Finished {
    /// The denial the gate settles, when the sandbox stopped the command.
    pub fn violation(&self) -> Option<&FinishedViolation> {
        match self {
            Finished::Violation(violation) => Some(violation),
            Finished::Ran | Finished::RefusedUnderGrant(_) => None,
        }
    }
}

/// A decoded violation with the context the card and the replay need.
#[derive(Clone, Debug)]
pub struct FinishedViolation {
    pub violation: Violation,
    pub mode: SandboxMode,
    pub backend: Option<BackendName>,
    /// The violation came from the replayed run, under this grant: no third run, the denial is
    /// final and the text names what was allowed.
    pub replayed_under: Option<GrantSubject>,
}

impl FinishedViolation {
    /// The violation came from the replayed run.
    pub fn after_replay(&self) -> bool {
        self.replayed_under.is_some()
    }
}

/// Where the backend comes from: the daemon's one host probe, taken when the folder engages
/// (production), or a fixed choice (tests, and a host that already knows it has none).
pub enum BackendSource {
    Detect,
    Fixed(Option<Box<dyn SandboxBackend>>),
}

/// Where the folder's egress proxy listens, as the policy renders it: the loopback port every
/// client is pointed at. `Some` means **bound**: nothing here starts
/// a proxy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProxyEndpointOwned {
    pub port: u16,
}

/// What the daemon gives the workspace when it builds the sandbox.
pub struct WorkspaceSandboxConfig {
    pub workspace_root: PathBuf,
    pub grok_home: PathBuf,
    pub user_home: Option<PathBuf>,
    /// Where git finds the user's global config, read once per folder from the daemon's
    /// environment (`GitConfigEnv::from_host`): the floor reads and protects it there.
    pub git_env: GitConfigEnv,
    /// The daemon's own endpoint directory: protected and denied for unix sockets. Required.
    pub control_socket_dir: PathBuf,
    /// An explicit fleet layer (`RemoteSettings.sandbox_mode`, the rollout switch). `None` reads
    /// it from the host's managed config file, stamped and re-read like the other two layers.
    pub remote: Option<RemoteSettings>,
    pub backend: BackendSource,
    pub clock: Arc<dyn Clock>,
}

/// The host probe, taken once per daemon: on macOS it
/// spawns `sandbox-exec` once with a bounded wait, so it runs off the runtime's workers and every
/// served folder reads the same result.
static HOST_PROBE: tokio::sync::OnceCell<HostProbe> = tokio::sync::OnceCell::const_new();

/// The daemon's one [`HostProbe`], probed on the first call.
pub async fn host_probe() -> HostProbe {
    HOST_PROBE
        .get_or_init(|| async {
            match tokio::task::spawn_blocking(HostProbe::run).await {
                Ok(probe) => probe,
                Err(error) => {
                    tracing::warn!(%error, "the sandbox host probe did not complete; no backend");
                    HostProbe::default()
                }
            }
        })
        .await
        .clone()
}

/// Whether the daemon's one host probe selects a backend compiled into this binary, as
/// [`WorkspaceSandbox::open`] decides it: the mode resolver's `backend_available` for a folder
/// no sandbox serves.
pub async fn host_backend_available() -> bool {
    detect_backend(&host_probe().await).is_some()
}

impl WorkspaceSandboxConfig {
    /// Production inputs for `workspace_root`: `xai_grok_config::grok_home()`, the user's home
    /// and git environment, the backend the daemon's one host probe picks once the folder
    /// engages, and the system clock. The managed layer is not an input: the sandbox reads
    /// `<grok_home>/managed_config.toml` itself, with the other two layers.
    pub fn for_root(workspace_root: PathBuf, control_socket_dir: PathBuf) -> Self {
        WorkspaceSandboxConfig {
            workspace_root,
            grok_home: xai_grok_config::grok_home(),
            user_home: xai_dirs::home_dir(),
            git_env: GitConfigEnv::from_host(),
            control_socket_dir,
            remote: None,
            backend: BackendSource::Detect,
            clock: Arc::new(SystemClock),
        }
    }
}

/// What one file looked like when the mode was last resolved from it: inode, modification time
/// and length. Every writer of these files (`sandbox.mode.set`, an editor's
/// save, an MDM's deploy) moves at least one of the three.
type FileStamp = Option<(u64, std::time::SystemTime, u64)>;

/// What the mode was resolved from (the folder owner resolves it once; the
/// next read resolves again only when one of these moved).
#[derive(Clone, Debug, PartialEq, Eq)]
struct ModeStamp {
    env: Option<String>,
    workspace_file: FileStamp,
    user_file: FileStamp,
    managed_file: FileStamp,
}

fn file_stamp(path: &Path) -> FileStamp {
    let meta = std::fs::metadata(path).ok()?;
    #[cfg(unix)]
    let ino = std::os::unix::fs::MetadataExt::ino(&meta);
    #[cfg(not(unix))]
    let ino = 0;
    Some((ino, meta.modified().ok()?, meta.len()))
}

/// Create the folder's own session directory when it is missing, and say why it is not safe when
/// it is not: a real directory (never a link, which is not followed), owned by this user and
/// closed to everyone else. `None` when it is safe.
fn session_dir_gap(grok_home: &Path, workspace_root: &Path) -> Option<String> {
    let cwd = workspace_root.to_string_lossy();
    let dir = xai_grok_config::sessions_cwd_dir_in(grok_home, &cwd);
    let shown = dir.display();
    if let Ok(meta) = std::fs::symlink_metadata(&dir)
        && !meta.file_type().is_dir()
    {
        return Some(format!("{shown} is a link or a file, not a directory"));
    }
    if let Err(error) = xai_grok_config::ensure_sessions_cwd_dir_in(grok_home, &cwd) {
        return Some(format!("{shown} could not be created: {error}"));
    }
    let meta = match std::fs::symlink_metadata(&dir) {
        Ok(meta) => meta,
        Err(error) => return Some(format!("{shown} cannot be read: {error}")),
    };
    if !meta.file_type().is_dir() {
        return Some(format!("{shown} is a link or a file, not a directory"));
    }
    #[cfg(unix)]
    {
        let owner = std::os::unix::fs::MetadataExt::uid(&meta);
        let mode = std::os::unix::fs::MetadataExt::mode(&meta);
        // SAFETY: geteuid takes no arguments and cannot fail
        if owner != unsafe { libc::geteuid() } {
            return Some(format!("{shown} is owned by uid {owner}, not this user"));
        }
        if mode & 0o077 != 0 {
            return Some(format!(
                "{shown} is open to other users (mode {:o})",
                mode & 0o777
            ));
        }
    }
    None
}

/// The mode owner's cache: the last resolution and what it was resolved from. `None` after
/// `sandbox.mode.set`, so the verb's reply is resolved afresh whatever the stamps say.
#[derive(Default)]
struct ModeCache {
    resolved: Option<(ModeStamp, ResolvedSandboxMode)>,
    /// A reader is reading the layers, with the lock released; the others answer with `resolved`
    /// meanwhile instead of waiting on a slow disk.
    resolving: bool,
    /// Bumped by `sandbox.mode.set`: a resolution begun before the write is not kept.
    generation: u64,
    /// How many times the layers were read from disk (the "once" pin).
    resolutions: u64,
    /// How many times the layers' files were stat-ed for their stamp: every
    /// [`WorkspaceSandbox::mode`] read is one, and the proxy's decisions are none.
    stamps: u64,
    /// A test's hold on the next layer read, standing in for a slow disk.
    #[cfg(test)]
    read_hold: Option<ModeReadHold>,
}

/// The next layer read tells `entered` it is in, then waits on `release` (a send or a drop).
#[cfg(test)]
struct ModeReadHold {
    entered: std::sync::mpsc::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
}

/// One snapshot of the grant store's live rows, for the readers that cannot take the store lock:
/// the synchronous `prepare` and the proxy's decider. Written by [`Engaged::refresh_live`] only.
#[derive(Default)]
struct LiveRows {
    /// The workspace and global rows every session shares, allow and deny
    /// (`GrantStore::live_shared`).
    shared: Vec<Grant>,
    /// "For this conversation" rows, each with the hub session that gave it
    /// (`GrantStore::live_session_rows`).
    sessions: Vec<(String, Grant)>,
}

impl LiveRows {
    /// The rows a call run for `session` sees: the shared ones and that session's own.
    fn rows_for(&self, session: Option<&str>) -> Vec<Grant> {
        let own = self
            .sessions
            .iter()
            .filter(|(owner, _)| Some(owner.as_str()) == session)
            .map(|(_, grant)| grant);
        self.shared.iter().chain(own).cloned().collect()
    }
}

/// What a folder pays for only once its mode is not `off`: the host probe's backend, the pinned
/// root, the profile, the protected floor inside the grant store, the session directory's check.
struct Engaged {
    /// Pinned once, when the folder engages: every policy build checks this pin.
    root: ServedRoot,
    tmp_dirs: &'static [ServedRoot],
    profile: SandboxProfile,
    backend: Option<Box<dyn SandboxBackend>>,
    store: tokio::sync::Mutex<GrantStore>,
    live: parking_lot::RwLock<LiveRows>,
    /// Why the folder's session directory is not safe, when it is not: `enforce` refuses every
    /// command meanwhile.
    session_dir: parking_lot::RwLock<Option<String>>,
}

/// The folder's mode and calls, with the rest ([`Engaged`]) built the first time the mode is not
/// `off`: under `off` a served folder probes nothing, scans nothing and opens no grant store.
pub struct WorkspaceSandbox {
    /// The root as served; the mode's layers are read under it.
    root: PathBuf,
    grok_home: PathBuf,
    user_home: Option<PathBuf>,
    git_env: GitConfigEnv,
    control_socket_dir: PathBuf,
    remote: Option<RemoteSettings>,
    writable: Arc<WritableLocations>,
    /// Taken by the one engage that builds [`Engaged`].
    backend_source: parking_lot::Mutex<Option<BackendSource>>,
    engaged: tokio::sync::OnceCell<Engaged>,
    clock: Arc<dyn Clock>,
    mode: parking_lot::Mutex<ModeCache>,
    calls: parking_lot::Mutex<CallTable>,
    observe: parking_lot::Mutex<ObserveSummary>,
    /// Informational-card acknowledgements and session ends, spawned off the call that raised them.
    tasks: OwnedTasks,
    /// One grantable card per hub session at a time ([`WorkspaceSandbox::card_turn`]); an entry
    /// lives as long as a gate holds or waits for the session's turn.
    card_turns: parking_lot::Mutex<HashMap<String, Weak<tokio::sync::Mutex<()>>>>,
    /// The folder's egress proxy and its decider, once `start_network` ran: only after the folder
    /// engaged under `enforce`.
    network: arc_swap::ArcSwapOption<network::NetworkSide>,
    /// Held across every start, stop and sync of `network`, so two shell calls dispatched at
    /// once cannot both bind a proxy for the folder; what it guards is in the guard.
    network_lifecycle: tokio::sync::Mutex<NetworkLifecycle>,
    /// A host that answers sandbox cards itself installs its transport here; the hub's
    /// permission channel is used otherwise.
    card_transport: parking_lot::RwLock<Option<Arc<dyn PermissionHookTransport>>>,
    /// What a test stages to run between the next spawn's policy build and its credential
    /// mint: the folder's stop racing the spawn.
    #[cfg(test)]
    before_mint: parking_lot::Mutex<Option<BeforeMint>>,
    /// How many times [`Engaged`] was built: one probe, one floor scan, one store open each.
    #[cfg(test)]
    engagements: std::sync::atomic::AtomicUsize,
    /// How many syncs a call's end scheduled ([`WorkspaceSandbox::resync_unless_enforced`]),
    /// and the signal each one gives when it is done.
    #[cfg(test)]
    proxy_resyncs: std::sync::atomic::AtomicUsize,
    #[cfg(test)]
    proxy_resynced: tokio::sync::Notify,
}

/// A test's stand-in for what lands between a spawn's policy build and its credential mint.
#[cfg(test)]
type BeforeMint = Box<dyn FnOnce(&WorkspaceSandbox) + Send>;

/// What the network lifecycle lock guards besides the start/stop/sync sequence itself.
#[derive(Default)]
struct NetworkLifecycle {
    /// Set by the daemon's unserve (`stop_network`): no later sync or start binds a proxy for
    /// the folder.
    closed: bool,
}

/// Background work its owner holds: each task runs until it finishes or the owner stops or drops
/// the set (a `CancellationToken` for the task, its handle in the `JoinSet`), never detached.
#[derive(Default)]
pub struct OwnedTasks {
    cancel: CancellationToken,
    tasks: parking_lot::Mutex<JoinSet<()>>,
}

impl OwnedTasks {
    pub fn spawn(&self, task: impl Future<Output = ()> + Send + 'static) {
        let cancel = self.cancel.clone();
        let mut tasks = self.tasks.lock();
        while tasks.try_join_next().is_some() {}
        tasks.spawn(async move {
            tokio::select! {
                () = cancel.cancelled() => {}
                () = task => {}
            }
        });
    }

    /// Cancel and abort every task now, before the set is dropped.
    pub fn stop(&self) {
        self.cancel.cancel();
        self.tasks.lock().abort_all();
    }
}

impl Drop for OwnedTasks {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl std::fmt::Debug for WorkspaceSandbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkspaceSandbox")
            .field("root", &self.root)
            .field("backend", &self.backend_name())
            .finish_non_exhaustive()
    }
}

impl WorkspaceSandbox {
    /// Serve the folder: its mode is resolved, and only a mode that is not `off` engages it (see
    /// [`WorkspaceSandbox::engage_unless_off`]). Never fails.
    pub async fn open(config: WorkspaceSandboxConfig) -> WorkspaceSandbox {
        let WorkspaceSandboxConfig {
            workspace_root,
            grok_home,
            user_home,
            git_env,
            control_socket_dir,
            remote,
            backend,
            clock,
        } = config;
        let sandbox = WorkspaceSandbox {
            root: workspace_root,
            grok_home,
            user_home,
            git_env,
            control_socket_dir,
            remote,
            writable: WritableLocations::daemon(),
            backend_source: parking_lot::Mutex::new(Some(backend)),
            engaged: tokio::sync::OnceCell::new(),
            clock,
            mode: parking_lot::Mutex::new(ModeCache::default()),
            calls: parking_lot::Mutex::new(CallTable::default()),
            observe: parking_lot::Mutex::new(ObserveSummary::default()),
            tasks: OwnedTasks::default(),
            card_turns: parking_lot::Mutex::new(HashMap::new()),
            network: arc_swap::ArcSwapOption::from(None),
            network_lifecycle: tokio::sync::Mutex::new(NetworkLifecycle::default()),
            card_transport: parking_lot::RwLock::new(None),
            #[cfg(test)]
            before_mint: parking_lot::Mutex::new(None),
            #[cfg(test)]
            engagements: std::sync::atomic::AtomicUsize::new(0),
            #[cfg(test)]
            proxy_resyncs: std::sync::atomic::AtomicUsize::new(0),
            #[cfg(test)]
            proxy_resynced: tokio::sync::Notify::new(),
        };
        sandbox.engage_unless_off().await;
        sandbox
    }

    /// Engage the folder when its mode is not `off`, and answer the mode as resolved after it: an
    /// `enforce` from the rollout switch is `off` on a host the probe found no backend on. Every
    /// async step that leads to a spawn calls this first; under `off` it only resolves the mode.
    pub async fn engage_unless_off(&self) -> SandboxMode {
        if self.mode() == SandboxMode::Off {
            return SandboxMode::Off;
        }
        self.engage().await;
        self.mode()
    }

    /// The engaged half, built on the first call whatever the mode: the grant verbs need the
    /// store even under `off`. A resolution made before, which assumed a backend, is dropped
    /// when the probe found none.
    async fn engage(&self) -> &Engaged {
        let mut built = false;
        let engaged = self
            .engaged
            .get_or_init(|| async {
                built = true;
                self.build_engaged().await
            })
            .await;
        if built && engaged.backend.is_none() {
            self.forget_resolution();
        }
        engaged
    }

    /// Never fails: an unresolved profile leaves the workspace writable, and no backend keeps
    /// `backend: None` (observe unwrapped, enforce refused). The backend source is taken after
    /// the last await, so a cancelled build leaves it for the next.
    async fn build_engaged(&self) -> Engaged {
        let detect = matches!(
            *self.backend_source.lock(),
            Some(BackendSource::Detect) | None
        );
        let probe = if detect {
            Some(host_probe().await)
        } else {
            None
        };
        // The floor keeps a command from creating the own session directory (as a link, or
        // holding a grant file), so it exists before the first command runs
        let session_dir = session_dir_gap(&self.grok_home, &self.root);
        if let Some(reason) = &session_dir {
            tracing::warn!(
                %reason,
                "own session directory is not safe; under enforce no command runs until it is"
            );
        }
        let profile = launch::resolve_profile(&self.root);
        let root = ServedRoot::pin(&self.root);
        let protected = xai_grok_sandbox::command::protected::floor(
            &xai_grok_sandbox::command::protected::ProtectedInputs {
                workspace_root: &root,
                grok_home: &self.grok_home,
                user_home: self.user_home.as_deref(),
                control_socket_dir: &self.control_socket_dir,
                git_env: &self.git_env,
            },
        );
        let store = GrantStore::open_in(
            &self.grok_home,
            &self.root,
            protected,
            self.user_home.as_deref(),
            self.clock.clone(),
        )
        .await;
        let backend = match self.backend_source.lock().take() {
            Some(BackendSource::Fixed(backend)) => backend,
            Some(BackendSource::Detect) | None => probe.as_ref().and_then(detect_backend),
        };
        tracing::info!(
            root = %self.root.display(),
            backend = ?backend.as_ref().map(|b| b.name()),
            grants = store.live().len(),
            "per-command sandbox ready"
        );
        #[cfg(test)]
        self.engagements
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let live = LiveRows {
            shared: store.live_shared(),
            sessions: store.live_session_rows(),
        };
        Engaged {
            root,
            tmp_dirs: daemon_tmp_dirs(),
            profile,
            backend,
            store: tokio::sync::Mutex::new(store),
            live: parking_lot::RwLock::new(live),
            session_dir: parking_lot::RwLock::new(session_dir),
        }
    }

    /// How many times the folder engaged (tests).
    #[cfg(test)]
    pub(crate) fn engagements(&self) -> usize {
        self.engagements.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// How many syncs a call's end scheduled so far (tests).
    #[cfg(test)]
    pub(crate) fn proxy_resyncs(&self) -> usize {
        self.proxy_resyncs.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// One scheduled sync has run to its end (tests): the proxy it stopped is closed, or it kept
    /// it. Bounded, so a sync that never ran fails the test instead of hanging it.
    #[cfg(test)]
    pub(crate) async fn proxy_resynced(&self) {
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            self.proxy_resynced.notified(),
        )
        .await
        .expect("the sync a call's end scheduled ran");
    }

    /// Run `task` owned by this sandbox: cancelled with it, never detached.
    pub(crate) fn spawn_owned(&self, task: impl Future<Output = ()> + Send + 'static) {
        self.tasks.spawn(task);
    }

    /// Hub session `session_id`'s turn to be shown a grantable sandbox card: held while one
    /// card waits for its answer, so a second violation in the session (another command's, or a
    /// connection the proxy held) posts its card only once the first is settled. Entries of
    /// sessions with no card waiting are dropped as they are met.
    pub(crate) async fn card_turn(&self, session_id: &str) -> tokio::sync::OwnedMutexGuard<()> {
        let turn = {
            let mut turns = self.card_turns.lock();
            turns.retain(|_, turn| turn.strong_count() > 0);
            match turns.get(session_id).and_then(Weak::upgrade) {
                Some(turn) => turn,
                None => {
                    let turn = Arc::new(tokio::sync::Mutex::new(()));
                    turns.insert(session_id.to_owned(), Arc::downgrade(&turn));
                    turn
                }
            }
        };
        turn.lock_owned().await
    }

    /// How many spawns are prepared and not yet finished (tests and `sandbox.status`).
    pub fn open_calls(&self) -> usize {
        self.calls.lock().prepared()
    }

    pub fn workspace_root(&self) -> &Path {
        &self.root
    }

    /// The files the mode is read from: the user's and the folder's `workspaced.toml`.
    pub(crate) fn mode_layers(&self) -> [PathBuf; 2] {
        [
            crate::sandbox_mode::user_config_path(&self.grok_home),
            crate::sandbox_mode::workspace_config_path(&self.root),
        ]
    }

    /// The store's clock: grant rows are stamped with it so their expiry is judged by the same
    /// clock that later reads them.
    pub fn now_unix(&self) -> i64 {
        self.clock.now_unix()
    }

    /// The backend, once the folder engaged; `None` before that.
    pub fn backend_name(&self) -> Option<BackendName> {
        self.backend().map(SandboxBackend::name)
    }

    fn backend(&self) -> Option<&dyn SandboxBackend> {
        self.engaged.get()?.backend.as_deref()
    }

    /// Whether the backend enforces less than the full model (the card's "(reduced sandbox)").
    pub fn reduced_sandbox(&self) -> bool {
        self.backend().is_some_and(|b| b.capabilities().reduced)
    }

    /// The mode as the layers resolve it (`GROK_SANDBOX_MODE`, the managed file's rollout switch,
    /// the user file, the workspace's file, default `off`), kept until one of its inputs moves or
    /// `sandbox.mode.set` invalidates it. A call reads it once, at [`WorkspaceSandbox::pin_mode`]
    /// or its first `prepare`, and runs under that mode to its end.
    pub fn mode(&self) -> SandboxMode {
        self.resolved_mode().mode
    }

    /// The mode as it was last resolved, from memory: no stamp, no file. What the folder's
    /// proxy decides a request with the session token under, and what tells it whether a
    /// request with no credential is admitted — a connection task never stats a config file —
    /// refreshed by the daemon's own reads: before each shell command it dispatches
    /// (`prepare`), on each `sync_network`, and by `sandbox.mode.set`. A flip made by editing a
    /// file therefore reaches the proxy with the next command or sync. A request with a call's
    /// credential is decided under the call's own mode, not this. With nothing resolved yet (a
    /// proxy started before any command), the layers are read once.
    pub fn published_mode(&self) -> SandboxMode {
        let published = self
            .mode
            .lock()
            .resolved
            .as_ref()
            .map(|(_, resolved)| resolved.mode);
        published.unwrap_or_else(|| self.mode())
    }

    /// [`WorkspaceSandbox::mode`] with the layer it came from (`sandbox.status`'s `source`). The
    /// layers are read with no lock held: while one reader reads
    /// them, the others get the last resolution, and with none yet they read the layers too.
    pub fn resolved_mode(&self) -> ResolvedSandboxMode {
        let stamp = self.mode_stamp();
        let generation = {
            let mut cache = self.mode.lock();
            cache.stamps += 1;
            match &cache.resolved {
                Some((cached, resolved)) if *cached == stamp || cache.resolving => {
                    return *resolved;
                }
                Some(_) | None => {}
            }
            cache.resolving = true;
            cache.generation
        };
        let resolved = self.resolve_mode_now(stamp.env.as_deref());
        let mut cache = self.mode.lock();
        cache.resolving = false;
        cache.resolutions += 1;
        if cache.generation != generation {
            return resolved;
        }
        if let Some((_, previous)) = &cache.resolved
            && previous.mode != resolved.mode
        {
            tracing::info!(
                root = %self.root.display(),
                from = <&str>::from(previous.mode),
                to = <&str>::from(resolved.mode),
                source = ?resolved.source,
                "sandbox mode changed on disk"
            );
        }
        cache.resolved = Some((stamp, resolved));
        resolved
    }

    /// How many times the mode was resolved from disk (tests).
    #[cfg(test)]
    pub(crate) fn mode_resolutions(&self) -> u64 {
        self.mode.lock().resolutions
    }

    /// How many times the layers' files were stat-ed for the mode's stamp (tests: the proxy's
    /// decisions add none).
    #[cfg(test)]
    pub(crate) fn mode_stamps(&self) -> u64 {
        self.mode.lock().stamps
    }

    /// What `finish` does on a decoded violation, for tests that settle one without a spawn:
    /// the settlement opens under `enforce`, the one mode a violation is decoded under.
    #[cfg(test)]
    pub(crate) fn open_settlement(&self, call_id: &CallId) -> Result<(), WorkspaceSandboxError> {
        let evicted = {
            let mut calls = self.calls.lock();
            let evicted = calls.open_settlement(call_id, SandboxMode::Enforce)?;
            self.revoke_released(&evicted);
            evicted
        };
        self.release_ended_holds(evicted);
        Ok(())
    }

    fn mode_stamp(&self) -> ModeStamp {
        ModeStamp {
            env: std::env::var(SANDBOX_MODE_ENV).ok(),
            workspace_file: file_stamp(&crate::sandbox_mode::workspace_config_path(&self.root)),
            user_file: file_stamp(&crate::sandbox_mode::user_config_path(&self.grok_home)),
            managed_file: file_stamp(&crate::sandbox_mode::managed_config_path(&self.grok_home)),
        }
    }

    /// The layers read from disk. Before the folder engaged a backend is assumed: only an
    /// `enforce` resolves differently without one, and engaging resolves again when it has none.
    fn resolve_mode_now(&self, env: Option<&str>) -> ResolvedSandboxMode {
        #[cfg(test)]
        {
            let hold = self.mode.lock().read_hold.take();
            if let Some(hold) = hold {
                let _ = hold.entered.send(());
                let _ = hold.release.recv();
            }
        }
        resolve_sandbox_mode_in(self.mode_inputs(env))
    }

    /// The layers' inputs: the explicit fleet layer when the config carries one, else the
    /// resolver reads the host's managed config file for it. Before the folder engaged a backend
    /// is assumed.
    fn mode_inputs<'a>(&'a self, env: Option<&'a str>) -> SandboxModeInputs<'a> {
        SandboxModeInputs {
            workspace_root: &self.root,
            grok_home: &self.grok_home,
            env,
            remote: self.remote.as_ref(),
            backend_available: self
                .engaged
                .get()
                .is_none_or(|engaged| engaged.backend.is_some()),
            writable: &self.writable,
        }
    }

    /// `sandbox.mode.set`: write `mode` into the folder's `.grok/workspaced.toml` and answer with
    /// the mode the layers resolve to *now* and its source, which the workspace layer (tighten
    /// only) may not have set. The caller then runs [`WorkspaceSandbox::sync_network`], which
    /// engages the folder when the mode is not `off`. A command already running keeps its mode
    /// ([`WorkspaceSandbox::running_under_another_mode`]).
    ///
    /// # Errors
    /// [`set_workspace_mode_in`]'s: a root that is not a directory, an unwritable `.grok/`, a file
    /// that is not TOML.
    pub fn set_workspace_mode(
        &self,
        mode: SandboxMode,
    ) -> Result<(PathBuf, ResolvedSandboxMode), SandboxModeWriteError> {
        let env = std::env::var(SANDBOX_MODE_ENV).ok();
        let from = self.published_mode();
        let (path, _) = set_workspace_mode_in(mode, self.mode_inputs(env.as_deref()))?;
        // Read again through the cache, so the copy the proxy's decider reads moves with it
        self.forget_resolution();
        let effective = self.resolved_mode();
        tracing::info!(
            root = %self.root.display(),
            from = <&str>::from(from),
            to = <&str>::from(effective.mode),
            source = ?effective.source,
            still_running = self.running_under_another_mode(effective.mode),
            "sandbox mode set; running commands keep the mode they started under"
        );
        Ok((path, effective))
    }

    /// A layer was written: the next reader reads the layers again, and a resolution begun
    /// before now is answered but not kept.
    fn forget_resolution(&self) {
        let mut cache = self.mode.lock();
        cache.resolved = None;
        cache.generation += 1;
    }

    /// Stage `between` to run inside the next `prepare`, after its policy is built and before
    /// its credential is minted: a stop racing the spawn.
    #[cfg(test)]
    pub(super) fn stage_before_next_mint_for_test(
        &self,
        between: impl FnOnce(&WorkspaceSandbox) + Send + 'static,
    ) {
        *self.before_mint.lock() = Some(Box::new(between));
    }

    /// What a test staged for this spawn, if anything; run once, between the build and the mint.
    #[cfg(test)]
    pub(super) fn run_staged_before_mint_for_test(&self) {
        let staged = self.before_mint.lock().take();
        if let Some(between) = staged {
            between(self);
        }
    }

    /// Hold the next layer read: the receiver hears once a reader is inside it, and the reader
    /// goes on reading when the sender sends or is dropped.
    #[cfg(test)]
    pub(crate) fn hold_next_mode_read(
        &self,
    ) -> (std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>) {
        let (entered, inside) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel();
        self.mode.lock().read_hold = Some(ModeReadHold {
            entered,
            release: released,
        });
        (inside, release)
    }

    /// Why the folder's session directory is not safe, checked again while it is not, so a
    /// failure that was transient heals; `None` once it is a private directory of this user, or
    /// before the folder engaged.
    pub fn session_dir_unsafe(&self) -> Option<String> {
        let engaged = self.engaged.get()?;
        if engaged.session_dir.read().is_none() {
            return None;
        }
        // Checked with the lock released: it creates and stats a directory
        let gap = session_dir_gap(&self.grok_home, &self.root);
        engaged.session_dir.write().clone_from(&gap);
        gap
    }

    /// What bounds a proposal or a typed folder for this workspace; `extra_bases` are the bases
    /// one command's environment named.
    pub(crate) fn bounds<'a>(&'a self, extra_bases: &'a [PathBuf]) -> ProposalBounds<'a> {
        ProposalBounds {
            workspace_root: &self.root,
            user_home: self.user_home.as_deref(),
            extra_bases,
        }
    }

    /// The result path for a shell command that went through `prepare`: under `enforce`, decode the
    /// exit status and `output`, the merged stdout+stderr. Under `observe` nothing is decoded, and
    /// the call ran with no proxy, so nothing is recorded here.
    pub async fn finish(&self, call: &CallId, exit: CommandExit, output: &[u8]) -> Finished {
        // The command is over: its proxy token stops working now, before anything is decoded,
        // and what its connections still hold is refused
        let (record, released) = {
            let mut calls = self.calls.lock();
            let (record, released) = calls.take_prepared(call);
            self.revoke_released(&released);
            (record, released)
        };
        self.release_ended_holds(released);
        let Some(record) = record else {
            return Finished::Ran;
        };
        let finished = match record.mode {
            SandboxMode::Off => Finished::Ran,
            SandboxMode::Observe => {
                metrics::command(record.mode, record.backend, SandboxCommandOutcome::Ran);
                Finished::Ran
            }
            SandboxMode::Enforce => {
                let input = || DecodeInput {
                    exit,
                    output,
                    partial_output: None,
                    cwd: &record.original.cwd,
                    argv: &record.original.args,
                    policy: &record.policy,
                    ran_sandboxed: record.sandboxed,
                    bounds: self.bounds(&record.extra_bases),
                };
                let finished = match decode(input()) {
                    Some(violation) => Finished::Violation(FinishedViolation {
                        violation,
                        mode: record.mode,
                        backend: record.backend,
                        replayed_under: record.replayed_under,
                    }),
                    // A replay stopped on the target its grant allows is the backend's
                    // limitation, not a violation to card and not the OS's refusal
                    None if record.replayed_under.is_some() => refused_under_grant(input())
                        .map_or(Finished::Ran, Finished::RefusedUnderGrant),
                    None => Finished::Ran,
                };
                let outcome = match finished {
                    Finished::Ran => SandboxCommandOutcome::Ran,
                    Finished::Violation(_) | Finished::RefusedUnderGrant(_) => {
                        SandboxCommandOutcome::Violation
                    }
                };
                metrics::command(record.mode, record.backend, outcome);
                finished
            }
        };
        // Only a decoded violation can lead to another spawn of this call (the replay), so only
        // then does what was settled while it ran carry over, the call's mode with it
        let evicted = {
            let mut calls = self.calls.lock();
            if finished.violation().is_some() {
                // A call the hub holds keeps its entry; only a stray's can be refused, and
                // nothing replays a stray
                match calls.open_settlement(call, record.mode) {
                    Ok(evicted) => {
                        self.revoke_released(&evicted);
                        Some(evicted)
                    }
                    Err(refused) => {
                        tracing::warn!(%call, ?refused, "the violation settles into no replay");
                        None
                    }
                }
            } else {
                calls.close_settlement(call);
                None
            }
        };
        if let Some(evicted) = evicted {
            self.release_ended_holds(evicted);
        }
        finished
    }

    /// Record an observation from the proxy's `blocked_requests` stream, keyed by the host the card
    /// would propose, with the verdict `enforce` would have reached.
    pub fn observe_blocked(
        &self,
        blocked: &xai_grok_sandbox::command::Blocked,
        verdict: xai_grok_sandbox::command::WouldVerdict,
    ) {
        let now = self.clock.now_unix();
        self.observe.lock().record(blocked, verdict, now);
        metrics::violation(SandboxMode::Observe, blocked, SandboxSettlement::Observed);
    }

    pub fn observe_summary(&self) -> ObserveSummary {
        self.observe.lock().clone()
    }

    /// The gate is about to run `call` a second time under a grant for `granted`: its next
    /// `prepare` is the replay, after which a violation is final (never a third run). A mark for
    /// a call whose result is already in is dropped.
    pub fn mark_replay(&self, call: &CallId, granted: GrantSubject) {
        self.calls.lock().mark_replay(call, granted);
    }

    /// The hub dispatched `call_id` under `mode`, the folder's mode as it read it to choose the
    /// call's path (the sandbox card or the pre-run prompt): the call's spawn, its retry, its
    /// replay and every connection the proxy decides for it run under that mode, even if the
    /// folder's mode changes before or while it runs. A second pin for a live call changes
    /// nothing; the call's final result drops the pin, and a call the hub never pinned takes the
    /// folder's mode at its first spawn.
    ///
    /// # Errors
    /// [`WorkspaceSandboxError::CallTableFull`]: there is no room for the call. A gated call must
    /// not run then; one approved at the pre-run prompt runs unpinned under its floor
    /// ([`WorkspaceSandbox::floor_mode`]), its spawn refused at `prepare` while the table is full.
    pub fn pin_mode(
        &self,
        call_id: &CallId,
        mode: SandboxMode,
    ) -> Result<(), WorkspaceSandboxError> {
        let evicted = {
            let mut calls = self.calls.lock();
            let evicted = calls.pin_mode(call_id, mode)?;
            self.revoke_released(&evicted);
            evicted
        };
        self.release_ended_holds(evicted);
        Ok(())
    }

    /// [`WorkspaceSandbox::pin_mode`] had no room for `call_id`, dispatched under `mode`: its spawn
    /// runs under the folder's mode then or `mode`, whichever is stronger, so a flip before the
    /// spawn cannot weaken it. `false`: no room for a floor either, and the call must not run.
    #[must_use]
    pub fn floor_mode(&self, call_id: &CallId, mode: SandboxMode) -> bool {
        self.calls.lock().floor_mode(call_id, mode)
    }

    /// The floor [`WorkspaceSandbox::floor_mode`] left for `call_id`, the least its spawn runs
    /// under; `None` for a call the table pinned or never had to floor.
    pub fn floor_of(&self, call_id: &CallId) -> Option<SandboxMode> {
        self.calls.lock().floor_of(call_id)
    }

    /// The mode `call` runs under, whatever the folder's is by now: pinned at dispatch or fixed
    /// by its spawn. `None` for a call the table holds no mode for.
    pub fn held_mode(&self, call: &CallId) -> Option<SandboxMode> {
        self.calls.lock().held_mode(call)
    }

    /// How many live calls run under a mode other than `mode`: what a change to `mode` leaves
    /// running as it was, for `sandbox.mode.set`'s reply.
    pub fn running_under_another_mode(&self, mode: SandboxMode) -> usize {
        self.calls.lock().running_under_another_mode(mode)
    }
}
