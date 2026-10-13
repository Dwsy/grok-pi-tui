//! The protected floor: paths no command may write and no grant may open, rendered after every
//! allow by every backend. One table, pinned by a test, so the desktop copy
//! ("Grok can never write to …"), the grant store's load-time filter and the backends agree on
//! the same set. Every entry is spelled canonically ([`canonical_path`]) and every question asked
//! of the floor canonicalises its path first, so `..`, a firmlink, a symlinked prefix or an APFS
//! case or normalisation variant never makes one path look like two.
//!
//! Sources of truth reused: [`xai_grok_config::TRUST_BOUNDARY_FILENAMES`] for the grok-home files
//! whose writability grants trust, and the hook directories the hook write-deny already protects.
//!
//! Accepted limit: only the workspace's own git directory and its submodules' are covered. A
//! repository nested in the workspace (`git init sub`, a vendored clone) keeps a writable
//! `.git/config` and `.git/hooks` that git later runs outside the sandbox; covering them would
//! put every `git init` or `git clone` in a workspace behind a card.

#[cfg(unix)]
use std::collections::{BTreeMap, VecDeque};
use std::ffi::OsStr;
#[cfg(unix)]
use std::ffi::{CString, OsString};
use std::io::Read as _;
#[cfg(unix)]
use std::io::Write as _;
#[cfg(unix)]
use std::os::fd::{AsFd as _, AsRawFd as _, BorrowedFd, FromRawFd as _, OwnedFd};
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt as _;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::sync::{Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use xai_grok_config::TRUST_BOUNDARY_FILENAMES;

use crate::command::canonical::{
    PathGlob, ServedRoot, canonical_path, dedup_paths, fold_dots, is_same_path, is_within,
};
use crate::command::git_config::{self, GitConfigEnv, GitMetadataUnread};

/// One floor entry. `covers` is the single matcher every consumer uses; the Seatbelt renderer
/// emits one filter shape per variant.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Protected {
    /// The path and everything beneath it.
    Path { path: PathBuf },
    /// An absolute gitignore-style glob (`<ws>/.git/modules/**/hooks`): every match and
    /// everything beneath it, whether or not the match exists yet.
    Glob { glob: String },
    /// Everything under `tree` except `except` and what lies beneath it: the other workspaces'
    /// session directories, present and future.
    TreeExcept { tree: PathBuf, except: PathBuf },
    /// This symlink itself, never what it points to: it cannot be re-pointed, and a path through
    /// it is judged where it leads.
    Node { path: PathBuf },
}

impl Protected {
    /// Whether `path` (as given) is this entry or lies beneath it. Component-wise, so `/ws/.gitx`
    /// is not covered by `/ws/.git/hooks`, and as the host's volumes compare ([`is_within`]).
    pub fn covers(&self, path: &Path) -> bool {
        match self {
            Protected::Path { path: root } => is_within(path, root),
            Protected::Node { path: node } => is_same_path(path, node),
            Protected::Glob { glob } => glob_matcher(glob)
                .is_some_and(|matcher| path.ancestors().any(|ancestor| matcher.is_match(ancestor))),
            Protected::TreeExcept { tree, except } => {
                is_within(path, tree) && !is_within(path, except)
            }
        }
    }

    /// Whether this entry can name something inside `root` — the path or the tree lies at or
    /// beneath it; for a glob, its literal prefix does, or `root` lies beneath the prefix, where
    /// a match can appear — so an allow of `root` must carve the entry out. (A root inside a
    /// tree-except's exception, the own session directory, needs no carve-out: the entry excepts
    /// it; a root anywhere else in the tree is refused as protected before it is rendered.)
    pub fn reaches_into(&self, root: &Path) -> bool {
        match self {
            Protected::Path { path } | Protected::Node { path } => is_within(path, root),
            Protected::TreeExcept { tree, .. } => is_within(tree, root),
            Protected::Glob { .. } => {
                let prefix = self.anchor();
                is_within(&prefix, root) || is_within(root, &prefix)
            }
        }
    }

    /// The literal path of a `Path` entry.
    pub fn as_path(&self) -> Option<&Path> {
        match self {
            Protected::Path { path } => Some(path),
            Protected::Glob { .. } | Protected::TreeExcept { .. } | Protected::Node { .. } => None,
        }
    }

    /// The node a rename could swap out from under this entry: the path, the tree, or a glob's
    /// literal prefix (its components before the first metacharacter).
    pub fn anchor(&self) -> PathBuf {
        match self {
            Protected::Path { path } | Protected::Node { path } => path.clone(),
            Protected::TreeExcept { tree, .. } => tree.clone(),
            Protected::Glob { glob } => Path::new(glob)
                .components()
                .take_while(|component| {
                    !component
                        .as_os_str()
                        .to_str()
                        .is_some_and(crate::deny::is_glob)
                })
                .collect(),
        }
    }
}

/// A directory itself, not a symlink to one or a file in its place: the only kind of ancestor an
/// unlink deny can pin.
pub fn is_real_directory(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_dir())
}

/// `*` stops at a separator and `**` spans directories, the reading the Seatbelt regex
/// translation gives the same glob; compared as [`is_within`] compares.
pub(super) fn glob_matcher(glob: &str) -> Option<PathGlob> {
    PathGlob::new(glob, true)
}

/// Whether the given path can never be written or granted: listed, beneath a listed entry, or
/// matched by a pattern — in its own (`..`-folded) spelling or its canonical one. Both are asked
/// so a symlinked or firmlinked prefix can neither hide a protected path nor be hidden by one.
pub fn is_protected(path: &Path, floor: &[Protected]) -> bool {
    let folded = fold_dots(path);
    let canonical = canonical_path(path);
    floor
        .iter()
        .any(|entry| entry.covers(&folded) || entry.covers(&canonical))
}

/// Whether no grant may name `root` and no card may offer it: [`is_protected`], or at or beneath
/// a glob entry's literal prefix, where a match can appear at any depth (a submodule added later
/// brings its `hooks`). A folder above the prefix stays grantable: the entry is carved out of it.
pub fn is_ungrantable(root: &Path, floor: &[Protected]) -> bool {
    if is_protected(root, floor) {
        return true;
    }
    let spellings = [fold_dots(root), canonical_path(root)];
    floor
        .iter()
        .filter(|entry| matches!(entry, Protected::Glob { .. }))
        .any(|entry| {
            let prefix = entry.anchor();
            spellings
                .iter()
                .any(|spelling| is_within(spelling, &prefix))
        })
}

/// Subpaths of the workspace root that stay read-only even though the root is writable: the
/// editors' and agents' own config trees. The git entries are derived from the repository's
/// layout by [`git_config::git_entries_in`].
pub const WORKSPACE_PROTECTED_SUBPATHS: &[&str] =
    &[".grok", ".cursor", ".claude", ".vscode", ".idea"];

/// The entries of a git directory that run or reconfigure code on the user's next git action:
/// `hooks/`, `config` and `config.worktree` (`core.hooksPath`, `core.fsmonitor`, filters; the
/// second is read under `extensions.worktreeConfig`) and `info/` (attributes and excludes).
/// Protected in the top-level `.git`, in every submodule's `.git/modules/**` and in the git
/// directory a worktree's `.git` file points at.
pub const GIT_DIR_PROTECTED_ENTRIES: &[&str] = &["hooks", "config", "config.worktree", "info"];

