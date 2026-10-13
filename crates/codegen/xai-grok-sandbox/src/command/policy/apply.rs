//! Widening a built [`SandboxPolicy`] by one grant, and the questions the decoder and observe
//! mode ask of a policy. Kept apart from the defaults so the floor rules read in one place: a
//! grant never enters `protected`, never drops a read deny (a profile `deny` holds like the
//! floor), and never changes the kernel network policy. Every path that reaches the floor here
//! goes through [`canonical_path`] first.

use std::path::{Path, PathBuf};

use crate::command::canonical::{
    canonical_path, contains_path, firmlink_spellings, fold_dots, is_same_path, is_within,
    normalize_top_level_alias,
};
use crate::command::grants::{Grant, GrantDecision, GrantSubject};
use crate::command::policy::{DenyEntry, NetworkPolicy, PolicyError, ReadPolicy, SandboxPolicy};
use crate::command::protected::{self, Protected};
use crate::command::violation::Blocked;

impl SandboxPolicy {
    /// Widen by one grant (the replay path). Refuses a subject inside `protected` or at or beneath
    /// a glob entry's literal prefix ([`protected::is_ungrantable`]), and a path subject at or
    /// beneath a read deny: a profile `deny` is final, like the floor, and no grant drops one, so
    /// a grant around a denied path leaves it denied. A deny row
    /// never widens: it leaves the policy as it is. A write root that does not exist yet and that
    /// a floor entry lies under is left out, so a tree moved in as it cannot carry that entry.
    ///
    /// A network grant is applied by the egress proxy's decider, not here: the rendered
    /// [`NetworkPolicy`] stays `Off` or `Proxy` whatever was granted, so the
    /// unix-socket and loopback floor never opens.
    ///
    /// # Errors
    /// [`PolicyError::Protected`] when the subject lies in the floor;
    /// [`PolicyError::ProfileDenied`] when a read deny covers it; [`PolicyError::NotAbsolute`]
    /// for a relative root; [`PolicyError::SymlinkedRoot`] when a component below the top level
    /// of a granted root is a symlink (the grant could otherwise point outside the tree the user
    /// saw on the card).
    pub fn with_grant(mut self, grant: &Grant) -> Result<SandboxPolicy, PolicyError> {
        self.apply_grant(grant)?;
        Ok(self)
    }

    /// [`SandboxPolicy::with_grant`] in place. Every arm validates before it changes anything,
    /// so a refused grant leaves the policy as it was.
    ///
    /// # Errors
    /// As [`SandboxPolicy::with_grant`].
    pub fn apply_grant(&mut self, grant: &Grant) -> Result<(), PolicyError> {
        self.apply_grant_under(grant, &[])
    }

    /// [`SandboxPolicy::apply_grant`] under the live deny rows' subjects, as
    /// [`SandboxPolicy::build`] applies each row: where a deny of the other write kind overlaps
    /// an allow, the deny wins. No backend can take a subtree out of one allowed root, so the
    /// overlap closes the smallest allowed unit holding it: a build-cache tree a path deny lies
    /// in or around, a write root into or around a tree under the family's deny.
    ///
    /// # Errors
    /// As [`SandboxPolicy::with_grant`].
    pub(super) fn apply_grant_under(
        &mut self,
        grant: &Grant,
        denies: &[&GrantSubject],
    ) -> Result<(), PolicyError> {
        if grant.decision == GrantDecision::Deny {
            return Ok(());
        }
        match &grant.subject {
            GrantSubject::FsWriteRoot { root } => {
                let root = self.validated_root(root)?;
                let family_denied = denies.contains(&&GrantSubject::BuildCaches)
                    && self
                        .build_cache_trees
                        .iter()
                        .any(|tree| overlaps(&root, tree));
                if family_denied {
                    tracing::debug!(root = %root.display(), "granted write root overlaps a build-cache tree the family's deny row closes; left out");
                } else if std::fs::symlink_metadata(&root).is_err()
                    && self.protected.iter().any(|entry| entry.reaches_into(&root))
                {
                    tracing::debug!(root = %root.display(), "granted write root does not exist yet and holds a floor entry; left out");
                } else if !contains_path(&self.write_roots, &root) {
                    self.write_roots.push(root);
                }
            }
            GrantSubject::FsRead { root } => {
                let root = self.validated_root(root)?;
                match &mut self.read {
                    ReadPolicy::AllExcept { .. } => {}
                    ReadPolicy::Roots { roots, .. } => {
                        if !contains_path(roots, &root) {
                            roots.push(root);
                        }
                    }
                }
            }
            GrantSubject::NetHost { host, .. } => {
                if matches!(self.network, NetworkPolicy::Off) {
                    tracing::warn!(host = %host, "network grant without an egress proxy has no enforcement point; network stays off");
                }
                // Under `Proxy` the decider consumes the live host grants directly
            }
            GrantSubject::BuildCaches => {
                // The trees are the daemon's curated table, already canonical and outside the
                // floor; the floor is still rendered after them
                for tree in &self.build_cache_trees {
                    let denied = denies.iter().any(|deny| {
                        matches!(deny, GrantSubject::FsWriteRoot { root } if overlaps(root, tree))
                    });
                    if denied {
                        tracing::debug!(tree = %tree.display(), "build-cache tree overlaps a path deny row; left out");
                    } else if !contains_path(&self.write_roots, tree) {
                        self.write_roots.push(tree.clone());
                    }
                }
            }
        }
        Ok(())
    }

