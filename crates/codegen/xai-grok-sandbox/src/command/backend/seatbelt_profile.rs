//! Renders a [`SandboxPolicy`] into Seatbelt SBPL text plus `-D` parameters for
//! `/usr/bin/sandbox-exec`. Invariants:
//! - Every path reaches the profile as a `(param "NAME")` reference, never inlined, so a path with
//!   SBPL metacharacters cannot rewrite the profile.
//! - Rules are emitted in the design's order (base, read, write, network, preferences, mandatory
//!   denies) and the mandatory denies come last, so no allow can re-open them under Seatbelt's
//!   last-match evaluation.
//! - Every generated rule carries `(with message "<tag>")`; `(deny default)` is re-declared with
//!   the tag after the verbatim base so default denials attribute to the command as well.
//! - A path that cannot be expressed (non-UTF-8, control characters, relative) is an error, never
//!   a silently dropped rule.
//! - A floor entry and a read deny are pinned alike ([`Pinned`]).
//! - The enforce profile opens the network only to the loopback proxy port: a blanket
//!   `network-outbound` allow would also reach every host unix socket and loopback service.

use std::path::{Path, PathBuf};

use crate::command::backend::{BackendName, CommandTag, SandboxCommandError};
use crate::command::canonical::{
    contains_path, firmlink_spellings, is_same_path, is_within, resolved_spellings,
};
use crate::command::policy::{DenyEntry, NetworkPolicy, ReadPolicy, SandboxPolicy};
use crate::command::protected::{self, Protected};
use crate::deny;

const BASE_POLICY: &str = include_str!("sbpl/base.sbpl");
const PLATFORM_DEFAULTS_POLICY: &str = include_str!("sbpl/platform_defaults.sbpl");
const PREFERENCES_POLICY: &str = include_str!("sbpl/preferences.sbpl");

/// `F_MAKECOMPRESSED` and `F_TRANSFEREXTENTS` mutate a file through a read-only descriptor.
const FCNTL_DENY: &str = "(fcntl-command 80 110)";

/// The rendered profile: `-p` text and the `-D NAME=VALUE` pairs in the order the profile first
/// names them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Sbpl {
    pub(crate) profile: String,
    pub(crate) params: Vec<(String, String)>,
}

/// Renders the enforce profile in the fixed assembly order.
pub(crate) fn render_enforce(
    policy: &SandboxPolicy,
    tag: &CommandTag,
) -> Result<Sbpl, SandboxCommandError> {
    let floor: Vec<Pinned<'_>> = policy
        .protected
        .iter()
        .map(Pinned::try_from)
        .collect::<Result<_, _>>()?;
    // A read deny the floor already names gets its pins there; only its read rule is added
    let denies: Vec<Pinned<'_>> = policy
        .read
        .deny()
        .iter()
        .map(Pinned::from)
        .filter(|entry| !floor.contains(entry))
        .collect();
    let pinned: Vec<Pinned<'_>> = floor.iter().chain(&denies).copied().collect();

    let mut out = ProfileBuilder::new(tag)?;
    out.raw(BASE_POLICY);
    out.raw("; grok: generated rules follow; default denials carry the command tag");
    out.rule("deny default");

    match &policy.read {
        ReadPolicy::AllExcept { .. } => out.rule("allow file-read*"),
        ReadPolicy::Roots { roots, .. } => {
            out.raw(PLATFORM_DEFAULTS_POLICY);
            // As spelled plus the `/private` toggle, never resolved (as for a write root): a read
            // root swapped for a symlink after the policy was built must not read its target
            let mut forms: Vec<PathBuf> = Vec::new();
            for form in roots.iter().flat_map(|root| firmlink_spellings(root)) {
                if !forms.contains(&form) {
                    forms.push(form);
                }
            }
            for (index, root) in forms.iter().enumerate() {
                let param = out.param(format!("READABLE_ROOT_{index}"), root)?;
                out.rule(&format!(
                    "allow file-read* (require-any (literal {param}) (subpath {param}))"
                ));
            }
        }
    }

    render_write_roots(&mut out, policy, &pinned)?;
    render_network(&mut out, policy)?;
    if matches!(policy.read, ReadPolicy::AllExcept { .. }) {
        out.raw(PREFERENCES_POLICY);
    }

    out.raw("; grok: mandatory denies; nothing below may be re-opened by a grant");
    render_mandatory(&mut out, policy, &floor, &denies)?;
    out.rule("deny mach-lookup (xpc-service-name-prefix \"\")");
    out.rule(&format!("deny system-fcntl {FCNTL_DENY}"));
    Ok(out.finish())
}

