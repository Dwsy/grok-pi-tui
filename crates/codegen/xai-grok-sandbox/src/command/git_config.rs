//! The floor's git entries ([`git_entries_in`]), the git layout its globs' present matches are
//! walked by ([`present_matches`]), and the git metadata they read: the `.git` file and the
//! `commondir` and `gitdir` pointers, and `core.hooksPath` from a repository config (`config`,
//! then `config.worktree`) or the user's global one, read in file order with includes followed
//! in place, the last assignment winning. The global config is where git looks for it under the
//! daemon's environment ([`GitConfigEnv`]): `GIT_CONFIG_GLOBAL` alone when set, else
//! `$XDG_CONFIG_HOME/git/config` (`~/.config/git/config`) and `~/.gitconfig`; each is read for
//! its hooks path and protected as a floor entry.
//!
//! The symlink rule: every file here is read *through* a symlink, as git reads it (a stow'd
//! `~/.gitconfig`, an include under `~/dotfiles`, a `gitdir:` reached through a linked
//! directory), and every file read is protected in each spelling a command could reach it by —
//! as named, its canonical path when a link leads elsewhere, and each link on the way, wherever
//! it lies — so a command can neither rewrite what git reads nor re-point the link git
//! follows. (A link is a [`Protected::Node`]: the link itself, never the tree it leads to, so
//! a grant under its target stands.) Nothing is
//! refused for being a symlink. Each file is opened once and judged by
//! `fstat` on the open handle (no check-then-open window; `O_NONBLOCK`, so a FIFO cannot stall
//! the daemon). What is refused is a file git would read and the floor cannot
//! ([`GitMetadataUnread`]: larger than [`GIT_METADATA_READ_LIMIT`], unreadable, an include chain
//! past its bound or an include it cannot resolve as git would (`~user/…`, `%(prefix)/…`), a
//! `modules` or `worktrees` tree too large to list): the policy carries it and
//! enforce runs no command until it is fixed, because the hooks tree such a file may name is
//! unknown.

use std::ffi::OsString;
use std::io::Read as _;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt as _;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::command::canonical::{canonical_path, fold_dots, is_same_path, is_within};
use crate::command::protected::{
    GIT_DIR_PROTECTED_ENTRIES, Protected, existing_children, glob_under,
};
#[cfg(unix)]
use crate::command::protected::{glob_matcher, unread_dir_refuses};

/// Largest `.git` file, pointer file or git config file (an included one too) read while
/// deriving the git entries. A larger one is [`GitMetadataUnread::TooLarge`], never a prefix.
const GIT_METADATA_READ_LIMIT: u64 = 1024 * 1024;

/// The one global config git reads instead of the home files when this is set (git reads
/// neither `~/.gitconfig` nor the XDG file then; `/dev/null` skips the level).
pub const GIT_CONFIG_GLOBAL_ENV: &str = "GIT_CONFIG_GLOBAL";

/// Where git reads `git/config` from instead of `~/.config` when this is set and not empty.
pub const XDG_CONFIG_HOME_ENV: &str = "XDG_CONFIG_HOME";

/// The environment git resolves its global config from: the daemon's, as the user's own git
/// inherits it, read once by [`GitConfigEnv::from_host`] and injected into [`git_entries_in`] so
/// the floor is testable without touching the process environment. A command's own
/// `GIT_CONFIG_GLOBAL=…` is not the floor's concern: the file it names is the command's, and the
/// hook it may set runs inside the sandbox.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GitConfigEnv {
    /// [`GIT_CONFIG_GLOBAL_ENV`] as set, or `None`.
    pub config_global: Option<OsString>,
    /// [`XDG_CONFIG_HOME_ENV`] as set, or `None`.
    pub xdg_config_home: Option<OsString>,
}

impl GitConfigEnv {
    /// The daemon's own environment.
    pub fn from_host() -> GitConfigEnv {
        GitConfigEnv::from_lookup(|name| std::env::var_os(name))
    }

    /// [`Self::from_host`] with the environment injected: `lookup` is asked for each variable
    /// by name, so the reading is testable without touching the process environment.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<OsString>) -> GitConfigEnv {
        GitConfigEnv {
            config_global: lookup(GIT_CONFIG_GLOBAL_ENV),
            xdg_config_home: lookup(XDG_CONFIG_HOME_ENV),
        }
    }

    /// The global config files git reads under this environment, in git's reading order — the
    /// XDG file, then `~/.gitconfig`; or `GIT_CONFIG_GLOBAL` alone, which replaces both (an
    /// empty value names no file, so git reads none) — with what could not be resolved: a
    /// relative value, which git reads from each command's working directory, names a file the
    /// floor cannot know.
    fn global_configs(
        &self,
        user_home: Option<&Path>,
    ) -> (Vec<PathBuf>, Option<GitMetadataUnread>) {
        let relative = |name: &str, value: &OsString| GitMetadataUnread::Unreadable {
            path: PathBuf::from(value),
            reason: format!(
                "{name} is a relative path, which git reads from each command's working directory"
            ),
        };
        if let Some(global) = &self.config_global {
            if global.is_empty() {
                return (Vec::new(), None);
            }
            let path = PathBuf::from(global);
            return if path.is_absolute() {
                (vec![path], None)
            } else {
                (Vec::new(), Some(relative(GIT_CONFIG_GLOBAL_ENV, global)))
            };
        }
        let mut configs = Vec::new();
        let mut unread = None;
        match self.xdg_config_home.as_ref().filter(|xdg| !xdg.is_empty()) {
            Some(xdg) if Path::new(xdg).is_absolute() => {
                configs.push(Path::new(xdg).join("git").join("config"));
            }
            Some(xdg) => unread = Some(relative(XDG_CONFIG_HOME_ENV, xdg)),
            None => configs.extend(user_home.map(|home| home.join(".config/git/config"))),
        }
        configs.extend(user_home.map(|home| home.join(".gitconfig")));
        (configs, unread)
    }
}