    /// Whether this policy already permits what was blocked; used by observe-mode classification
    /// and by the decoder to drop noise. A connection is "allowed" by the policy only under
    /// `Full`; under `Proxy` the decider, not the policy, knows. A coarse denial quotes the
    /// command's own spelling (`/var/folders/…`, a `..`, a symlinked prefix) while roots and the
    /// floor are stored canonically, so the path is canonicalised before it is compared; the
    /// floor is consulted first and in both spellings, so no spelling makes a protected path
    /// look writable. A read asks the read denies first in every mode, then the mode's allow.
    pub fn would_allow(&self, blocked: &Blocked) -> bool {
        match blocked {
            Blocked::FsWrite { path } => {
                !self.is_protected(path) && !self.is_read_floor(path) && self.allows_write(path)
            }
            Blocked::FsRead { path } => {
                if self.read_denies_covering(path).next().is_some() {
                    return false;
                }
                match &self.read {
                    ReadPolicy::AllExcept { .. } => true,
                    ReadPolicy::Roots { roots, .. } => {
                        // Roots as the renderer spells them, never resolved: Seatbelt meets a
                        // symlinked root's target under the target's path, which no root allows
                        let canonical = canonical_path(path);
                        roots
                            .iter()
                            .flat_map(|root| firmlink_spellings(root))
                            .any(|root| is_within(&canonical, &root))
                    }
                }
            }
            // A connection is never the kernel policy's to allow: the proxy's decider answers it
            Blocked::Net { .. } | Blocked::Capability { .. } | Blocked::Unknown { .. } => false,
        }
    }

    /// Whether a read deny covers `path`: a secret store, the grok home's auth material, another
    /// workspace's sessions, a profile `deny`. No grant drops one, so a card for it could only
    /// fail again and the decoder offers none.
    pub fn is_read_floor(&self, path: &Path) -> bool {
        self.read_denies_covering(path).next().is_some()
    }

    /// Whether a read deny lies strictly inside `root` (its path, tree or glob root): a card
    /// never offers such a folder, since the grant would leave the denied part out of what the
    /// user was shown as allowed.
    pub fn holds_read_deny(&self, root: &Path) -> bool {
        let spellings = [fold_dots(root), canonical_path(root)];
        self.read.deny().iter().any(|entry| {
            let anchor = match entry {
                DenyEntry::Path(path) => path,
                DenyEntry::Glob { root, .. } => root,
                DenyEntry::TreeExcept { tree, .. } => tree,
            };
            spellings
                .iter()
                .any(|root| !is_same_path(anchor, root) && is_within(anchor, root))
        })
    }

    /// [`validated_grant_root`], then refused when a read deny covers the root.
    fn validated_root(&self, root: &Path) -> Result<PathBuf, PolicyError> {
        let root = validated_grant_root(root, &self.protected)?;
        if self.is_read_floor(&root) {
            return Err(PolicyError::ProfileDenied { path: root });
        }
        Ok(root)
    }

    /// The read denies that cover `path`, in its own or its canonical spelling.
    fn read_denies_covering<'a>(&'a self, path: &'a Path) -> impl Iterator<Item = &'a DenyEntry> {
        let canonical = canonical_path(path);
        self.read
            .deny()
            .iter()
            .filter(move |entry| entry.covers(path) || entry.covers(&canonical))
    }

    /// Whether `path` lies in the floor, in its own or its canonical spelling, or is a node a
    /// rename could plant the floor through: the `.git` node of a write root
    /// ([`SandboxPolicy::git_dir_nodes`]), a glob's literal prefix inside one
    /// ([`SandboxPolicy::glob_prefix_nodes`]) or a missing ancestor
    /// ([`SandboxPolicy::missing_ancestor_nodes`]).
    pub fn is_protected(&self, path: &Path) -> bool {
        if protected::is_protected(path, &self.protected) {
            return true;
        }
        let spellings = [fold_dots(path), canonical_path(path)];
        self.git_dir_nodes()
            .into_iter()
            .chain(self.glob_prefix_nodes())
            .chain(self.missing_ancestor_nodes())
            .any(|node| {
                spellings
                    .iter()
                    .any(|spelling| is_same_path(spelling, &node))
            })
    }

    /// Each ancestor of a floor entry ([`Protected::anchor`]) that a command could create by
    /// moving a staged tree in: inside a write root and not a directory yet. Under a `$HOME` grant
    /// a missing `~/.config/git` is one; moved in, it would bring the `config` the floor names
    /// with it. A backend renders each as a literal deny, like the `.git` node.
    pub fn missing_ancestor_nodes(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = Vec::new();
        for entry in &self.protected {
            for ancestor in self.renameable_ancestors(&entry.anchor()) {
                if !protected::is_real_directory(&ancestor) && !contains_path(&out, &ancestor) {
                    out.push(ancestor);
                }
            }
        }
        out
    }

