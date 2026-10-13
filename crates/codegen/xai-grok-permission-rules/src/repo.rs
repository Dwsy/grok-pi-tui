//! Shared git-repo dir-chain primitive.
//!
//! One `git2` discovery and one walk from cwd up to the root.
//! The folder-trust gate reuses it across the many repo-local config marker checks it runs back-to-back.
//! Lives in its own module because it is a generic repo-walk primitive.
//! `xai-grok-agent` re-exports it, and `xai-grok-workspace` consumes it through that path.

use std::path::{Path, PathBuf};

/// Git worktree root for `cwd` plus the cwd-to-root chain, from one `git2` discovery and one walk.
/// Shared so the folder-trust gate and loaders cannot drift, and so startup does not repeat the walk.
/// Outside a git repo `git_root` is `None` and `dirs` is just `[cwd]`.
#[derive(Debug, Clone)]
pub struct RepoDirChain {
    /// Git worktree root (`workdir`), or `None` when `cwd` is not inside a repo.
    pub git_root: Option<PathBuf>,
    /// `cwd` up to and including `git_root`, cwd-first (`[cwd]` with no repo).
    pub dirs: Vec<PathBuf>,
}

impl RepoDirChain {
    /// Resolve the chain for `cwd`: ONE `git2` discovery and ONE upward walk.
    pub fn resolve(cwd: &Path) -> Self {
        Self::resolve_under_home(cwd, xai_dirs::home_dir().as_deref())
    }

    pub fn resolve_under_home(cwd: &Path, home: Option<&Path>) -> Self {
        let git_root = git2::Repository::discover(cwd)
            .ok()
            .and_then(|repo| repo.workdir().map(|p| p.to_path_buf()))
            // Home-is-a-git-repo: a walk up to $HOME must not treat the whole home subtree as one repo.
            // Otherwise home-level `.grok`/plugins would look repo-local. Drop it so cwd is probed as no-repo.
            // Home is compared canonically to match the symlink handling below.
            .filter(|root| {
                home.is_none_or(|home| canonical_or_raw(root) != canonical_or_raw(home))
            });

        let mut dirs = Vec::new();
        if let Some(ref root) = git_root {
            // Canonicalize only for the stop test so a symlinked cwd still halts at the worktree root.
            // Pushed dirs keep their original spelling. Do not reduce this to `starts_with`: a mid-chain absolute symlink would walk past the root.
            let root_canonical = dunce::canonicalize(root).unwrap_or_else(|_| root.clone());
            let mut current = Some(cwd.to_path_buf());
            while let Some(dir) = current {
                let dir_canonical = dunce::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
                let parent = dir.parent().map(|p| p.to_path_buf());
                dirs.push(dir);
                if dir_canonical == root_canonical {
                    break;
                }
                current = parent;
            }
        } else {
            dirs.push(cwd.to_path_buf());
        }

        Self { git_root, dirs }
    }
}

pub fn canonical_or_raw(path: &Path) -> PathBuf {
    dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}