/// Nesting bound on followed config includes: git refuses a deeper chain (and so runs no hook),
/// but a conditional include it would not follow is followed here, so the bound refuses too.
const GIT_CONFIG_INCLUDE_DEPTH: usize = 10;

/// Config files read per resolution, so a file that includes itself many times cannot fan out.
const GIT_CONFIG_FILES_LIMIT: usize = 64;

/// Directories listed under `.git/modules` and `.git/worktrees` for the configs to read; a
/// command can create directories there, so the walk stops and refuses past it.
const GIT_DIRS_LIMIT: usize = 512;

/// Symlinks followed resolving one path, the kernel's bound on a lookup, so a link loop ends.
const SYMLINK_HOPS: usize = 40;

/// The pointer files of a linked worktree's git directory: to the shared directory, and back to
/// the worktree's `.git` file. Git follows both.
const WORKTREE_POINTER_FILES: &[&str] = &["commondir", "gitdir"];

/// The per-worktree config git reads after `config` under `extensions.worktreeConfig`.
const WORKTREE_CONFIG: &str = "config.worktree";

/// The hooks git runs by name from a hooks directory (githooks(5)).
const GIT_HOOK_NAMES: &str = "applypatch-msg pre-applypatch post-applypatch pre-commit \
    pre-merge-commit prepare-commit-msg commit-msg post-commit pre-rebase post-checkout \
    post-merge pre-push pre-receive update proc-receive post-receive post-update \
    reference-transaction push-to-checkout pre-auto-gc post-rewrite sendemail-validate \
    fsmonitor-watchman post-index-change p4-changelist p4-prepare-changelist \
    p4-post-changelist p4-pre-submit";

/// A path the git layout or a git config names, and the files git reads or runs beneath it
/// (whitespace-separated; none for a path git reads as a file).
type GitPath = (PathBuf, &'static str);

/// A file or tree git reads that the floor could not read as git reads it. What
/// `core.hooksPath` or include it names is unknown, so `SandboxPolicy` keeps the list and
/// enforce runs no command while it is not empty; observe and off are unaffected.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GitMetadataUnread {
    #[error("{path} is larger than {limit} bytes")]
    TooLarge { path: PathBuf, limit: u64 },
    #[error("{path} cannot be read: {reason}")]
    Unreadable { path: PathBuf, reason: String },
    /// A `core.hooksPath` or include value holding bytes that are not UTF-8: the path it names
    /// cannot be spelled here.
    #[error("{path} names a path that is not UTF-8")]
    NotUtf8 { path: PathBuf },
    #[error("{path} is included deeper than {depth} levels or past the {files}th config file")]
    IncludesUnfollowed {
        path: PathBuf,
        depth: usize,
        files: usize,
    },
    #[error("{tree} holds more than {limit} directories")]
    GitDirsUnlisted { tree: PathBuf, limit: usize },
}

/// What [`git_entries_in`] derives: the floor entries, and what it could not read.
#[derive(Debug, Default)]
pub struct GitEntries {
    pub protected: Vec<Protected>,
    pub unread: Vec<GitMetadataUnread>,
}

