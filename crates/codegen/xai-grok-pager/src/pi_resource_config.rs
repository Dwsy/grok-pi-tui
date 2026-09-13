//! Pi `config` compatibility model for the external Pi profile.
//!
//! This module deliberately owns no terminal UI. It reads Pi-compatible
//! settings and resource locations so the Pager can render them with native
//! Grok components.

use std::collections::{BTreeSet, HashMap};
use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

const CONFIG_DIR_NAME: &str = ".pi";
const RESOURCE_TYPES: [PiResourceType; 4] = [
    PiResourceType::Extensions,
    PiResourceType::Skills,
    PiResourceType::Prompts,
    PiResourceType::Themes,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Ord, PartialOrd)]
pub enum PiResourceType {
    Extensions,
    Skills,
    Prompts,
    Themes,
}

impl PiResourceType {
    pub const fn settings_key(self) -> &'static str {
        match self {
            Self::Extensions => "extensions",
            Self::Skills => "skills",
            Self::Prompts => "prompts",
            Self::Themes => "themes",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Extensions => "Extensions",
            Self::Skills => "Skills",
            Self::Prompts => "Prompts",
            Self::Themes => "Themes",
        }
    }

    fn matches_file(self, path: &Path) -> bool {
        match self {
            Self::Extensions => matches!(
                path.extension().and_then(|extension| extension.to_str()),
                Some("ts" | "js")
            ),
            Self::Skills | Self::Prompts => {
                path.extension().and_then(|extension| extension.to_str()) == Some("md")
            }
            Self::Themes => {
                path.extension().and_then(|extension| extension.to_str()) == Some("json")
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Ord, PartialOrd)]
pub enum PiResourceScope {
    User,
    Project,
}

impl PiResourceScope {
    pub const fn label(self) -> &'static str {
        match self {
            Self::User => "Global",
            Self::Project => "Project",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Ord, PartialOrd)]
pub enum PiResourceOrigin {
    Auto,
    Settings,
    Package,
}

impl PiResourceOrigin {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Settings => "settings",
            Self::Package => "package",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PiProjectOverride {
    Inherit,
    Load,
    Unload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PiResource {
    pub path: PathBuf,
    pub resource_type: PiResourceType,
    pub scope: PiResourceScope,
    pub origin: PiResourceOrigin,
    pub source: String,
    pub base_dir: PathBuf,
    pub enabled: bool,
    pub inherited_enabled: bool,
    pub project_override: PiProjectOverride,
}

impl PiResource {
    pub fn display_name(&self) -> String {
        if self.resource_type == PiResourceType::Skills
            && self.path.file_name().and_then(|name| name.to_str()) == Some("SKILL.md")
        {
            return self
                .path
                .parent()
                .and_then(|parent| parent.file_name())
                .and_then(|name| name.to_str())
                .unwrap_or("SKILL.md")
                .to_owned();
        }
        self.path
            .strip_prefix(&self.base_dir)
            .ok()
            .unwrap_or(&self.path)
            .to_string_lossy()
            .replace('\\', "/")
    }

    pub fn identity(&self) -> (PiResourceType, PathBuf) {
        (self.resource_type, canonical_or_clean(&self.path))
    }
}

#[derive(Debug, Clone)]
pub struct PiResourceCatalog {
    pub resources: Vec<PiResource>,
    pub project_trusted: bool,
    pub agent_dir: PathBuf,
    pub cwd: PathBuf,
}

fn resource_precedence(resource: &PiResource) -> u8 {
    match (resource.origin, resource.scope) {
        (PiResourceOrigin::Settings, PiResourceScope::Project) => 0,
        (PiResourceOrigin::Auto, PiResourceScope::Project) => 1,
        (PiResourceOrigin::Settings, PiResourceScope::User) => 2,
        (PiResourceOrigin::Auto, PiResourceScope::User) => 3,
        (PiResourceOrigin::Package, _) => 4,
    }
}

impl PiResourceCatalog {
    /// Load the resource catalog using the persisted trust store only.
    pub fn load(cwd: PathBuf) -> Result<Self> {
        Self::load_with_trust(cwd, None)
    }

    /// Load the resource catalog with an optional runtime trust override.
    ///
    /// `trust_override` mirrors Pi's `--approve` / `--no-approve` flags:
    /// - `Some(true)`  — treat the project as trusted for this run (like `--approve`)
    /// - `Some(false)` — treat the project as untrusted for this run (like `--no-approve`)
    /// - `None`        — fall back to the persisted `trust.json` decision
    ///
    /// **Agent-home adaptation** (matches Pi `getAgentDir()`):
    /// When `cwd` *is* the agent home, that directory is the user config/plugin root
    /// (`extensions/`, `settings.json`, packages), not a product project. User-scope
    /// discovery still runs (plugins load). Project-scope discovery is skipped unless
    /// `trust_override == Some(true)`, so `cwd/.pi` is not double-scanned as a project
    /// on top of the same tree.
    pub fn load_with_trust(cwd: PathBuf, trust_override: Option<bool>) -> Result<Self> {
        let agent_dir = agent_dir()?;
        let at_agent_home = canonical_or_clean(&cwd) == agent_dir;
        // Agent home = user resource root, never a project workspace by default.
        let project_trusted = match trust_override {
            Some(override_val) => override_val,
            None if at_agent_home => false,
            None => project_is_trusted(&agent_dir, &cwd)?,
        };
        let user_settings = SettingsDocument::load(&agent_dir.join("settings.json"))?;
        let project_dir = cwd.join(CONFIG_DIR_NAME);
        let project_settings = if project_trusted {
            SettingsDocument::load(&project_dir.join("settings.json"))?
        } else {
            SettingsDocument::default()
        };

        let mut resources = Vec::new();
        // User plugins always resolve from agent home (works when cwd is agent home).
        resources.extend(discover_scope(
            PiResourceScope::User,
            &agent_dir,
            &user_settings,
            &agent_dir,
        ));
        if project_trusted {
            resources.extend(discover_scope(
                PiResourceScope::Project,
                &project_dir,
                &project_settings,
                &agent_dir,
            ));
        }

        let user_enabled: HashMap<_, _> = resources
            .iter()
            .filter(|resource| resource.scope == PiResourceScope::User)
            .map(|resource| (resource.identity(), resource.enabled))
            .collect();
        for resource in &mut resources {
            if resource.scope == PiResourceScope::Project {
                resource.inherited_enabled = user_enabled
                    .get(&resource.identity())
                    .copied()
                    .unwrap_or(resource.enabled);
                resource.project_override = project_override(&project_settings, resource);
            }
        }

        resources.sort_by(|left, right| {
            (
                resource_precedence(left),
                left.source.as_str(),
                left.resource_type,
                &left.path,
            )
                .cmp(&(
                    resource_precedence(right),
                    right.source.as_str(),
                    right.resource_type,
                    &right.path,
                ))
        });
        resources.dedup_by(|left, right| left.identity() == right.identity());

        Ok(Self {
            resources,
            project_trusted,
            agent_dir,
            cwd,
        })
    }

    pub fn resources_for_scope(&self, scope: PiResourceScope) -> Vec<&PiResource> {
        self.resources
            .iter()
            .filter(|resource| resource.scope == scope || scope == PiResourceScope::Project)
            .collect()
    }
}

#[derive(Debug, Clone, Default)]
struct SettingsDocument {
    root: Map<String, Value>,
}

impl SettingsDocument {
    fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let source = fs::read_to_string(path)
            .with_context(|| format!("failed to read Pi settings {}", path.display()))?;
        let value: Value = serde_json::from_str(&source)
            .with_context(|| format!("invalid Pi settings JSON {}", path.display()))?;
        let Some(root) = value.as_object() else {
            bail!("invalid Pi settings {}: expected an object", path.display());
        };
        Ok(Self { root: root.clone() })
    }

    fn resource_patterns(&self, kind: PiResourceType) -> Vec<String> {
        self.root
            .get(kind.settings_key())
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect()
    }

    fn packages(&self) -> Vec<PackageSource> {
        self.root
            .get("packages")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(PackageSource::from_value)
            .collect()
    }
}

#[derive(Debug, Clone)]
struct PackageSource {
    source: String,
    autoload: bool,
    // Pi changes manifest fallback behavior when a package is represented as
    // an object, even if that object omits resource-type filters.
    filtered: bool,
    filters: HashMap<PiResourceType, Vec<String>>,
}

impl PackageSource {
    fn from_value(value: &Value) -> Option<Self> {
        if let Some(source) = value.as_str() {
            return Some(Self {
                source: source.to_owned(),
                autoload: true,
                filtered: false,
                filters: HashMap::new(),
            });
        }
        let object = value.as_object()?;
        let source = object.get("source")?.as_str()?.to_owned();
        let autoload = object
            .get("autoload")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let filters = RESOURCE_TYPES
            .iter()
            .filter_map(|kind| {
                let entries = object
                    .get(kind.settings_key())
                    .and_then(Value::as_array)?
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                Some((*kind, entries))
            })
            .collect();
        Some(Self {
            source,
            autoload,
            filtered: true,
            filters,
        })
    }
}

// `agent_dir` is Pi home (`getAgentDir()`): npm/git packages always resolve
// here, even when `config_dir` is a project `.pi` directory.
fn discover_scope(
    scope: PiResourceScope,
    config_dir: &Path,
    settings: &SettingsDocument,
    agent_dir: &Path,
) -> Vec<PiResource> {
    let mut resources = Vec::new();
    for kind in RESOURCE_TYPES {
        let patterns = settings.resource_patterns(kind);
        for path in explicit_resource_paths(config_dir, kind, &patterns) {
            let enabled = enabled_by_overrides(&path, &patterns, config_dir);
            resources.push(resource(
                path,
                kind,
                scope,
                PiResourceOrigin::Settings,
                "local".to_owned(),
                config_dir.to_path_buf(),
                enabled,
            ));
        }
        for path in auto_resource_paths(config_dir, kind, scope) {
            let enabled = enabled_by_overrides(&path, &patterns, config_dir);
            resources.push(resource(
                path,
                kind,
                scope,
                PiResourceOrigin::Auto,
                "auto".to_owned(),
                config_dir.to_path_buf(),
                enabled,
            ));
        }
    }

    for package in settings.packages() {
        // base_dir = settings root (user agent dir or project `.pi`);
        // agent_dir = Pi home for npm:/git: layout (matches PackageManager).
        let Some(package_dir) = package_dir(&package.source, config_dir, agent_dir) else {
            continue;
        };
        for kind in RESOURCE_TYPES {
            let filters = package.filters.get(&kind);
            for path in package_resource_paths(&package_dir, kind, package.filtered) {
                let Some(enabled) =
                    package_enabled_state(&path, package.autoload, filters, &package_dir)
                else {
                    continue;
                };
                resources.push(resource(
                    path,
                    kind,
                    scope,
                    PiResourceOrigin::Package,
                    package.source.clone(),
                    package_dir.clone(),
                    enabled,
                ));
            }
        }
    }
    resources
}

fn resource(
    path: PathBuf,
    resource_type: PiResourceType,
    scope: PiResourceScope,
    origin: PiResourceOrigin,
    source: String,
    base_dir: PathBuf,
    enabled: bool,
) -> PiResource {
    PiResource {
        path,
        resource_type,
        scope,
        origin,
        source,
        base_dir,
        enabled,
        inherited_enabled: enabled,
        project_override: PiProjectOverride::Inherit,
    }
}

fn explicit_resource_paths(
    base_dir: &Path,
    kind: PiResourceType,
    patterns: &[String],
) -> Vec<PathBuf> {
    patterns
        .iter()
        .filter(|entry| !is_pattern(entry))
        .flat_map(|entry| discover_resource_path(&resolve_path(base_dir, entry), kind))
        .collect()
}

fn auto_resource_paths(
    base_dir: &Path,
    kind: PiResourceType,
    scope: PiResourceScope,
) -> Vec<PathBuf> {
    let mut dirs = vec![base_dir.join(match kind {
        PiResourceType::Extensions => "extensions",
        PiResourceType::Skills => "skills",
        PiResourceType::Prompts => "prompts",
        PiResourceType::Themes => "themes",
    })];
    if kind == PiResourceType::Skills {
        if scope == PiResourceScope::User {
            if let Some(home) = xai_dirs::home_dir() {
                dirs.push(home.join(".agents/skills"));
            }
        } else if let Some(cwd) = base_dir.parent() {
            dirs.extend(ancestor_agent_skill_dirs(cwd));
        }
    }
    dirs.into_iter()
        .flat_map(|dir| match kind {
            PiResourceType::Extensions => auto_extension_paths(&dir),
            PiResourceType::Skills => discover_resource_path(&dir, kind),
            PiResourceType::Prompts | PiResourceType::Themes => direct_resource_paths(&dir, kind),
        })
        .collect()
}

/// Pi's auto-discovery treats extension directories as packages, not as source
/// trees. A directory contributes its declared `pi.extensions` entries or its
/// root `index.ts` / `index.js`; it never contributes nested implementation or
/// test files as standalone extensions.
fn auto_extension_paths(dir: &Path) -> Vec<PathBuf> {
    if let Some(entries) = extension_entry_paths(dir) {
        return entries;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths = BTreeSet::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.starts_with('.') || name == "node_modules" {
            continue;
        }
        if path.is_file() {
            if PiResourceType::Extensions.matches_file(&path) {
                paths.insert(canonical_or_clean(&path));
            }
        } else if path.is_dir() {
            paths.extend(extension_entry_paths(&path).unwrap_or_default());
        }
    }
    paths.into_iter().collect()
}

fn extension_entry_paths(dir: &Path) -> Option<Vec<PathBuf>> {
    let manifest_entries = fs::read_to_string(dir.join("package.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| value.get("pi").cloned())
        .and_then(|pi| pi.get("extensions").cloned())
        .and_then(|entries| entries.as_array().cloned());
    if let Some(entries) = manifest_entries {
        let resolved = entries
            .iter()
            .filter_map(Value::as_str)
            .map(|entry| resolve_path(dir, entry))
            .filter(|path| path.is_file() && PiResourceType::Extensions.matches_file(path))
            .map(|path| canonical_or_clean(&path))
            .collect::<Vec<_>>();
        if !resolved.is_empty() {
            return Some(resolved);
        }
    }
    [dir.join("index.ts"), dir.join("index.js")]
        .into_iter()
        .find(|path| path.is_file())
        .map(|path| vec![canonical_or_clean(&path)])
}

fn direct_resource_paths(dir: &Path, kind: PiResourceType) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && kind.matches_file(path))
        .map(|path| canonical_or_clean(&path))
        .collect()
}

fn ancestor_agent_skill_dirs(cwd: &Path) -> Vec<PathBuf> {
    let mut directories = Vec::new();
    let mut current = canonical_or_clean(cwd);
    loop {
        directories.push(current.join(".agents/skills"));
        if current.join(".git").exists() {
            break;
        }
        let Some(parent) = current.parent() else {
            break;
        };
        if parent == current {
            break;
        }
        current = parent.to_path_buf();
    }
    directories
}

fn package_resource_paths(
    package_dir: &Path,
    kind: PiResourceType,
    use_conventions_for_missing_manifest_type: bool,
) -> Vec<PathBuf> {
    let manifest = fs::read_to_string(package_dir.join("package.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| value.get("pi").cloned());

    if let Some(manifest) = manifest {
        let Some(entries) = manifest.get(kind.settings_key()).and_then(Value::as_array) else {
            // A bare package source with a pi manifest loads only declared
            // categories. A package object uses Pi's filter path, whose
            // missing category falls back to the conventional directory.
            return use_conventions_for_missing_manifest_type
                .then(|| package_convention_resource_paths(package_dir, kind))
                .unwrap_or_default();
        };
        let paths = entries
            .iter()
            .filter_map(Value::as_str)
            .filter(|entry| !starts_with_any(entry, &['!', '+', '-']))
            .flat_map(|entry| {
                manifest_entry_resource_paths(&resolve_path(package_dir, entry), kind)
            })
            .collect::<Vec<_>>();
        let manifest_overrides = entries
            .iter()
            .filter_map(Value::as_str)
            .filter(|entry| starts_with_any(entry, &['!', '+', '-']))
            .map(str::to_owned)
            .collect::<Vec<_>>();
        return paths
            .into_iter()
            .filter(|path| enabled_by_patterns(path, &manifest_overrides, package_dir))
            .collect();
    }

    package_convention_resource_paths(package_dir, kind)
}

fn manifest_entry_resource_paths(path: &Path, kind: PiResourceType) -> Vec<PathBuf> {
    match kind {
        // Pi's collectFilesFromPaths() treats an extension directory as an
        // extension package: it contributes package.json pi.entries or its
        // index.ts/index.js, never every implementation file below it.
        PiResourceType::Extensions if path.is_dir() => auto_extension_paths(path),
        _ => discover_resource_path(path, kind),
    }
}

fn package_convention_resource_paths(package_dir: &Path, kind: PiResourceType) -> Vec<PathBuf> {
    let convention_dir = package_dir.join(kind.settings_key());
    match kind {
        PiResourceType::Extensions => auto_extension_paths(&convention_dir),
        _ => discover_resource_path(&convention_dir, kind),
    }
}

fn package_enabled_state(
    path: &Path,
    autoload: bool,
    filters: Option<&Vec<String>>,
    base_dir: &Path,
) -> Option<bool> {
    match (autoload, filters) {
        (true, None) => Some(true),
        // Pi treats an explicitly empty filter array as an instruction to
        // disable this resource category, rather than as a missing filter.
        (true, Some(filters)) if filters.is_empty() => Some(false),
        (true, Some(filters)) => Some(enabled_by_patterns(path, filters, base_dir)),
        // autoload:false is a delta package: only resources named by its
        // filters enter Pi's resolver; unmatched resources must not appear.
        (false, None) => None,
        (false, Some(filters)) => has_autoload_disabled_match(path, filters, base_dir)
            .then(|| enabled_when_autoload_disabled(path, filters, base_dir)),
    }
}

fn has_autoload_disabled_match(path: &Path, patterns: &[String], base_dir: &Path) -> bool {
    patterns.iter().any(|pattern| {
        let target = pattern.trim_start_matches(['+', '-', '!']);
        if starts_with_any(pattern, &['+', '-']) {
            matches_exact(path, target, base_dir)
        } else {
            matches_pattern(path, target, base_dir)
        }
    })
}

fn discover_resource_path(path: &Path, kind: PiResourceType) -> Vec<PathBuf> {
    if path.is_file() {
        return kind
            .matches_file(path)
            .then(|| vec![canonical_or_clean(path)])
            .unwrap_or_default();
    }
    if !path.is_dir() {
        return Vec::new();
    }
    let root = canonical_or_clean(path);
    let mut builder = WalkBuilder::new(path);
    builder
        // Pi skips hidden entries *inside* a resource root. Enabling the
        // WalkBuilder hidden filter would also reject every child of ~/.pi.
        .hidden(false)
        .git_ignore(true)
        .ignore(true)
        // Pi begins ignore matching at the supplied resource root; parent
        // ignore files must not suppress an installed package's entries.
        .parents(false)
        .require_git(false)
        .filter_entry(move |entry| {
            let name = entry.file_name().to_str().unwrap_or_default();
            entry.path() == root || (name != "node_modules" && !name.starts_with('.'))
        });
    let mut entries = BTreeSet::new();
    for entry in builder.build().flatten() {
        let candidate = entry.path();
        if !candidate.is_file() || !kind.matches_file(candidate) {
            continue;
        }
        if kind == PiResourceType::Skills
            && candidate.file_name().and_then(|name| name.to_str()) != Some("SKILL.md")
        {
            continue;
        }
        entries.insert(canonical_or_clean(candidate));
    }
    entries.into_iter().collect()
}

fn enabled_by_overrides(path: &Path, entries: &[String], base_dir: &Path) -> bool {
    let overrides = entries
        .iter()
        .filter(|entry| starts_with_any(entry, &['!', '+', '-']))
        .cloned()
        .collect::<Vec<_>>();
    enabled_by_patterns(path, &overrides, base_dir)
}

fn enabled_by_patterns(path: &Path, patterns: &[String], base_dir: &Path) -> bool {
    let includes = patterns
        .iter()
        .filter(|pattern| !is_pattern(pattern))
        .collect::<Vec<_>>();
    let mut enabled = includes.is_empty()
        || includes
            .iter()
            .any(|pattern| matches_pattern(path, pattern, base_dir));

    // Pi applies overrides by category, not left-to-right input ordering:
    // excludes → force-includes → force-excludes. In particular, a '-' always
    // wins over a '+' for the same resource.
    if patterns.iter().any(|pattern| {
        pattern
            .strip_prefix('!')
            .is_some_and(|target| matches_pattern(path, target, base_dir))
    }) {
        enabled = false;
    }
    if patterns.iter().any(|pattern| {
        pattern
            .strip_prefix('+')
            .is_some_and(|target| matches_exact(path, target, base_dir))
    }) {
        enabled = true;
    }
    if patterns.iter().any(|pattern| {
        pattern
            .strip_prefix('-')
            .is_some_and(|target| matches_exact(path, target, base_dir))
    }) {
        enabled = false;
    }
    enabled
}

fn enabled_when_autoload_disabled(path: &Path, patterns: &[String], base_dir: &Path) -> bool {
    patterns.iter().any(|pattern| {
        let target = pattern.trim_start_matches(['+', '-', '!']);
        let matches = if starts_with_any(pattern, &['+', '-']) {
            matches_exact(path, target, base_dir)
        } else {
            matches_pattern(path, target, base_dir)
        };
        matches && !starts_with_any(pattern, &['-', '!'])
    })
}

fn project_override(settings: &SettingsDocument, resource: &PiResource) -> PiProjectOverride {
    let patterns = if resource.origin == PiResourceOrigin::Package {
        settings
            .packages()
            .into_iter()
            .find(|package| package.source == resource.source)
            .and_then(|package| package.filters.get(&resource.resource_type).cloned())
            .unwrap_or_default()
    } else {
        settings.resource_patterns(resource.resource_type)
    };
    for pattern in patterns.into_iter().rev() {
        let target = pattern.trim_start_matches(['+', '-', '!']);
        if matches_exact(&resource.path, target, &resource.base_dir) {
            return if starts_with_any(&pattern, &['-', '!']) {
                PiProjectOverride::Unload
            } else if pattern.starts_with('+') {
                PiProjectOverride::Load
            } else {
                PiProjectOverride::Inherit
            };
        }
    }
    PiProjectOverride::Inherit
}

fn is_pattern(value: &str) -> bool {
    starts_with_any(value, &['!', '+', '-']) || value.contains('*') || value.contains('?')
}

fn starts_with_any(value: &str, prefixes: &[char]) -> bool {
    value
        .chars()
        .next()
        .is_some_and(|first| prefixes.contains(&first))
}

fn matches_pattern(path: &Path, pattern: &str, base_dir: &Path) -> bool {
    let candidate = normalize_path(path);
    let relative = path
        .strip_prefix(base_dir)
        .map(normalize_path)
        .unwrap_or_else(|_| candidate.clone());
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let parent = if path.file_name().and_then(|name| name.to_str()) == Some("SKILL.md") {
        path.parent().map(normalize_path)
    } else {
        None
    };
    let parent_relative = path
        .parent()
        .and_then(|parent| parent.strip_prefix(base_dir).ok())
        .map(normalize_path);
    let pattern = pattern.replace('\\', "/");
    globset::Glob::new(&pattern)
        .ok()
        .map(|glob| glob.compile_matcher())
        .is_some_and(|matcher| {
            matcher.is_match(&relative)
                || matcher.is_match(name)
                || matcher.is_match(&candidate)
                || parent
                    .as_ref()
                    .is_some_and(|parent| matcher.is_match(parent))
                || parent_relative
                    .as_ref()
                    .is_some_and(|parent| matcher.is_match(parent))
        })
}

fn matches_exact(path: &Path, pattern: &str, base_dir: &Path) -> bool {
    let normalized = pattern.trim_start_matches("./").replace('\\', "/");
    normalized == normalize_path(path)
        || normalized
            == path
                .strip_prefix(base_dir)
                .map(normalize_path)
                .unwrap_or_default()
        || (path.file_name().and_then(|name| name.to_str()) == Some("SKILL.md")
            && path.parent().is_some_and(|parent| {
                normalized == normalize_path(parent)
                    || normalized
                        == parent
                            .strip_prefix(base_dir)
                            .map(normalize_path)
                            .unwrap_or_default()
            }))
}

fn package_dir(source: &str, base_dir: &Path, agent_dir: &Path) -> Option<PathBuf> {
    let source = source.trim();
    if source.starts_with("npm:") {
        let name = npm_package_name(source.strip_prefix("npm:")?);
        let directory = agent_dir.join("npm/node_modules").join(name);
        return directory.is_dir().then_some(directory);
    }
    if source.starts_with("git:")
        || source.starts_with("http://")
        || source.starts_with("https://")
        || source.starts_with("ssh://")
        || source.starts_with("git://")
        || source.starts_with("git@")
    {
        return git_package_dir(source, agent_dir);
    }
    let directory = resolve_path(base_dir, source);
    directory.is_dir().then_some(directory)
}

fn npm_package_name(spec: &str) -> &str {
    if spec.starts_with('@') {
        let mut separators = spec.match_indices('@').skip(1);
        if let Some((index, _)) = separators.next() {
            return &spec[..index];
        }
        return spec;
    }
    spec.split('@').next().unwrap_or(spec)
}

fn git_package_dir(source: &str, agent_dir: &Path) -> Option<PathBuf> {
    let source = source.trim().strip_prefix("git:").unwrap_or(source.trim());
    let trimmed = source
        .trim_start_matches("git+")
        .split(['#', '?'])
        .next()
        .unwrap_or(source)
        .trim_end_matches(".git");

    let (host, path) = if let Some(rest) = trimmed.strip_prefix("git@") {
        // SSH shorthand: git@github.com:owner/repo
        rest.split_once(':')?
    } else if let Some((_, rest)) = trimmed.split_once("://") {
        // https://, ssh:// and git:// URLs
        rest.split_once('/')?
    } else {
        // Pi's git: prefix accepts hosted shorthand: git:github.com/owner/repo
        let (host, path) = trimmed.split_once('/')?;
        (host, path)
    };
    let candidate = agent_dir.join("git").join(host).join(path);
    candidate.is_dir().then_some(candidate)
}

fn project_is_trusted(agent_dir: &Path, cwd: &Path) -> Result<bool> {
    let trust_path = agent_dir.join("trust.json");
    if !trust_path.exists() {
        return Ok(false);
    }
    let source = fs::read_to_string(&trust_path)
        .with_context(|| format!("failed to read Pi trust store {}", trust_path.display()))?;
    let trust: HashMap<String, Option<bool>> = serde_json::from_str(&source)
        .with_context(|| format!("invalid Pi trust store {}", trust_path.display()))?;
    let mut current = canonical_or_clean(cwd);
    loop {
        if let Some(Some(decision)) = trust.get(&normalize_path(&current)) {
            return Ok(*decision);
        }
        let Some(parent) = current.parent() else {
            return Ok(false);
        };
        if parent == current {
            return Ok(false);
        }
        current = parent.to_path_buf();
    }
}

/// Resolve Pi agent home, matching `getAgentDir()` in
/// `pi-main/packages/coding-agent/src/config.ts`:
/// 1. `PI_CODING_AGENT_DIR` (tilde-expanded) when set and non-empty
/// 2. else `~/.pi/agent`
///
/// Result is canonicalized when the path exists.
pub fn resolve_pi_agent_dir() -> Result<PathBuf> {
    if let Ok(path) = env::var("PI_CODING_AGENT_DIR") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return Ok(canonical_or_clean(&expand_tilde(Path::new(trimmed))));
        }
    }
    let home = xai_dirs::home_dir().context("could not resolve home directory for Pi config")?;
    Ok(canonical_or_clean(
        &home.join(CONFIG_DIR_NAME).join("agent"),
    ))
}

fn agent_dir() -> Result<PathBuf> {
    resolve_pi_agent_dir()
}

/// True when `cwd` is Pi's agent home (config/cache root, not a project workspace).
///
/// Uses the same resolution as [`resolve_pi_agent_dir`] so a custom
/// `PI_CODING_AGENT_DIR` is recognized, not only `~/.pi/agent`.
pub fn is_pi_agent_home(cwd: &Path) -> bool {
    let Ok(agent) = resolve_pi_agent_dir() else {
        return false;
    };
    canonical_or_clean(cwd) == agent
}

/// Expand a leading `~` / `~/` like Pi `normalizePath({ expandTilde: true })`.
fn expand_tilde(path: &Path) -> PathBuf {
    let raw = path.as_os_str();
    if raw == "~" {
        return xai_dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
    }
    let Some(s) = path.to_str() else {
        return path.to_path_buf();
    };
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = xai_dirs::home_dir() {
            return home.join(rest);
        }
    }
    #[cfg(windows)]
    if let Some(rest) = s.strip_prefix("~\\") {
        if let Some(home) = xai_dirs::home_dir() {
            return home.join(rest);
        }
    }
    path.to_path_buf()
}

fn resolve_path(base_dir: &Path, value: &str) -> PathBuf {
    let path = Path::new(value);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base_dir.join(path)
    }
}

