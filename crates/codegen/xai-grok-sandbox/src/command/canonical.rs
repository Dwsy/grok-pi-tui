//! One spelling for a path wherever the floor is consulted: the grant
//! boundary (`with_grant`, the store's load, the card reply), the policy's own tables and the
//! decoder's protected check all call [`canonical_path`], so `..`, the `/private` firmlinks, a
//! symlinked prefix, and — on APFS — the on-disk case and Unicode form can never make one path
//! look like two. And one comparison: every containment or equality test in `command::` goes
//! through [`is_within`], [`is_same_path`] or [`PathGlob`], which fold both sides as the host's
//! volumes compare names ([`VolumeRule`]), for a path that does not exist yet as much as for one
//! that does.

use std::borrow::Cow;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

use unicode_normalization::UnicodeNormalization as _;

/// The canonical spelling of `path`: `.`/`..` folded lexically, the longest existing prefix
/// resolved through the filesystem (symlinks, firmlinks, APFS case and normalisation) with the
/// rest appended as written, the top-level `/private` alias applied to what does not exist yet,
/// and on macOS the text in NFC. A relative path is folded and returned relative.
pub fn canonical_path(path: &Path) -> PathBuf {
    let folded = fold_dots(path);
    if !folded.is_absolute() {
        return folded;
    }
    let resolved = resolve_existing_prefix(&folded);
    nfc(normalize_top_level_alias(&resolved))
}

/// Lexical `.`/`..` folding, no filesystem access. `..` above the root stays at the root; a
/// relative path keeps its leading `..` components.
pub fn fold_dots(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !matches!(
                    out.components().next_back(),
                    None | Some(Component::RootDir | Component::ParentDir)
                ) {
                    out.pop();
                } else if !out.has_root() {
                    out.push(component);
                }
            }
            Component::RootDir | Component::Prefix(_) | Component::Normal(_) => {
                out.push(component);
            }
        }
    }
    out
}

/// How the host's volumes compare names, and so how [`is_within`] and its siblings compare paths.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum VolumeRule {
    /// Byte for byte, as Linux filesystems compare.
    Exact,
    /// The APFS defaults: case- and normalisation-insensitive, and `/tmp`, `/var`, `/etc` the
    /// same trees as `/private/{tmp,var,etc}`.
    Apfs,
}

impl VolumeRule {
    const HOST: VolumeRule = if cfg!(target_os = "macos") {
        VolumeRule::Apfs
    } else {
        VolumeRule::Exact
    };

    fn current() -> VolumeRule {
        #[cfg(test)]
        if let Some(rule) = TEST_VOLUME_RULE.get() {
            return rule;
        }
        VolumeRule::HOST
    }
}

#[cfg(test)]
thread_local! {
    static TEST_VOLUME_RULE: std::cell::Cell<Option<VolumeRule>> =
        const { std::cell::Cell::new(None) };
}

#[cfg(test)]
struct RestoreVolumeRule(Option<VolumeRule>);

#[cfg(test)]
impl Drop for RestoreVolumeRule {
    fn drop(&mut self) {
        TEST_VOLUME_RULE.set(self.0);
    }
}

/// Runs `f` with this thread's comparisons folding as `rule` does, so one host checks both
/// hosts' answers. Filesystem spellings ([`canonical_path`]) keep the host's rule.
#[cfg(test)]
pub(crate) fn with_volume_rule<T>(rule: VolumeRule, f: impl FnOnce() -> T) -> T {
    let _restore = RestoreVolumeRule(TEST_VOLUME_RULE.replace(Some(rule)));
    f()
}

/// Whether `path` is `root` or lies beneath it, component-wise, in [`comparison_key`]'s
/// spelling: on macOS `.GIT/hooks` meets a floor entry `.git/hooks` and `/tmp/x` meets a root
/// `/private/tmp`, whether or not either exists yet.
pub fn is_within(path: &Path, root: &Path) -> bool {
    comparison_key(path).starts_with(comparison_key(root))
}

/// Whether `a` and `b` name the same path, folded as the host's volumes compare names (on APFS,
/// case and Unicode form). Lexical only, like every comparison here.
pub fn is_same_path(a: &Path, b: &Path) -> bool {
    comparison_key(a) == comparison_key(b)
}

