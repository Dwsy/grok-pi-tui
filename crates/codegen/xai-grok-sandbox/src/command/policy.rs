//! Policy as data: what one command may read, write, and reach. Built from the workspace's
//! [`crate::SandboxProfile`] plus the live grants, rendered by a [`crate::command::SandboxBackend`].
//! `protected` is applied last so no grant can widen into it. This file holds the types and
//! [`SandboxPolicy::build`]; the default tables live with the floor in [`protected`], which never
//! reads this module, and widening by a grant lives in [`apply`].

mod apply;

pub(crate) use apply::symlink_free_spelling;
pub use protected::{
    BUILD_CACHE_TREES, GROK_HOME_SECRET_GLOBS, SECRET_READ_DENY_DIRS, SECRET_READ_DENY_FILES,
    VERIFIED_BUILD_CACHES, default_tmp_dirs,
};

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use serde::{Deserialize, Serialize};

use crate::command::canonical::{
    PathGlob, RootMoved, ServedRoot, canonical_path, contains_path, dedup_paths, fold_dots,
    is_same_path, is_within, strip_within,
};
use crate::command::env::{EnvGlobs, InvalidEnvGlob};
use crate::command::git_config::{GitConfigEnv, GitMetadataUnread};
use crate::command::grant_store::recorded_write_roots;
use crate::command::grants::{Grant, GrantDecision, GrantSubject};
use crate::command::protected::{self, Protected, ProtectedInputs};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxPolicy {
    pub read: ReadPolicy,
    /// Canonical roots a command may write under, minus whatever `protected` carves out of them.
    pub write_roots: Vec<PathBuf>,
    pub network: NetworkPolicy,
    pub env: EnvPolicy,
    /// The floor: never writable, never grantable, rendered after every allow by every backend.
    /// The one source of truth for what stays read-only inside a writable root.
    pub protected: Vec<Protected>,
    /// The curated build-cache trees under the user's home ([`BUILD_CACHE_TREES`]), canonical:
    /// a [`GrantSubject::BuildCaches`] grant adds them to `write_roots`; until then a write into
    /// one outside its verified subpaths is the violation that proposes that grant.
    /// Empty on a host with no home directory.
    pub build_cache_trees: Vec<PathBuf>,
    /// Git files the floor had to read and could not (too large, unreadable, past the include
    /// bounds): the hooks path such a file may set is unknown, so enforce runs no command while
    /// one is listed; observe records and runs as before.
    #[serde(default)]
    pub unread_git_metadata: Vec<GitMetadataUnread>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReadPolicy {
    /// Read everywhere except the deny entries.
    AllExcept { deny: Vec<DenyEntry> },
    /// Read only under the roots, and never the deny entries there: a profile without default read.
    Roots {
        roots: Vec<PathBuf>,
        deny: Vec<DenyEntry>,
    },
}

impl ReadPolicy {
    /// The read denies, in every mode: the secret stores, the grok home's auth material, the
    /// daemon's own files and the profile's entries. Every backend renders them after its read
    /// allows and [`SandboxPolicy::would_allow`] asks them first, so no mode can drop them.
    pub fn deny(&self) -> &[DenyEntry] {
        match self {
            ReadPolicy::AllExcept { deny } | ReadPolicy::Roots { deny, .. } => deny,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum DenyEntry {
    Path(PathBuf),
    /// A gitignore-style `tail` (the dialect `crate::deny` validates on both platforms) below the
    /// literal directory `root`: the workspace, the home directory or the grok home for an
    /// anchored pattern, `/` for an absolute one. The root is matched as written and never parsed
    /// as a pattern, so a `[` in a workspace's own path stays part of its name.
    Glob {
        root: PathBuf,
        tail: String,
    },
    /// Everything under `tree` except `except`: the other workspaces' session directories.
    TreeExcept {
        tree: PathBuf,
        except: PathBuf,
    },
}

impl DenyEntry {
    /// Whether the entry denies `path`, as the floor's [`is_within`] compares (APFS case, Unicode
    /// form and `/private` alias on macOS). A glob that does not parse covers every path, so the
    /// classification never reads as allowed a read the renderer would refuse to render.
    pub(crate) fn covers(&self, path: &Path) -> bool {
        match self {
            DenyEntry::Path(root) => is_within(path, root),
            DenyEntry::Glob { root, tail } => {
                let Some(glob) = PathGlob::new(tail, false) else {
                    return true;
                };
                strip_within(path, root).is_some_and(|below| glob.is_match(&below))
            }
            DenyEntry::TreeExcept { tree, except } => {
                is_within(path, tree) && !is_within(path, except)
            }
        }
    }
}

/// The kernel policy is always `Off` or `Proxy`: a network grant, including
/// "all hosts", is a decision the egress proxy's decider makes and never changes the kernel
/// policy, so the control socket and the runtime sockets stay unreachable whatever the user
/// allowed. Name resolution goes through the proxy too (the child never opens the resolver).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NetworkPolicy {
    Off,
    /// Only loopback to the egress proxy on `port`.
    Proxy {
        port: u16,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvPolicy {
    /// Variable names removed from the child environment.
    pub exclude_globs: EnvGlobs,
    /// Variables set after exclusion (the proxy pointers).
    pub set: BTreeMap<String, String>,
}

impl EnvPolicy {
    /// On by default: the child otherwise inherits every secret in the daemon's environment.
    pub fn default_excludes() -> Vec<String> {
        [
            "*KEY*",
            "*SECRET*",
            "*TOKEN*",
            "*PASSWORD*",
            "*CREDENTIAL*",
            "*_PAT",
            "LD_PRELOAD",
            "LD_LIBRARY_PATH",
            "DYLD_*",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    }

    /// The proxy pointers every HTTP client honours, plus loopback left direct.
    pub fn proxy_vars(port: u16) -> BTreeMap<String, String> {
        Self::proxy_vars_at(&format!("http://127.0.0.1:{port}"))
    }

    /// [`Self::proxy_vars`] for a full proxy URL, so the pointer can carry the per-call
    /// credential (`http://grok:<token>@127.0.0.1:<port>`). Both spellings are set: curl reads
    /// only `http_proxy` for plain-HTTP targets (the upper-case name is ignored there as a CGI
    /// precaution), while most other clients read the upper-case one first.
    pub fn proxy_vars_at(url: &str) -> BTreeMap<String, String> {
        const NO_PROXY: &str = "localhost,127.0.0.1,::1";
        BTreeMap::from([
            ("HTTP_PROXY".to_owned(), url.to_owned()),
            ("HTTPS_PROXY".to_owned(), url.to_owned()),
            ("ALL_PROXY".to_owned(), url.to_owned()),
            ("NO_PROXY".to_owned(), NO_PROXY.to_owned()),
            ("http_proxy".to_owned(), url.to_owned()),
            ("https_proxy".to_owned(), url.to_owned()),
            ("all_proxy".to_owned(), url.to_owned()),
            ("no_proxy".to_owned(), NO_PROXY.to_owned()),
        ])
    }

    /// Whether `name` matches an exclusion glob (ASCII case-insensitive, as environment variable
    /// names conventionally are).
    pub fn excludes(&self, name: &str) -> bool {
        self.exclude_globs.is_match(OsStr::new(name))
    }
}

/// The folder's egress proxy, listening on loopback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProxyEndpoint {
    pub port: u16,
}

/// Everything [`SandboxPolicy::build`] reads. `grants` are the rows
/// [`crate::command::grant_store::allows_not_denied`] keeps: the live allows no deny covers,
/// and the denies that overlap one.
pub struct PolicyInputs<'a> {
    /// Pinned when the folder was served; the build checks it first and then anchors every
    /// workspace rule at its real path.
    pub workspace_root: &'a ServedRoot,
    pub profile: &'a crate::SandboxProfile,
    pub grants: &'a [Grant],
    pub proxy: Option<ProxyEndpoint>,
    /// Pinned once per daemon ([`daemon_tmp_dirs`]); the build checks each and writes its real
    /// path, never resolved again.
    pub tmp_dirs: &'a [ServedRoot],
    /// The daemon's own endpoint directory: protected, read-denied and denied for unix sockets.
    /// Required: a policy without it would leave the daemon's own
    /// socket reachable from the command.
    pub control_socket_dir: &'a Path,
    /// `xai_grok_config::grok_home()` in production; injected so tests never read the environment.
    pub grok_home: &'a Path,
    /// `xai_dirs::home_dir()` in production; `None` when the host has no home directory.
    pub user_home: Option<&'a Path>,
    /// [`GitConfigEnv::from_host`] in production: where the floor reads the user's global git
    /// config; injected so tests never read the environment.
    pub git_env: &'a GitConfigEnv,
}

#[derive(Debug, thiserror::Error)]
pub enum PolicyError {
    #[error("grant target is protected and can never be allowed: {path}")]
    Protected { path: PathBuf },
    #[error("grant target is denied by a `deny` entry of the sandbox profile: {path}")]
    ProfileDenied { path: PathBuf },
    #[error("path is not absolute: {path}")]
    NotAbsolute { path: PathBuf },
    #[error("writable root has a symlink in a non-top-level component: {path}")]
    SymlinkedRoot { path: PathBuf },
    #[error("cannot tell whether a component of {path} is a symlink: {reason}")]
    UninspectableRoot { path: PathBuf, reason: String },
    #[error(
        "{path} has another hard link, {alias} (st_nlink={nlink}), that a command could write it \
         through; remove the extra link or run this folder in observe mode"
    )]
    HardLinkedProtected {
        path: PathBuf,
        nlink: u64,
        alias: PathBuf,
    },
    #[error(
        "{path} has another hard link, and enforce cannot verify it lies outside {root}: too \
         many entries to search, or a directory it cannot list; remove the extra link or run \
         this folder in observe mode"
    )]
    HardLinkUnverified { path: PathBuf, root: PathBuf },
    #[error(
        "{tree} cannot be checked for hard-linked protected files: too many entries, or a \
         directory it cannot list; move some out or run this folder in observe mode"
    )]
    ProtectedTreeUnchecked { tree: PathBuf },
    #[error(
        "{unread}; the hooks path it may set is unknown, so enforce runs no command; fix the \
         file or run this folder in observe mode"
    )]
    GitMetadataUnread { unread: GitMetadataUnread },
    #[error(transparent)]
    InvalidEnvGlob(#[from] InvalidEnvGlob),
    #[error(transparent)]
    RootMoved(#[from] RootMoved),
    #[error(
        "the temporary directory {} now resolves to {}; restart the daemon",
        .0.pinned.display(),
        .0.now.display()
    )]
    TmpDirMoved(RootMoved),
}

