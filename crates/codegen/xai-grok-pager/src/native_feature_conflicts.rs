//! Native-feature ↔ package conflict table.
//!
//! **SSOT for defaults:** `assets/native_feature_conflicts.toml` (embedded).
//! **Runtime overlays (edit without rebuild):**
//! 1. `$GROK_HOME/native-feature-conflicts.toml`
//! 2. `$GROK_PROJECT_DIR/native-feature-conflicts.toml` (project sidecar)
//!
//! Later layers merge on top: per-feature package lists are **unioned**;
//! non-empty `reason` overwrites. Used by resource admission + F2 blurbs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Deserialize;

/// Sidecar filename under `$GROK_HOME` and `$GROK_PROJECT_DIR`.
pub const SIDECAR_FILE: &str = "native-feature-conflicts.toml";

const EMBEDDED: &str = include_str!("../assets/native_feature_conflicts.toml");

#[derive(Debug, Clone, Deserialize, Default)]
struct FileRoot {
    #[serde(default)]
    features: HashMap<String, FeatureConflicts>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct FeatureConflicts {
    #[serde(default)]
    reason: String,
    #[serde(default)]
    packages: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct FeatureConflictTable {
    features: HashMap<String, FeatureConflicts>,
}

impl FeatureConflictTable {
    /// Process-wide table: embedded defaults + user/project overlays.
    pub fn load() -> &'static Self {
        static TABLE: OnceLock<FeatureConflictTable> = OnceLock::new();
        TABLE.get_or_init(Self::load_layered)
    }

    /// Embedded defaults only (tests / no disk).
    pub fn load_embedded() -> Self {
        Self::from_toml_str(EMBEDDED)
            .unwrap_or_else(|e| panic!("embedded native_feature_conflicts.toml parse error: {e}"))
    }

    fn load_layered() -> Self {
        let mut table = Self::load_embedded();
        for path in overlay_paths() {
            if let Ok(content) = std::fs::read_to_string(&path) {
                match Self::from_toml_str(&content) {
                    Ok(overlay) => table.merge_overlay(&overlay),
                    Err(e) => {
                        tracing::warn!(
                            path = %path.display(),
                            error = %e,
                            "ignoring invalid native-feature-conflicts.toml"
                        );
                    }
                }
            }
        }
        table
    }

    fn from_toml_str(s: &str) -> Result<Self, toml::de::Error> {
        let root: FileRoot = toml::from_str(s)?;
        Ok(Self {
            features: root.features,
        })
    }

    /// Merge another table: package union per feature; non-empty reason wins.
    pub fn merge_overlay(&mut self, other: &Self) {
        for (key, incoming) in &other.features {
            let entry = self.features.entry(key.clone()).or_default();
            if !incoming.reason.is_empty() {
                entry.reason = incoming.reason.clone();
            }
            for pkg in &incoming.packages {
                if !entry.packages.iter().any(|p| p == pkg) {
                    entry.packages.push(pkg.clone());
                }
            }
        }
    }

    /// Packages blocked when `feature_key` is enabled (e.g. `pi_goal`).
    pub fn packages_for(&self, feature_key: &str) -> &[String] {
        self.features
            .get(feature_key)
            .map(|f| f.packages.as_slice())
            .unwrap_or(&[])
    }

    /// All packages that should be blocked for the given enabled feature keys.
    pub fn packages_for_enabled<'a>(
        &'a self,
        enabled: impl IntoIterator<Item = &'a str>,
    ) -> Vec<&'a str> {
        let mut out = Vec::new();
        for key in enabled {
            for pkg in self.packages_for(key) {
                if !out.contains(&pkg.as_str()) {
                    out.push(pkg.as_str());
                }
            }
        }
        out
    }

    pub fn reason_for(&self, feature_key: &str) -> &str {
        self.features
            .get(feature_key)
            .map(|f| f.reason.as_str())
            .unwrap_or("")
    }

    /// F2 blurb: "When on, blocks: npm:foo, npm:bar (reason)."
    pub fn f2_block_note(&self, feature_key: &str) -> Option<String> {
        let feature = self.features.get(feature_key)?;
        if feature.packages.is_empty() {
            return None;
        }
        let list = feature.packages.join(", ");
        if feature.reason.is_empty() {
            Some(format!("When on, blocks: {list}."))
        } else {
            Some(format!(
                "When on, blocks: {list} ({}).",
                feature.reason.trim_end_matches('.')
            ))
        }
    }
}