fn canonical_or_clean(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn normalize_path(path: &Path) -> String {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(component),
        }
    }
    normalized.to_string_lossy().replace('\\', "/")
}

impl PiResourceCatalog {
    /// Persist the global enable/disable state using the same `+path` / `-path`
    /// representation as Pi's ConfigSelector. Runtime resource loading remains
    /// restart/reload-bound, so callers must make that boundary visible to users.
    pub fn set_global_enabled(&self, resource: &PiResource, enabled: bool) -> Result<()> {
        if resource.scope != PiResourceScope::User {
            bail!("global resource mutation requires a user-scoped resource");
        }
        let settings_path = self.agent_dir.join("settings.json");
        mutate_settings(&settings_path, |root| {
            if resource.origin == PiResourceOrigin::Package {
                update_package_filter(root, resource, enabled, None, &self.cwd)
            } else {
                update_top_level_filter(root, resource, enabled, None)
            }
        })
    }

    /// Persist one project-local Pi override. Project writes are rejected unless
    /// Pi's own trust store authoritatively marks the active project trusted.
    pub fn set_project_override(
        &self,
        resource: &PiResource,
        override_state: PiProjectOverride,
    ) -> Result<()> {
        if !self.project_trusted {
            bail!("Pi project is not trusted; refusing to write project settings");
        }
        let settings_path = self.cwd.join(CONFIG_DIR_NAME).join("settings.json");
        mutate_settings(&settings_path, |root| {
            if resource.origin == PiResourceOrigin::Package {
                update_package_filter(
                    root,
                    resource,
                    override_state == PiProjectOverride::Load,
                    Some(override_state),
                    &self.cwd,
                )
            } else {
                update_top_level_filter(
                    root,
                    resource,
                    override_state == PiProjectOverride::Load,
                    Some(override_state),
                )
            }
        })
    }
}