impl SandboxPolicy {
    /// profile ∪ live grants, protected list applied last so a grant can never widen into it. A
    /// live grant [`SandboxPolicy::apply_grant`] refuses (its root now protected or reached
    /// through a symlink) is left out and logged: one stale row narrows the policy, it never
    /// refuses every command. A curated cache path with a symlinked component is left out the
    /// same way. The deny rows among the grants close what they overlap of an allow of the other
    /// write kind ([`SandboxPolicy::apply_grant_under`]).
    ///
    /// # Errors
    /// [`PolicyError::NotAbsolute`] for a relative workspace root, or for a `~` deny entry on a
    /// host with no home directory; [`PolicyError::RootMoved`] once the root's spelling resolves
    /// anywhere but its pinned real path ([`ServedRoot::check`]), [`PolicyError::TmpDirMoved`]
    /// once a temporary directory's does; [`PolicyError::InvalidEnvGlob`] when the exclusion
    /// table does not compile.
    pub fn build(inputs: PolicyInputs<'_>) -> Result<SandboxPolicy, PolicyError> {
        let spelled = inputs.workspace_root.spelled();
        if !spelled.is_absolute() {
            return Err(PolicyError::NotAbsolute {
                path: spelled.to_path_buf(),
            });
        }
        inputs.workspace_root.check()?;
        for dir in inputs.tmp_dirs {
            dir.check().map_err(PolicyError::TmpDirMoved)?;
        }
        SandboxPolicy::build_at_pin(&inputs)
    }

