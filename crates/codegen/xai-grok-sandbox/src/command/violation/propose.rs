//! The grant a violation proposes on its card. A file path proposes a directory: the nearest
//! existing one when it is specific enough, otherwise the highest *missing* ancestor under a
//! known base — the directory the command was about to create (`$PYTHONUSERBASE`,
//! `~/.local/lib`). When no directory survives the climb the one file is
//! proposed, never an ancestor — and a target that is itself a too-broad
//! directory (`/Users`, the home) is informational. A connection read from the output proposes
//! nothing: the proxy never saw it, so it is informational. Every coarse
//! proposal is a guess the card asks the user to confirm; the wire sets that flag.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::command::canonical::{canonical_path, contains_path, depth, is_same_path, is_within};
use crate::command::grants::GrantSubject;
use crate::command::policy::{SandboxPolicy, symlink_free_spelling};
use crate::command::violation::coarse::nearest_existing_dir;
use crate::command::violation::{Blocked, Disposition, InformationalReason};

/// What bounds the directory a proposal may name.
#[derive(Clone, Copy, Debug)]
pub struct ProposalBounds<'a> {
    /// A proposal never climbs to the workspace or one of its ancestors.
    pub workspace_root: &'a Path,
    /// Never proposed itself; the user-level bases (`~/.local`, `~/Library`, `~/.config`) hang
    /// off it. `None` when the host has no home directory.
    pub user_home: Option<&'a Path>,
    /// Bases the command's environment names — see [`bases_from_env`].
    pub extra_bases: &'a [PathBuf],
}

/// The bases a command's environment can add: `$PYTHONUSERBASE` (pip's `--user` target) and
/// `$XDG_DATA_HOME`.
pub fn bases_from_env(vars: impl IntoIterator<Item = (OsString, OsString)>) -> Vec<PathBuf> {
    vars.into_iter()
        .filter(|(name, _)| name == "PYTHONUSERBASE" || name == "XDG_DATA_HOME")
        .map(|(_, value)| PathBuf::from(value))
        .filter(|path| path.is_absolute())
        .collect()
}

/// Home-relative bases: specific enough by depth, yet never offered as a whole — a grant on
/// `~/.local` or `~/Library` would cover far more than the command touched. Below one of these
/// the first missing directory is offered instead.
const HOME_BASES: &[&str] = &[
    "Library",
    "Library/Application Support",
    "Library/Python",
    ".config",
    ".local",
    ".local/share",
];

/// Directories a proposal never names, whatever exists on disk.
const NEVER_PROPOSED: &[&str] = &[
    "/", "/Users", "/home", "/opt", "/usr", "/var", "/etc", "/private", "/Library", "/System",
];

/// A proposed directory has at least this many components below the root ([`depth`]): `/tmp/x`,
/// its `/private/tmp/x` spelling on macOS, or `/usr/local` is too broad to offer on a card.
const MIN_PROPOSED_COMPONENTS: usize = 3;

pub(super) struct Proposal {
    pub subject: Option<GrantSubject>,
    pub disposition: Disposition,
}

/// The grant the card offers and whether it may be offered at all. A protected path or directory
/// proposes nothing, nor does a path a read deny covers ([`SandboxPolicy::is_read_floor`]: a
/// profile `deny` is final like the floor, read or write) or a
/// folder at or beneath a glob entry's literal prefix
/// ([`SandboxPolicy::is_ungrantable`]), one spelled through a symlink (the store refuses it, and
/// the card would show a folder other than the one granted) or one that resolves to a folder too
/// broad to offer; a connection read from a stopped command's output is one the proxy never
/// saw (it holds and asks about the ones it sees, mid-command), so no host grant can take effect
/// and the card only informs. A folder that holds a read deny is never offered: the proposal
/// narrows toward the target until none lies inside it ([`narrowed_past_denies`]).
pub(super) fn propose(
    blocked: &Blocked,
    policy: &SandboxPolicy,
    bounds: &ProposalBounds<'_>,
) -> Proposal {
    let informational = |reason: InformationalReason| Proposal {
        subject: None,
        disposition: Disposition::informational(reason),
    };
    match blocked {
        Blocked::FsWrite { path } | Blocked::FsRead { path } => {
            if policy.is_protected(path) {
                return informational(InformationalReason::ProtectedTarget);
            }
            if policy.is_read_floor(path) {
                return informational(InformationalReason::ProfileDeny);
            }
            // A write into a curated build-cache tree (outside its verified subpaths, or the
            // policy would have allowed it) proposes the family, not a directory: one card for
            // the workspace, whatever subpath the toolchain touched first
            if matches!(blocked, Blocked::FsWrite { .. }) && policy.in_build_caches(path) {
                return Proposal {
                    subject: Some(GrantSubject::BuildCaches),
                    disposition: Disposition::Grantable,
                };
            }
            // When the climb refuses (workspace ancestor, home, never-proposed root) the card
            // offers the one file, never the ancestor directory; a target
            // that is itself too broad and not a file (`/Users`, home, a missing `/srv`) gets none
            let root = match proposal_dir(path, bounds) {
                Some(dir) => dir,
                None if is_too_broad(path, bounds) && !path.is_file() => {
                    return informational(InformationalReason::ProtectedTarget);
                }
                None => path.to_path_buf(),
            };
            let Some(root) = narrowed_past_denies(root, path, policy) else {
                return informational(InformationalReason::ProfileDeny);
            };
            if policy.is_ungrantable(&root)
                || symlink_free_spelling(&root).is_err()
                || resolves_too_broad(&root, bounds)
            {
                return informational(InformationalReason::ProtectedTarget);
            }
            let subject = if matches!(blocked, Blocked::FsWrite { .. }) {
                GrantSubject::FsWriteRoot { root }
            } else {
                GrantSubject::FsRead { root }
            };
            Proposal {
                subject: Some(subject),
                disposition: Disposition::Grantable,
            }
        }
        Blocked::Net { .. } => informational(InformationalReason::UnproxiedNetwork),
        Blocked::Capability { .. } => informational(InformationalReason::Capability),
        // No target: never a card; the disposition is moot
        Blocked::Unknown { .. } => informational(InformationalReason::Unattributed),
    }
}

