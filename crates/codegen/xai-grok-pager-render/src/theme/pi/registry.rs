//! In-process catalog of discoverable Pi themes.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use super::load::{LoadError, load_from_path, load_theme_palette_from_str};
use super::map::map_pi_theme;
use crate::theme::Theme;

/// Canonical id prefix for Pi themes in Grok config / slash UI.
pub const PI_THEME_PREFIX: &str = "pi:";

macro_rules! builtin_theme {
    ($file:literal) => {
        include_str!(concat!("../../../assets/pi-themes/", $file))
    };
}

/// Pi themes embedded into the binary. Keep this manifest in filename order so
/// additions are reviewable and deterministic; registry presentation still
/// sorts by the JSON `name` field.
const BUILTIN_THEMES: &[&str] = &[
    builtin_theme!("alabaster.json"),
    builtin_theme!("amethyst.json"),
    builtin_theme!("anthracite.json"),
    builtin_theme!("basalt.json"),
    builtin_theme!("birch.json"),
    builtin_theme!("dark-abyss.json"),
    builtin_theme!("dark-arctic.json"),
    builtin_theme!("dark-aurora.json"),
    builtin_theme!("dark-catppuccin.json"),
    builtin_theme!("dark-cavern.json"),
    builtin_theme!("dark-copper.json"),
    builtin_theme!("dark-cosmos.json"),
    builtin_theme!("dark-cyberpunk.json"),
    builtin_theme!("dark-dracula.json"),
    builtin_theme!("dark-eclipse.json"),
    builtin_theme!("dark-ember.json"),
    builtin_theme!("dark-equinox.json"),
    builtin_theme!("dark-forest.json"),
    builtin_theme!("dark-github.json"),
    builtin_theme!("dark-gruvbox.json"),
    builtin_theme!("dark-lavender.json"),
    builtin_theme!("dark-lunar.json"),
    builtin_theme!("dark-midnight.json"),
    builtin_theme!("dark-monochrome.json"),
    builtin_theme!("dark-monokai.json"),
    builtin_theme!("dark-nebula.json"),
    builtin_theme!("dark-nord.json"),
    builtin_theme!("dark-ocean.json"),
    builtin_theme!("dark-one.json"),
    builtin_theme!("dark-poimandres.json"),
    builtin_theme!("dark-rainforest.json"),
    builtin_theme!("dark-reef.json"),
    builtin_theme!("dark-retro.json"),
    builtin_theme!("dark-rose-pine.json"),
    builtin_theme!("dark-sakura.json"),
    builtin_theme!("dark-slate.json"),
    builtin_theme!("dark-solarized.json"),
    builtin_theme!("dark-solstice.json"),
    builtin_theme!("dark-starfall.json"),
    builtin_theme!("dark-sunset.json"),
    builtin_theme!("dark-swamp.json"),
    builtin_theme!("dark-synthwave.json"),
    builtin_theme!("dark-taiga.json"),
    builtin_theme!("dark-terminal.json"),
    builtin_theme!("dark-tokyo-night.json"),
    builtin_theme!("dark-tundra.json"),
    builtin_theme!("dark-twilight.json"),
    builtin_theme!("dark-volcanic.json"),
    builtin_theme!("dark.json"),
    builtin_theme!("graphite.json"),
    builtin_theme!("light-arctic.json"),
    builtin_theme!("light-aurora-day.json"),
    builtin_theme!("light-canyon.json"),
    builtin_theme!("light-catppuccin.json"),
    builtin_theme!("light-cirrus.json"),
    builtin_theme!("light-coral.json"),
    builtin_theme!("light-cyberpunk.json"),
    builtin_theme!("light-dawn.json"),
    builtin_theme!("light-dunes.json"),
    builtin_theme!("light-eucalyptus.json"),
    builtin_theme!("light-forest.json"),
    builtin_theme!("light-frost.json"),
    builtin_theme!("light-github.json"),
    builtin_theme!("light-glacier.json"),
    builtin_theme!("light-gruvbox.json"),
    builtin_theme!("light-haze.json"),
    builtin_theme!("light-honeycomb.json"),
    builtin_theme!("light-lagoon.json"),
    builtin_theme!("light-lavender.json"),
    builtin_theme!("light-meadow.json"),
    builtin_theme!("light-mint.json"),
    builtin_theme!("light-monochrome.json"),
    builtin_theme!("light-ocean.json"),
    builtin_theme!("light-one.json"),
    builtin_theme!("light-opal.json"),
    builtin_theme!("light-orchard.json"),
    builtin_theme!("light-paper.json"),
    builtin_theme!("light-poimandres.json"),
    builtin_theme!("light-prism.json"),
    builtin_theme!("light-retro.json"),
    builtin_theme!("light-sand.json"),
    builtin_theme!("light-savanna.json"),
    builtin_theme!("light-solarized.json"),
    builtin_theme!("light-soleil.json"),
    builtin_theme!("light-sunset.json"),
    builtin_theme!("light-synthwave.json"),
    builtin_theme!("light-tokyo-night.json"),
    builtin_theme!("light-wetland.json"),
    builtin_theme!("light-zenith.json"),
    builtin_theme!("light.json"),
    builtin_theme!("limestone.json"),
    builtin_theme!("mahogany.json"),
    builtin_theme!("marble.json"),
    builtin_theme!("obsidian.json"),
    builtin_theme!("onyx.json"),
    builtin_theme!("pearl.json"),
    builtin_theme!("porcelain.json"),
    builtin_theme!("quartz.json"),
    builtin_theme!("sandstone.json"),
    builtin_theme!("titanium.json"),
    builtin_theme!("transparent-light.json"),
    builtin_theme!("transparent.json"),
];