    /// [`SandboxPolicy::build`] past the root's check: nothing from here resolves the root's
    /// spelling, so a link retargeted since the check moves no rule.
    fn build_at_pin(inputs: &PolicyInputs<'_>) -> Result<SandboxPolicy, PolicyError> {
        let protected_inputs = ProtectedInputs {
            workspace_root: inputs.workspace_root,
            grok_home: inputs.grok_home,
            user_home: inputs.user_home,
            control_socket_dir: inputs.control_socket_dir,
            git_env: inputs.git_env,
        };
        let floor = protected::floor_with_unread(&protected_inputs);
        let grok_home = canonical_path(inputs.grok_home);
        let own_session_dir = protected::own_session_dir(&protected_inputs);
        let deny = read_denies_for(inputs, &grok_home, own_session_dir)?;
        let (network, set) = network_for(inputs.proxy);
        let mut policy = SandboxPolicy {
            read: read_policy_for(inputs.profile, inputs.workspace_root, deny),
            write_roots: write_roots_for(inputs, &protected_inputs, &grok_home),
            network,
            env: EnvPolicy {
                exclude_globs: EnvGlobs::new(EnvPolicy::default_excludes())?,
                set,
            },
            protected: protected_for(floor.protected, inputs.profile, inputs.workspace_root),
            build_cache_trees: build_cache_trees_for(inputs.user_home),
            unread_git_metadata: floor.unread_git_metadata,
        };
        let denies: Vec<&GrantSubject> = inputs
            .grants
            .iter()
            .filter(|grant| grant.decision == GrantDecision::Deny)
            .map(|grant| &grant.subject)
            .collect();
        for grant in inputs.grants {
            if let Err(error) = policy.apply_grant_under(grant, &denies) {
                tracing::warn!(grant = %grant.id, %error, "live grant no longer applies; left out");
            }
        }
        Ok(policy)
    }
}