/// The git entries of the workspace's own repository, derived from its layout: a `.git`
/// directory contributes its [`GIT_DIR_PROTECTED_ENTRIES`], the same entries under every
/// submodule git directory (`.git/modules/**`, a pattern so a submodule added later is covered)
/// and the pointer files and `config.worktree` of every linked worktree it holds
/// (`.git/worktrees/*`); a `.git` *file* (linked worktree) is protected itself, and the git
/// directory it names — with its pointer files — plus that directory's `commondir` contribute
/// their entries. `core.hooksPath` in a protected `config` or `config.worktree`, in every
/// submodule's, or in the user's global git config protects the tree it names too, read and
/// resolved as git does ([`GitScan::hooks_path_in`]); every include read on the way is protected
/// like the config, in every spelling ([`GitScan::protect`]). A path so named at or above the
/// workspace, the home or one of `write_roots` (so `/` too) is [`narrowed`] to the files git
/// reads or runs beneath it. Symlinks are followed as git follows them (module doc); what
/// cannot be read as git reads it is returned as `unread`.
///
/// The global config is resolved under `env`, the daemon's ([`GitConfigEnv::global_configs`]):
/// each file git would read there is read for its hooks path and protected — as named and where
/// a link leads — when a regular file, a directory or nothing yet, never a device
/// (`GIT_CONFIG_GLOBAL=/dev/null`); a value the floor cannot resolve is returned as `unread`.
pub fn git_entries_in(
    ws: &Path,
    user_home: Option<&Path>,
    write_roots: &[PathBuf],
    env: &GitConfigEnv,
) -> GitEntries {
    let mut scan = GitScan::default();
    let workspace = [ws.to_path_buf()];
    let dot_git = ws.join(".git");
    let mut out: Vec<Protected> = Vec::new();
    match std::fs::metadata(&dot_git) {
        Ok(meta) if meta.is_file() => {
            scan.protect(&dot_git, "");
            if let Some(git_dir) = scan.pointer_target(&dot_git, ws, Some("gitdir:")) {
                for file in WORKTREE_POINTER_FILES {
                    scan.protect(&git_dir.join(file), "");
                }
                let common = scan.pointer_target(&git_dir.join("commondir"), &git_dir, None);
                // Git ignores a `core.worktree` for a linked worktree: the workspace anchors
                for dir in common.iter().chain([&git_dir]) {
                    scan.git_dir_entries(dir);
                    scan.hooks_path_entries(dir, &workspace, user_home);
                }
            }
        }
        _ => {
            // A directory, or nothing yet: the top-level layout is protected before `git init` too
            let dot_git = canonical_path(&dot_git);
            let modules = dot_git.join("modules");
            let worktrees = dot_git.join("worktrees");
            let globs: Option<Vec<Protected>> = GIT_DIR_PROTECTED_ENTRIES
                .iter()
                .map(|entry| glob_under(&modules, &format!("**/{entry}")))
                .chain(
                    WORKTREE_POINTER_FILES
                        .iter()
                        .chain([&WORKTREE_CONFIG])
                        .map(|file| glob_under(&worktrees, &format!("*/{file}"))),
                )
                .collect();
            let by_pattern = globs.is_some();
            out.extend(globs.into_iter().flatten());
            for git_dir in scan.submodule_git_dirs(&modules) {
                if !by_pattern {
                    scan.git_dir_entries(&git_dir);
                }
                scan.hooks_path_entries(&git_dir, &workspace, user_home);
            }
            // Every linked worktree anchors the shared config's relative `core.hooksPath` too
            let mut anchors = workspace.to_vec();
            for (dir, root) in scan.linked_worktrees(&worktrees) {
                if !by_pattern {
                    for file in WORKTREE_POINTER_FILES.iter().chain([&WORKTREE_CONFIG]) {
                        scan.protect(&dir.join(file), "");
                    }
                }
                scan.hooks_path_in(&dir.join(WORKTREE_CONFIG), root.as_slice(), user_home);
                anchors.extend(root);
            }
            scan.git_dir_entries(&dot_git);
            scan.hooks_path_entries(&dot_git, &anchors, user_home);
        }
    }
    // Git also honours a `core.hooksPath` set in the user's global config, wherever the
    // environment puts it; a device there (`/dev/null`, an empty config) is read, not protected
    let (global_configs, unresolved) = env.global_configs(user_home);
    scan.unread.extend(unresolved);
    for config in &global_configs {
        if is_file_dir_or_missing(config) {
            scan.protect(config, "");
        }
        scan.hooks_path_in(config, &workspace, user_home);
    }
    let roots: Vec<PathBuf> = std::iter::once(ws)
        .chain(user_home)
        .chain(write_roots.iter().map(PathBuf::as_path))
        .map(canonical_path)
        .collect();
    for (path, beneath) in scan.paths {
        out.extend(narrowed(path, beneath, &roots));
    }
    out.extend(scan.nodes.into_iter().map(|path| Protected::Node { path }));
    out.sort();
    out.dedup();
    GitEntries {
        protected: out,
        unread: scan.unread,
    }
}

/// Whether `path` (followed) is something a floor entry can stand for: a regular file, a
/// directory, or nothing yet — never a device, so a config that is `/dev/null` (git's "no
/// config") does not put `/dev/null` in the floor.
fn is_file_dir_or_missing(path: &Path) -> bool {
    match std::fs::metadata(path) {
        Ok(meta) => meta.is_file() || meta.is_dir(),
        Err(error) => error.kind() == std::io::ErrorKind::NotFound,
    }
}

/// `path` as a floor entry, unless it is at or above one of `roots` — or, lying outside them,
/// leads there (a linked home) — where the tree would refuse every write beneath it: then only
/// the files git reads or runs there (`beneath`).
fn narrowed(path: PathBuf, beneath: &str, roots: &[PathBuf]) -> Vec<Protected> {
    let holds_a_root = |path: &Path| roots.iter().any(|root| is_within(root, path));
    let outside = !roots.iter().any(|root| is_within(&path, root));
    if !(holds_a_root(&path) || (outside && holds_a_root(&canonical_path(&path)))) {
        return vec![Protected::Path { path }];
    }
    beneath
        .split_whitespace()
        .map(|name| Protected::Path {
            path: path.join(name),
        })
        .collect()
}