fn mutate_settings(
    path: &Path,
    update: impl FnOnce(&mut Map<String, Value>) -> Result<()>,
) -> Result<()> {
    let lock_path = path.with_extension("json.lock");
    let parent = path
        .parent()
        .context("Pi settings file must have a parent directory")?;
    fs::create_dir_all(parent).with_context(|| {
        format!(
            "failed to create Pi settings directory {}",
            parent.display()
        )
    })?;
    let mut acquired = false;
    for attempt in 0..10 {
        match fs::create_dir(&lock_path) {
            Ok(()) => {
                acquired = true;
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && attempt < 9 => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("failed to acquire Pi settings lock {}", lock_path.display())
                });
            }
        }
    }
    if !acquired {
        bail!("failed to acquire Pi settings lock {}", lock_path.display());
    }

    let result = (|| {
        let mut root = if path.exists() {
            let source = fs::read_to_string(path)
                .with_context(|| format!("failed to read Pi settings {}", path.display()))?;
            serde_json::from_str::<Value>(&source)
                .with_context(|| format!("invalid Pi settings JSON {}", path.display()))?
                .as_object()
                .cloned()
                .context("Pi settings root must be an object")?
        } else {
            Map::new()
        };
        update(&mut root)?;
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create Pi settings directory {}",
                parent.display()
            )
        })?;
        fs::write(path, format!("{}\n", serde_json::to_string_pretty(&root)?))
            .with_context(|| format!("failed to write Pi settings {}", path.display()))
    })();
    let release_result = fs::remove_dir(&lock_path)
        .with_context(|| format!("failed to release Pi settings lock {}", lock_path.display()));
    result.and(release_result)
}