/// The floor plus the hook sources the process-wide profile write-denies (a `hooks-paths` entry
/// anywhere too): grok runs the hooks they define outside any sandbox. A source spelled under the
/// workspace root is taken at the root's real path ([`ServedRoot::anchor`]).
fn protected_for(
    mut protected: Vec<Protected>,
    profile: &crate::SandboxProfile,
    root: &ServedRoot,
) -> Vec<Protected> {
    for source in profile.write_deny.iter().filter(|s| s.path.is_absolute()) {
        let source = root.anchor(&source.path);
        for path in [fold_dots(&source), canonical_path(&source)] {
            protected.push(Protected::Path { path });
        }
    }
    protected.sort();
    protected.dedup();
    protected
}

/// The workspace, the profile's absolute `read_write` entries and the default write roots, deduped:
/// pinned real paths as given, an entry under the workspace folded (left out through a link), the
/// rest canonical; of the grok home a child command gets only the session command directory.
fn write_roots_for(
    inputs: &PolicyInputs<'_>,
    protected_inputs: &ProtectedInputs<'_>,
    grok_home: &Path,
) -> Vec<PathBuf> {
    let root = inputs.workspace_root;
    let tmp_dirs: Vec<PathBuf> = inputs
        .tmp_dirs
        .iter()
        .map(|dir| dir.real().into())
        .collect();
    let mut write_roots: Vec<PathBuf> = vec![root.real().to_path_buf()];
    write_roots.extend(
        inputs
            .profile
            .read_write
            .iter()
            .filter(|path| path.is_absolute())
            .filter(|path| {
                !inputs
                    .tmp_dirs
                    .iter()
                    .any(|dir| is_same_path(path, dir.spelled()))
            })
            .map(|path| fold_dots(&root.anchor(path)))
            .filter(|path| !is_same_path(path, root.real()))
            .filter_map(|path| {
                if !is_within(&path, root.real()) {
                    return Some(canonical_path(&path));
                }
                symlink_free_spelling(&path)
                    .inspect_err(|error| tracing::warn!(%error, "profile write root left out"))
                    .ok()
            })
            .filter(|path| !is_same_path(path, grok_home)),
    );
    write_roots.extend(protected::default_write_roots(protected_inputs, &tmp_dirs));
    write_roots.sort();
    dedup_paths(&mut write_roots);
    write_roots
}

/// [`BUILD_CACHE_TREES`] under the canonical home, less any reached through a symlink; empty on a
/// host with no home directory.
fn build_cache_trees_for(user_home: Option<&Path>) -> Vec<PathBuf> {
    user_home
        .map(canonical_path)
        .map(|home| {
            BUILD_CACHE_TREES
                .iter()
                .filter(|rel| !protected::symlinked_below(&home, rel))
                .map(|rel| home.join(rel))
                .collect()
        })
        .unwrap_or_default()
}

/// Every place a command the daemon wraps may write, or may have written: a `workspaced.toml`
/// that is a symlink may not resolve into it, or a command could rewrite the mode. The temporary
/// directories and every build-cache tree (any folder's commands may be let write them), the
/// write roots of each policy built in this process ([`WritableLocations::record`], grants of
/// every scope included) and what the grok home records of every folder
/// ([`recorded_write_roots`]). A recorded root stays: a command may outlive its grant.
#[derive(Debug, Default)]
pub struct WritableLocations {
    seeded: Vec<PathBuf>,
    recorded: Mutex<Vec<PathBuf>>,
}