/// What git reads or runs beneath a git directory's entry: the hooks, the files of `info/`, and
/// nothing beneath `config` or `config.worktree`, files.
fn read_beneath(entry: &str) -> &'static str {
    match entry {
        "hooks" => GIT_HOOK_NAMES,
        "info" => "exclude attributes sparse-checkout grafts",
        _ => "",
    }
}

/// Collects, while the git layout is read, every path to protect and every file that could not
/// be read as git reads it.
#[derive(Default)]
struct GitScan {
    paths: Vec<GitPath>,
    nodes: Vec<PathBuf>,
    unread: Vec<GitMetadataUnread>,
}

impl GitScan {
    fn push(&mut self, path: PathBuf, beneath: &'static str) {
        if !self.paths.iter().any(|(known, _)| known == &path) {
            self.paths.push((path, beneath));
        }
    }

    /// `path` as a floor entry with `beneath`, as named and in the spellings a link adds
    /// ([`GitScan::protect_links`]).
    fn protect(&mut self, path: &Path, beneath: &'static str) {
        self.push(fold_dots(path), beneath);
        self.protect_links(path, beneath);
    }

    /// The spellings a symlink adds to `path`: its canonical path when a link leads elsewhere —
    /// a regular file, a directory or nothing yet; never a device, so `~/.gitconfig -> /dev/null`
    /// does not protect `/dev/null` — and each link on the way ([`GitScan::link_nodes`]).
    fn protect_links(&mut self, path: &Path, beneath: &'static str) {
        let folded = fold_dots(path);
        let canonical = canonical_path(&folded);
        if !is_same_path(&canonical, &folded) && is_file_dir_or_missing(&canonical) {
            self.push(canonical, beneath);
        }
        self.protect_link_nodes(&folded);
    }

    /// Each link on the way to `folded` ([`GitScan::link_nodes`]) as a [`Protected::Node`]: the
    /// link itself, never the tree it leads to, so a grant under its target stands.
    fn protect_link_nodes(&mut self, folded: &Path) {
        for node in self.link_nodes(folded) {
            if !self.nodes.contains(&node) {
                self.nodes.push(node);
            }
        }
    }

    /// Each symlink the kernel meets resolving `folded`, in a link's target too, as the node it
    /// sees (`<canonical parent>/<name>`): the node a command would re-point, wherever it lies,
    /// since a grant can make any tree writable.
    fn link_nodes(&self, folded: &Path) -> Vec<PathBuf> {
        let components = |path: &Path| -> Vec<OsString> {
            path.components()
                .rev()
                .map(|c| c.as_os_str().into())
                .collect()
        };
        let mut rest = components(folded);
        let mut prefix = PathBuf::new();
        let mut out = Vec::new();
        let mut hops = 0;
        while let Some(name) = rest.pop() {
            if name == ".." {
                prefix.pop();
            } else if name != "." {
                prefix.push(&name);
            }
            let Ok(target) = std::fs::read_link(&prefix) else {
                continue;
            };
            if let (Some(parent), Some(node)) = (prefix.parent(), prefix.file_name()) {
                out.push(canonical_path(parent).join(node));
            }
            prefix.pop();
            hops += 1;
            if hops > SYMLINK_HOPS {
                break;
            }
            rest.extend(components(&target));
        }
        out
    }

    /// The text of a file git reads, through a symlink as git reads it, or `None` for a missing
    /// one (git skips it too). The spellings a link adds are protected, so no command can
    /// rewrite the target or re-point the link; a file git would read and the floor cannot is
    /// recorded as unread.
    fn read(&mut self, file: &Path) -> Option<String> {
        self.protect_links(file, "");
        match read_git_file(file) {
            Ok(text) => text,
            Err(unread) => {
                self.unread.push(unread);
                None
            }
        }
    }

    /// A git directory's [`GIT_DIR_PROTECTED_ENTRIES`].
    fn git_dir_entries(&mut self, git_dir: &Path) {
        for entry in GIT_DIR_PROTECTED_ENTRIES {
            self.protect(&git_dir.join(entry), read_beneath(entry));
        }
    }

    /// The trees a `core.hooksPath` in a git directory's `config`, then its `config.worktree`,
    /// names; both are read whether or not `extensions.worktreeConfig` is set.
    fn hooks_path_entries(
        &mut self,
        git_dir: &Path,
        anchors: &[PathBuf],
        user_home: Option<&Path>,
    ) {
        for config in ["config", WORKTREE_CONFIG] {
            self.hooks_path_in(&git_dir.join(config), anchors, user_home);
        }
    }

    /// The trees a `core.hooksPath` in `config` names (the effective value and every conditional
    /// one, [`ConfigHooksPath`]), a relative one resolved against each of `anchors` — the working
    /// trees git resolves it against — and against the file's `core.worktree` when it sets one.
    fn hooks_path_in(&mut self, config: &Path, anchors: &[PathBuf], user_home: Option<&Path>) {
        let mut hooks = ConfigHooksPath::default();
        hooks.visit(self, config, user_home, 0, false);
        let mut anchors = anchors.to_vec();
        if let Some(worktree) = hooks
            .worktree
            .as_deref()
            .and_then(|value| include_path(value, config, user_home).ok())
        {
            anchors.push(worktree);
        }
        for tree in hooks.trees(&anchors, user_home) {
            self.protect(&tree, GIT_HOOK_NAMES);
        }
    }