/// The directory a grant for `path` proposes. The nearest existing directory is proposed when it
/// is specific enough and not a base. When it is too broad ([`is_too_broad`]: the home directory,
/// a workspace ancestor, a top-level system directory, anything shallower than
/// [`MIN_PROPOSED_COMPONENTS`]) or a base the card never offers whole (`~/.local`, `~/Library`,
/// `$PYTHONUSERBASE` — [`ProposalBounds::extra_bases`]), the proposal is the highest *missing*
/// ancestor that is specific enough: the directory the command was about to create, so a fresh
/// `pip install --user` tree is offered once at `$PYTHONUSERBASE` (or `~/.local/lib`) rather than
/// once per subdirectory. `None` when no directory survives; the caller then offers the file
/// itself. Existence and kind are asked through symlinks, as [`nearest_existing_dir`] asks them.
fn proposal_dir(path: &Path, bounds: &ProposalBounds<'_>) -> Option<PathBuf> {
    let nearest = nearest_existing_dir(path);
    if !is_too_broad(&nearest, bounds) && !is_base(&nearest, bounds) {
        return Some(nearest);
    }
    let mut highest_allowed: Option<PathBuf> = None;
    let mut cursor = path.parent();
    while let Some(dir) = cursor {
        if dir.exists() {
            break;
        }
        if !is_too_broad(dir, bounds) {
            highest_allowed = Some(dir.to_path_buf());
        }
        cursor = dir.parent();
    }
    highest_allowed
}

/// `root` or the first folder below it on the way to `path` that holds no read deny
/// ([`SandboxPolicy::holds_read_deny`]), ending at `path` itself; `None` when even `path` holds
/// one (a folder around a denied file), or `root` is not an ancestor of `path` as spelled.
fn narrowed_past_denies(root: PathBuf, path: &Path, policy: &SandboxPolicy) -> Option<PathBuf> {
    if !policy.holds_read_deny(&root) {
        return Some(root);
    }
    let mut below = path.strip_prefix(&root).ok()?.components();
    let mut candidate = root;
    while policy.holds_read_deny(&candidate) {
        candidate.push(below.next()?);
    }
    Some(candidate)
}

fn is_base(dir: &Path, bounds: &ProposalBounds<'_>) -> bool {
    contains_path(bounds.extra_bases, dir)
        || bounds.user_home.is_some_and(|home| {
            HOME_BASES
                .iter()
                .any(|rel| is_same_path(&home.join(rel), dir))
        })
}

/// Whether `dir` is too broad for a card to offer as a grant root — the same cap a folder the
/// user types into the card is held to.
pub fn is_too_broad(dir: &Path, bounds: &ProposalBounds<'_>) -> bool {
    is_too_broad_anywhere(dir, bounds.user_home)
        || (!is_same_path(dir, bounds.workspace_root) && is_within(bounds.workspace_root, dir))
}

/// The part of [`is_too_broad`] that holds in every workspace, which the grant store applies to
/// every row it takes: a top-level system directory, the home, anything shallower than
/// [`MIN_PROPOSED_COMPONENTS`].
pub(crate) fn is_too_broad_anywhere(dir: &Path, user_home: Option<&Path>) -> bool {
    NEVER_PROPOSED
        .iter()
        .any(|never| is_same_path(Path::new(never), dir))
        || user_home.is_some_and(|home| is_same_path(home, dir))
        || depth(dir) < MIN_PROPOSED_COMPONENTS
}

/// Whether `root` resolves to a folder too broad to offer: the store keeps a root's canonical
/// spelling, so `<ws>/out -> /` would grant `/`. A file target is judged as spelled.
fn resolves_too_broad(root: &Path, bounds: &ProposalBounds<'_>) -> bool {
    let canonical = canonical_path(root);
    let (ws, home) = (
        canonical_path(bounds.workspace_root),
        bounds.user_home.map(canonical_path),
    );
    let resolved = ProposalBounds {
        workspace_root: &ws,
        user_home: home.as_deref(),
        extra_bases: bounds.extra_bases,
    };
    !canonical.is_file() && is_too_broad(&canonical, &resolved)
}

#[cfg(test)]
#[path = "propose_tests.rs"]
mod tests;