impl WritableLocations {
    /// Seeded with `tmp_dirs` and each build-cache tree under `user_home`, granted or not.
    pub fn new(user_home: Option<&Path>, tmp_dirs: &[PathBuf]) -> WritableLocations {
        let caches = user_home.map(canonical_path).into_iter().flat_map(|home| {
            BUILD_CACHE_TREES
                .iter()
                .map(move |rel| canonical_path(&home.join(rel)))
        });
        let mut seeded: Vec<PathBuf> = tmp_dirs
            .iter()
            .map(|dir| canonical_path(dir))
            .chain(caches)
            .collect();
        dedup_paths(&mut seeded);
        WritableLocations {
            seeded,
            recorded: Mutex::default(),
        }
    }

    /// The daemon's one set, seeded from [`daemon_tmp_dirs`] and the user's home on first use.
    pub fn daemon() -> Arc<WritableLocations> {
        static DAEMON: OnceLock<Arc<WritableLocations>> = OnceLock::new();
        DAEMON
            .get_or_init(|| {
                let home = xai_dirs::home_dir();
                let tmp_dirs: Vec<PathBuf> = daemon_tmp_dirs()
                    .iter()
                    .map(|dir| dir.real().into())
                    .collect();
                Arc::new(WritableLocations::new(home.as_deref(), &tmp_dirs))
            })
            .clone()
    }

    /// Adds `policy`'s write roots; called before any command runs under it.
    pub fn record(&self, policy: &SandboxPolicy) {
        let mut recorded = self.recorded.lock().unwrap_or_else(PoisonError::into_inner);
        for root in &policy.write_roots {
            if !contains_path(&recorded, root) {
                recorded.push(root.clone());
            }
        }
    }

    /// Whether `path`, spelled as resolved, lies where a command may write or may have written:
    /// in a seeded or recorded root, in `workspace_root`, or in a root the grok home records.
    ///
    /// # Errors
    /// What the grok home records cannot be read ([`recorded_write_roots`]): a command may then
    /// have been let write anywhere.
    pub fn holds(
        &self,
        path: &Path,
        workspace_root: &Path,
        grok_home: &Path,
    ) -> std::io::Result<bool> {
        let within = |root: &PathBuf| is_within(path, root);
        if self.seeded.iter().any(within) || is_within(path, &canonical_path(workspace_root)) {
            return Ok(true);
        }
        let recorded = self.recorded.lock().unwrap_or_else(PoisonError::into_inner);
        if recorded.iter().any(within) {
            return Ok(true);
        }
        drop(recorded);
        Ok(recorded_write_roots(grok_home)?.iter().any(within))
    }
}

/// The daemon's temporary directories ([`default_tmp_dirs`] of its `$TMPDIR`), each pinned on
/// first use for the daemon's lifetime: a directory swapped for a link later refuses every build.
pub fn daemon_tmp_dirs() -> &'static [ServedRoot] {
    static PINS: OnceLock<Vec<ServedRoot>> = OnceLock::new();
    PINS.get_or_init(|| {
        let tmpdir = std::env::var_os("TMPDIR").map(PathBuf::from);
        let home = xai_dirs::home_dir();
        let dirs = default_tmp_dirs(tmpdir.as_deref(), home.as_deref());
        dirs.into_iter().map(ServedRoot::pin).collect()
    })
}