    /// The directory a one-line pointer file names (`gitdir: <path>` in a `.git` file, the bare
    /// path in `commondir`), resolved against `base` and followed through any symlink as git
    /// follows it, when it exists as a directory: canonical, so its entries are spelled as the
    /// kernel sees them, with every link on the way protected so no command can re-point one.
    fn pointer_target(
        &mut self,
        file: &Path,
        base: &Path,
        prefix: Option<&str>,
    ) -> Option<PathBuf> {
        let text = self.read(file)?;
        let line = text.lines().next()?.trim();
        let target = match prefix {
            Some(prefix) => line.strip_prefix(prefix)?.trim(),
            None => line,
        };
        if target.is_empty() {
            return None;
        }
        let target = fold_dots(&base.join(target));
        if !std::fs::metadata(&target).is_ok_and(|meta| meta.is_dir()) {
            return None;
        }
        self.protect_link_nodes(&target);
        Some(canonical_path(&target))
    }

    /// The submodule git directories under `modules` now: every directory, since a submodule at
    /// `a/b` lives at `modules/a/b` and only `HEAD` tells a git directory from the `a` on the way;
    /// a git directory is walked further only into its own `modules`. Enumerated whether or not
    /// the entries are protected by pattern, since every submodule's `core.hooksPath` is read.
    /// Stops and records [`GitMetadataUnread::GitDirsUnlisted`] past [`GIT_DIRS_LIMIT`].
    fn submodule_git_dirs(&mut self, modules: &Path) -> Vec<PathBuf> {
        let mut pending = std::collections::VecDeque::from([modules.to_path_buf()]);
        let mut out = Vec::new();
        while let Some(dir) = pending.pop_front() {
            for child in existing_children(&dir, |_| true) {
                if !std::fs::symlink_metadata(&child).is_ok_and(|meta| meta.is_dir()) {
                    continue;
                }
                if out.len() >= GIT_DIRS_LIMIT {
                    self.unread.push(GitMetadataUnread::GitDirsUnlisted {
                        tree: modules.to_path_buf(),
                        limit: GIT_DIRS_LIMIT,
                    });
                    return out;
                }
                let is_git_dir =
                    std::fs::symlink_metadata(child.join("HEAD")).is_ok_and(|meta| meta.is_file());
                pending.push_back(if is_git_dir {
                    child.join("modules")
                } else {
                    child.clone()
                });
                out.push(child);
            }
        }
        out
    }

    /// The linked worktrees (`.git/worktrees/*`), each with its root as its `gitdir` pointer spells
    /// it (the parent of the `.git` file it names), which anchors that worktree's relative
    /// `core.hooksPath`. Bounded like the modules walk.
    fn linked_worktrees(&mut self, worktrees: &Path) -> Vec<(PathBuf, Option<PathBuf>)> {
        let mut out = Vec::new();
        for dir in existing_children(worktrees, |_| true) {
            if !std::fs::symlink_metadata(&dir).is_ok_and(|meta| meta.is_dir()) {
                continue;
            }
            if out.len() >= GIT_DIRS_LIMIT {
                self.unread.push(GitMetadataUnread::GitDirsUnlisted {
                    tree: worktrees.to_path_buf(),
                    limit: GIT_DIRS_LIMIT,
                });
                return out;
            }
            let root = self
                .read(&dir.join("gitdir"))
                .and_then(|text| text.lines().next().map(str::trim).map(str::to_owned))
                .filter(|line| !line.is_empty())
                .and_then(|line| fold_dots(&dir.join(line)).parent().map(Path::to_path_buf));
            out.push((dir, root));
        }
        out
    }
}

/// One read of a file git reads, opened once and judged by `fstat` on the open handle: `Ok(None)`
/// for a missing file or a directory (no config to git: it skips the first and warns past the
/// second), `Err` for one git would read and the floor cannot — larger than
/// [`GIT_METADATA_READ_LIMIT`] (never a prefix), or not readable. What git reads is read:
/// `~/.gitconfig -> /dev/null` is an empty config, and a FIFO yields what it holds
/// (`O_NONBLOCK`, so it cannot stall the daemon). Bytes that are not UTF-8 are replaced: git's
/// syntax is ASCII, and a value that carries them is caught where it is used
/// ([`ConfigHooksPath::assign`]).
fn read_git_file(file: &Path) -> Result<Option<String>, GitMetadataUnread> {
    let unreadable = |error: &std::io::Error| GitMetadataUnread::Unreadable {
        path: file.to_path_buf(),
        reason: error.to_string(),
    };
    let too_large = || GitMetadataUnread::TooLarge {
        path: file.to_path_buf(),
        limit: GIT_METADATA_READ_LIMIT,
    };
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NONBLOCK);
    let mut handle = match options.open(file) {
        Ok(handle) => handle,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(None);
        }
        Err(error) => return Err(unreadable(&error)),
    };
    let meta = handle.metadata().map_err(|error| unreadable(&error))?;
    if meta.is_dir() {
        return Ok(None);
    }
    if meta.len() > GIT_METADATA_READ_LIMIT {
        return Err(too_large());
    }
    let mut bytes = Vec::new();
    (&mut handle)
        .take(GIT_METADATA_READ_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| unreadable(&error))?;
    if u64::try_from(bytes.len()).is_ok_and(|len| len > GIT_METADATA_READ_LIMIT) {
        return Err(too_large());
    }
    Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
}