/// Metadata for a registered Pi theme (palette may be loaded lazily).
#[derive(Debug, Clone)]
pub struct PiThemeMeta {
    /// Theme `name` field from JSON.
    pub name: String,
    /// Canonical id: `pi:<name>`.
    pub id: String,
    /// Source path if loaded from disk; `None` for embedded builtins.
    pub path: Option<PathBuf>,
    pub builtin: bool,
}

/// Result of a discovery pass.
#[derive(Debug, Clone, Default)]
pub struct DiscoveryReport {
    pub loaded: usize,
    pub skipped: usize,
    pub errors: Vec<String>,
}

struct RegistryState {
    /// name → meta (first registration wins).
    by_name: HashMap<String, PiThemeMeta>,
    /// Cached palettes for builtins and already-loaded customs.
    palettes: HashMap<String, Theme>,
    initialized: bool,
}

impl RegistryState {
    fn empty() -> Self {
        Self {
            by_name: HashMap::new(),
            palettes: HashMap::new(),
            initialized: false,
        }
    }
}

static REGISTRY: LazyLock<Mutex<RegistryState>> =
    LazyLock::new(|| Mutex::new(RegistryState::empty()));

/// Build the canonical id for a Pi theme name.
pub fn theme_id(name: &str) -> String {
    format!("{PI_THEME_PREFIX}{name}")
}

/// Returns `Some(name)` if `id` is a Pi theme id (`pi:name` or bare name
/// that is registered). Prefer the `pi:` form for persistence.
pub fn parse_pi_theme_id(id: &str) -> Option<String> {
    let trimmed = id.trim();
    if let Some(rest) = trimmed.strip_prefix(PI_THEME_PREFIX) {
        if rest.is_empty() {
            return None;
        }
        return Some(rest.to_string());
    }
    None
}

/// Whether a theme setting string refers to a Pi theme.
pub fn is_pi_theme_id(id: &str) -> bool {
    parse_pi_theme_id(id).is_some()
}

/// Ensure builtins are registered (idempotent). Call before list/load.
pub fn ensure_builtins() {
    let mut guard = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
    if guard.initialized {
        return;
    }
    for json in BUILTIN_THEMES {
        register_builtin(&mut guard, json);
    }
    guard.initialized = true;
}