fn update_top_level_filter(
    root: &mut Map<String, Value>,
    resource: &PiResource,
    enabled: bool,
    project_override: Option<PiProjectOverride>,
) -> Result<()> {
    let key = resource.resource_type.settings_key();
    let mut entries = string_array(root.get(key));
    let pattern = if project_override.is_some() && resource.scope == PiResourceScope::User {
        normalize_path(&resource.path)
    } else {
        resource_pattern(resource)
    };
    entries.retain(|entry| !pattern_targets_resource(entry, &pattern));
    match project_override {
        Some(PiProjectOverride::Inherit) => {}
        Some(PiProjectOverride::Load) => {
            if resource.scope == PiResourceScope::User && !entries.contains(&pattern) {
                entries.push(pattern.clone());
            }
            entries.push(format!("+{pattern}"));
        }
        Some(PiProjectOverride::Unload) => {
            if resource.scope == PiResourceScope::User && !entries.contains(&pattern) {
                entries.push(pattern.clone());
            }
            entries.push(format!("-{pattern}"));
        }
        None => entries.push(format!("{}{pattern}", if enabled { '+' } else { '-' })),
    }
    root.insert(
        key.to_owned(),
        Value::Array(entries.into_iter().map(Value::String).collect()),
    );
    Ok(())
}