fn render_write_roots(
    out: &mut ProfileBuilder,
    policy: &SandboxPolicy,
    pinned: &[Pinned<'_>],
) -> Result<(), SandboxCommandError> {
    let mut root_index = 0usize;
    let mut root_params: Vec<String> = Vec::new();
    for root in &policy.write_roots {
        let root_forms = firmlink_spellings(root);
        let Some((first, aliases)) = root_forms.split_first() else {
            continue;
        };
        // The `-D` list reads as the profile does: the root's param, then the carve-outs shared
        // by every spelling of the root (registered once, under this first index), then the
        // aliases' params
        let first_index = root_index;
        let mut params = vec![out.param(format!("WRITABLE_ROOT_{first_index}"), first)?];
        // Pinned entries reaching into the root are carved out of the allow and denied last, so the
        // allow never covers them; the filters name only `(param …)`s, so one set serves every
        // spelling
        let mut carve_outs = String::new();
        for (index, entry) in pinned
            .iter()
            .filter(|entry| root_forms.iter().any(|form| entry.reaches_into(form)))
            .enumerate()
        {
            let prefix = format!("WRITABLE_ROOT_{first_index}_EXCLUDED_{index}");
            for filter in pinned_filters(out, entry, &prefix)? {
                carve_outs.push_str(&format!(" (require-not {})", filter.file()));
            }
        }
        for alias in aliases {
            root_index += 1;
            params.push(out.param(format!("WRITABLE_ROOT_{root_index}"), alias)?);
        }
        root_index += 1;
        for root_param in params {
            // `literal` as well as `subpath`, as for a floor entry: `subpath` alone does not
            // match the creation of the root itself, and a verified build cache (`~/.cargo/
            // registry/cache`, cargo's `.package-cache` lock file) may not exist yet
            out.rule(&format!(
                "allow file-write* (require-all (require-any (literal {root_param}) (subpath {root_param})){carve_outs})"
            ));
            root_params.push(root_param);
        }
    }
    // A root directory is an authority boundary reused to build the next policy: never renamed or
    // unlinked from inside. Pinned after every allow, so a parent root's allow cannot reopen it
    for root_param in root_params {
        out.rule(&format!(
            "deny file-write-unlink (require-all (literal {root_param}) (vnode-type DIRECTORY))"
        ));
    }
    Ok(())
}

/// One entry of the mandatory block: a floor entry or a read deny, in the shape the renderer
/// selects it by. Both get the same pins — `mv .env aside` followed by a read of `aside` would
/// pass a read deny that only a `file-read*` rule held — so a floor glob is carried as a pattern
/// below `/`, the shape a profile deny already has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pinned<'a> {
    /// The path and everything beneath it.
    Path(&'a Path),
    /// The gitignore-style `tail` below the literal directory `root`: every match and everything
    /// beneath it, whether or not the match exists yet.
    Glob { root: &'a Path, tail: &'a str },
    /// Everything under `tree` except `except` and what lies beneath it.
    TreeExcept { tree: &'a Path, except: &'a Path },
    /// The symlink itself ([`Protected::Node`]), never what it points to.
    Node(&'a Path),
}

impl<'a> TryFrom<&'a Protected> for Pinned<'a> {
    type Error = SandboxCommandError;

    /// A floor glob is absolute by construction; a relative one has no base here and is refused
    /// rather than anchored at a guess.
    fn try_from(entry: &'a Protected) -> Result<Pinned<'a>, SandboxCommandError> {
        Ok(match entry {
            Protected::Path { path } => Pinned::Path(path),
            Protected::Glob { glob } => Pinned::Glob {
                root: Path::new("/"),
                tail: glob
                    .strip_prefix('/')
                    .ok_or_else(|| unrenderable(format!("deny glob must be absolute: {glob}")))?,
            },
            Protected::TreeExcept { tree, except } => Pinned::TreeExcept { tree, except },
            Protected::Node { path } => Pinned::Node(path),
        })
    }
}

impl<'a> From<&'a DenyEntry> for Pinned<'a> {
    fn from(entry: &'a DenyEntry) -> Pinned<'a> {
        match entry {
            DenyEntry::Path(path) => Pinned::Path(path),
            DenyEntry::Glob { root, tail } => Pinned::Glob { root, tail },
            DenyEntry::TreeExcept { tree, except } => Pinned::TreeExcept { tree, except },
        }
    }
}

impl Pinned<'_> {
    /// The node a rename could swap out from under the entry: the path, the tree, or a glob's
    /// literal prefix (its root plus the tail's components before the first pattern), as
    /// [`Protected::anchor`] gives it for a floor entry.
    fn anchor(&self) -> PathBuf {
        match self {
            Pinned::Path(path) | Pinned::Node(path) => path.to_path_buf(),
            Pinned::TreeExcept { tree, .. } => tree.to_path_buf(),
            Pinned::Glob { root, tail } => {
                let prefix: PathBuf = Path::new(tail)
                    .components()
                    .take_while(|component| {
                        !component.as_os_str().to_str().is_some_and(deny::is_glob)
                    })
                    .collect();
                if prefix.as_os_str().is_empty() {
                    root.to_path_buf()
                } else {
                    root.join(prefix)
                }
            }
        }
    }

    /// Whether the entry can name something inside `root`, as [`Protected::reaches_into`] asks
    /// for a floor entry — the path or the tree lies at or beneath it; for a glob, its literal
    /// prefix does, or `root` lies beneath the prefix, where a match can appear.
    fn reaches_into(&self, root: &Path) -> bool {
        match self {
            Pinned::Path(path) | Pinned::Node(path) => is_within(path, root),
            Pinned::TreeExcept { tree, .. } => is_within(tree, root),
            Pinned::Glob { .. } => {
                let prefix = self.anchor();
                is_within(&prefix, root) || is_within(root, &prefix)
            }
        }
    }
}

/// One SBPL path filter, kept as a tree so the same selection renders for a file operation and
/// for the unix socket the path may name: `(remote unix-socket …)` goes around every leaf, and a
/// `require-*` combinator stays at the rule level, where Seatbelt accepts it.
#[derive(Clone, Debug)]
enum PathFilter {
    /// A complete `(literal …)`, `(subpath …)` or `(regex #"…")` term.
    Leaf(String),
    All(Vec<PathFilter>),
    Any(Vec<PathFilter>),
    Not(Box<PathFilter>),
}

impl PathFilter {
    /// The filter as a `file-*` rule uses it.
    fn file(&self) -> String {
        self.render(&|leaf| leaf.to_owned())
    }

    /// The filter selecting the same paths as a unix socket's, for a `network-outbound` rule.
    fn unix_socket(&self) -> String {
        self.render(&|leaf| format!("(remote unix-socket {leaf})"))
    }

    fn render(&self, leaf: &dyn Fn(&str) -> String) -> String {
        let join = |parts: &[PathFilter]| {
            parts
                .iter()
                .map(|part| part.render(leaf))
                .collect::<Vec<_>>()
                .join(" ")
        };
        match self {
            PathFilter::Leaf(term) => leaf(term),
            PathFilter::All(parts) => format!("(require-all {})", join(parts)),
            PathFilter::Any(parts) => format!("(require-any {})", join(parts)),
            PathFilter::Not(part) => format!("(require-not {})", part.render(leaf)),
        }
    }
}

/// `filters` as one rule's filter list, where several filters are alternatives.
fn file_filters(filters: &[PathFilter]) -> String {
    filters
        .iter()
        .map(PathFilter::file)
        .collect::<Vec<_>>()
        .join(" ")
}

/// The SBPL filters that select one pinned entry, each a complete term usable as a rule filter or
/// inside `require-not`. `literal` as well as `subpath` for a path: `subpath` alone leaves
/// first-time creation of the protected directory itself (`mkdir .grok`) open. A glob is one
/// anchored regex per alias form, for the match and for what lies beneath it. A tree-except is
/// `(require-all (require-any (literal T) (subpath T)) (require-not (subpath E)) …)`.
fn pinned_filters(
    out: &mut ProfileBuilder,
    entry: &Pinned<'_>,
    param_prefix: &str,
) -> Result<Vec<PathFilter>, SandboxCommandError> {
    match entry {
        Pinned::Path(path) => {
            let mut filters = Vec::new();
            for (index, form) in expand_aliases(&[*path]).iter().enumerate() {
                let param = out.param(format!("{param_prefix}_{index}"), form)?;
                filters.push(PathFilter::Leaf(format!("(literal {param})")));
                filters.push(PathFilter::Leaf(format!("(subpath {param})")));
            }
            Ok(filters)
        }
        // Only the parent's spellings: resolving the node itself would deny its target's tree
        Pinned::Node(node) => {
            let (Some(parent), Some(name)) = (node.parent(), node.file_name()) else {
                return Err(unrenderable(format!(
                    "node names no entry: {}",
                    node.display()
                )));
            };
            let mut filters = Vec::new();
            for (index, form) in expand_aliases(&[parent]).iter().enumerate() {
                let param = out.param(format!("{param_prefix}_{index}"), &form.join(name))?;
                filters.push(PathFilter::Leaf(format!("(literal {param})")));
            }
            Ok(filters)
        }
        Pinned::Glob { root, tail } => {
            let mut filters = glob_filters(root, tail)?;
            filters.extend(glob_filters(root, &format!("{tail}/**"))?);
            Ok(filters)
        }
        Pinned::TreeExcept { tree, except } => {
            let mut filters = Vec::new();
            let except_forms = firmlink_spellings(except);
            for (index, form) in expand_aliases(&[*tree]).iter().enumerate() {
                let tree_param = out.param(format!("{param_prefix}_TREE_{index}"), form)?;
                let mut parts = vec![PathFilter::Any(vec![
                    PathFilter::Leaf(format!("(literal {tree_param})")),
                    PathFilter::Leaf(format!("(subpath {tree_param})")),
                ])];
                for (except_index, except_form) in except_forms.iter().enumerate() {
                    let except_param = out.param(
                        format!("{param_prefix}_TREE_{index}_EXCEPT_{except_index}"),
                        except_form,
                    )?;
                    parts.push(PathFilter::Not(Box::new(PathFilter::Leaf(format!(
                        "(subpath {except_param})"
                    )))));
                }
                filters.push(PathFilter::All(parts));
            }
            Ok(filters)
        }
    }
}

fn render_network(
    out: &mut ProfileBuilder,
    policy: &SandboxPolicy,
) -> Result<(), SandboxCommandError> {
    match &policy.network {
        NetworkPolicy::Off => {}
        NetworkPolicy::Proxy { port } => {
            out.rule(&format!(
                "allow network-outbound (remote ip \"localhost:{port}\")"
            ));
        }
    }
    Ok(())
}

/// The mandatory block, in order: the floor and the read denies write-, link- and socket-denied,
/// the rename-checked nodes ([`render_nodes`]), the ancestor pins ([`pinned_directories`]), then
/// the read denies themselves, after every read allow, so Seatbelt's last match refuses them.
fn render_mandatory(
    out: &mut ProfileBuilder,
    policy: &SandboxPolicy,
    floor: &[Pinned<'_>],
    denies: &[Pinned<'_>],
) -> Result<(), SandboxCommandError> {
    for (index, entry) in floor.iter().enumerate() {
        let filters = pinned_filters(out, entry, &format!("PROTECTED_{index}"))?;
        deny_writes_links_and_sockets(out, &filters);
    }
    let mut read_filters: Vec<Vec<PathFilter>> = Vec::with_capacity(policy.read.deny().len());
    for (index, entry) in policy.read.deny().iter().enumerate() {
        let pinned = Pinned::from(entry);
        let filters = pinned_filters(out, &pinned, &format!("DENY_{index}"))?;
        if denies.contains(&pinned) {
            deny_writes_links_and_sockets(out, &filters);
        }
        read_filters.push(filters);
    }
    render_nodes(out, policy, denies)?;
    let pinned: Vec<Pinned<'_>> = floor.iter().chain(denies).copied().collect();
    render_ancestor_pins(out, policy, &pinned)?;
    for filters in &read_filters {
        out.rule(&format!("deny file-read* {}", file_filters(filters)));
    }
    Ok(())
}

fn deny_writes_links_and_sockets(out: &mut ProfileBuilder, filters: &[PathFilter]) {
    out.rule(&format!(
        "deny file-write* file-link {}",
        file_filters(filters)
    ));
    let sockets = filters
        .iter()
        .map(PathFilter::unix_socket)
        .collect::<Vec<_>>()
        .join(" ");
    out.rule(&format!("deny network-outbound {sockets}"));
}

/// The nodes a rename is checked against, each a `literal` deny and never `subpath`, so the
/// rename is refused and every operation inside works: the write roots' `.git` nodes, the floor's
/// glob prefixes and missing ancestors as the policy derives them, and the read denies' own
/// ([`deny_nodes`]). One rule body; the families keep their names so a profile reads why a node
/// is there, and a node two families derive is rendered once, under the first.
fn render_nodes(
    out: &mut ProfileBuilder,
    policy: &SandboxPolicy,
    denies: &[Pinned<'_>],
) -> Result<(), SandboxCommandError> {
    let families: [(&str, Vec<PathBuf>); 4] = [
        ("GIT_DIR_NODE", policy.git_dir_nodes()),
        ("GLOB_PREFIX_NODE", policy.glob_prefix_nodes()),
        ("ANCESTOR_NODE", policy.missing_ancestor_nodes()),
        ("DENY_NODE", deny_nodes(denies, policy)),
    ];
    let mut rendered: Vec<PathBuf> = Vec::new();
    for (family, nodes) in families {
        let fresh: Vec<PathBuf> = nodes
            .into_iter()
            .filter(|node| !contains_path(&rendered, node))
            .collect();
        for (index, node) in expand_aliases(&fresh).iter().enumerate() {
            let param = out.param(format!("{family}_{index}"), node)?;
            out.rule(&format!("deny file-write* file-link (literal {param})"));
        }
        rendered.extend(fresh);
    }
    Ok(())
}

/// The read denies' nodes, as [`SandboxPolicy::glob_prefix_nodes`] and
/// [`SandboxPolicy::missing_ancestor_nodes`] derive the floor's: a glob's literal prefix strictly
/// inside a write root (`<ws>/secrets` for `secrets/**`, whether or not it exists yet), and every
/// ancestor of an entry inside a write root that is not a directory yet.
fn deny_nodes(denies: &[Pinned<'_>], policy: &SandboxPolicy) -> Vec<PathBuf> {
    let mut nodes: Vec<PathBuf> = Vec::new();
    for entry in denies {
        let anchor = entry.anchor();
        let strictly_inside_a_root = policy
            .write_roots
            .iter()
            .any(|root| !is_same_path(&anchor, root) && is_within(&anchor, root));
        if matches!(entry, Pinned::Glob { .. })
            && strictly_inside_a_root
            && !contains_path(&nodes, &anchor)
        {
            nodes.push(anchor.clone());
        }
        for ancestor in policy.renameable_ancestors(&anchor) {
            if !protected::is_real_directory(&ancestor) && !contains_path(&nodes, &ancestor) {
                nodes.push(ancestor);
            }
        }
    }
    nodes
}

/// Every existing ancestor of a pinned entry inside a writable root, held against rename with one
/// `file-write-unlink` deny each.
fn render_ancestor_pins(
    out: &mut ProfileBuilder,
    policy: &SandboxPolicy,
    entries: &[Pinned<'_>],
) -> Result<(), SandboxCommandError> {
    let mut ancestors: Vec<PathBuf> = Vec::new();
    for entry in entries {
        for ancestor in pinned_directories(entry, policy) {
            if !contains_path(&ancestors, &ancestor) {
                ancestors.push(ancestor);
            }
        }
    }
    for (index, ancestor) in expand_aliases(&ancestors).iter().enumerate() {
        let param = out.param(format!("PROTECTED_ANCESTOR_{index}"), ancestor)?;
        out.rule(&format!(
            "deny file-write-unlink (require-all (vnode-type DIRECTORY) (literal {param}))"
        ));
    }
    Ok(())
}

/// The directories a rename must not move for `entry` to keep naming the same tree: its existing
/// ancestors inside a writable root, the root included and past a missing one too (`tools` while
/// `tools/git` does not exist yet), and, for a glob, the literal prefix itself, which the glob's
/// own regexes do not cover (renaming `.git/modules` aside would otherwise let hooks be written
/// under the new name and moved back).
fn pinned_directories(entry: &Pinned<'_>, policy: &SandboxPolicy) -> Vec<PathBuf> {
    let anchor = entry.anchor();
    let mut pinned: Vec<PathBuf> = anchor
        .ancestors()
        .skip(1)
        .filter(|ancestor| {
            policy
                .write_roots
                .iter()
                .any(|root| is_within(ancestor, root))
        })
        .filter(|ancestor| protected::is_real_directory(ancestor))
        .map(Path::to_path_buf)
        .collect();
    let inside_a_root = policy
        .write_roots
        .iter()
        .any(|root| !is_same_path(&anchor, root) && is_within(&anchor, root));
    if matches!(entry, Pinned::Glob { .. })
        && inside_a_root
        && protected::is_real_directory(&anchor)
    {
        pinned.push(anchor);
    }
    pinned
}

/// Anchored `(regex #"…")` filters for the pattern `tail` below the literal directory `root`. The
/// tail's literal lead-in (`secrets` of `secrets/**/key`) joins the root as the anchor
/// ([`deny::split_glob_root`]) and every spelling Seatbelt may meet for the anchor is covered
/// ([`resolved_spellings`]: as written, its resolved form when a segment is a symlink, each with
/// its `/private` toggle), exactly as a path deny is. The anchor is regex-escaped as written (a
/// `[` in a workspace's own name stays a character) and only the glob part is translated. A floor
/// glob passes with `/` as its root. A relative root has no anchor here, and a tail that names its
/// own root would silently replace the given one; both are refused.
fn glob_filters(root: &Path, tail: &str) -> Result<Vec<PathFilter>, SandboxCommandError> {
    if !root.is_absolute() || root.to_str().is_none() {
        return Err(unrenderable(format!(
            "deny glob root must be absolute UTF-8: {root:?}"
        )));
    }
    if tail.starts_with('/') {
        return Err(unrenderable(format!(
            "deny glob tail must be relative to its root: {tail}"
        )));
    }
    deny::validate_deny_glob(tail).map_err(|e| unrenderable(e.to_string()))?;
    let (anchor, pattern) = deny::split_glob_root(root, tail);
    let filters: Vec<PathFilter> = resolved_spellings(&anchor)
        .iter()
        .filter_map(|spelling| deny::anchored_glob_regex(spelling, &pattern))
        .map(|regex| {
            deny::seatbelt_regex_filter(&regex)
                .map(PathFilter::Leaf)
                .ok_or_else(|| unrenderable(format!("cannot express deny glob: {tail}")))
        })
        .collect::<Result<_, _>>()?;
    if filters.is_empty() {
        return Err(unrenderable(format!("cannot anchor deny glob: {tail}")));
    }
    Ok(filters)
}

/// Every spelling Seatbelt may meet for each path ([`resolved_spellings`]), deduplicated in order,
/// for denies and floor entries only: a read or write root or a tree's exception uses
/// [`firmlink_spellings`], never resolved, so no path swapped in since the build is added.
fn expand_aliases<P: AsRef<Path>>(paths: &[P]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for path in paths {
        for form in resolved_spellings(path.as_ref()) {
            if !out.contains(&form) {
                out.push(form);
            }
        }
    }
    out
}

fn unrenderable(reason: String) -> SandboxCommandError {
    SandboxCommandError::Unrenderable {
        backend: BackendName::Seatbelt,
        reason,
    }
}

struct ProfileBuilder {
    lines: Vec<String>,
    params: Vec<(String, String)>,
    tag_modifier: String,
}

impl ProfileBuilder {
    fn new(tag: &CommandTag) -> Result<ProfileBuilder, SandboxCommandError> {
        let text = tag.as_ref();
        // The tag lands inside an SBPL string literal and a `log` predicate; keep it to characters
        // neither parser can misread.
        if text.is_empty()
            || !text
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
        {
            return Err(unrenderable(format!(
                "command tag has unsupported characters: {text}"
            )));
        }
        Ok(ProfileBuilder {
            lines: Vec::new(),
            params: Vec::new(),
            tag_modifier: format!("(with message \"{text}\")"),
        })
    }

    fn raw(&mut self, text: &str) {
        self.lines.push(text.trim_end().to_owned());
    }

    fn rule(&mut self, body: &str) {
        self.lines.push(format!("({body} {})", self.tag_modifier));
    }

    /// Registers `path` as `-D name=path` and returns the `(param "name")` reference.
    fn param(&mut self, name: String, path: &Path) -> Result<String, SandboxCommandError> {
        let Some(value) = path.to_str() else {
            return Err(unrenderable(format!("path is not UTF-8: {path:?}")));
        };
        if !path.is_absolute() {
            return Err(unrenderable(format!("path is not absolute: {value}")));
        }
        if value.chars().any(char::is_control) {
            return Err(unrenderable(format!(
                "path contains control characters: {value:?}"
            )));
        }
        let reference = format!("(param \"{name}\")");
        self.params.push((name, value.to_owned()));
        Ok(reference)
    }

    fn finish(self) -> Sbpl {
        let mut profile = self.lines.join("\n");
        profile.push('\n');
        Sbpl {
            profile,
            params: self.params,
        }
    }
}