fn register_builtin(state: &mut RegistryState, json: &str) {
    match load_theme_palette_from_str(json) {
        Ok((name, palette)) => {
            let id = theme_id(&name);
            state.by_name.entry(name.clone()).or_insert(PiThemeMeta {
                name: name.clone(),
                id,
                path: None,
                builtin: true,
            });
            state.palettes.insert(name, palette);
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to load embedded Pi builtin theme");
        }
    }
}

/// Discover Pi themes from standard locations (Pi-aligned).
///
/// Order (first wins on name collision):
/// 1. Embedded builtins from `assets/pi-themes/*.json`
/// 2. `~/.pi/agent/themes/*.json`
/// 3. `<cwd>/.pi/themes/*.json`
/// 4. Paths from `PI_THEME_PATHS` (os path separator list)
pub fn init_discovery(cwd: &Path) -> DiscoveryReport {
    ensure_builtins();
    let mut report = DiscoveryReport::default();
    // Count builtins already present.
    {
        let guard = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
        report.loaded = guard.by_name.len();
    }
    scan_discovered_locations(cwd, &mut report);
    tracing::info!(
        target: "pi_theme",
        loaded = report.loaded,
        skipped = report.skipped,
        errors = report.errors.len(),
        "Pi theme discovery finished"
    );
    report
}

/// Re-scan Pi theme directories after `/reload` so newly added/changed JSON files
/// appear in Grok's `/theme` list without restarting the process.
///
/// Unlike first-load discovery, this reloads palettes for themes that already
/// have a file path so on-disk edits take effect. Builtins stay first-wins.
/// If a Pi theme is currently applied, its palette is re-applied after rescan.
pub fn rediscover(cwd: &Path) -> DiscoveryReport {
    ensure_builtins();
    let mut report = DiscoveryReport::default();
    rescan_discovered_locations(cwd, &mut report);
    {
        let guard = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
        report.loaded = guard.by_name.len();
    }
    let current = crate::theme::Theme::current_display_id();
    if is_pi_theme_id(&current) {
        if let Err(error) = apply_pi_theme(&current) {
            tracing::warn!(
                target: "pi_theme",
                %error,
                theme = %current,
                "failed to re-apply active Pi theme after rediscovery"
            );
            report.errors.push(format!("re-apply {current}: {error}"));
        }
    }
    tracing::info!(
        target: "pi_theme",
        loaded = report.loaded,
        skipped = report.skipped,
        errors = report.errors.len(),
        "Pi theme rediscovery finished"
    );
    report
}

fn scan_discovered_locations(cwd: &Path, report: &mut DiscoveryReport) {
    if let Some(home) = dirs::home_dir() {
        let global = home.join(".pi").join("agent").join("themes");
        scan_dir(&global, report);
    }

    let project = cwd.join(".pi").join("themes");
    scan_dir(&project, report);

    if let Ok(extra) = std::env::var("PI_THEME_PATHS") {
        for part in std::env::split_paths(&extra) {
            if part.is_dir() {
                scan_dir(&part, report);
            } else if part.is_file() {
                try_register_file(&part, report);
            }
        }
    }
}

fn rescan_discovered_locations(cwd: &Path, report: &mut DiscoveryReport) {
    // Pi reload replaces its resource loader rather than incrementally adding
    // to it. Drop all file-backed themes first so edits take effect, deleted
    // themes disappear, and normal first-source-wins ordering is rebuilt.
    clear_custom_file_themes();

    if let Some(home) = dirs::home_dir() {
        let global = home.join(".pi").join("agent").join("themes");
        scan_dir(&global, report);
    }

    let project = cwd.join(".pi").join("themes");
    scan_dir(&project, report);

    if let Ok(extra) = std::env::var("PI_THEME_PATHS") {
        for part in std::env::split_paths(&extra) {
            if part.is_dir() {
                scan_dir(&part, report);
            } else if part.is_file() {
                try_register_file(&part, report);
            }
        }
    }
}