/// The present matches of a floor `glob` beneath its literal `prefix`, walked breadth-first
/// without following a symlink; `Err(prefix)` once more than `limit` directories would be
/// queued, `Err(dir)` for a directory whose entries cannot all be read
/// ([`unread_dir_refuses`], the prefix's owner guarding it). Beneath `**`, a directory holding
/// a `HEAD` file is a git directory, whose only subdirectory that can hold an entry git reads
/// is `modules`, so no object store is walked.
#[cfg(unix)]
pub(super) fn present_matches(
    glob: &str,
    prefix: &Path,
    limit: usize,
) -> Result<Vec<PathBuf>, PathBuf> {
    let Some(matcher) = glob_matcher(glob) else {
        return Ok(Vec::new());
    };
    let spans = glob.contains("**");
    let depth = Path::new(glob).components().count() - prefix.components().count();
    let owners: Vec<u32> = std::fs::symlink_metadata(prefix)
        .map(|meta| meta.uid())
        .into_iter()
        .collect();
    let mut pending = std::collections::VecDeque::from([(prefix.to_path_buf(), 1)]);
    let (mut found, mut queued) = (Vec::new(), 0);
    while let Some((dir, level)) = pending.pop_front() {
        let head = std::fs::symlink_metadata(dir.join("HEAD"));
        let git_dir = spans && head.is_ok_and(|meta| meta.is_file());
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if unread_dir_refuses(&dir, &error, &owners) => return Err(dir),
            Err(_) => continue,
        };
        for entry in entries {
            let entry = entry.map_err(|_| dir.clone())?;
            let child = entry.path();
            let kind = match entry.file_type() {
                Ok(kind) => kind,
                Err(error) if unread_dir_refuses(&dir, &error, &owners) => return Err(dir),
                Err(_) => continue,
            };
            if kind.is_symlink() {
                continue;
            } else if matcher.is_match(&child) {
                found.push(child);
            } else if kind.is_dir()
                && (spans || level < depth)
                && (!git_dir || entry.file_name() == "modules")
            {
                queued += 1;
                if queued > limit {
                    return Err(prefix.to_path_buf());
                }
                pending.push_back((child, level + 1));
            }
        }
    }
    Ok(found)
}

/// The trees a `core.hooksPath` value names as git reads it: `~` and `~/…` under the user's home
/// (nothing when there is none), an absolute path as is, a relative one under each of the
/// working trees in `anchors`. Git looks `~user/…` up in the user database, which is not read
/// here, so both homes it names on the usual layout are protected: the user's own and the
/// sibling `<home>/../user`. Spelled as named: [`GitScan::protect`] adds where links lead.
fn hooks_path_trees(value: &str, anchors: &[PathBuf], user_home: Option<&Path>) -> Vec<PathBuf> {
    match (value.strip_prefix('~'), user_home) {
        (Some(_), None) => Vec::new(),
        (Some(rest), Some(home)) => {
            let (user, rest) = rest.split_once('/').unwrap_or((rest, ""));
            let mut homes = vec![home.to_path_buf()];
            if !user.is_empty() {
                homes.extend(home.parent().map(|parent| parent.join(user)));
            }
            homes.iter().map(|home| home.join(rest)).collect()
        }
        (None, _) if Path::new(value).is_absolute() => vec![PathBuf::from(value)],
        (None, _) => anchors.iter().map(|anchor| anchor.join(value)).collect(),
    }
}

/// `core.hooksPath` as git reads one config file: in file order, following `[include]` and
/// `[includeIf]` files in place (relative to the including file, `~` under the user's home, a
/// missing one skipped, one [`include_path`] cannot resolve unread). The last assignment wins
/// and an empty one unsets. `core.worktree` is
/// read the same way, for where a relative hooks path is anchored.
#[derive(Default)]
struct ConfigHooksPath {
    effective: Option<String>,
    /// Values set under an `[includeIf]`: its condition is not evaluated here, so they are
    /// protected beside the effective one.
    conditional: Vec<String>,
    /// The last `core.worktree`, as written.
    worktree: Option<String>,
    files_read: usize,
}