fn update_package_filter(
    root: &mut Map<String, Value>,
    resource: &PiResource,
    enabled: bool,
    project_override: Option<PiProjectOverride>,
    project_cwd: &Path,
) -> Result<()> {
    let mut packages = root
        .remove("packages")
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();
    let index = packages.iter().position(|entry| {
        entry.as_str() == Some(resource.source.as_str())
            || entry
                .as_object()
                .and_then(|object| object.get("source"))
                .and_then(Value::as_str)
                == Some(resource.source.as_str())
    });
    let index = match index {
        Some(index) => index,
        None if project_override.is_some_and(|state| state != PiProjectOverride::Inherit) => {
            let source = project_package_source(resource, project_cwd);
            packages.push(serde_json::json!({"source": source, "autoload": false}));
            packages.len() - 1
        }
        None => return Ok(()),
    };

    let mut package = match packages[index].take() {
        Value::String(source) => {
            let mut object = Map::new();
            object.insert("source".to_owned(), Value::String(source));
            object
        }
        Value::Object(object) => object,
        _ => bail!("invalid Pi package source entry"),
    };
    let key = resource.resource_type.settings_key();
    let pattern = resource_pattern(resource);
    let mut entries = string_array(package.get(key));
    entries.retain(|entry| !pattern_targets_resource(entry, &pattern));
    match project_override {
        Some(PiProjectOverride::Inherit) => {}
        Some(PiProjectOverride::Load) => entries.push(format!("+{pattern}")),
        Some(PiProjectOverride::Unload) => entries.push(format!("-{pattern}")),
        None => entries.push(format!("{}{pattern}", if enabled { '+' } else { '-' })),
    }
    if entries.is_empty() {
        package.remove(key);
    } else {
        package.insert(
            key.to_owned(),
            Value::Array(entries.into_iter().map(Value::String).collect()),
        );
    }

    let has_filters = RESOURCE_TYPES
        .iter()
        .any(|kind| package.contains_key(kind.settings_key()));
    let autoload_false = package.get("autoload") == Some(&Value::Bool(false));
    packages[index] = if !has_filters && !autoload_false {
        Value::String(
            package
                .remove("source")
                .and_then(|value| value.as_str().map(str::to_owned))
                .context("Pi package source must be a string")?,
        )
    } else if !has_filters && autoload_false {
        packages.remove(index);
        root.insert("packages".to_owned(), Value::Array(packages));
        return Ok(());
    } else {
        Value::Object(package)
    };
    root.insert("packages".to_owned(), Value::Array(packages));
    Ok(())
}