/// Keep embedded builtins but discard every file-backed registration before a
/// Pi reload discovery pass. Rebuilding from disk is both simpler and more
/// faithful than mutating cached entries: it handles edits, deletions, and
/// priority changes in one operation.
fn clear_custom_file_themes() {
    let mut guard = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
    let builtin_names: HashSet<String> = guard
        .by_name
        .iter()
        .filter(|(_, meta)| meta.builtin)
        .map(|(name, _)| name.clone())
        .collect();
    guard.by_name.retain(|name, _| builtin_names.contains(name));
    guard
        .palettes
        .retain(|name, _| builtin_names.contains(name));
}

fn scan_dir(dir: &Path, report: &mut DiscoveryReport) {
    for path in theme_json_paths(dir) {
        try_register_file(&path, report);
    }
}

fn theme_json_paths(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("json"))
        })
        .collect();
    paths.sort();
    paths
}

fn try_register_file(path: &Path, report: &mut DiscoveryReport) {
    match load_from_path(path) {
        Ok(doc) => {
            let mut guard = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
            if guard.by_name.contains_key(&doc.name) {
                report.skipped += 1;
                tracing::debug!(
                    target: "pi_theme",
                    name = %doc.name,
                    path = %path.display(),
                    "Pi theme name collision — keeping first registration"
                );
                return;
            }
            match map_pi_theme(&doc) {
                Ok(palette) => {
                    let name = doc.name.clone();
                    let id = theme_id(&name);
                    guard.by_name.insert(
                        name.clone(),
                        PiThemeMeta {
                            name: name.clone(),
                            id,
                            path: Some(path.to_path_buf()),
                            builtin: false,
                        },
                    );
                    guard.palettes.insert(name, palette);
                    report.loaded += 1;
                }
                Err(e) => {
                    report.errors.push(format!("{}: {e}", path.display()));
                }
            }
        }
        Err(e) => {
            report.errors.push(format!("{}: {e}", path.display()));
        }
    }
}

/// List all registered Pi themes (sorted by name).
pub fn list_themes() -> Vec<PiThemeMeta> {
    ensure_builtins();
    let guard = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
    let mut list: Vec<_> = guard.by_name.values().cloned().collect();
    list.sort_by(|a, b| a.name.cmp(&b.name));
    list
}

/// Load the palette for a Pi theme id (`pi:name` or registered name via prefix).
pub fn load_palette(id: &str) -> Result<(String, Theme), LoadError> {
    ensure_builtins();
    let name = parse_pi_theme_id(id).ok_or_else(|| LoadError::InvalidName(id.to_string()))?;
    let path_to_reload = {
        let guard = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(palette) = guard.palettes.get(&name) {
            return Ok((theme_id(&name), *palette));
        }
        // Registered with path but palette missing (should not happen) — reload.
        guard.by_name.get(&name).and_then(|meta| meta.path.clone())
    };
    if let Some(path) = path_to_reload {
        let (n, palette) = super::load::load_theme_palette(&path)?;
        let mut guard = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
        guard.palettes.insert(n.clone(), palette);
        return Ok((theme_id(&n), palette));
    }
    Err(LoadError::InvalidName(format!("unknown Pi theme: {id}")))
}

/// Apply a Pi theme by id into the Grok custom palette slot.
pub fn apply_pi_theme(id: &str) -> Result<String, LoadError> {
    let (canonical_id, palette) = load_palette(id)?;
    crate::theme::Theme::apply_custom(canonical_id.clone(), palette);
    Ok(canonical_id)
}

/// Clear the in-process catalog (tests / re-discovery).
pub fn reset_registry() {
    let mut guard = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
    *guard = RegistryState::empty();
}

/// Alias used by unit tests in this crate and dependent packages.
pub fn reset_for_test() {
    reset_registry();
}

#[cfg(test)]
mod tests {
    use super::*;