/// The read denies every mode keeps ([`ReadPolicy::deny`]): the secret stores, the grok home's
/// auth material and grant locks, the other workspaces' sessions, the profile's `deny` entries
/// and the daemon's control-socket directory.
///
/// # Errors
/// [`PolicyError::NotAbsolute`] for a `~` deny entry on a host with no home directory.
fn read_denies_for(
    inputs: &PolicyInputs<'_>,
    grok_home: &Path,
    own_session_dir: PathBuf,
) -> Result<Vec<DenyEntry>, PolicyError> {
    let mut deny: Vec<DenyEntry> = Vec::new();
    if let Some(home) = inputs.user_home {
        let home = canonical_path(home);
        // A secret store spelled through a symlink is denied at its target too, as the floor
        // protects it there
        for secret in SECRET_READ_DENY_DIRS
            .iter()
            .chain(SECRET_READ_DENY_FILES)
            .map(|rel| home.join(rel))
        {
            let resolved = canonical_path(&secret);
            if resolved != secret {
                deny.push(DenyEntry::Path(resolved));
            }
            deny.push(DenyEntry::Path(secret));
        }
    }
    deny.extend(GROK_HOME_SECRET_GLOBS.iter().map(|glob| DenyEntry::Glob {
        root: grok_home.to_path_buf(),
        tail: (*glob).to_owned(),
    }));
    // The grant files' lock sidecars: a command able to open one could hold its `flock`
    deny.extend(
        [grok_home, &own_session_dir]
            .map(|dir| DenyEntry::Path(dir.join(protected::GRANTS_LOCK_FILENAME))),
    );
    // Other workspaces' transcripts and indexes are not this command's to read
    deny.push(DenyEntry::TreeExcept {
        tree: grok_home.join("sessions"),
        except: own_session_dir,
    });
    for entry in &inputs.profile.deny {
        deny.push(absolute_deny_entry(
            entry,
            inputs.workspace_root,
            inputs.user_home,
        )?);
    }
    deny.push(DenyEntry::Path(canonical_path(inputs.control_socket_dir)));
    Ok(deny)
}

/// Read everywhere but the denies, or, for a profile without default read, only under its
/// `read_only` roots, resolved once here as write roots are (one in the workspace only folded: a
/// command may have linked it elsewhere), and the workspace's pinned real path.
fn read_policy_for(
    profile: &crate::SandboxProfile,
    workspace_root: &ServedRoot,
    deny: Vec<DenyEntry>,
) -> ReadPolicy {
    if profile.default_read {
        ReadPolicy::AllExcept { deny }
    } else {
        let real = workspace_root.real();
        let mut roots: Vec<PathBuf> = profile
            .read_only
            .iter()
            .map(|root| fold_dots(&workspace_root.anchor(root)))
            .map(|root| {
                if is_within(&root, real) {
                    root
                } else {
                    canonical_path(&root)
                }
            })
            .collect();
        roots.push(real.to_path_buf());
        ReadPolicy::Roots { roots, deny }
    }
}

/// The kernel network policy and the proxy pointers: loopback to the folder's egress proxy, or
/// no network at all.
fn network_for(proxy: Option<ProxyEndpoint>) -> (NetworkPolicy, BTreeMap<String, String>) {
    match proxy {
        Some(proxy) => (
            NetworkPolicy::Proxy { port: proxy.port },
            EnvPolicy::proxy_vars(proxy.port),
        ),
        None => (NetworkPolicy::Off, BTreeMap::new()),
    }
}

/// A profile deny entry as the policy carries it: every path and glob is absolute. `~/…`
/// expands to the user's home; a relative entry (`.env`, `secrets/**`) is anchored at the
/// workspace, as `crate::deny` anchors the `sandbox.toml` dialect; an absolute one is taken as
/// given, or at the workspace's real path when spelled under it. The home is anchored in its
/// canonical spelling and the workspace at its pinned real path, as the write roots are: the
/// kernel meets `<ws>/.env` where the workspace resolves, not where its spelling points.
///
/// # Errors
/// [`PolicyError::NotAbsolute`] for a `~` entry on a host with no home directory.
fn absolute_deny_entry(
    entry: &Path,
    workspace_root: &ServedRoot,
    user_home: Option<&Path>,
) -> Result<DenyEntry, PolicyError> {
    let is_glob = entry.to_str().is_some_and(crate::deny::is_glob);
    let anchored = workspace_root.anchor(entry);
    let (root, rest) = if let Ok(rest) = entry.strip_prefix("~") {
        let home = user_home.ok_or_else(|| PolicyError::NotAbsolute {
            path: entry.to_path_buf(),
        })?;
        (canonical_path(home), rest)
    } else if let Ok(rest) = anchored.strip_prefix("/") {
        (PathBuf::from("/"), rest)
    } else {
        (workspace_root.real().to_path_buf(), entry)
    };
    Ok(if is_glob {
        DenyEntry::Glob {
            root,
            tail: rest.to_string_lossy().into_owned(),
        }
    } else {
        DenyEntry::Path(root.join(rest))
    })
}

#[cfg(test)]
#[path = "policy_tests.rs"]
mod tests;
