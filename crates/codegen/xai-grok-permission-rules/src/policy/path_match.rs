//! Path spellings that Read/Edit/Grep rules match against, and the symlink resolution behind the escalate-only re-checks.

use std::cell::OnceCell;
use std::path::{Component, Path, PathBuf};

use xai_grok_paths::normalize_lexically;

/// Cwd that relative path rules anchor to, kept as written and with symlinks resolved
pub(crate) struct RuleBase<'a> {
    /// The cwd as the caller wrote it
    pub(crate) lexical: &'a Path,
    lexical_normalized: PathBuf,
    /// The cwd with symlinks resolved on first use, `None` when that fails or changes nothing
    physical: OnceCell<Option<PathBuf>>,
}

impl<'a> RuleBase<'a> {
    pub(crate) fn new(cwd: &'a Path) -> Self {
        RuleBase {
            lexical: cwd,
            lexical_normalized: normalize_lexically(cwd),
            physical: OnceCell::new(),
        }
    }

    /// The same cwd with its physical form withheld, for a path whose `..` was collapsed as text before matching
    pub(crate) fn without_physical(&self) -> Self {
        RuleBase {
            lexical: self.lexical,
            lexical_normalized: self.lexical_normalized.clone(),
            physical: OnceCell::from(None),
        }
    }

    fn physical(&self) -> Option<&Path> {
        self.physical
            .get_or_init(|| {
                self.lexical
                    .is_absolute()
                    .then_some(self.lexical)
                    .and_then(resolve_following_symlinks)
                    .filter(|physical| *physical != self.lexical_normalized)
            })
            .as_deref()
    }
}

/// Normalized absolute form, plus cwd-relative and `./`-prefixed spellings for a path under either cwd (so `Read(./**)` matches bare `src/main.rs`).
/// A path under only the physical cwd also gets its absolute spelling under the written cwd, so logically written absolute rules match it.
/// A path written with `..` gets no physical-cwd forms here; the escalate-only target re-check supplies them.
/// Normalization never leaves `.`/`..` in the forms, so a relative spelling is produced only for paths genuinely under one of the cwds.
/// Tilde paths are matched literally only (see [`is_tilde_path`]).
pub(crate) fn path_match_forms(path: &str, base: Option<&RuleBase<'_>>) -> Vec<String> {
    let abs = absolute_normalized_path(path, base.map(|base| base.lexical));
    let mut forms = vec![path_match_string(&abs)];

    if let Some(base) = base {
        let rel = abs.strip_prefix(&base.lexical_normalized).ok().or_else(|| {
            // `..` was collapsed against the written cwd, so the result says nothing about the physical one
            if path_has_parent_dir(Path::new(path)) {
                return None;
            }
            let rel = abs.strip_prefix(base.physical()?).ok()?;
            let written: PathBuf = base
                .lexical_normalized
                .components()
                .chain(rel.components())
                .collect();
            forms.push(path_match_string(&written));
            Some(rel)
        });
        if let Some(rel) = rel {
            let rel_s = path_match_string(rel);
            if rel_s.is_empty() || rel_s == "." {
                forms.extend([".".to_owned(), "./".to_owned()]);
            } else {
                forms.push(format!("./{rel_s}"));
                forms.push(rel_s);
            }
        }
    } else if abs.is_relative() && !path_has_parent_dir(&abs) && !is_tilde_path(&abs) {
        // No session cwd: still offer `./form` so `./**` matches bare relatives.
        let lex_s = path_match_string(&abs);
        if lex_s != "." && !lex_s.is_empty() {
            forms.push(format!("./{lex_s}"));
        }
    }
    forms
}

pub(crate) fn absolute_normalized_path(path: &str, cwd: Option<&Path>) -> PathBuf {
    let raw = Path::new(path);
    if is_tilde_path(raw) {
        // Kept raw: no cwd-join and no collapse; collapsing `~/../x` to `x` would make it look workspace-relative
        return raw.to_path_buf();
    }
    let joined = match cwd {
        Some(cwd) if !raw.is_absolute() => cwd.join(raw),
        _ => raw.to_path_buf(),
    };
    normalize_lexically(&joined)
}