impl ConfigHooksPath {
    /// Reads `config` through `scan`, so every include is protected as it is read and a file
    /// past a bound is recorded rather than skipped.
    fn visit(
        &mut self,
        scan: &mut GitScan,
        config: &Path,
        user_home: Option<&Path>,
        depth: usize,
        conditional: bool,
    ) {
        if depth > GIT_CONFIG_INCLUDE_DEPTH || self.files_read >= GIT_CONFIG_FILES_LIMIT {
            scan.unread.push(GitMetadataUnread::IncludesUnfollowed {
                path: config.to_path_buf(),
                depth: GIT_CONFIG_INCLUDE_DEPTH,
                files: GIT_CONFIG_FILES_LIMIT,
            });
            return;
        }
        let Some(text) = scan.read(config) else {
            return;
        };
        self.files_read += 1;
        let (entries, rejected_line) = config_entries(&text);
        if let Some(line) = rejected_line {
            scan.unread.push(GitMetadataUnread::Unreadable {
                path: config.to_path_buf(),
                reason: format!(
                    "its line {line} does not parse as git config, so nothing from there on is read"
                ),
            });
        }
        for entry in entries {
            let name = (
                entry.section.as_str(),
                entry.subsection.as_deref(),
                entry.key.as_str(),
            );
            let include_is_conditional = match name {
                ("core", None, "hookspath") => {
                    self.assign(scan, config, entry.value, conditional);
                    continue;
                }
                ("core", None, "worktree") => {
                    if let Some(value) = entry.value.filter(|value| !value.is_empty()) {
                        self.worktree = Some(value);
                    }
                    continue;
                }
                ("include", None, "path") => conditional,
                ("includeif", Some(_), "path") => true,
                _ => continue,
            };
            let Some(value) = entry.value.as_deref() else {
                continue;
            };
            if value.contains('\u{fffd}') {
                scan.unread.push(GitMetadataUnread::NotUtf8 {
                    path: config.to_path_buf(),
                });
                continue;
            }
            // Git expands `%(prefix)/` to its own install prefix, which is not known here
            let include = match value.strip_prefix("%(prefix)/") {
                Some(_) => Err("is under git's install prefix"),
                None => include_path(value, config, user_home),
            };
            let include = match include {
                Ok(include) => include,
                Err(reason) => {
                    scan.unread.push(GitMetadataUnread::Unreadable {
                        path: config.to_path_buf(),
                        reason: format!("its include {value:?} {reason}, not resolved here"),
                    });
                    continue;
                }
            };
            // Protected whether or not it exists yet, so no command can create it
            scan.protect(&include, "");
            self.visit(scan, &include, user_home, depth + 1, include_is_conditional);
        }
    }

    /// A bare `hooksPath` with no `=` is an error git refuses to run with, so it changes nothing;
    /// one whose bytes were not UTF-8 names a path that cannot be spelled here, so it is unread.
    fn assign(
        &mut self,
        scan: &mut GitScan,
        config: &Path,
        value: Option<String>,
        conditional: bool,
    ) {
        let Some(value) = value else {
            return;
        };
        if value.contains('\u{fffd}') {
            scan.unread.push(GitMetadataUnread::NotUtf8 {
                path: config.to_path_buf(),
            });
            return;
        }
        if !conditional {
            self.effective = (!value.is_empty()).then_some(value);
        } else if !value.is_empty() && !self.conditional.contains(&value) {
            self.conditional.push(value);
        }
    }

    /// Each hooks tree resolved by [`hooks_path_trees`].
    fn trees(&self, anchors: &[PathBuf], user_home: Option<&Path>) -> Vec<PathBuf> {
        self.effective
            .iter()
            .chain(&self.conditional)
            .flat_map(|value| hooks_path_trees(value, anchors, user_home))
            .collect()
    }
}

/// An include's `path` as git resolves it: `~` and `~/…` under the user's home, an absolute path
/// as is, a relative one beside the including file. Else why it cannot be resolved as git would:
/// `~user` (git looks it up in the user database, not read here), `~` with no home, an empty
/// value, no including directory. `core.worktree` resolves the same way against its git directory.
fn include_path(
    value: &str,
    config: &Path,
    user_home: Option<&Path>,
) -> Result<PathBuf, &'static str> {
    match value.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => user_home
            .map(|home| home.join(rest.trim_start_matches('/')))
            .ok_or("is under a home directory that is not known"),
        Some(_) => Err("is under another user's home"),
        None if value.is_empty() => Err("is empty"),
        None if Path::new(value).is_absolute() => Ok(PathBuf::from(value)),
        None => config
            .parent()
            .map(|dir| dir.join(value))
            .ok_or("is relative to no directory"),
    }
}

/// One `key [= value]` of a git config file under its `[section "subsection"]`: section and key
/// lowercased as git folds them, a quoted subsection verbatim and a dotted one lowercased, `None`
/// for a bare key.
struct ConfigEntry {
    section: String,
    subsection: Option<String>,
    key: String,
    value: Option<String>,
}