/// Whether `paths` holds `path`, by [`is_same_path`].
pub(crate) fn contains_path(paths: &[PathBuf], path: &Path) -> bool {
    paths.iter().any(|candidate| is_same_path(candidate, path))
}

/// Drops every path [`is_same_path`] to an earlier one, keeping the first spelling and the order.
pub(crate) fn dedup_paths(paths: &mut Vec<PathBuf>) {
    let mut kept: Vec<PathBuf> = Vec::with_capacity(paths.len());
    for path in paths.drain(..) {
        if !contains_path(&kept, &path) {
            kept.push(path);
        }
    }
    *paths = kept;
}

/// What lies below `root` in `path`, in [`comparison_key`]'s spelling; `None` when `path` is not
/// within `root`.
pub(crate) fn strip_within(path: &Path, root: &Path) -> Option<PathBuf> {
    comparison_key(path)
        .strip_prefix(comparison_key(root))
        .ok()
        .map(Path::to_path_buf)
}

/// The workspace root as the folder was served: the spelling it was opened by and the real path
/// that spelling resolved to then. The daemon pins it once per serve, and every policy input
/// derived from the root carries the pin, never the spelling alone.
///
/// Invariants:
///
/// 1. **Resolved once.** [`ServedRoot::pin`] resolves the spelling. No rule a policy anchors at
///    the workspace resolves it again: the write root, the read root, the profile's entries
///    spelled under the root, the relative deny anchor and the floor's workspace entries sit at
///    the pinned real path, so a link retargeted after the check moves none of them.
/// 2. **The spelling is identity only.** It keys the folder's session directory, as the hub
///    names it, and nothing the kernel enforces.
/// 3. **Checked before every build.** [`ServedRoot::check`] fails once the spelling resolves
///    anywhere but the pinned real path (a link retargeted or removed, the real path replaced by
///    a link), and `SandboxPolicy::build` checks first. Every command the sandbox wraps is refused
///    with [`RootMoved`]'s one reason.
/// 4. **Refused until served again.** A refusal lasts as long as the pin: a link put back does
///    not lift it; only a new pin, the folder served again, does.
/// 5. **An unmoved link is no refusal.** A root opened through a link that has not moved builds
///    the policy its real path gets.
#[derive(Debug)]
pub struct ServedRoot {
    spelled: PathBuf,
    real: PathBuf,
    /// Where the spelling resolved when a check first found it moved: set once, never cleared.
    moved_to: OnceLock<PathBuf>,
}

/// A served root's spelling no longer resolves to the real path it was pinned at.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("the folder's real path changed from {pinned} to {now}; re-open the folder")]
pub struct RootMoved {
    pub pinned: PathBuf,
    pub now: PathBuf,
}

impl ServedRoot {
    /// Pins `spelled` at the real path it resolves to now ([`canonical_path`]).
    pub fn pin(spelled: impl Into<PathBuf>) -> ServedRoot {
        let spelled = spelled.into();
        let real = canonical_path(&spelled);
        ServedRoot {
            spelled,
            real,
            moved_to: OnceLock::new(),
        }
    }

    /// The root as the folder was opened: its identity and its session directory's key.
    pub fn spelled(&self) -> &Path {
        &self.spelled
    }

    /// The real path the spelling resolved to when pinned.
    pub fn real(&self) -> &Path {
        &self.real
    }

    /// # Errors
    /// [`RootMoved`] once the spelling resolves anywhere but [`ServedRoot::real`], and from then
    /// on for this pin, wherever the spelling resolves later.
    pub fn check(&self) -> Result<(), RootMoved> {
        let now = match self.moved_to.get() {
            Some(now) => now,
            None => {
                let now = canonical_path(&self.spelled);
                if is_same_path(&now, &self.real) {
                    return Ok(());
                }
                self.moved_to.get_or_init(|| now)
            }
        };
        Err(RootMoved {
            pinned: self.real.clone(),
            now: now.clone(),
        })
    }