/// Paths checked after embedded defaults (user → project).
fn overlay_paths() -> Vec<PathBuf> {
    let mut paths = Vec::with_capacity(2);
    paths.push(xai_grok_tools::util::grok_home::grok_home().join(SIDECAR_FILE));
    if let Ok(cwd) = std::env::current_dir() {
        if let Some(proj) = find_project_sidecar(&cwd) {
            paths.push(proj);
        }
    }
    paths
}

fn find_project_sidecar(cwd: &Path) -> Option<PathBuf> {
    let mut dir = cwd.to_path_buf();
    loop {
        let candidate = xai_grok_config::project_config_dir(&dir).join(SIDECAR_FILE);
        if candidate.is_file() {
            return Some(candidate);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Append conflict note to a base F2 description. Leaks once per key for `'static`.
pub fn f2_description_with_conflicts(base: &'static str, feature_key: &str) -> &'static str {
    static CACHE: OnceLock<std::sync::Mutex<HashMap<String, &'static str>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(s) = guard.get(feature_key) {
        return s;
    }
    let table = FeatureConflictTable::load();
    let text = match table.f2_block_note(feature_key) {
        Some(note) => format!("{base} {note}"),
        None => base.to_owned(),
    };
    let leaked: &'static str = Box::leak(text.into_boxed_str());
    guard.insert(feature_key.to_owned(), leaked);
    leaked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_table_has_expected_features() {
        let t = FeatureConflictTable::load_embedded();
        assert!(
            t.packages_for("pi_ask_user_question")
                .iter()
                .any(|p| p.contains("rpiv-ask-user-question"))
        );
        assert!(
            t.packages_for("pi_todo")
                .iter()
                .any(|p| p == "npm:@juicesharp/rpiv-todo")
        );
        assert!(t.packages_for("pi_goal").iter().any(|p| p == "npm:pi-goal"));
        assert!(
            t.packages_for("pi_subagents")
                .iter()
                .any(|p| p == "npm:pi-subagents")
        );
        assert!(
            t.packages_for("pi_workflows")
                .iter()
                .any(|p| p.contains("pi-dynamic-workflows"))
        );
        assert!(t.packages_for("pi_btw").iter().any(|p| p == "npm:pi-btw"));
    }

    #[test]
    fn f2_note_lists_packages() {
        let t = FeatureConflictTable::load_embedded();
        let note = t.f2_block_note("pi_goal").expect("note");
        assert!(note.contains("npm:pi-goal"));
        assert!(note.starts_with("When on, blocks:"));
    }

    #[test]
    fn overlay_unions_packages_and_overwrites_reason() {
        let mut base = FeatureConflictTable::load_embedded();
        let overlay = FeatureConflictTable::from_toml_str(
            r#"
[features.pi_goal]
reason = "custom reason"
packages = ["npm:extra-goal-pkg"]
"#,
        )
        .unwrap();
        base.merge_overlay(&overlay);
        assert!(
            base.packages_for("pi_goal")
                .iter()
                .any(|p| p == "npm:pi-goal")
        );
        assert!(
            base.packages_for("pi_goal")
                .iter()
                .any(|p| p == "npm:extra-goal-pkg")
        );
        assert_eq!(base.reason_for("pi_goal"), "custom reason");
    }

    #[test]
    fn overlay_can_add_new_feature_key() {
        let mut base = FeatureConflictTable::load_embedded();
        let overlay = FeatureConflictTable::from_toml_str(
            r#"
[features.pi_loop]
reason = "hypothetical"
packages = ["npm:some-loop"]
"#,
        )
        .unwrap();
        base.merge_overlay(&overlay);
        assert_eq!(base.packages_for("pi_loop"), &["npm:some-loop".to_owned()]);
    }
}