    /// Whether no card may offer `root` and no grant may name it: [`SandboxPolicy::is_protected`],
    /// or [`protected::is_ungrantable`] (at or beneath a glob's literal prefix).
    pub fn is_ungrantable(&self, root: &Path) -> bool {
        self.is_protected(root) || protected::is_ungrantable(root, &self.protected)
    }

    /// Each glob entry's literal prefix strictly inside a write root (`<ws>/.git/modules`), as a
    /// node: its matches are denied beneath it, so it can be neither moved away nor created by
    /// moving a staged tree in. A backend renders each as a literal deny, like the `.git` node.
    pub fn glob_prefix_nodes(&self) -> Vec<PathBuf> {
        self.protected
            .iter()
            .filter(|entry| matches!(entry, Protected::Glob { .. }))
            .map(Protected::anchor)
            .filter(|prefix| {
                self.write_roots
                    .iter()
                    .any(|root| !is_same_path(prefix, root) && is_within(prefix, root))
            })
            .collect()
    }

    /// `path`'s proper ancestors a command could rename or create: strictly inside a write root
    /// and not a root themselves (a backend pins each root on its own).
    pub fn renameable_ancestors(&self, path: &Path) -> Vec<PathBuf> {
        path.ancestors()
            .skip(1)
            .filter(|ancestor| {
                !contains_path(&self.write_roots, ancestor)
                    && self
                        .write_roots
                        .iter()
                        .any(|root| is_within(ancestor, root))
            })
            .map(Path::to_path_buf)
            .collect()
    }

    /// `<root>/.git` for every write root: the node itself, never what lies beneath it. The floor
    /// names the git directory's entries, not the directory, and a rename is checked only
    /// against the node it moves — so without this a staged `hooks/pre-commit` could be moved in
    /// as `.git`, or the whole git directory moved away with its protected entries. Everything
    /// inside stays writable; a backend renders each node as a literal deny.
    pub fn git_dir_nodes(&self) -> Vec<PathBuf> {
        self.write_roots
            .iter()
            .map(|root| root.join(protected::GIT_DIR_NAME))
            .collect()
    }

    /// Whether `path` lies in one of the curated build-cache trees — the write a
    /// [`GrantSubject::BuildCaches`] grant would allow. The floor is the
    /// caller's question, asked first.
    pub fn in_build_caches(&self, path: &Path) -> bool {
        let canonical = canonical_path(path);
        self.build_cache_trees
            .iter()
            .any(|tree| is_within(&canonical, tree))
    }

    /// Whether some write root covers `path`; the floor is the caller's question.
    fn allows_write(&self, path: &Path) -> bool {
        let canonical = canonical_path(path);
        self.write_roots
            .iter()
            .any(|root| is_within(&canonical, root))
    }
}

/// Whether either tree holds the other.
fn overlaps(a: &Path, b: &Path) -> bool {
    is_within(a, b) || is_within(b, a)
}

/// A grant root must be absolute, outside the floor, and free of symlinks below its top-level
/// component: the card showed the user a path, and a symlink deeper in it could redirect the
/// grant to a tree they never saw. The symlink check runs on the `..`-folded, alias-normalised
/// spelling; what passes is then stored canonically (existing prefix resolved, NFC), so the
/// floor and the write roots compare one spelling.
fn validated_grant_root(root: &Path, protected: &[Protected]) -> Result<PathBuf, PolicyError> {
    if !root.is_absolute() {
        return Err(PolicyError::NotAbsolute {
            path: root.to_path_buf(),
        });
    }
    let folded = symlink_free_spelling(root)?;
    let root = canonical_path(&folded);
    if protected::is_ungrantable(&root, protected) {
        return Err(PolicyError::Protected { path: root });
    }
    Ok(root)
}

/// The absolute `root` `..`-folded and alias-normalised, when no component below its top-level
/// one is a symlink; the grant store asks it of the spelling a grant arrives in, before that is
/// canonicalised.
pub(crate) fn symlink_free_spelling(root: &Path) -> Result<PathBuf, PolicyError> {
    let folded = normalize_top_level_alias(&fold_dots(root));
    let mut walk = PathBuf::new();
    for (index, component) in folded.components().enumerate() {
        walk.push(component);
        // index 0 is the root dir, 1 the top-level component (alias-normalised above)
        if index <= 1 {
            continue;
        }
        // A missing component ends the walk (nothing below it exists to follow); any other
        // error refuses: a component that cannot be inspected might be a link
        match std::fs::symlink_metadata(&walk) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(PolicyError::SymlinkedRoot { path: folded });
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => {
                return Err(PolicyError::UninspectableRoot {
                    path: folded,
                    reason: error.to_string(),
                });
            }
        }
    }
    Ok(folded)
}