/// The git directory's name under a root. Its node is protected in every write root
/// (`SandboxPolicy::git_dir_nodes`), its contents only through [`GIT_DIR_PROTECTED_ENTRIES`].
pub const GIT_DIR_NAME: &str = ".git";

/// The daemon's own settings file in the grok home (`[sandbox] mode`, the user layer): a command
/// that could write it would switch the sandbox off for every command after it.
pub const DAEMON_SETTINGS_FILENAME: &str = "workspaced.toml";

/// Grok-home entries beyond [`TRUST_BOUNDARY_FILENAMES`]: hook sources, the global grant file and
/// its lock sidecar, the daemon's settings file, and everything grok runs, starts or loads into a
/// prompt from its home: the installed binary (`bin/grok`, a link into `downloads`), the vendored
/// search tools, plugins (installed, their data and the marketplace clones they install from),
/// skills, agents, personas, roles, rules, workflows and the bundled copies, memories, the user
/// guide the model reads, the MCP and LSP server configs the daemon starts what they name from,
/// extensions, and the CLI's own settings. The whole grok home is a floor tree too; a workspace
/// served from inside it (a grok-managed worktree) is the one thing there a command may write.
pub const GROK_HOME_PROTECTED_NAMES: &[&str] = &[
    "hooks",
    "hooks-paths",
    GLOBAL_GRANTS_FILENAME,
    GRANTS_LOCK_FILENAME,
    DAEMON_SETTINGS_FILENAME,
    "bin",
    "downloads",
    "vendor",
    "plugins",
    "installed-plugins",
    "plugin-data",
    "marketplace-cache",
    "skills",
    "agents",
    "personas",
    "roles",
    "rules",
    "workflows",
    "bundled",
    "memory",
    "memory-v2",
    "docs",
    "mcp.json",
    "lsp.json",
    "extensions",
    "pager.toml",
];

/// Home-relative files that run or reconfigure code on the next login, build, install or editor
/// start: the shell rc files (and the aliases and login environment they load), the git configs,
/// cargo's config, credentials and `env`, gradle's init script and properties, the maven, pip
/// and yarn configs (a registry or `yarnPath` swap), and vim's and tmux's rc files. The
/// credential files of [`SECRET_READ_DENY_FILES`] join them in the floor.
pub const HOME_PROTECTED_FILES: &[&str] = &[
    ".bashrc",
    ".bash_aliases",
    ".bash_login",
    ".bash_logout",
    ".bash_profile",
    ".profile",
    ".pam_environment",
    ".zshenv",
    ".zprofile",
    ".zshrc",
    ".zlogin",
    ".zlogout",
    ".config/fish/config.fish",
    ".gitconfig",
    ".config/git/config",
    ".cargo/config.toml",
    ".cargo/config",
    ".cargo/credentials.toml",
    ".cargo/credentials",
    ".cargo/env",
    ".gradle/init.gradle",
    ".gradle/gradle.properties",
    ".m2/settings.xml",
    ".pip/pip.conf",
    ".config/pip/pip.conf",
    ".yarnrc",
    ".yarnrc.yml",
    ".vimrc",
    ".tmux.conf",
];

/// Home-relative directories that run code without a login or on the next login: the user's
/// `PATH` entries, fish's auto-sourced snippets and autoloaded functions, gradle's init scripts,
/// neovim's config, the desktop session's autostart entries, user services and environment, and
/// macOS's persistence trees (launchd agents and daemons, the login-items store). The secret
/// stores of [`SECRET_READ_DENY_DIRS`] join them in the floor, so a card never offers "always
/// allow writes to `~/.ssh`" and no read grant opens them.
pub const HOME_PROTECTED_DIRS: &[&str] = &[
    ".local/bin",
    ".cargo/bin",
    ".config/fish/conf.d",
    ".config/fish/functions",
    ".gradle/init.d",
    ".config/nvim",
    ".config/autostart",
    ".config/systemd/user",
    ".config/environment.d",
    "Library/LaunchAgents",
    "Library/LaunchDaemons",
    "Library/Application Support/com.apple.backgroundtaskmanagementagent",
];

/// Home-relative toolchain, shell-framework, editor-plugin and CLI-plugin trees: code the user
/// runs next, unsandboxed (a rustup toolchain's `cargo`, an nvm node, oh-my-zsh from `.zshrc`,
/// vim plugins, VS Code extensions, docker CLI plugins). Each can hold 10⁵ files, so the
/// hard-link check lists one as far as its bound and never refuses on it, like a secret store.
pub const HOME_PROTECTED_TOOL_TREES: &[&str] = &[
    ".rustup/toolchains",
    ".nvm",
    ".oh-my-zsh",
    ".vim",
    ".vscode/extensions",
    ".docker/cli-plugins",
];

/// Home-relative build caches writable by default: cargo's lock files, which sit directly under
/// `~/.cargo` (cargo cannot run without them); npm's log directory, whose denial would print an
/// `EPERM` the decoder reads as the violation on every npm run; and two caches whose content is
/// checked on every read against a digest: npm's integrity-addressed `_cacache` and pip's HTTP
/// caches. That digest comes from a lockfile or from index metadata cached beside the content.
/// Not here, so in [`BUILD_CACHE_TREES`]' grant: cargo's `.crate` archives (a cached archive is
/// unpacked without its checksum), cargo's index (the checksums themselves), Go's download cache
/// (a cached zip is checked by its stored hash, not rehashed) and pip's `wheels` and `selfcheck`
/// (built wheels install unverified).
pub const VERIFIED_BUILD_CACHES: &[&str] = &[
    ".cargo/.package-cache",
    ".cargo/.package-cache-mutate",
    ".cargo/.global-cache",
    ".cargo/.global-cache-journal",
    ".npm/_cacache",
    ".npm/_logs",
    ".cache/pip/http",
    ".cache/pip/http-v2",
    "Library/Caches/pip/http",
    "Library/Caches/pip/http-v2",
];

/// Home-relative build-cache trees a [`crate::command::GrantSubject::BuildCaches`] grant makes
/// writable, as one family: what the toolchain unpacks, runs or trusts without re-verifying —
/// cargo's registry (archives, index, unpacked sources) and git checkouts, npm's `_npx`,
/// pre-commit's environments and pip's wheels under `~/.cache`, Go's module tree and download
/// cache, pnpm's store (linked into projects unverified), Gradle's and Maven's jars. A poisoned
/// file in one runs in the user's *unsandboxed* toolchain on the next build, so the first write
/// into a tree — outside its [`VERIFIED_BUILD_CACHES`] — is one card for the workspace. No tree
/// is in the floor, and `~/.cargo` itself is in neither table: its binaries, config and
/// credentials are floor entries ([`HOME_PROTECTED_FILES`], [`HOME_PROTECTED_DIRS`]).
pub const BUILD_CACHE_TREES: &[&str] = &[
    ".cargo/registry",
    ".cargo/git",
    ".npm",
    ".cache",
    "Library/Caches",
    ".pnpm-store",
    "go/pkg/mod",
    ".gradle/caches",
    ".m2/repository",
];