    /// `path` re-anchored at the real path when it is spelled under the root, so resolving it
    /// never follows the root's spelling again; any other path as given.
    pub(crate) fn anchor<'p>(&self, path: &'p Path) -> Cow<'p, Path> {
        if self.spelled == self.real {
            return Cow::Borrowed(path);
        }
        let folded = fold_dots(path);
        let spelled = fold_dots(&self.spelled);
        let below = folded
            .ancestors()
            .find(|ancestor| is_same_path(ancestor, &spelled))
            .and_then(|ancestor| folded.strip_prefix(ancestor).ok());
        match below {
            Some(rest) if rest.as_os_str().is_empty() => Cow::Owned(self.real.clone()),
            Some(rest) => Cow::Owned(self.real.join(rest)),
            None => Cow::Borrowed(path),
        }
    }
}

/// The named components below the root in [`comparison_key`]'s spelling, the `/private` of an
/// aliased top-level tree not counted: under APFS `/private/tmp/x` is as deep as `/tmp/x`.
pub(crate) fn depth(path: &Path) -> usize {
    let key = comparison_key(path);
    let named = key
        .components()
        .filter(|component| matches!(component, Component::Normal(_)))
        .count();
    let aliased = VolumeRule::current() == VolumeRule::Apfs
        && FIRMLINKS
            .iter()
            .any(|alias| key.starts_with(Path::new(PRIVATE).join(alias)));
    named.saturating_sub(usize::from(aliased))
}

/// The top-level trees macOS firmlinks to `/private/<name>`: the one table every alias fold and
/// every rendered spelling reads.
pub(crate) const FIRMLINKS: &[&str] = &["tmp", "var", "etc"];

const PRIVATE: &str = "/private";

/// Every literal spelling a rule for `path` must name where the kernel matches text byte for byte
/// (Seatbelt): `path` as given and its canonical spelling ([`canonical_path`]), each also with its
/// `/private` firmlink prefix toggled, deduplicated byte-exact.
pub fn resolved_spellings(path: &Path) -> Vec<PathBuf> {
    let canonical = canonical_path(path);
    let mut out = firmlink_spellings(path);
    for spelling in firmlink_spellings(&canonical) {
        if !out.contains(&spelling) {
            out.push(spelling);
        }
    }
    out
}

/// `path` as given plus its `/private` firmlink toggle (`/tmp/x` and `/private/tmp/x`), never
/// resolved: for a path already canonical, where resolving again could only add a path swapped in
/// since.
pub fn firmlink_spellings(path: &Path) -> Vec<PathBuf> {
    let mut out = vec![path.to_path_buf()];
    out.extend(toggle_firmlink(path).filter(|toggled| toggled != path));
    out
}

fn toggle_firmlink(path: &Path) -> Option<PathBuf> {
    let mut components = path.components();
    if components.next() != Some(Component::RootDir) {
        return None;
    }
    let first = components.next()?.as_os_str().to_str()?;
    let (mut out, rest) = if FIRMLINKS.contains(&first) {
        (Path::new(PRIVATE).join(first), components)
    } else if first == &PRIVATE[1..] {
        let name = components.next()?.as_os_str().to_str()?;
        if !FIRMLINKS.contains(&name) {
            return None;
        }
        (Path::new("/").join(name), components)
    } else {
        return None;
    };
    out.extend(rest);
    Some(out)
}

/// A glob matched as [`is_within`] compares: case-insensitively under APFS, against
/// [`comparison_key`]'s spelling, with its own literal prefix alias-folded like a path.
#[derive(Clone)]
pub(crate) struct PathGlob(std::sync::Arc<globset::GlobMatcher>);

/// A daemon's globs are the floor's and the profile's for its folders; past this many distinct
/// ones the memo starts over rather than grow. The test suite's thousands of scratch floors stay
/// far below the test limit, so no test sees another's eviction.
const GLOB_MEMO_LIMIT: usize = if cfg!(test) { 1 << 20 } else { 1024 };

type GlobKey = (String, bool, VolumeRule);

static GLOB_MEMO: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<GlobKey, Option<PathGlob>>>,
> = std::sync::LazyLock::new(Default::default);