/// The entries of a git config file in order, tokenised as git does: `#`/`;` comments, a key
/// allowed on its header's line, quoted and escaped values, `\` line continuations. Parsing
/// stops at a line git rejects (git then runs no command), returned so the file is unread.
fn config_entries(text: &str) -> (Vec<ConfigEntry>, Option<usize>) {
    let text = text
        .strip_prefix('\u{feff}')
        .unwrap_or(text)
        .replace("\r\n", "\n");
    let mut chars = text.chars();
    let mut header: Option<(String, Option<String>)> = None;
    let mut out = Vec::new();
    while let Some(c) = chars.next() {
        match c {
            c if is_git_space(c) => {}
            '#' | ';' => {
                chars.find(|&c| c == '\n');
            }
            '[' => match config_header(&mut chars) {
                Some(parsed) => header = Some(parsed),
                None => return (out, Some(line_read(&text, &chars))),
            },
            c if c.is_ascii_alphabetic() => {
                let Some((key, value)) = config_key_value(c, &mut chars) else {
                    return (out, Some(line_read(&text, &chars)));
                };
                if let Some((section, subsection)) = &header {
                    out.push(ConfigEntry {
                        section: section.clone(),
                        subsection: subsection.clone(),
                        key,
                        value,
                    });
                }
            }
            _ => return (out, Some(line_read(&text, &chars))),
        }
    }
    (out, None)
}

/// The line, counted from 1, of the last character `chars` took from `text`.
fn line_read(text: &str, chars: &std::str::Chars<'_>) -> usize {
    let read = &text[..text.len() - chars.as_str().len()];
    read.strip_suffix('\n')
        .unwrap_or(read)
        .matches('\n')
        .count()
        + 1
}

fn is_git_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r')
}

/// `section]`, `section "subsection"]` or the older `section.subsection]` after the `[`: git splits
/// a dotted name at its first `.` and lowercases it, and rejects one holding a character outside
/// `[A-Za-z0-9.-]` (`[includeIf.gitdir:/path]`) as a bad config line, running no command.
fn config_header(chars: &mut std::str::Chars<'_>) -> Option<(String, Option<String>)> {
    let mut name = String::new();
    let quoted = loop {
        match chars.next()? {
            ']' if name.is_empty() => return None,
            ']' => break None,
            '\n' => return None,
            c if is_git_space(c) => break Some(config_subsection(chars)?),
            c if c.is_ascii_alphanumeric() || c == '-' || c == '.' => {
                name.push(c.to_ascii_lowercase());
            }
            _ => return None,
        }
    };
    let (section, dotted) = match name.split_once('.') {
        Some((section, dotted)) => (section.to_owned(), Some(dotted.to_owned())),
        None => (name, None),
    };
    let subsection = match (dotted, quoted) {
        (Some(dotted), Some(quoted)) => Some(format!("{dotted}.{quoted}")),
        (dotted, quoted) => dotted.or(quoted),
    };
    Some((section, subsection))
}

/// `"subsection"]`, after the space that ends the section name.
fn config_subsection(chars: &mut std::str::Chars<'_>) -> Option<String> {
    let mut c = chars.next()?;
    while c != '\n' && is_git_space(c) {
        c = chars.next()?;
    }
    if c != '"' {
        return None;
    }
    let mut subsection = String::new();
    loop {
        match chars.next()? {
            '"' => break,
            '\n' => return None,
            '\\' => subsection.push(chars.next().filter(|&c| c != '\n')?),
            c => subsection.push(c),
        }
    }
    (chars.next()? == ']').then_some(subsection)
}

/// The rest of a key starting with `first`, then its value when an `=` follows.
fn config_key_value(
    first: char,
    chars: &mut std::str::Chars<'_>,
) -> Option<(String, Option<String>)> {
    let mut key = first.to_ascii_lowercase().to_string();
    let mut c = chars.next();
    while let Some(k) = c.filter(|k| k.is_ascii_alphanumeric() || *k == '-') {
        key.push(k.to_ascii_lowercase());
        c = chars.next();
    }
    while matches!(c, Some(' ' | '\t')) {
        c = chars.next();
    }
    match c {
        None | Some('\n') => Some((key, None)),
        Some('=') => Some((key, Some(config_value(chars)?))),
        Some(_) => None,
    }
}

/// A value up to its end of line: outside quotes, surrounding space dropped, inner runs kept
/// and a comment ends it; escapes `\\ \" \n \t \b`; `\` at the end of a line continues it.
fn config_value(chars: &mut std::str::Chars<'_>) -> Option<String> {
    let mut value = String::new();
    let (mut quoted, mut comment, mut spaces) = (false, false, 0);
    loop {
        let c = chars.next().unwrap_or('\n');
        if c == '\n' {
            return (!quoted).then_some(value);
        }
        if comment {
            continue;
        }
        if is_git_space(c) && !quoted {
            spaces += usize::from(!value.is_empty());
            continue;
        }
        if !quoted && matches!(c, '#' | ';') {
            comment = true;
            continue;
        }
        value.push_str(&" ".repeat(spaces));
        spaces = 0;
        match c {
            '\\' => match chars.next().unwrap_or('\n') {
                '\n' => {}
                't' => value.push('\t'),
                'b' => value.push('\u{8}'),
                'n' => value.push('\n'),
                escaped @ ('\\' | '"') => value.push(escaped),
                _ => return None,
            },
            '"' => quoted = !quoted,
            c => value.push(c),
        }
    }
}

#[cfg(test)]
#[path = "git_config_tests.rs"]
mod tests;