/// A leading `~` is expanded to home by the tools *after* this gate, so it must never be treated as cwd-relative.
/// A manufactured `./~/…` would satisfy `./**` while escaping to home; tilde paths are matched literally, as patterns treat `~`.
pub(crate) fn is_tilde_path(path: &Path) -> bool {
    matches!(
        path.components().next(),
        Some(Component::Normal(first)) if first.to_string_lossy().starts_with('~')
    )
}

/// Cwd-join without collapsing `.`/`..`, so physical resolve sees `..` after a link.
pub(crate) fn raw_absolute_tool_path(path: &str, cwd: Option<&Path>) -> Option<String> {
    let raw = Path::new(path);
    if is_tilde_path(raw) {
        return None;
    }
    let joined = match cwd {
        Some(cwd) if !raw.is_absolute() => cwd.join(raw),
        _ => raw.to_path_buf(),
    };
    joined.is_absolute().then(|| path_match_string(&joined))
}

pub(crate) fn path_match_string(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

pub(crate) fn path_has_parent_dir(path: &Path) -> bool {
    path.components().any(|c| matches!(c, Component::ParentDir))
}

/// True if any existing component of `absolute` is a symlink.
fn path_has_symlink(absolute: &str) -> bool {
    let path = Path::new(absolute);
    if !path.is_absolute() {
        return false;
    }
    let mut prefix = PathBuf::new();
    for comp in path.components() {
        prefix.push(comp);
        if std::fs::symlink_metadata(&prefix).is_ok_and(|meta| meta.file_type().is_symlink()) {
            return true;
        }
    }
    false
}

/// Canonical target, or `None` on relative input, cycles, depth limits, or fs errors.
fn resolve_symlink_target(absolute: &str) -> Option<String> {
    let path = Path::new(absolute);
    if !path.is_absolute() {
        return None;
    }
    let resolved = resolve_following_symlinks(path)?;
    Some(path_match_string(&normalize_lexically(&resolved)))
}

/// Follow every symlink, including dangling leaves and missing trailing components.
pub fn resolve_following_symlinks(path: &Path) -> Option<PathBuf> {
    fn walk(path: &Path, depth: usize) -> Option<PathBuf> {
        const MAX_SYMLINK_DEPTH: usize = 40;
        if depth > MAX_SYMLINK_DEPTH {
            return None;
        }
        // `dunce` avoids Windows `\\?\` verbatim paths (repo convention).
        if let Ok(canonical) = dunce::canonicalize(path) {
            return Some(canonical);
        }
        // Parent-first so a dangling or not-yet-created leaf still follows links.
        let parent = path.parent()?;
        let file_name = path.file_name()?;
        let resolved_parent = walk(parent, depth + 1)?;
        let candidate = resolved_parent.join(file_name);
        // NotFound is a new path; any other metadata error fails closed.
        let metadata = match std::fs::symlink_metadata(&candidate) {
            Ok(metadata) => Some(metadata),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return None,
        };
        if metadata.is_some_and(|metadata| metadata.file_type().is_symlink()) {
            // Unreadable link target fails closed rather than treating the link path as real.
            let target = std::fs::read_link(&candidate).ok()?;
            let target = if target.is_absolute() {
                target
            } else {
                resolved_parent.join(target)
            };
            return walk(&target, depth + 1);
        }
        Some(candidate)
    }
    walk(path, 0)
}

/// The result of following symlinks on an absolute path, used by the escalate-only re-checks.
#[derive(Debug)]
pub(crate) enum SymlinkFollow {
    None,
    Target(String),
    Unresolvable,
}

pub(crate) fn follow_absolute_symlink(raw_absolute: &str, lexical_absolute: &str) -> SymlinkFollow {
    match resolve_symlink_target(raw_absolute) {
        Some(resolved) if resolved != lexical_absolute => SymlinkFollow::Target(resolved),
        Some(_) => SymlinkFollow::None,
        None if path_has_symlink(raw_absolute) => SymlinkFollow::Unresolvable,
        None => SymlinkFollow::None,
    }
}

#[cfg(all(test, unix))]
#[path = "path_match_tests.rs"]
mod tests;