    // Registry state is process-global and these tests intentionally reset it.
    // Share the renderer-wide theme lock so parallel tests cannot erase a
    // sibling test's discovered palette between `list_themes` and
    // `load_palette`.
    fn registry_test_guard() -> std::sync::MutexGuard<'static, ()> {
        crate::theme::cache::test_lock()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    #[test]
    fn builtins_registered() {
        let _guard = registry_test_guard();
        reset_registry();
        ensure_builtins();
        let list = list_themes();
        let names: Vec<_> = list.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(list.len(), BUILTIN_THEMES.len());
        assert!(names.contains(&"dark"));
        assert!(names.contains(&"light"));
        assert!(names.contains(&"transparent"));
        assert!(names.contains(&"transparent-light"));
        assert!(names.contains(&"dark-tokyo-night"));
        assert!(names.contains(&"alabaster"));
        let (id, theme) = load_palette("pi:dark").unwrap();
        assert_eq!(id, "pi:dark");
        assert!(theme.is_dark());
    }

    #[test]
    fn discover_custom_file() {
        let _guard = registry_test_guard();
        reset_registry();
        let dir = tempfile::tempdir().unwrap();
        let themes = dir.path().join(".pi").join("themes");
        std::fs::create_dir_all(&themes).unwrap();
        // Minimal valid theme based on dark with renamed name.
        let mut json = include_str!("../../../assets/pi-themes/dark.json").to_string();
        json = json.replacen("\"dark\"", "\"custom-test\"", 1);
        std::fs::write(themes.join("custom-test.json"), json).unwrap();

        let report = init_discovery(dir.path());
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        let list = list_themes();
        assert!(list.iter().any(|t| t.name == "custom-test"));
        let (_id, _) = load_palette("pi:custom-test").unwrap();
    }

    #[test]
    fn parse_id() {
        let _guard = registry_test_guard();
        assert_eq!(parse_pi_theme_id("pi:dark").as_deref(), Some("dark"));
        assert_eq!(parse_pi_theme_id("pi:").as_deref(), None);
        assert_eq!(parse_pi_theme_id("dark").as_deref(), None);
    }

    #[test]
    fn rediscover_picks_up_new_theme_files() {
        let _guard = registry_test_guard();
        reset_registry();
        let dir = tempfile::tempdir().unwrap();
        let themes = dir.path().join(".pi").join("themes");
        std::fs::create_dir_all(&themes).unwrap();

        // First discovery: empty project themes dir (builtins only).
        let first = init_discovery(dir.path());
        assert!(first.errors.is_empty(), "{:?}", first.errors);
        assert!(!list_themes().iter().any(|t| t.name == "reload-new"));

        let mut json = include_str!("../../../assets/pi-themes/dark.json").to_string();
        json = json.replacen("\"dark\"", "\"reload-new\"", 1);
        std::fs::write(themes.join("reload-new.json"), json).unwrap();

        let report = rediscover(dir.path());
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert!(list_themes().iter().any(|t| t.name == "reload-new"));
        let (_id, _) = load_palette("pi:reload-new").unwrap();
    }

    #[test]
    fn rediscover_replaces_edited_files_and_removes_deleted_files() {
        let _guard = registry_test_guard();
        reset_registry();
        let dir = tempfile::tempdir().unwrap();
        let themes = dir.path().join(".pi").join("themes");
        std::fs::create_dir_all(&themes).unwrap();
        let path = themes.join("reload-refresh.json");

        let mut initial = include_str!("../../../assets/pi-themes/dark.json").to_string();
        initial = initial.replacen("\"dark\"", "\"reload-refresh\"", 1);
        std::fs::write(&path, initial).unwrap();
        let report = init_discovery(dir.path());
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        let (_, before) = load_palette("pi:reload-refresh").unwrap();

        let mut edited = include_str!("../../../assets/pi-themes/dark.json").to_string();
        edited = edited.replacen("\"dark\"", "\"reload-refresh\"", 1);
        edited = edited.replacen("\"#d4d4d4\"", "\"#010203\"", 1);
        std::fs::write(&path, edited).unwrap();
        let report = rediscover(dir.path());
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        let (_, after) = load_palette("pi:reload-refresh").unwrap();
        assert_ne!(before.text_primary, after.text_primary);

        std::fs::remove_file(&path).unwrap();
        let report = rediscover(dir.path());
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert!(
            !list_themes()
                .iter()
                .any(|theme| theme.name == "reload-refresh")
        );
        assert!(load_palette("pi:reload-refresh").is_err());
    }
}