fn project_package_source(resource: &PiResource, project_cwd: &Path) -> String {
    if resource.source.starts_with("npm:")
        || resource.source.starts_with("http://")
        || resource.source.starts_with("https://")
        || resource.source.starts_with("git@")
    {
        return resource.source.clone();
    }
    let _project_config_dir = project_cwd.join(CONFIG_DIR_NAME);
    // Pi accepts absolute local package sources. Retaining the resolved path
    // avoids a lossy relative-path conversion across volumes.
    normalize_path(&resource.base_dir)
}

fn string_array(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

fn resource_pattern(resource: &PiResource) -> String {
    resource
        .path
        .strip_prefix(&resource.base_dir)
        .map(normalize_path)
        .unwrap_or_else(|_| normalize_path(&resource.path))
}

fn pattern_targets_resource(entry: &str, pattern: &str) -> bool {
    entry.trim_start_matches(['+', '-', '!']).replace('\\', "/") == pattern
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn npm_scope_and_version_are_removed_from_install_path() {
        assert_eq!(npm_package_name("@scope/pkg@1.2.3"), "@scope/pkg");
        assert_eq!(npm_package_name("plain@1.2.3"), "plain");
        assert_eq!(npm_package_name("plain"), "plain");
    }

    #[test]
    fn git_package_dir_resolves_https_and_ssh() {
        let tmp = tempfile::tempdir().unwrap();
        let agent_dir = tmp.path().to_path_buf();

        // Create the expected directory so is_dir() passes.
        let repo_dir = agent_dir.join("git/github.com/owner/repo");
        std::fs::create_dir_all(&repo_dir).unwrap();

        // Pi git: hosted shorthand.
        let dir = git_package_dir("git:github.com/owner/repo", &agent_dir);
        assert_eq!(dir, Some(repo_dir.clone()));

        // HTTPS URL
        let dir = git_package_dir("https://github.com/owner/repo", &agent_dir);
        assert_eq!(dir, Some(repo_dir.clone()));

        // SSH URL (git@host:path form)
        let dir = git_package_dir("git@github.com:owner/repo", &agent_dir);
        assert_eq!(dir, Some(repo_dir.clone()));

        // git+ prefix and .git suffix stripped
        let dir = git_package_dir("git+https://github.com/owner/repo.git", &agent_dir);
        assert_eq!(dir, Some(repo_dir.clone()));

        // Branch/query fragments stripped
        let dir = git_package_dir("https://github.com/owner/repo#main", &agent_dir);
        assert_eq!(dir, Some(repo_dir));

        // Non-existent directory returns None
        assert!(git_package_dir("https://github.com/other/missing", &agent_dir).is_none());
    }

    #[test]
    fn git_prefixed_package_source_uses_pi_git_cache_layout() {
        let temp = tempfile::tempdir().expect("temp directory");
        let package = temp.path().join("git/github.com/owner/repo");
        fs::create_dir_all(&package).expect("package directory");
        fs::write(
            package.join("package.json"),
            r#"{"pi":{"extensions":["index.ts"]}}"#,
        )
        .expect("manifest");
        fs::write(package.join("index.ts"), "export default () => {}; ").expect("extension");
        let settings = SettingsDocument {
            root: serde_json::json!({"packages": ["git:github.com/owner/repo"]})
                .as_object()
                .expect("settings object")
                .clone(),
        };

        let resources = discover_scope(PiResourceScope::User, temp.path(), &settings, temp.path())
            .into_iter()
            .filter(|resource| resource.origin == PiResourceOrigin::Package)
            .collect::<Vec<_>>();
        assert_eq!(resources.len(), 1);
        assert_eq!(
            resources[0].path,
            canonical_or_clean(&package.join("index.ts"))
        );
        assert_eq!(resources[0].source, "git:github.com/owner/repo");
        assert!(resources[0].enabled);
    }

    #[test]
    fn auto_resources_ignore_unrelated_explicit_entries_when_enabled() {
        let temp = tempfile::tempdir().expect("temp directory");
        let extensions = temp.path().join("extensions");
        fs::create_dir_all(&extensions).expect("extensions directory");
        fs::write(extensions.join("auto.ts"), "export default () => {}; ").expect("auto extension");
        fs::write(temp.path().join("outside.ts"), "export default () => {}; ")
            .expect("explicit extension");
        let settings = SettingsDocument {
            root: serde_json::json!({"extensions": ["outside.ts"]})
                .as_object()
                .expect("settings object")
                .clone(),
        };

        let auto = discover_scope(PiResourceScope::User, temp.path(), &settings, temp.path())
            .into_iter()
            .find(|resource| resource.path == canonical_or_clean(&extensions.join("auto.ts")))
            .expect("auto-discovered extension");
        assert!(auto.enabled);
    }

    #[test]
    fn package_extension_directory_manifest_uses_only_declared_entry() {
        let temp = tempfile::tempdir().expect("temp directory");
        let package = temp.path().join("package");
        let extension = package.join("extensions/example");
        fs::create_dir_all(&extension).expect("extension directory");
        fs::write(
            package.join("package.json"),
            r#"{"pi":{"extensions":["extensions"]}}"#,
        )
        .expect("manifest");
        fs::write(extension.join("index.ts"), "export default () => {}; ")
            .expect("extension entry");
        fs::write(
            extension.join("internal.ts"),
            "export const helper = true; ",
        )
        .expect("internal module");

        assert_eq!(
            package_resource_paths(&package, PiResourceType::Extensions, false),
            vec![canonical_or_clean(&extension.join("index.ts"))]
        );
    }

    #[test]
    fn package_extension_directory_prefers_root_index_over_test_files() {
        let temp = tempfile::tempdir().expect("temp directory");
        let package = temp.path().join("package");
        let extensions = package.join("extensions");
        fs::create_dir_all(&extensions).expect("extensions directory");
        fs::write(
            package.join("package.json"),
            r#"{"pi":{"extensions":["extensions"]}}"#,
        )
        .expect("manifest");
        fs::write(extensions.join("index.ts"), "export default () => {}; ")
            .expect("extension entry");
        fs::write(
            extensions.join("overlay.compat.test.ts"),
            "import 'bun:test'; ",
        )
        .expect("test module");
        fs::write(
            extensions.join("overlay.ts"),
            "export const overlay = true; ",
        )
        .expect("internal module");

        assert_eq!(
            package_resource_paths(&package, PiResourceType::Extensions, false),
            vec![canonical_or_clean(&extensions.join("index.ts"))]
        );
    }

    #[test]
    fn package_empty_filter_disables_all_declared_resources() {
        let temp = tempfile::tempdir().expect("temp directory");
        let package = temp.path().join("package");
        fs::create_dir_all(&package).expect("package directory");
        fs::write(
            package.join("package.json"),
            r#"{"pi":{"extensions":["index.ts"]}}"#,
        )
        .expect("manifest");
        fs::write(package.join("index.ts"), "export default () => {}; ").expect("extension");
        let settings = SettingsDocument {
            root: serde_json::json!({
                "packages": [{"source": "./package", "extensions": []}]
            })
            .as_object()
            .expect("settings object")
            .clone(),
        };

        let resources = discover_scope(PiResourceScope::User, temp.path(), &settings, temp.path())
            .into_iter()
            .filter(|resource| resource.origin == PiResourceOrigin::Package)
            .collect::<Vec<_>>();
        assert_eq!(resources.len(), 1);
        assert!(!resources[0].enabled);
    }

    #[test]
    fn package_manifest_omitted_category_does_not_fall_back_to_conventions() {
        let temp = tempfile::tempdir().expect("temp directory");
        let package = temp.path().join("package");
        fs::create_dir_all(package.join("skills/example")).expect("skills directory");
        fs::write(
            package.join("package.json"),
            r#"{"pi":{"extensions":["index.ts"]}}"#,
        )
        .expect("manifest");
        fs::write(package.join("index.ts"), "export default () => {}; ").expect("extension");
        fs::write(package.join("skills/example/SKILL.md"), "# skill").expect("skill");
        let settings = SettingsDocument {
            root: serde_json::json!({"packages": ["./package"]})
                .as_object()
                .expect("settings object")
                .clone(),
        };

        let resources = discover_scope(PiResourceScope::User, temp.path(), &settings, temp.path())
            .into_iter()
            .filter(|resource| resource.origin == PiResourceOrigin::Package)
            .collect::<Vec<_>>();
        assert_eq!(resources.len(), 1);
        assert_eq!(resources[0].resource_type, PiResourceType::Extensions);
    }

    #[test]
    fn package_object_missing_manifest_category_uses_convention_directory() {
        let temp = tempfile::tempdir().expect("temp directory");
        let package = temp.path().join("package");
        fs::create_dir_all(package.join("skills/example")).expect("skills directory");
        fs::write(
            package.join("package.json"),
            r#"{"pi":{"extensions":["index.ts"]}}"#,
        )
        .expect("manifest");
        fs::write(package.join("index.ts"), "export default () => {}; ").expect("extension");
        fs::write(package.join("skills/example/SKILL.md"), "# skill").expect("skill");
        let settings = SettingsDocument {
            root: serde_json::json!({"packages": [{"source": "./package"}]})
                .as_object()
                .expect("settings object")
                .clone(),
        };

        let resources = discover_scope(PiResourceScope::User, temp.path(), &settings, temp.path())
            .into_iter()
            .filter(|resource| resource.origin == PiResourceOrigin::Package)
            .collect::<Vec<_>>();
        assert_eq!(resources.len(), 2);
        assert!(
            resources
                .iter()
                .any(|resource| resource.resource_type == PiResourceType::Skills)
        );
    }

    #[test]
    fn force_exclude_wins_over_include_regardless_of_array_order() {
        let base = Path::new("/tmp/pi");
        let path = Path::new("/tmp/pi/extensions/example.ts");
        assert!(!enabled_by_patterns(
            path,
            &[
                "*.ts".to_owned(),
                "-extensions/example.ts".to_owned(),
                "+extensions/example.ts".to_owned()
            ],
            base,
        ));
    }

    #[test]
    fn project_override_classifies_exact_patterns() {
        let resource = resource(
            PathBuf::from("/tmp/project/.pi/extensions/example.ts"),
            PiResourceType::Extensions,
            PiResourceScope::Project,
            PiResourceOrigin::Auto,
            "auto".to_owned(),
            PathBuf::from("/tmp/project/.pi"),
            true,
        );
        let settings = SettingsDocument {
            root: serde_json::json!({"extensions": ["-extensions/example.ts"]})
                .as_object()
                .expect("object")
                .clone(),
        };
        assert_eq!(
            project_override(&settings, &resource),
            PiProjectOverride::Unload
        );
    }

    #[test]
    fn settings_resources_precede_auto_resources_within_each_scope() {
        let settings = resource(
            PathBuf::from("/tmp/project/.pi/extensions/example.ts"),
            PiResourceType::Extensions,
            PiResourceScope::Project,
            PiResourceOrigin::Settings,
            "local".to_owned(),
            PathBuf::from("/tmp/project/.pi"),
            true,
        );
        let auto = resource(
            settings.path.clone(),
            PiResourceType::Extensions,
            PiResourceScope::Project,
            PiResourceOrigin::Auto,
            "auto".to_owned(),
            settings.base_dir.clone(),
            true,
        );
        assert!(resource_precedence(&settings) < resource_precedence(&auto));
    }

    #[test]
    fn auto_extension_discovery_uses_package_entries_not_nested_sources() {
        let temp = tempfile::tempdir().expect("temp directory");
        let extensions = temp.path().join("extensions");
        let package = extensions.join("example");
        fs::create_dir_all(package.join("src")).expect("package source directory");
        fs::write(package.join("index.ts"), "export default () => {}; ").expect("entry");
        fs::write(
            package.join("src/internal.ts"),
            "export const internal = true;",
        )
        .expect("nested source");

        assert_eq!(
            auto_extension_paths(&extensions),
            vec![canonical_or_clean(&package.join("index.ts"))]
        );
    }

    #[test]
    fn package_manifest_projects_all_resource_types_even_when_disabled() {
        let temp = tempfile::tempdir().expect("temp directory");
        let package = temp.path().join("subagents");
        fs::create_dir_all(package.join("skills/pi-subagents")).expect("skills directory");
        fs::create_dir_all(package.join("prompts")).expect("prompts directory");
        fs::write(
            package.join("package.json"),
            r#"{"pi":{"extensions":["./index.ts"],"skills":["./skills"],"prompts":["./prompts"]}}"#,
        )
        .expect("manifest");
        fs::write(package.join("index.ts"), "export default () => {};").expect("extension");
        fs::write(package.join("skills/pi-subagents/SKILL.md"), "# skill").expect("skill");
        for name in ["one", "two"] {
            fs::write(package.join(format!("prompts/{name}.md")), "# prompt").expect("prompt");
        }
        let settings = SettingsDocument {
            root: serde_json::json!({
                "packages": [{
                    "source": "./subagents",
                    "extensions": ["-index.ts"],
                    "skills": ["-skills/pi-subagents/SKILL.md"],
                    "prompts": ["-prompts/one.md", "-prompts/two.md"]
                }]
            })
            .as_object()
            .expect("settings object")
            .clone(),
        };

        let resources = discover_scope(PiResourceScope::User, temp.path(), &settings, temp.path())
            .into_iter()
            .filter(|resource| resource.origin == PiResourceOrigin::Package)
            .collect::<Vec<_>>();
        assert_eq!(resources.len(), 4);
        assert_eq!(
            resources
                .iter()
                .filter(|resource| resource.resource_type == PiResourceType::Prompts)
                .count(),
            2
        );
    }

    #[test]
    fn global_toggle_writes_pi_force_exclude() {
        let temp = tempfile::tempdir().expect("temp directory");
        let resource = resource(
            temp.path().join("extensions/example.ts"),
            PiResourceType::Extensions,
            PiResourceScope::User,
            PiResourceOrigin::Auto,
            "auto".to_owned(),
            temp.path().to_path_buf(),
            true,
        );
        let catalog = PiResourceCatalog {
            resources: vec![resource.clone()],
            project_trusted: false,
            agent_dir: temp.path().to_path_buf(),
            cwd: temp.path().join("project"),
        };
        catalog
            .set_global_enabled(&resource, false)
            .expect("toggle must persist");
        let settings: Value = serde_json::from_str(
            &fs::read_to_string(temp.path().join("settings.json")).expect("settings file"),
        )
        .expect("valid settings JSON");
        assert_eq!(
            settings["extensions"],
            serde_json::json!(["-extensions/example.ts"])
        );
    }

    #[test]
    fn untrusted_project_does_not_write_project_override() {
        let temp = tempfile::tempdir().expect("temp directory");
        let resource = resource(
            temp.path().join("agent/extensions/example.ts"),
            PiResourceType::Extensions,
            PiResourceScope::User,
            PiResourceOrigin::Auto,
            "auto".to_owned(),
            temp.path().join("agent"),
            true,
        );
        let cwd = temp.path().join("project");
        let catalog = PiResourceCatalog {
            resources: vec![resource.clone()],
            project_trusted: false,
            agent_dir: temp.path().join("agent"),
            cwd: cwd.clone(),
        };

        let error = catalog
            .set_project_override(&resource, PiProjectOverride::Unload)
            .expect_err("untrusted project must reject override writes");

        assert!(error.to_string().contains("not trusted"));
        assert!(
            !cwd.join(CONFIG_DIR_NAME).join("settings.json").exists(),
            "the rejected mutation must not create project settings"
        );
    }

    #[test]
    fn resource_discovery_allows_hidden_pi_ancestors() {
        let temp = tempfile::tempdir().expect("temp directory");
        let skills = temp
            .path()
            .join(".pi/agent/git/github.com/owner/repo/skills");
        let skill = skills.join("example/SKILL.md");
        fs::create_dir_all(skill.parent().expect("skill parent")).expect("skills directory");
        fs::write(&skill, "# skill").expect("skill");

        assert_eq!(
            discover_resource_path(&skills, PiResourceType::Skills),
            vec![canonical_or_clean(&skill)]
        );
    }

    #[test]
    fn resolve_pi_agent_dir_respects_env_and_tilde() {
        let temp = tempfile::tempdir().expect("temp directory");
        let custom = temp.path().join("custom-agent");
        fs::create_dir_all(&custom).expect("custom agent dir");

        let previous = env::var_os("PI_CODING_AGENT_DIR");
        // SAFETY: test-only env mutation; restored below.
        unsafe { env::set_var("PI_CODING_AGENT_DIR", &custom) };
        let resolved = resolve_pi_agent_dir().expect("resolve custom agent dir");
        assert_eq!(resolved, canonical_or_clean(&custom));
        assert!(is_pi_agent_home(&custom));
        assert!(!is_pi_agent_home(temp.path()));

        match previous {
            Some(value) => unsafe { env::set_var("PI_CODING_AGENT_DIR", value) },
            None => unsafe { env::remove_var("PI_CODING_AGENT_DIR") },
        }
    }

    #[test]
    fn agent_home_cwd_loads_user_plugins_not_project_scope() {
        let temp = tempfile::tempdir().expect("temp directory");
        let agent = temp.path().join("agent");
        let ext_dir = agent.join("extensions");
        fs::create_dir_all(&ext_dir).expect("extensions");
        fs::write(ext_dir.join("plugin.ts"), "export default () => {}; ").expect("plugin");
        // Nested project-looking package under agent/.pi — must NOT load as Project
        // when cwd is agent home (unless trust_override forces it).
        let project_pkg = agent.join(".pi");
        fs::create_dir_all(project_pkg.join("extensions")).expect("project ext");
        fs::write(
            project_pkg.join("extensions/project-only.ts"),
            "export default () => {}; ",
        )
        .expect("project ext");
        fs::write(
            project_pkg.join("settings.json"),
            r#"{"extensions":["+extensions/project-only.ts"]}"#,
        )
        .expect("project settings");
        fs::write(
            agent.join("settings.json"),
            r#"{"extensions":["+extensions/plugin.ts"]}"#,
        )
        .expect("user settings");

        let previous = env::var_os("PI_CODING_AGENT_DIR");
        unsafe { env::set_var("PI_CODING_AGENT_DIR", &agent) };

        let catalog = PiResourceCatalog::load_with_trust(agent.clone(), None).expect("catalog");
        assert!(!catalog.project_trusted);
        let paths: Vec<_> = catalog
            .resources
            .iter()
            .filter(|r| r.resource_type == PiResourceType::Extensions && r.enabled)
            .map(|r| r.path.clone())
            .collect();
        assert!(
            paths.iter().any(|p| p.ends_with("plugin.ts")),
            "user plugin must load when cwd is agent home: {paths:?}"
        );
        assert!(
            !paths.iter().any(|p| p.ends_with("project-only.ts")),
            "project-scope under agent/.pi must not load by default: {paths:?}"
        );

        // Explicit --approve still allows project scope for power users.
        let approved =
            PiResourceCatalog::load_with_trust(agent.clone(), Some(true)).expect("approved");
        assert!(approved.project_trusted);

        match previous {
            Some(value) => unsafe { env::set_var("PI_CODING_AGENT_DIR", value) },
            None => unsafe { env::remove_var("PI_CODING_AGENT_DIR") },
        }
    }

    #[test]
    fn expand_tilde_matches_pi_normalize_path() {
        let home = xai_dirs::home_dir().expect("home");
        assert_eq!(expand_tilde(Path::new("~")), home);
        assert_eq!(expand_tilde(Path::new("~/agent")), home.join("agent"));
        assert_eq!(
            expand_tilde(Path::new("/abs/path")),
            PathBuf::from("/abs/path")
        );
    }
}