#[cfg(test)]
thread_local! {
    /// Globs this thread compiled (memo misses).
    pub(crate) static GLOB_COMPILES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

impl PathGlob {
    /// `None` for a glob that does not parse. `literal_separator`: `*` stops at a `/`. Compiled
    /// once per glob, rule and separator mode, then shared: every floor and deny query asks the
    /// same few globs.
    pub(crate) fn new(glob: &str, literal_separator: bool) -> Option<PathGlob> {
        let key = (glob.to_owned(), literal_separator, VolumeRule::current());
        let mut memo = GLOB_MEMO
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(compiled) = memo.get(&key) {
            return compiled.clone();
        }
        if memo.len() >= GLOB_MEMO_LIMIT {
            memo.clear();
        }
        let compiled = Self::compile(glob, literal_separator, key.2);
        memo.insert(key, compiled.clone());
        compiled
    }

    fn compile(glob: &str, literal_separator: bool, rule: VolumeRule) -> Option<PathGlob> {
        #[cfg(test)]
        GLOB_COMPILES.set(GLOB_COMPILES.get() + 1);
        let text: Cow<'_, str> = match rule {
            VolumeRule::Exact => Cow::Borrowed(glob),
            VolumeRule::Apfs => Cow::Owned(alias_fold(Path::new(glob)).to_str()?.nfc().collect()),
        };
        globset::GlobBuilder::new(&text)
            .literal_separator(literal_separator)
            .case_insensitive(rule == VolumeRule::Apfs)
            .build()
            .ok()
            .map(|glob| PathGlob(std::sync::Arc::new(glob.compile_matcher())))
    }

    pub(crate) fn is_match(&self, path: &Path) -> bool {
        self.0.is_match(comparison_key(path))
    }
}

/// The spelling every comparison here uses: the path as given under [`VolumeRule::Exact`]; under
/// APFS the top-level alias applied and every component, existing or missing, lowercased and in
/// NFC. Lexical only: a caller that needs a symlink to decide canonicalises first.
fn comparison_key(path: &Path) -> Cow<'_, Path> {
    match VolumeRule::current() {
        VolumeRule::Exact => Cow::Borrowed(path),
        VolumeRule::Apfs => Cow::Owned(alias_fold(path).components().map(fold_component).collect()),
    }
}

fn fold_component(component: Component<'_>) -> OsString {
    match (component, component.as_os_str().to_str()) {
        (Component::Normal(_), Some(name)) => {
            OsString::from(name.to_lowercase().nfc().collect::<String>())
        }
        _ => component.as_os_str().to_os_string(),
    }
}

/// The host's macOS firmlinks applied: `/tmp`, `/var`, `/etc` spelled as `/private/{tmp,var,etc}`.
pub(crate) fn normalize_top_level_alias(path: &Path) -> PathBuf {
    match VolumeRule::HOST {
        VolumeRule::Apfs => alias_fold(path).into_owned(),
        VolumeRule::Exact => path.to_path_buf(),
    }
}

fn alias_fold(path: &Path) -> Cow<'_, Path> {
    let mut components = path.components();
    match (components.next(), components.next()) {
        (Some(Component::RootDir), Some(Component::Normal(first)))
            if first.to_str().is_some_and(|first| {
                FIRMLINKS
                    .iter()
                    .any(|alias| first.eq_ignore_ascii_case(alias))
            }) =>
        {
            let mut out = PathBuf::from(PRIVATE);
            out.push(first);
            out.extend(components);
            Cow::Owned(out)
        }
        _ => Cow::Borrowed(path),
    }
}

/// The longest existing prefix through `realpath`, the remaining components appended as given.
fn resolve_existing_prefix(path: &Path) -> PathBuf {
    let mut tail: Vec<&std::ffi::OsStr> = Vec::new();
    let mut prefix = path;
    loop {
        if let Ok(resolved) = dunce::canonicalize(prefix) {
            return tail
                .iter()
                .rev()
                .fold(resolved, |acc, component| acc.join(component));
        }
        let (Some(name), Some(parent)) = (prefix.file_name(), prefix.parent()) else {
            return path.to_path_buf();
        };
        tail.push(name);
        prefix = parent;
    }
}

/// APFS is normalisation-insensitive: `é` spelled as one code point or as `e` + combining
/// accent names the same file, so both spellings fold to NFC before any comparison.
#[cfg(target_os = "macos")]
fn nfc(path: PathBuf) -> PathBuf {
    match path.to_str() {
        Some(text) if !text.is_ascii() => PathBuf::from(text.nfc().collect::<String>()),
        _ => path,
    }
}

#[cfg(not(target_os = "macos"))]
fn nfc(path: PathBuf) -> PathBuf {
    path
}

#[cfg(test)]
#[path = "canonical_tests.rs"]
mod tests;