/// Home-relative secret stores denied for read regardless of profile and read mode. Every entry is
/// also in the protected floor, so no read or write grant can open one.
pub const SECRET_READ_DENY_DIRS: &[&str] = &[
    ".ssh",
    ".aws",
    ".gnupg",
    "Library/Keychains",
    "Library/Application Support/Google/Chrome",
    "Library/Application Support/Firefox",
    ".config/google-chrome",
    ".config/chromium",
    ".mozilla",
];

/// Home-relative credential files denied for read regardless of profile and read mode:
/// package-registry, container, cluster and forge tokens. Every entry is also in the floor.
pub const SECRET_READ_DENY_FILES: &[&str] = &[
    ".netrc",
    ".npmrc",
    ".pypirc",
    ".docker/config.json",
    ".kube/config",
    ".config/gh/hosts.yml",
    ".git-credentials",
];

/// Grok-home glob prefixes denied for read: the auth material.
pub const GROK_HOME_SECRET_GLOBS: &[&str] = &["auth*", "credentials*"];

/// The temporary directories a command may write by default: `$TMPDIR` (or the platform default)
/// plus the platform's shared temp roots. Every entry is absolute; a relative `$TMPDIR` is ignored.
/// `user_home` is kept for the per-user temp roots a later platform adds; macOS has none.
pub fn default_tmp_dirs(tmpdir_env: Option<&Path>, _user_home: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(dir) = tmpdir_env.filter(|dir| dir.is_absolute()) {
        dirs.push(dir.to_path_buf());
    }
    dirs.push(std::env::temp_dir());
    if cfg!(target_os = "macos") {
        dirs.push(PathBuf::from("/private/tmp"));
        dirs.push(PathBuf::from("/private/var/folders"));
    } else if cfg!(unix) {
        dirs.push(PathBuf::from("/tmp"));
        dirs.push(PathBuf::from("/var/tmp"));
    }
    dirs.retain(|dir| dir.is_absolute());
    dirs.sort();
    dedup_paths(&mut dirs);
    dirs
}

/// What a command may write before the workspace, profile and grants add to it, for the policy
/// and the floor's git entries: the temp directories (pinned, as given), the verified caches (one
/// with a symlinked component left out, [`symlinked_below`]) and the session command directory.
pub fn default_write_roots(inputs: &ProtectedInputs<'_>, tmp_dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = tmp_dirs.to_vec();
    if let Some(home) = inputs.user_home.map(canonical_path) {
        roots.extend(
            VERIFIED_BUILD_CACHES
                .iter()
                .filter(|rel| !symlinked_below(&home, rel))
                .map(|rel| home.join(rel)),
        );
    }
    if session_command_dir_is_writable(inputs) {
        roots.push(session_command_dir(inputs));
    }
    roots
}

/// Whether a component of the home-relative `rel` is a symlink. A curated cache path is kept in
/// its raw spelling (never followed), so a relocated or planted link leaves it out of the policy
/// instead of refusing every command at wrap.
pub(crate) fn symlinked_below(home: &Path, rel: &str) -> bool {
    let mut walk = home.to_path_buf();
    Path::new(rel).components().any(|component| {
        walk.push(component);
        std::fs::symlink_metadata(&walk).is_ok_and(|meta| meta.file_type().is_symlink())
    })
}

/// `<grok_home>/sandbox_grants.toml` and `<grok_home>/sessions/<root>/sandbox_grants.toml`.
pub const GLOBAL_GRANTS_FILENAME: &str = "sandbox_grants.toml";

/// The lock sidecar beside each grant file. A command that could open it could `flock` it and
/// stall every grant edit, so it is in the floor (no write-open) and the policy read-denies it.
pub const GRANTS_LOCK_FILENAME: &str = "sandbox_grants.toml.lock";

/// Who must own a file [`open_nofollow`] opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileOwner {
    /// The daemon's user (its effective uid): a file the daemon keeps and reads back as policy.
    Daemon,
    /// Anyone: repository content, which another user may have checked out.
    Any,
}

/// `CreateFile`'s flag that opens a reparse point (a symlink or a junction) itself, not its target.
#[cfg(windows)]
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

/// Opens `path` so that nothing a command left in its place is followed: the last component is
/// opened `O_NOFOLLOW` (a symlink is refused, never resolved) and `O_NONBLOCK` (a FIFO cannot
/// stall the daemon), and the handle is refused unless `fstat` on it shows a regular file owned
/// as `owner` asks. Windows: the open does not follow a reparse point
/// (`FILE_FLAG_OPEN_REPARSE_POINT`), so a link in the path's place is what the handle
/// describes and is refused; there is no owner to check. A refusal is
/// [`std::io::ErrorKind::InvalidInput`]. `options` is used as given plus those flags.
pub fn open_nofollow(
    path: &Path,
    options: &mut std::fs::OpenOptions,
    owner: FileOwner,
) -> std::io::Result<std::fs::File> {
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::custom_flags(options, libc::O_NOFOLLOW | libc::O_NONBLOCK);
    #[cfg(windows)]
    std::os::windows::fs::OpenOptionsExt::custom_flags(options, FILE_FLAG_OPEN_REPARSE_POINT);
    let file = options.open(path).map_err(|error| {
        #[cfg(unix)]
        if error.raw_os_error() == Some(libc::ELOOP) {
            return refused_file("the file is a symlink");
        }
        error
    })?;
    judged(file, owner)
}

/// [`open_nofollow`] for `name` inside the directory `dir` holds open, not for a path: nothing
/// above `name` is resolved again, so a directory renamed or swapped for a symlink since `dir`
/// was opened cannot redirect the open. `create` opens read-write and creates a missing file
/// owner-only (a lock sidecar) with `O_EXCL`, reopening it when a concurrent creator made it
/// first, so every racer opens the one file; otherwise the open is read-only.
#[cfg(unix)]
pub(crate) fn open_nofollow_at(
    dir: BorrowedFd<'_>,
    name: &OsStr,
    create: bool,
    owner: FileOwner,
) -> std::io::Result<std::fs::File> {
    let open = |access| {
        openat(
            Some(dir),
            name,
            access | libc::O_NOFOLLOW | libc::O_NONBLOCK,
            0o600,
        )
    };
    let opened = if create {
        // macOS fails a bare O_CREAT with ENOENT when another thread creates the name at the same
        // moment, so an existing file is opened plainly and a missing one is made with O_EXCL
        let mut attempts = 3;
        loop {
            match open(libc::O_RDWR) {
                Err(error) if error.raw_os_error() == Some(libc::ENOENT) => {}
                opened => break opened,
            }
            match open(libc::O_RDWR | libc::O_CREAT | libc::O_EXCL) {
                Err(error) if error.raw_os_error() == Some(libc::EEXIST) && attempts > 1 => {
                    attempts -= 1;
                }
                created => break created,
            }
        }
    } else {
        open(libc::O_RDONLY)
    };
    let fd = opened.map_err(|error| match error.raw_os_error() {
        Some(libc::ELOOP) => refused_file("the file is a symlink"),
        _ => error,
    })?;
    judged(std::fs::File::from(fd), owner)
}

/// `name` inside the directory `dir` holds (the working directory when `None`), opened
/// `O_DIRECTORY|O_NOFOLLOW` and judged by `fstat` on the handle: a symlink, a non-directory or a
/// directory not owned as `owner` asks is refused ([`std::io::ErrorKind::InvalidInput`]).
#[cfg(unix)]
pub(crate) fn open_dir_nofollow_at(
    dir: Option<BorrowedFd<'_>>,
    name: &Path,
    owner: FileOwner,
) -> std::io::Result<OwnedFd> {
    let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW;
    let fd =
        openat(dir, name.as_os_str(), flags, 0).map_err(|error| match error.raw_os_error() {
            Some(libc::ELOOP | libc::ENOTDIR) => refused_file("a symlink or not a directory"),
            _ => error,
        })?;
    let dir = std::fs::File::from(fd);
    let meta = dir.metadata()?;
    if !meta.is_dir() || (owner == FileOwner::Daemon && !owned_by_daemon(&meta)) {
        return Err(refused_file("not a directory of the daemon's user"));
    }
    Ok(OwnedFd::from(dir))
}

/// `openat(2)`, close-on-exec, creating a file with `mode` when `flags` asks to create one.
#[cfg(unix)]
fn openat(
    dir: Option<BorrowedFd<'_>>,
    name: &OsStr,
    flags: libc::c_int,
    mode: libc::c_uint,
) -> std::io::Result<OwnedFd> {
    let name = c_name(name)?;
    let dir = dir.map_or(libc::AT_FDCWD, |dir| dir.as_raw_fd());
    // SAFETY: `name` is NUL-terminated and `dir` is an open descriptor (or AT_FDCWD) for the call
    let fd = unsafe { libc::openat(dir, name.as_ptr(), flags | libc::O_CLOEXEC, mode) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `fd` was just opened by this call and nothing else owns it
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

#[cfg(unix)]
fn c_name(name: &OsStr) -> std::io::Result<CString> {
    CString::new(name.as_bytes()).map_err(|_| refused_file("the name holds a NUL"))
}

/// A directory below a trusted `anchor`, held by handle: each directory down is opened without
/// following a link and judged on its handle, and all I/O goes through the last, so a directory
/// swapped for a symlink after the walk redirects none (elsewhere judged by path).
pub struct HeldDir {
    #[cfg(unix)]
    fd: OwnedFd,
    #[cfg(not(unix))]
    dir: PathBuf,
}

#[cfg(unix)]
impl HeldDir {
    /// Walks from `anchor` down to `dir`, each directory owned as `owner` asks; `create` makes a
    /// missing one (owner-only for [`FileOwner::Daemon`]). A refused directory is
    /// [`std::io::ErrorKind::InvalidInput`], naming it; a missing one otherwise `NotFound`.
    pub fn open(
        anchor: &Path,
        dir: &Path,
        owner: FileOwner,
        create: bool,
    ) -> std::io::Result<HeldDir> {
        let below = dir.strip_prefix(anchor).map_err(|_| refused_dir(dir))?;
        if create && owner == FileOwner::Daemon {
            xai_grok_config::create_dir_all_owner_only(anchor)?;
        }
        let mut fd =
            open_dir_nofollow_at(None, anchor, owner).map_err(|e| judged_dir(anchor, e))?;
        let mut at = anchor.to_path_buf();
        for component in below.components() {
            let std::path::Component::Normal(name) = component else {
                return Err(refused_dir(dir));
            };
            at.push(name);
            if create {
                mkdirat(fd.as_fd(), name, owner)?;
            }
            fd = open_dir_nofollow_at(Some(fd.as_fd()), Path::new(name), owner)
                .map_err(|e| judged_dir(&at, e))?;
        }
        Ok(HeldDir { fd })
    }

    /// The text of `name` in this directory, as [`read_nofollow`] reads a path.
    pub fn read(&self, name: &OsStr, max_bytes: u64, owner: FileOwner) -> std::io::Result<String> {
        read_nofollow_at(self.fd.as_fd(), name, max_bytes, owner)
    }

    /// `name` opened read-write, created owner-only when missing ([`open_nofollow_at`]).
    pub(crate) fn open_lock(&self, name: &OsStr) -> std::io::Result<std::fs::File> {
        open_nofollow_at(self.fd.as_fd(), name, true, FileOwner::Daemon)
    }

    /// An exclusive `flock` on the held directory, for writers of a file with no lock sidecar,
    /// taken in turn among this process's writers ([`lock_in_turn`]); the lock belongs to the
    /// open description the duplicate shares, so it lasts until this handle drops.
    pub fn lock(&self, wait: Duration) -> std::io::Result<()> {
        lock_in_turn(&std::fs::File::from(self.fd.try_clone()?), wait)
    }

    /// Whether the held directory is the one `path` names now (same device and inode); a
    /// missing `path` is not.
    pub fn is(&self, path: &Path) -> std::io::Result<bool> {
        let held = std::fs::File::from(self.fd.try_clone()?).metadata()?;
        match std::fs::metadata(path) {
            Ok(named) => Ok((held.dev(), held.ino()) == (named.dev(), named.ino())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// Replaces `name` with `contents` via an `O_EXCL` temp renamed in this handle, so a link at
    /// `name` is replaced, never written through; if what lands is not the temp written, it is
    /// removed and the write refused.
    pub fn replace(&self, name: &OsStr, contents: &str, owner: FileOwner) -> std::io::Result<()> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut temp = OsString::from(".");
        temp.push(name);
        temp.push(format!(".{}.{n}.tmp", std::process::id()));
        let mode = if owner == FileOwner::Daemon {
            0o600
        } else {
            0o666
        };
        let flags = libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW;
        let mut file = std::fs::File::from(openat(Some(self.fd.as_fd()), &temp, flags, mode)?);
        let (dir, from, to) = (self.fd.as_raw_fd(), c_name(&temp)?, c_name(name)?);
        let renamed = file.write_all(contents.as_bytes()).and_then(|()| {
            file.sync_all()?;
            #[cfg(test)]
            BEFORE_HELD_RENAME.with_borrow_mut(|hook| hook.as_mut().map(|hook| hook(&temp)));
            // SAFETY: both names are NUL-terminated and `dir` is open for the call
            match unsafe { libc::renameat(dir, from.as_ptr(), dir, to.as_ptr()) } {
                0 => Ok(()),
                _ => Err(std::io::Error::last_os_error()),
            }
        });
        if let Err(error) = renamed {
            // SAFETY: as for the rename
            unsafe { libc::unlinkat(dir, from.as_ptr(), 0) };
            return Err(error);
        }
        let flags = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK;
        let landed = match openat(Some(self.fd.as_fd()), name, flags, 0) {
            Ok(landed) => Some(std::fs::File::from(landed).metadata()?),
            Err(error) if error.raw_os_error() == Some(libc::ELOOP) => None,
            Err(error) => return Err(error),
        };
        let written = file.metadata()?;
        if landed
            .is_none_or(|landed| (landed.dev(), landed.ino()) != (written.dev(), written.ino()))
        {
            // What fstat showed at `name` (or a link, ELOOP) came in under the temp name, not the
            // user's file, which the rename replaced: removed, so no later read takes it
            // SAFETY: as for the rename
            unsafe { libc::unlinkat(dir, to.as_ptr(), 0) };
            return Err(refused_file(
                "the file renamed into place is not the one written",
            ));
        }
        // SAFETY: `dir` is open for the call
        if unsafe { libc::fsync(dir) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
}

/// An exclusive `flock` on `lock` within `wait` (a lock still held then is `TimedOut`), taken in
/// turn among this process's writers of the file ([`Turn`]): the waiter queues on the turnstile
/// first, so a writer coming back the moment it released the `flock` queues behind every waiter.
///
/// `flock` has no queue: on release the kernel wakes the waiters, and the releaser, back within
/// microseconds, takes it again before any of them runs, so a poller at 10 ms loses every round
/// of a burst and even a waiter blocked in the kernel loses tens. One `wait` bounds turn and lock.
///
/// A writer in another process is not in the queue: it contends on the poll as before, under the
/// same `wait`, and a lock it holds the whole of it is `TimedOut` as before.
#[cfg(unix)]
pub(crate) fn lock_in_turn(lock: &std::fs::File, wait: Duration) -> std::io::Result<()> {
    let file = lock.metadata()?;
    let deadline = Instant::now() + wait;
    let turn = Turn::queue((file.dev(), file.ino()));
    turn.wait_for(deadline, wait)?;
    retry_until(deadline, wait, || try_flock(lock))
}

/// The `flock` polled until `wait` runs out: no device and inode to key a turnstile on off unix.
#[cfg(not(unix))]
pub(crate) fn lock_in_turn(lock: &std::fs::File, wait: Duration) -> std::io::Result<()> {
    retry_until(Instant::now() + wait, wait, || try_flock(lock))
}

/// The writers of each file in this process ([`lock_in_turn`]), by device and inode, in the
/// order they came; the front of a queue polls the `flock`, the rest wait to be front.
#[cfg(unix)]
struct Turnstile {
    queues: Mutex<Queues>,
    /// Notified whenever a queue changes.
    moved: Condvar,
}

#[cfg(unix)]
impl Turnstile {
    /// The queues; a poisoned lock is taken anyway, its sections leaving them consistent.
    fn queues(&self) -> std::sync::MutexGuard<'_, Queues> {
        self.queues.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(unix)]
struct Queues {
    /// The tickets handed out so far; the next is one more.
    issued: u64,
    /// The tickets waiting at each file, front first; a file with none has no entry.
    waiting: BTreeMap<(u64, u64), VecDeque<u64>>,
}

#[cfg(unix)]
static TURNSTILE: Turnstile = Turnstile {
    queues: Mutex::new(Queues {
        issued: 0,
        waiting: BTreeMap::new(),
    }),
    moved: Condvar::new(),
};

/// A writer's place in the queue of one file, taken at the back; dropped, it is given up and
/// the next writer woken. Kept until the `flock` is taken, so a writer coming back the moment
/// it released the file queues behind every waiter already there.
#[cfg(unix)]
struct Turn {
    key: (u64, u64),
    ticket: u64,
}

#[cfg(unix)]
impl Turn {
    /// Joins the back of the queue of the file `key` names.
    fn queue(key: (u64, u64)) -> Turn {
        let mut queues = TURNSTILE.queues();
        let ticket = queues.issued;
        queues.issued += 1;
        queues.waiting.entry(key).or_default().push_back(ticket);
        Turn { key, ticket }
    }

    /// Waits to be at the front of the queue, until `deadline`: another's still then is
    /// `TimedOut`, naming `wait`.
    fn wait_for(&self, deadline: Instant, wait: Duration) -> std::io::Result<()> {
        let mut queues = TURNSTILE.queues();
        loop {
            let front = queues.waiting.get(&self.key).and_then(VecDeque::front);
            if front == Some(&self.ticket) {
                return Ok(());
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(timed_out(wait));
            }
            queues = TURNSTILE
                .moved
                .wait_timeout(queues, deadline - now)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

#[cfg(unix)]
impl Drop for Turn {
    fn drop(&mut self) {
        let mut queues = TURNSTILE.queues();
        if let Some(waiting) = queues.waiting.get_mut(&self.key) {
            waiting.retain(|ticket| *ticket != self.ticket);
            if waiting.is_empty() {
                queues.waiting.remove(&self.key);
            }
        }
        drop(queues);
        TURNSTILE.moved.notify_all();
    }
}

/// One `try_lock_exclusive`: `Ok(false)` while another open description holds the `flock`.
fn try_flock(lock: &std::fs::File) -> std::io::Result<bool> {
    match fs2::FileExt::try_lock_exclusive(lock) {
        Ok(()) => Ok(true),
        Err(held) if held.raw_os_error() == fs2::lock_contended_error().raw_os_error() => Ok(false),
        Err(error) => Err(error),
    }
}

/// Retries `attempt` every 10 ms while it is `Ok(false)`, until `deadline`: the lock it asks for
/// still held then is `TimedOut`, naming `wait`, the whole of it.
fn retry_until(
    deadline: Instant,
    wait: Duration,
    mut attempt: impl FnMut() -> std::io::Result<bool>,
) -> std::io::Result<()> {
    loop {
        if attempt()? {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(timed_out(wait));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The lock asked for stayed held the whole of `wait`.
fn timed_out(wait: Duration) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        format!("the lock stayed held for {wait:?}"),
    )
}

#[cfg(all(test, unix))]
type RenameHook = Option<Box<dyn FnMut(&OsStr)>>;

#[cfg(all(test, unix))]
thread_local! {
    /// Runs on the temp's name just before [`HeldDir::replace`] renames it.
    pub(crate) static BEFORE_HELD_RENAME: std::cell::RefCell<RenameHook> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(not(unix))]
impl HeldDir {
    pub fn open(
        anchor: &Path,
        dir: &Path,
        owner: FileOwner,
        create: bool,
    ) -> std::io::Result<HeldDir> {
        let below = dir.strip_prefix(anchor).map_err(|_| refused_dir(dir))?;
        let mut at = anchor.to_path_buf();
        for component in std::iter::once(None).chain(below.components().map(Some)) {
            at.extend(component);
            match std::fs::symlink_metadata(&at) {
                Ok(meta)
                    if meta.is_dir() && (owner == FileOwner::Any || owned_by_daemon(&meta)) => {}
                Ok(_) => return Err(refused_dir(&at)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound && create => {
                    std::fs::create_dir(&at)?;
                }
                Err(error) => return Err(error),
            }
        }
        Ok(HeldDir {
            dir: dir.to_path_buf(),
        })
    }

    pub fn read(&self, name: &OsStr, max_bytes: u64, owner: FileOwner) -> std::io::Result<String> {
        read_nofollow(&self.dir.join(name), max_bytes, owner)
    }

    pub(crate) fn open_lock(&self, name: &OsStr) -> std::io::Result<std::fs::File> {
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        open_nofollow(&self.dir.join(name), &mut options, FileOwner::Daemon)
    }

    pub fn is(&self, path: &Path) -> std::io::Result<bool> {
        Ok(is_same_path(
            &canonical_path(&self.dir),
            &canonical_path(path),
        ))
    }

    pub fn replace(&self, name: &OsStr, contents: &str, owner: FileOwner) -> std::io::Result<()> {
        let mode = (owner == FileOwner::Daemon).then_some(0o600);
        xai_grok_config::fs_atomic::write_atomically_verified(&self.dir.join(name), contents, mode)
    }
}

/// `mkdirat(2)`, owner-only for [`FileOwner::Daemon`]; an entry already there is left for the
/// open that follows to judge.
#[cfg(unix)]
fn mkdirat(dir: BorrowedFd<'_>, name: &OsStr, owner: FileOwner) -> std::io::Result<()> {
    let name = c_name(name)?;
    let mode: libc::mode_t = if owner == FileOwner::Daemon {
        0o700
    } else {
        0o777
    };
    // SAFETY: `name` is NUL-terminated and `dir` is open for the call
    if unsafe { libc::mkdirat(dir.as_raw_fd(), name.as_ptr(), mode) } == 0 {
        return Ok(());
    }
    match std::io::Error::last_os_error() {
        error if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        error => Err(error),
    }
}

#[cfg(unix)]
fn judged_dir(at: &Path, error: std::io::Error) -> std::io::Error {
    match error.kind() {
        std::io::ErrorKind::InvalidInput => refused_dir(at),
        _ => error,
    }
}

fn refused_dir(at: &Path) -> std::io::Error {
    refused_file(&format!(
        "{} is a symlink, not a directory, or another user's",
        at.display()
    ))
}

/// `file` when `fstat` on it shows a regular file owned as `owner` asks, else the refusal.
fn judged(file: std::fs::File, owner: FileOwner) -> std::io::Result<std::fs::File> {
    let meta = file.metadata()?;
    if meta.file_type().is_symlink() {
        return Err(refused_file("the file is a symlink"));
    }
    if !meta.is_file() {
        return Err(refused_file("the file is not a regular file"));
    }
    if owner == FileOwner::Daemon && !owned_by_daemon(&meta) {
        return Err(refused_file("the file is owned by another user"));
    }
    Ok(file)
}

/// The text of `path`, opened by [`open_nofollow`] and read up to `max_bytes`; a longer file is
/// [`std::io::ErrorKind::FileTooLarge`], so a hostile file cannot make the daemon allocate
/// without bound.
pub fn read_nofollow(path: &Path, max_bytes: u64, owner: FileOwner) -> std::io::Result<String> {
    read_bounded(
        open_nofollow(path, std::fs::OpenOptions::new().read(true), owner)?,
        max_bytes,
    )
}

/// [`read_nofollow`] for `name` inside the directory `dir` holds open ([`open_nofollow_at`]).
#[cfg(unix)]
pub(crate) fn read_nofollow_at(
    dir: BorrowedFd<'_>,
    name: &OsStr,
    max_bytes: u64,
    owner: FileOwner,
) -> std::io::Result<String> {
    read_bounded(open_nofollow_at(dir, name, false, owner)?, max_bytes)
}

fn read_bounded(file: std::fs::File, max_bytes: u64) -> std::io::Result<String> {
    let mut text = String::new();
    file.take(max_bytes.saturating_add(1))
        .read_to_string(&mut text)?;
    if text.len() as u64 > max_bytes {
        return Err(std::io::Error::new(
            std::io::ErrorKind::FileTooLarge,
            format!("the file is larger than {max_bytes} bytes"),
        ));
    }
    Ok(text)
}

fn refused_file(reason: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, reason)
}

/// Whether the daemon's user (its effective uid) owns what `meta` describes; always on a host
/// without file owners.
#[cfg(unix)]
pub(crate) fn owned_by_daemon(meta: &std::fs::Metadata) -> bool {
    // SAFETY: geteuid has no preconditions and cannot fail
    meta.uid() == unsafe { libc::geteuid() }
}

#[cfg(not(unix))]
pub(crate) fn owned_by_daemon(_meta: &std::fs::Metadata) -> bool {
    true
}

pub struct ProtectedInputs<'a> {
    /// The floor's workspace entries sit at its pinned real path; its spelling keys the own
    /// session directory.
    pub workspace_root: &'a ServedRoot,
    pub grok_home: &'a Path,
    pub user_home: Option<&'a Path>,
    /// The daemon's own endpoint directory: read-, write- and connect-denied. Required.
    pub control_socket_dir: &'a Path,
    /// [`GitConfigEnv::from_host`] in production: where the floor reads the user's global git
    /// config; injected so tests never read the environment.
    pub git_env: &'a GitConfigEnv,
}

/// The floor and what it could not derive: a git file the floor must read (a config for
/// `core.hooksPath`, a worktree pointer) that was too large, unreadable, or past the include
/// bounds. The floor still holds every entry the readable files gave; enforce refuses to run a
/// command while `unread_git_metadata` is non-empty, since the hooks path such a file may set is
/// unknown.
#[derive(Debug, Default)]
pub struct Floor {
    pub protected: Vec<Protected>,
    pub unread_git_metadata: Vec<GitMetadataUnread>,
}

/// The full floor for one workspace, canonical, sorted and deduplicated.
pub fn floor(inputs: &ProtectedInputs<'_>) -> Vec<Protected> {
    floor_with_unread(inputs).protected
}

/// [`floor`] together with the git files it could not read.
pub fn floor_with_unread(inputs: &ProtectedInputs<'_>) -> Floor {
    let ws = inputs.workspace_root.real().to_path_buf();
    let grok_home = canonical_path(inputs.grok_home);
    let home = inputs.user_home.map(canonical_path);
    let own_session_dir = own_session_dir(inputs);
    let command_dir = session_command_dir(inputs);
    let path = |p: PathBuf| Protected::Path { path: p };
    let mut out: Vec<Protected> = WORKSPACE_PROTECTED_SUBPATHS
        .iter()
        .map(|sub| path(ws.join(sub)))
        .collect();
    let tmp_dirs = default_tmp_dirs(None, inputs.user_home);
    let tmp_dirs: Vec<PathBuf> = tmp_dirs.iter().map(|dir| canonical_path(dir)).collect();
    let write_roots = default_write_roots(inputs, &tmp_dirs);
    let git = git_config::git_entries_in(&ws, inputs.user_home, &write_roots, inputs.git_env);
    out.extend(git.protected);
    out.extend(
        TRUST_BOUNDARY_FILENAMES
            .iter()
            .chain(GROK_HOME_PROTECTED_NAMES)
            .map(|name| path(grok_home.join(name))),
    );
    if let Some(home) = &home {
        out.extend(
            HOME_PROTECTED_FILES
                .iter()
                .chain(HOME_PROTECTED_DIRS)
                .chain(HOME_PROTECTED_TOOL_TREES)
                .chain(SECRET_READ_DENY_DIRS)
                .chain(SECRET_READ_DENY_FILES)
                .map(|rel| path(home.join(rel))),
        );
    }
    out.push(path(canonical_path(inputs.control_socket_dir)));
    // grok runs or loads what its home holds outside any sandbox: a workspace served from inside
    // it is the one writable tree there, and otherwise the session command directory is
    out.push(Protected::TreeExcept {
        tree: grok_home.clone(),
        except: if is_within(&ws, &grok_home) {
            ws.clone()
        } else {
            command_dir.clone()
        },
    });
    // Every other workspace's session directory, present or future, and all of the own one but
    // its command directory: the grant files, their lock, temp names and `permission*.toml`
    out.push(Protected::TreeExcept {
        tree: grok_home.join("sessions"),
        except: command_dir,
    });
    // Named too, so the hard-link check sees the grant files a command could reach another way
    out.push(path(own_session_dir.join(GLOBAL_GRANTS_FILENAME)));
    out.push(path(own_session_dir.join(GRANTS_LOCK_FILENAME)));
    out.push(path(own_session_dir.join("permission.toml")));
    let permission_file = PathGlob::new("permission*.toml", true);
    let is_permission_file = |name: &str| {
        permission_file
            .as_ref()
            .is_some_and(|glob| glob.is_match(Path::new(name)))
    };
    out.extend(
        existing_children(&own_session_dir, is_permission_file)
            .into_iter()
            .map(path),
    );
    // Every other folder's grant files once one has a second link, which is all the check needs:
    // naming each folder's would grow every profile with each folder ever served
    #[cfg(unix)]
    out.extend(
        linked_grant_files(&grok_home.join("sessions"), |name| {
            [GLOBAL_GRANTS_FILENAME, GRANTS_LOCK_FILENAME].contains(&name)
                || is_permission_file(name)
        })
        .into_iter()
        .map(path),
    );
    // An entry spelled through a symlink (`~/.kube/config -> config-prod`) names its target too:
    // the kernel and the canonical query both meet the resolved path
    let resolved: Vec<Protected> = out
        .iter()
        .filter_map(Protected::as_path)
        .map(canonical_path)
        .map(path)
        .collect();
    out.extend(resolved);
    out.sort();
    out.dedup();
    Floor {
        protected: out,
        unread_git_metadata: git.unread,
    }
}

/// `<prefix>/<tail>` as a glob entry, or `None` when the prefix itself carries a glob
/// metacharacter (`proj[1]`) or a character no backend's glob accepts (`{{slug}}`, a
/// backslash), and so cannot be spelled as a literal; callers enumerate instead.
pub(super) fn glob_under(prefix: &Path, tail: &str) -> Option<Protected> {
    let text = prefix.to_str()?;
    (!crate::deny::is_glob(text) && !text.contains(['{', '}', '\\'])).then(|| Protected::Glob {
        glob: format!("{text}/{tail}"),
    })
}

/// The entries directly under `dir` whose name satisfies `keep`; none when `dir` is a symlink,
/// which is never listed through.
pub(super) fn existing_children(dir: &Path, keep: impl Fn(&str) -> bool) -> Vec<PathBuf> {
    if !is_real_directory(dir) {
        return Vec::new();
    }
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_name().to_str().is_some_and(&keep))
        .map(|entry| entry.path())
        .collect()
}

/// Each folder's grant files under `sessions` that have a second link; neither `sessions` nor a
/// folder directory is listed through a symlink, as the grant store never reads through one.
#[cfg(unix)]
fn linked_grant_files(sessions: &Path, is_grant_file: impl Fn(&str) -> bool) -> Vec<PathBuf> {
    existing_children(sessions, |_| true)
        .iter()
        .flat_map(|folder| existing_children(folder, &is_grant_file))
        .filter(|file| {
            std::fs::symlink_metadata(file).is_ok_and(|meta| meta.is_file() && meta.nlink() > 1)
        })
        .collect()
}

/// `<grok_home>/sessions/<enc(root)>` for the workspace as the hub spells it (the encoding is of
/// the root's given text), in the canonical spelling of the grok home.
pub fn own_session_dir(inputs: &ProtectedInputs<'_>) -> PathBuf {
    canonical_path(&own_session_dir_as_named(inputs))
}

/// The one directory under the own session directory a command may write
/// ([`session_command_dir`]); everything else there is floor.
pub const SESSION_COMMAND_DIRNAME: &str = "commands";

/// `<own session dir>/commands`, canonical: the child's session scratch, the floor's one exception
/// in the grok home for a workspace outside it.
pub fn session_command_dir(inputs: &ProtectedInputs<'_>) -> PathBuf {
    own_session_dir(inputs).join(SESSION_COMMAND_DIRNAME)
}

/// Whether the session command directory is a write root: the daemon made it and the own session
/// directory, each a directory itself (a symlink in either place, planted before the daemon
/// created it, would redirect the writes), and the workspace is not served from inside the grok
/// home, where the floor excepts only the workspace. Never created here.
pub fn session_command_dir_is_writable(inputs: &ProtectedInputs<'_>) -> bool {
    let own = own_session_dir_as_named(inputs);
    !is_within(
        inputs.workspace_root.real(),
        &canonical_path(inputs.grok_home),
    ) && is_real_directory(&own)
        && is_real_directory(&own.join(SESSION_COMMAND_DIRNAME))
}

fn own_session_dir_as_named(inputs: &ProtectedInputs<'_>) -> PathBuf {
    xai_grok_config::sessions_cwd_dir_in(
        inputs.grok_home,
        &inputs.workspace_root.spelled().to_string_lossy(),
    )
}

/// A protected regular file a command could write through another name, or a check that could
/// not tell: the enforce backend refuses to render a profile while one exists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HardLinked {
    /// The protected file, or the floor entry whose tree could not be listed.
    pub path: PathBuf,
    /// The file's link count; 0 when no single file is named.
    pub nlink: u64,
    pub alias: Alias,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Alias {
    /// A name of the file inside a write root that no floor entry covers.
    Writable(PathBuf),
    /// The file has a link no protected name accounts for, and `root` is too large to search.
    Unsearched { root: PathBuf },
    /// `tree` holds more protected entries than one walk lists.
    Unlisted { tree: PathBuf },
}

/// Entries one hard-link walk reads (a protected tree, a write root searched for an alias) and
/// directories one floor glob's walk queues, before the check stops and refuses.
#[cfg(unix)]
pub(super) const HARD_LINK_SCAN_LIMIT: usize = 4096;

#[cfg(all(test, unix))]
thread_local! {
    /// Write roots this thread walked in search of an alias.
    pub(super) static ROOT_SEARCHES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// Entries every hard-link walk on this thread read.
    pub(super) static ENTRIES_READ: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// A protected regular file is refused when a command could write it through another name: a
/// link inside a write root that no floor entry covers. Its link count bounds the search: a file
/// whose links are all protected names found here (`~/.bashrc` linked to `~/.bash_profile`) needs
/// none, and no write root is walked unless some file has a link left over. Files come from every
/// file entry and from the trees and glob matches that lie inside a write root; a tree outside
/// them is not listed (rustup's `~/.cargo/bin` proxies are links of one another). Every walk is
/// bounded, and one that cannot finish refuses ([`Alias`]), except the listing of a secret store
/// or a tool tree ([`SECRET_READ_DENY_DIRS`], [`HOME_PROTECTED_TOOL_TREES`]), whose names stay
/// protected either way and which must not refuse every command in a root that holds it. Every
/// call searches afresh: a link made deep in a write root leaves the root's own times unchanged.
#[cfg(unix)]
pub fn hard_linked_protected_file(
    floor: &[Protected],
    write_roots: &[PathBuf],
) -> Option<HardLinked> {
    let within_a_root = |path: &Path| write_roots.iter().any(|root| is_within(path, root));
    let mut linked = LinkedFiles::default();
    for entry in floor {
        let listed = match entry {
            // A browser profile or a toolchain holds 10⁵ files: listed as far as the bound,
            // never a refusal
            Protected::Path { path }
                if SECRET_READ_DENY_DIRS
                    .iter()
                    .chain(HOME_PROTECTED_TOOL_TREES)
                    .any(|rel| path.ends_with(rel)) =>
            {
                linked.add_path(path, within_a_root(path)).or(Ok(()))
            }
            Protected::Path { path } => linked.add_path(path, within_a_root(path)),
            Protected::Glob { glob } if within_a_root(&entry.anchor()) => {
                linked.add_glob_matches(glob, &entry.anchor())
            }
            Protected::Glob { .. } | Protected::TreeExcept { .. } | Protected::Node { .. } => {
                Ok(())
            }
        };
        if let Err(tree) = listed {
            return Some(HardLinked {
                path: entry.as_path().unwrap_or(&tree).to_path_buf(),
                nlink: 0,
                alias: Alias::Unlisted { tree },
            });
        }
    }
    linked.writable_alias(write_roots, floor)
}

/// The protected regular files with more than one link: one row per inode, with every protected
/// name found for it.
#[cfg(unix)]
#[derive(Default)]
struct LinkedFiles(Vec<LinkedFile>);

#[cfg(unix)]
struct LinkedFile {
    inode: (u64, u64),
    nlink: u64,
    owner: u32,
    names: Vec<PathBuf>,
}

#[cfg(unix)]
impl LinkedFiles {
    /// One name per link: `path` is counted in its canonical spelling, so the floor naming one
    /// link twice (as written and resolved, through a firmlink or a symlinked parent) never
    /// reads as a second link accounted for.
    fn add(&mut self, path: &Path, meta: &std::fs::Metadata) {
        if !meta.is_file() || meta.nlink() < 2 {
            return;
        }
        let inode = (meta.dev(), meta.ino());
        let name = canonical_path(path);
        match self.0.iter_mut().find(|row| row.inode == inode) {
            Some(row) if row.names.iter().any(|known| is_same_path(known, &name)) => {}
            Some(row) => row.names.push(name),
            None => self.0.push(LinkedFile {
                inode,
                nlink: meta.nlink(),
                owner: meta.uid(),
                names: vec![name],
            }),
        }
    }

    /// A protected file, or every file under a protected directory when `list_dir` holds.
    fn add_path(&mut self, path: &Path, list_dir: bool) -> Result<(), PathBuf> {
        match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.is_dir() && list_dir => walk(path, &[meta.uid()], |child, meta| {
                self.add(child, meta);
                false
            })
            .map(drop),
            Ok(meta) => {
                self.add(path, &meta);
                Ok(())
            }
            Err(_) => Ok(()),
        }
    }

    fn add_glob_matches(&mut self, glob: &str, prefix: &Path) -> Result<(), PathBuf> {
        let found = git_config::present_matches(glob, prefix, HARD_LINK_SCAN_LIMIT)?;
        found.iter().try_for_each(|path| self.add_path(path, true))
    }

    /// A name inside a write root, covered by no floor entry, of a file whose protected names
    /// leave a link unaccounted for; `Unsearched` for the first root too large to search. A hard
    /// link never crosses a device, so a root on none of the files' devices is not searched.
    fn writable_alias(&self, write_roots: &[PathBuf], floor: &[Protected]) -> Option<HardLinked> {
        let open: Vec<&LinkedFile> = self
            .0
            .iter()
            .filter(|file| (file.names.len() as u64) < file.nlink)
            .collect();
        let first = *open.first()?;
        let owners: Vec<u32> = open.iter().map(|file| file.owner).collect();
        let devices: Vec<u64> = open.iter().map(|file| file.inode.0).collect();
        write_roots.iter().find_map(|root| {
            if std::fs::symlink_metadata(root).is_ok_and(|meta| !devices.contains(&meta.dev())) {
                return None;
            }
            #[cfg(test)]
            ROOT_SEARCHES.set(ROOT_SEARCHES.get() + 1);
            let mut hit = first;
            let found = walk(root, &owners, |path, meta| {
                let inode = (meta.dev(), meta.ino());
                match open.iter().find(|file| file.inode == inode) {
                    Some(file) if !is_protected(path, floor) => {
                        hit = file;
                        true
                    }
                    _ => false,
                }
            });
            let alias = match found {
                Ok(None) => return None,
                Ok(Some(path)) => Alias::Writable(path),
                Err(root) => Alias::Unsearched { root },
            };
            Some(HardLinked {
                path: hit.names.first()?.clone(),
                nlink: hit.nlink,
                alias,
            })
        })
    }
}

/// Walks the tree under `root` breadth-first without following a symlink: a symlink counts as the
/// one entry it is and is never queued or offered to `hit` (its own inode is no file's, and a
/// write through it is judged at its target). `Ok(Some(path))` for the first entry `hit`
/// accepts, `Err(root)` once more than [`HARD_LINK_SCAN_LIMIT`] entries would be read, and
/// `Err(dir)` for a directory whose entries cannot all be read ([`unread_dir_refuses`]).
#[cfg(unix)]
fn walk(
    root: &Path,
    owners: &[u32],
    mut hit: impl FnMut(&Path, &std::fs::Metadata) -> bool,
) -> Result<Option<PathBuf>, PathBuf> {
    let mut pending = std::collections::VecDeque::from([root.to_path_buf()]);
    let mut read = 0;
    while let Some(dir) = pending.pop_front() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if unread_dir_refuses(&dir, &error, owners) => return Err(dir),
            Err(_) => continue,
        };
        for entry in entries {
            let entry = entry.map_err(|_| dir.clone())?;
            read += 1;
            #[cfg(test)]
            ENTRIES_READ.set(ENTRIES_READ.get() + 1);
            if read > HARD_LINK_SCAN_LIMIT {
                return Err(root.to_path_buf());
            }
            if entry.file_type().is_ok_and(|kind| kind.is_symlink()) {
                continue;
            }
            let path = entry.path();
            let meta = match std::fs::symlink_metadata(&path) {
                Ok(meta) => meta,
                Err(error) if unread_dir_refuses(&dir, &error, owners) => return Err(dir),
                Err(_) => continue,
            };
            if hit(&path, &meta) {
                return Ok(Some(path));
            }
            if meta.is_dir() {
                pending.push_back(path);
            }
        }
    }
    Ok(None)
}

/// Whether a walk that failed to read `dir` or an entry of it must refuse: a name beneath it can
/// be reached, as the directory can be searched, or made reachable, as one of `owners` (whose
/// files the walk guards) owns it. Gone meanwhile, or another user's unsearchable directory
/// (`/tmp/systemd-private-*`), it holds no name a command could write through.
#[cfg(unix)]
pub(super) fn unread_dir_refuses(dir: &Path, error: &std::io::Error, owners: &[u32]) -> bool {
    error.kind() != std::io::ErrorKind::NotFound
        && (std::fs::symlink_metadata(dir.join(".")).is_ok()
            || std::fs::symlink_metadata(dir).is_ok_and(|meta| owners.contains(&meta.uid())))
}

#[cfg(not(unix))]
pub fn hard_linked_protected_file(
    _floor: &[Protected],
    _write_roots: &[PathBuf],
) -> Option<HardLinked> {
    None
}

#[cfg(test)]
#[path = "protected_tests.rs"]
mod tests;
