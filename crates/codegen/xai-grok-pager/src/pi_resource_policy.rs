//! grok-pi resource admission policy.
//!
//! Evaluates the [`PiResourceCatalog`] discovered by [`crate::pi_resource_config`]
//! and produces a [`PiLaunchPlan`] containing only the resources that should be
//! passed to the Pi RPC child process.  Resources that conflict with the Grok
//! Pager renderer (tool renderers, TUI header/footer overrides, raw terminal
//! hooks) are blocked by default; users can override via an explicit allowlist.
//!
//! The policy does **not** re-implement Pi's resource discovery or loading.
//! It only decides *which already-discovered resource paths* are forwarded to
//! Pi via `--extension` / `--prompt-template` / `--theme` flags after the
//! corresponding `--no-*` discovery flags. Skills remain Pi-owned so its
//! settings and reload lifecycle stay authoritative.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::pi_resource_config::{PiResource, PiResourceCatalog, PiResourceType};

// ── Default blocklist ────────────────────────────────────────────────────────

/// Package sources that are unconditionally blocked unless the user
/// explicitly allows them.  These are known to conflict with the Grok Pager
/// renderer or RPC transport.
pub(crate) const DEFAULT_BLOCKED_SOURCES: &[&str] = &[
    // Re-registers all seven built-in tools with Pi-TUI renderers and
    // monkey-patches `pi.registerTool`, breaking the RPC tool pipeline.
    "npm:pi-tool-display",
    // Pi-TUI-only footer/header overlays; no value under Pager.
    "npm:pi-custom-header",
    "npm:pi-custom-footer",
    // Pi-TUI pretty printers (syntax-highlighted reads, colored bash, tree
    // listings). Hijacks tool output rendering; conflicts with Pager cards.
    "npm:@heyhuynhgiabuu/pi-pretty",
    "npm:pi-pretty",
    // Pi Open TUI is a terminal UI renderer; it conflicts with Grok Pager.
    "npm:pi-open-tui",
];

/// File-name fragments that indicate a Pi-TUI renderer extension.
/// Matched case-insensitively against the resource file name.
const RENDERER_NAME_HINTS: &[&str] = &[
    "tool-display",
    "custom-header",
    "custom-footer",
    // Scoped or path-cloned installs of pi-pretty (package / dir name).
    "pi-pretty",
];

// ── Policy configuration ─────────────────────────────────────────────────────

/// Dedicated project-local sidecar file under `$GROK_PROJECT_DIR`
/// (default `.grok-pi/pi-resources.toml` for grok-pi).
///
/// Hand-edited 外挂: edit freely without touching full `config.toml`.
pub const PROJECT_SIDECAR_FILE: &str = "pi-resources.toml";

/// User-facing policy knobs.
///
/// Layered load (later layers merge on top):
/// 1. user `$GROK_HOME/config.toml` → `[pi.resources]`
/// 2. nearest project `$GROK_PROJECT_DIR/config.toml` → `[pi.resources]`
/// 3. nearest project `$GROK_PROJECT_DIR/pi-resources.toml` (flat or `[pi.resources]`)
///
/// Lists (`allow` / `block`) are unioned. `disable_heuristics` is overridden
/// only when the key is present in a later layer.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ResourcePolicy {
    /// Package sources or path fragments the user explicitly allows even if
    /// they would otherwise be blocked.  Takes precedence over the default
    /// blocklist and renderer heuristics.
    pub allow: Vec<String>,
    /// Additional package sources or path fragments the user wants blocked
    /// beyond the built-in defaults.
    pub block: Vec<String>,
    /// When `true`, skip the renderer-name heuristic and only apply the
    /// explicit source lists.  Useful for power users who want full control.
    pub disable_heuristics: bool,
    /// Runtime-only F2 / bridge feature keys currently enabled for this process
    /// (e.g. `pi_goal`, `pi_ask_user_question`, `pi_subagents`).
    /// Drives conditional package blocks from
    /// `assets/native_feature_conflicts.toml`. Not persisted.
    #[serde(skip)]
    pub enabled_native_features: Vec<String>,
    /// Native feature conflicts that cannot be overridden by user `allow`.
    /// Reserved for same-tool-name collisions where loading both providers
    /// would make registration order determine behavior. Not persisted.
    #[serde(skip)]
    pub forced_native_features: Vec<String>,
}

impl ResourcePolicy {
    /// Load layered policy: user home → project config → project sidecar.
    /// Returns `Default` when no layers exist.
    pub fn load_from_config() -> Self {
        let mut policy = Self::default();
        let home = xai_grok_tools::util::grok_home::grok_home().join("config.toml");
        policy.merge_partial(&load_partial_from_path(&home));

        if let Ok(cwd) = std::env::current_dir() {
            if let Some((project_config, sidecar)) = project_policy_paths(&cwd) {
                policy.merge_partial(&load_partial_from_path(&project_config));
                policy.merge_partial(&load_partial_from_path(&sidecar));
            }
        }
        policy
    }

    /// Merge another full policy: list union; `disable_heuristics` overwritten.
    pub fn merge_layer(&mut self, other: &Self) {
        for item in &other.allow {
            if !self.allow.iter().any(|x| x == item) {
                self.allow.push(item.clone());
            }
        }
        for item in &other.block {
            if !self.block.iter().any(|x| x == item) {
                self.block.push(item.clone());
            }
        }
        self.disable_heuristics = other.disable_heuristics;
    }

    /// Merge a partial layer: list union; heuristics only if key was present.
    fn merge_partial(&mut self, other: &PartialPolicy) {
        for item in &other.allow {
            if !self.allow.iter().any(|x| x == item) {
                self.allow.push(item.clone());
            }
        }
        for item in &other.block {
            if !self.block.iter().any(|x| x == item) {
                self.block.push(item.clone());
            }
        }
        if let Some(flag) = other.disable_heuristics {
            self.disable_heuristics = flag;
        }
    }

    /// Path-injectable variant for testing / single-file load.
    /// Accepts either `[pi.resources]` or top-level `allow`/`block` keys
    /// (sidecar style).
    pub fn load_from_path(path: &Path) -> Self {
        let partial = load_partial_from_path(path);
        ResourcePolicy {
            allow: partial.allow,
            block: partial.block,
            disable_heuristics: partial.disable_heuristics.unwrap_or(false),
            enabled_native_features: Vec::new(),
            forced_native_features: Vec::new(),
        }
    }

    /// Persist the policy to `$GROK_HOME/config.toml` under `[pi.resources]`,
    /// preserving all other tables and keys. Project sidecar is hand-edited.
    pub fn save_to_config(&self) -> std::io::Result<()> {
        let path = xai_grok_tools::util::grok_home::grok_home().join("config.toml");
        self.save_to_path(&path)
    }

    /// Path-injectable variant for testing.
    pub fn save_to_path(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = std::fs::read_to_string(path).unwrap_or_default();
        let mut doc: toml_edit::DocumentMut = content
            .parse()
            .unwrap_or_else(|_| toml_edit::DocumentMut::new());

        let table = doc
            .entry("pi")
            .or_insert(toml_edit::Item::Table(toml_edit::Table::new()))
            .as_table_mut()
            .expect("[pi] must be a table");
        let resources = table
            .entry("resources")
            .or_insert(toml_edit::Item::Table(toml_edit::Table::new()))
            .as_table_mut()
            .expect("[pi.resources] must be a table");

        resources["allow"] = toml_edit::value(
            self.allow
                .iter()
                .map(|s| toml_edit::Value::from(s.as_str()))
                .collect::<toml_edit::Array>(),
        );
        resources["block"] = toml_edit::value(
            self.block
                .iter()
                .map(|s| toml_edit::Value::from(s.as_str()))
                .collect::<toml_edit::Array>(),
        );
        resources["disable_heuristics"] = toml_edit::value(self.disable_heuristics);

        std::fs::write(path, doc.to_string())
    }
}

/// Partial layer so missing `disable_heuristics` does not clobber earlier layers.
#[derive(Debug, Clone, Default)]
struct PartialPolicy {
    allow: Vec<String>,
    block: Vec<String>,
    disable_heuristics: Option<bool>,
}

fn load_partial_from_path(path: &Path) -> PartialPolicy {
    let Ok(content) = std::fs::read_to_string(path) else {
        return PartialPolicy::default();
    };
    let Ok(doc) = toml::from_str::<toml::Value>(&content) else {
        return PartialPolicy::default();
    };

    // Prefer `[pi.resources]`; fall back to top-level keys for the sidecar.
    let table = doc
        .get("pi")
        .and_then(|pi| pi.get("resources"))
        .or_else(|| {
            if doc.get("allow").is_some()
                || doc.get("block").is_some()
                || doc.get("disable_heuristics").is_some()
            {
                Some(&doc)
            } else {
                None
            }
        });

    let Some(table) = table else {
        return PartialPolicy::default();
    };

    PartialPolicy {
        allow: table
            .get("allow")
            .and_then(|v| v.clone().try_into().ok())
            .unwrap_or_default(),
        block: table
            .get("block")
            .and_then(|v| v.clone().try_into().ok())
            .unwrap_or_default(),
        disable_heuristics: table.get("disable_heuristics").and_then(|v| v.as_bool()),
    }
}

/// Nearest project that has either `config.toml` or `pi-resources.toml`
/// under `$GROK_PROJECT_DIR` (default `.grok-pi` for grok-pi).
fn project_policy_paths(cwd: &Path) -> Option<(PathBuf, PathBuf)> {
    for dir in cwd.ancestors() {
        let proj = xai_grok_config::project_config_dir(dir);
        let config = proj.join("config.toml");
        let sidecar = proj.join(PROJECT_SIDECAR_FILE);
        if config.is_file() || sidecar.is_file() {
            return Some((config, sidecar));
        }
        // Stop at repo root so we do not pick an unrelated parent project.
        if dir.join(".git").exists() {
            break;
        }
    }
    None
}

impl ResourcePolicy {
    /// Evaluate the catalog and return the launch plan.
    pub fn evaluate(&self, catalog: &PiResourceCatalog) -> PiLaunchPlan {
        let mut plan = PiLaunchPlan::default();

        for resource in &catalog.resources {
            let decision = self.decide(resource);
            match decision {
                Decision::Allow => plan.add(resource),
                Decision::Block { reason } => {
                    plan.blocked.push(BlockedResource {
                        path: resource.path.clone(),
                        source: resource.source.clone(),
                        resource_type: resource.resource_type,
                        reason,
                    });
                }
            }
        }

        plan
    }

    fn decide(&self, resource: &PiResource) -> Decision {
        // 1. Pi owns its own enable/disable settings. A resource disabled in
        //    Pi settings is never loaded, regardless of host policy.
        if !resource.enabled {
            return Decision::Block {
                reason: "disabled in Pi settings".to_owned(),
            };
        }

        // 2. Same-tool-name native conflicts are hard blocks. These must win
        //    over user `allow`, otherwise extension registration order would
        //    decide which provider owns the tool name.
        if let Some(reason) =
            self.forced_feature_conflict_reason(&resource.source, resource.path.as_path())
        {
            return Decision::Block { reason };
        }

        // 3. User explicit allow overrides the ordinary host blocklist and
        //    heuristics only — it cannot re-enable Pi-disabled resources or
        //    forced native conflicts.
        if self.matches_any(&resource.source, &resource.path, &self.allow) {
            return Decision::Allow;
        }

        // 4. User explicit block.
        if self.matches_any(&resource.source, &resource.path, &self.block) {
            return Decision::Block {
                reason: "blocked by user policy".to_owned(),
            };
        }

        // 5. Built-in default blocklist (source match).
        for blocked in DEFAULT_BLOCKED_SOURCES {
            if resource.source == *blocked {
                return Decision::Block {
                    reason: format!(
                        "default-blocked: {} conflicts with Grok Pager renderer",
                        blocked
                    ),
                };
            }
        }

        // 5b. Feature-gated conflicts from assets/native_feature_conflicts.toml.
        if let Some(reason) =
            self.feature_conflict_reason(&resource.source, resource.path.as_path())
        {
            return Decision::Block { reason };
        }

        // 6. Renderer name heuristic (unless disabled).
        if !self.disable_heuristics {
            let file_name = resource
                .path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_lowercase();
            let parent_name = resource
                .path
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_lowercase();
            for hint in RENDERER_NAME_HINTS {
                if file_name.contains(hint) || parent_name.contains(hint) {
                    return Decision::Block {
                        reason: format!(
                            "heuristic: '{}' looks like a Pi-TUI renderer (matches '{}')",
                            resource.display_name(),
                            hint
                        ),
                    };
                }
            }
        }

        Decision::Allow
    }

    /// Check whether an explicit CLI path (e.g. from `grok-pi -e <path>`)
    /// should be blocked by the host policy.  Returns `Some(reason)` if the
    /// path is blocked, `None` if it is allowed.
    ///
    /// This is used to filter user-supplied `--extension` paths that were
    /// already written into `pi_args` before the catalog-based evaluation.
    pub fn check_explicit_path(&self, path: &str) -> Option<String> {
        let path_buf = Path::new(path);

        // Same-tool-name native conflicts cannot be bypassed by an explicit
        // extension path or user `allow`.
        if let Some(reason) = self.forced_feature_conflict_reason(path, path_buf) {
            return Some(reason);
        }

        // User explicit allow wins over ordinary host policy.
        if self.matches_any(path, path_buf, &self.allow) {
            return None;
        }

        // User explicit block.
        if self.matches_any(path, path_buf, &self.block) {
            return Some("blocked by user policy".to_owned());
        }

        // Default blocklist: match against known conflicting package names.
        for blocked in DEFAULT_BLOCKED_SOURCES {
            let pkg_name = blocked.strip_prefix("npm:").unwrap_or(blocked);
            if path.contains(pkg_name) {
                return Some(format!(
                    "default-blocked: {} conflicts with Grok Pager renderer",
                    blocked
                ));
            }
        }

        if let Some(reason) = self.feature_conflict_reason(path, path_buf) {
            return Some(reason);
        }

        // Renderer name heuristic.
        if !self.disable_heuristics {
            let file_name = path_buf
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_lowercase();
            let parent_name = path_buf
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_lowercase();
            for hint in RENDERER_NAME_HINTS {
                if file_name.contains(hint) || parent_name.contains(hint) {
                    return Some(format!(
                        "heuristic: path looks like a Pi-TUI renderer (matches '{}')",
                        hint
                    ));
                }
            }
        }

        None
    }

    /// Match against packages listed for forced native features.
    fn forced_feature_conflict_reason(&self, source_or_path: &str, path: &Path) -> Option<String> {
        self.feature_conflict_reason_for(&self.forced_native_features, source_or_path, path)
    }

    /// Match against packages listed for currently enabled native features.
    /// `source_or_path` is either `resource.source` (exact `npm:…`) or a CLI path.
    fn feature_conflict_reason(&self, source_or_path: &str, path: &Path) -> Option<String> {
        self.feature_conflict_reason_for(&self.enabled_native_features, source_or_path, path)
    }

    fn feature_conflict_reason_for(
        &self,
        features: &[String],
        source_or_path: &str,
        path: &Path,
    ) -> Option<String> {
        if features.is_empty() {
            return None;
        }
        let table = crate::native_feature_conflicts::FeatureConflictTable::load();
        let path_str = path.to_string_lossy();
        for feature in features {
            for blocked in table.packages_for(feature) {
                let exact = source_or_path == blocked.as_str();
                let pkg_name = blocked.strip_prefix("npm:").unwrap_or(blocked.as_str());
                let path_hit = path_str.contains(pkg_name) || source_or_path.contains(pkg_name);
                if exact || path_hit {
                    let reason = table.reason_for(feature);
                    return Some(if reason.is_empty() {
                        format!(
                            "default-blocked: {blocked} conflicts with native feature `{feature}`"
                        )
                    } else {
                        format!(
                            "default-blocked: {blocked} conflicts with native feature `{feature}` ({reason})"
                        )
                    });
                }
            }
        }
        None
    }

    fn matches_any(&self, source: &str, path: &Path, patterns: &[String]) -> bool {
        let path_str = path.to_string_lossy();
        patterns.iter().any(|pattern| {
            // `auto` and `local` are discovery labels, not stable resource
            // identities. Never let a legacy policy entry for either label
            // affect every discovered resource.
            let shared_discovery_label = matches!(pattern.as_str(), "auto" | "local");
            (!shared_discovery_label && source == pattern.as_str())
                || path_str.contains(pattern.as_str())
                || path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|name| name == pattern.as_str())
        })
    }
}

#[derive(Debug)]
enum Decision {
    Allow,
    Block { reason: String },
}

// ── Launch plan ──────────────────────────────────────────────────────────────

/// The concrete set of CLI arguments to pass to the Pi RPC child process.
#[derive(Debug, Clone, Default)]
pub struct PiLaunchPlan {
    /// Approved extension paths.
    pub extensions: Vec<PathBuf>,
    /// Approved skill paths.
    pub skills: Vec<PathBuf>,
    /// Approved prompt template paths.
    pub prompts: Vec<PathBuf>,
    /// Approved theme paths.
    pub themes: Vec<PathBuf>,
    /// Resources that were blocked, with reasons (for diagnostics / UI).
    pub blocked: Vec<BlockedResource>,
}

impl PiLaunchPlan {
    fn add(&mut self, resource: &PiResource) {
        match resource.resource_type {
            PiResourceType::Extensions => self.extensions.push(resource.path.clone()),
            PiResourceType::Skills => self.skills.push(resource.path.clone()),
            PiResourceType::Prompts => self.prompts.push(resource.path.clone()),
            PiResourceType::Themes => self.themes.push(resource.path.clone()),
        }
    }

    /// Append host-governed `--no-*` flags and explicit resource paths to `args`.
    ///
    /// Skills are intentionally omitted: Pi owns skill discovery, settings,
    /// and reload behavior. Bridge extensions injected by grok-pi (subagent,
    /// bash, recap, etc.) are not part of this plan; the caller appends them
    /// separately so they always load regardless of policy.
    pub fn append_args(&self, args: &mut Vec<String>) {
        args.push("--no-extensions".to_string());
        args.push("--no-prompt-templates".to_string());
        args.push("--no-themes".to_string());

        for path in &self.extensions {
            args.extend([
                "--extension".to_string(),
                path.to_string_lossy().into_owned(),
            ]);
        }
        for path in &self.prompts {
            args.extend([
                "--prompt-template".to_string(),
                path.to_string_lossy().into_owned(),
            ]);
        }
        for path in &self.themes {
            args.extend(["--theme".to_string(), path.to_string_lossy().into_owned()]);
        }
    }

    /// Human-readable summary of blocked resources for startup diagnostics.
    pub fn blocked_summary(&self) -> Option<String> {
        if self.blocked.is_empty() {
            return None;
        }
        let mut lines = Vec::new();
        for blocked in &self.blocked {
            lines.push(format!(
                "  • {} ({}) — {}",
                blocked.path.display(),
                blocked.source,
                blocked.reason,
            ));
        }
        Some(format!(
            "grok-pi resource policy blocked {} resource(s):\n{}",
            self.blocked.len(),
            lines.join("\n")
        ))
    }
}

/// A resource that was prevented from loading.
#[derive(Debug, Clone)]
pub struct BlockedResource {
    pub path: PathBuf,
    pub source: String,
    pub resource_type: PiResourceType,
    pub reason: String,
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pi_resource_config::{PiProjectOverride, PiResourceOrigin, PiResourceScope};

    fn make_resource(
        path: &str,
        source: &str,
        resource_type: PiResourceType,
        enabled: bool,
    ) -> PiResource {
        PiResource {
            path: PathBuf::from(path),
            resource_type,
            scope: PiResourceScope::User,
            origin: PiResourceOrigin::Package,
            source: source.to_owned(),
            base_dir: PathBuf::from("/home/user/.pi/agent"),
            enabled,
            inherited_enabled: enabled,
            project_override: PiProjectOverride::Inherit,
        }
    }

    fn make_catalog(resources: Vec<PiResource>) -> PiResourceCatalog {
        PiResourceCatalog {
            resources,
            project_trusted: false,
            agent_dir: PathBuf::from("/home/user/.pi/agent"),
            cwd: PathBuf::from("/home/user/project"),
        }
    }

    #[test]
    fn pi_tool_display_blocked_by_default() {
        let catalog = make_catalog(vec![
            make_resource(
                "/home/user/.pi/agent/npm/node_modules/pi-tool-display/index.ts",
                "npm:pi-tool-display",
                PiResourceType::Extensions,
                true,
            ),
            make_resource(
                "/home/user/.pi/agent/extensions/my-tool/index.ts",
                "local",
                PiResourceType::Extensions,
                true,
            ),
        ]);
        let policy = ResourcePolicy::default();
        let plan = policy.evaluate(&catalog);

        assert_eq!(plan.extensions.len(), 1);
        assert!(plan.extensions[0].ends_with("my-tool/index.ts"));
        assert_eq!(plan.blocked.len(), 1);
        assert!(plan.blocked[0].reason.contains("pi-tool-display"));
    }

    #[test]
    fn pi_pretty_blocked_by_default() {
        let catalog = make_catalog(vec![
            make_resource(
                "/home/user/.pi/agent/npm/node_modules/@heyhuynhgiabuu/pi-pretty/index.ts",
                "npm:@heyhuynhgiabuu/pi-pretty",
                PiResourceType::Extensions,
                true,
            ),
            make_resource(
                "/home/user/.pi/agent/npm/node_modules/pi-pretty/index.ts",
                "npm:pi-pretty",
                PiResourceType::Extensions,
                true,
            ),
            make_resource(
                "/home/user/.pi/agent/extensions/my-tool/index.ts",
                "local",
                PiResourceType::Extensions,
                true,
            ),
        ]);
        let policy = ResourcePolicy::default();
        let plan = policy.evaluate(&catalog);

        assert_eq!(plan.extensions.len(), 1);
        assert!(plan.extensions[0].ends_with("my-tool/index.ts"));
        assert_eq!(plan.blocked.len(), 2);
        assert!(plan.blocked.iter().any(|b| b.reason.contains("pi-pretty")));
    }

    #[test]
    fn pi_pretty_user_allow_overrides_default() {
        let catalog = make_catalog(vec![make_resource(
            "/home/user/.pi/agent/npm/node_modules/@heyhuynhgiabuu/pi-pretty/index.ts",
            "npm:@heyhuynhgiabuu/pi-pretty",
            PiResourceType::Extensions,
            true,
        )]);
        let policy = ResourcePolicy {
            allow: vec!["npm:@heyhuynhgiabuu/pi-pretty".to_owned()],
            ..Default::default()
        };
        let plan = policy.evaluate(&catalog);

        assert_eq!(plan.extensions.len(), 1);
        assert!(plan.blocked.is_empty());
    }

    #[test]
    fn rpiv_ask_user_question_allowed_when_native_qa_off() {
        let catalog = make_catalog(vec![make_resource(
            "/home/user/.pi/agent/npm/node_modules/@juicesharp/rpiv-ask-user-question/index.ts",
            "npm:@juicesharp/rpiv-ask-user-question",
            PiResourceType::Extensions,
            true,
        )]);
        let policy = ResourcePolicy::default();
        let plan = policy.evaluate(&catalog);

        assert_eq!(plan.extensions.len(), 1);
        assert!(plan.blocked.is_empty());
        assert!(policy
            .check_explicit_path(
                "/home/user/.pi/agent/npm/node_modules/@juicesharp/rpiv-ask-user-question/index.ts",
            )
            .is_none());
    }

    #[test]
    fn rpiv_ask_user_question_blocked_when_native_qa_on() {
        let catalog = make_catalog(vec![
            make_resource(
                "/home/user/.pi/agent/npm/node_modules/@juicesharp/rpiv-ask-user-question/index.ts",
                "npm:@juicesharp/rpiv-ask-user-question",
                PiResourceType::Extensions,
                true,
            ),
            make_resource(
                "/home/user/.pi/agent/extensions/my-tool/index.ts",
                "local",
                PiResourceType::Extensions,
                true,
            ),
        ]);
        let policy = ResourcePolicy {
            enabled_native_features: vec!["pi_ask_user_question".to_owned()],
            ..Default::default()
        };
        let plan = policy.evaluate(&catalog);

        assert_eq!(plan.extensions.len(), 1);
        assert!(plan.extensions[0].ends_with("my-tool/index.ts"));
        assert_eq!(plan.blocked.len(), 1);
        assert!(
            plan.blocked[0]
                .reason
                .contains("@juicesharp/rpiv-ask-user-question")
        );

        let reason = policy.check_explicit_path(
            "/home/user/.pi/agent/npm/node_modules/@juicesharp/rpiv-ask-user-question/index.ts",
        );
        assert!(
            reason
                .unwrap()
                .contains("@juicesharp/rpiv-ask-user-question")
        );
    }

    #[test]
    fn rpiv_todo_blocked_only_while_native_todo_is_on() {
        let catalog = make_catalog(vec![make_resource(
            "/home/user/.pi/agent/npm/node_modules/@juicesharp/rpiv-todo/index.ts",
            "npm:@juicesharp/rpiv-todo",
            PiResourceType::Extensions,
            true,
        )]);
        assert!(
            ResourcePolicy::default()
                .evaluate(&catalog)
                .blocked
                .is_empty()
        );

        let on = ResourcePolicy {
            enabled_native_features: vec!["pi_todo".to_owned()],
            ..Default::default()
        };
        let plan = on.evaluate(&catalog);
        assert_eq!(plan.blocked.len(), 1);
        assert!(plan.blocked[0].reason.contains("@juicesharp/rpiv-todo"));

        // Preserve the existing policy contract for ordinary feature conflicts:
        // a user allow remains authoritative unless the host marks the native
        // feature as a same-tool-name forced conflict.
        let allowed = ResourcePolicy {
            allow: vec!["npm:@juicesharp/rpiv-todo".to_owned()],
            enabled_native_features: vec!["pi_todo".to_owned()],
            ..Default::default()
        };
        assert!(allowed.evaluate(&catalog).blocked.is_empty());

        let forced = ResourcePolicy {
            allow: vec!["npm:@juicesharp/rpiv-todo".to_owned()],
            enabled_native_features: vec!["pi_todo".to_owned()],
            forced_native_features: vec!["pi_todo".to_owned()],
            ..Default::default()
        };
        let plan = forced.evaluate(&catalog);
        assert_eq!(plan.blocked.len(), 1);
        assert!(plan.blocked[0].reason.contains("@juicesharp/rpiv-todo"));
        assert!(
            forced
                .check_explicit_path(
                    "/home/user/.pi/agent/npm/node_modules/@juicesharp/rpiv-todo/index.ts"
                )
                .is_some()
        );
    }

    #[test]
    fn pi_goal_packages_blocked_when_goal_on() {
        let catalog = make_catalog(vec![make_resource(
            "/home/user/.pi/agent/npm/node_modules/@narumitw/pi-goal/index.ts",
            "npm:@narumitw/pi-goal",
            PiResourceType::Extensions,
            true,
        )]);
        let off = ResourcePolicy::default();
        assert!(off.evaluate(&catalog).blocked.is_empty());

        let on = ResourcePolicy {
            enabled_native_features: vec!["pi_goal".to_owned()],
            ..Default::default()
        };
        let plan = on.evaluate(&catalog);
        assert_eq!(plan.blocked.len(), 1);
        assert!(plan.blocked[0].reason.contains("pi-goal"));
    }

    #[test]
    fn pi_subagent_packages_are_blocked_only_while_native_subagents_are_on() {
        let catalog = make_catalog(vec![make_resource(
            "/home/user/.pi/agent/npm/node_modules/pi-subagents/index.ts",
            "npm:pi-subagents",
            PiResourceType::Extensions,
            true,
        )]);
        assert!(
            ResourcePolicy::default()
                .evaluate(&catalog)
                .blocked
                .is_empty()
        );

        let on = ResourcePolicy {
            enabled_native_features: vec!["pi_subagents".to_owned()],
            ..Default::default()
        };
        let plan = on.evaluate(&catalog);
        assert_eq!(plan.blocked.len(), 1);
        assert!(plan.blocked[0].reason.contains("pi-subagents"));
    }

    #[test]
    fn user_allow_overrides_default_block() {
        let catalog = make_catalog(vec![make_resource(
            "/home/user/.pi/agent/npm/node_modules/pi-tool-display/index.ts",
            "npm:pi-tool-display",
            PiResourceType::Extensions,
            true,
        )]);
        let policy = ResourcePolicy {
            allow: vec!["npm:pi-tool-display".to_owned()],
            ..Default::default()
        };
        let plan = policy.evaluate(&catalog);

        assert_eq!(plan.extensions.len(), 1);
        assert!(plan.blocked.is_empty());
    }

    #[test]
    fn shared_discovery_labels_do_not_match_every_resource() {
        let catalog = make_catalog(vec![
            make_resource(
                "/home/user/.pi/agent/extensions/alpha.ts",
                "auto",
                PiResourceType::Extensions,
                true,
            ),
            make_resource(
                "/home/user/.pi/agent/extensions/beta.ts",
                "auto",
                PiResourceType::Extensions,
                true,
            ),
        ]);
        let policy = ResourcePolicy {
            block: vec!["auto".to_owned()],
            ..Default::default()
        };
        let plan = policy.evaluate(&catalog);

        assert_eq!(plan.extensions.len(), 2);
        assert!(plan.blocked.is_empty());
    }

    #[test]
    fn user_block_adds_to_defaults() {
        let catalog = make_catalog(vec![make_resource(
            "/home/user/.pi/agent/extensions/noisy-ext/index.ts",
            "local",
            PiResourceType::Extensions,
            true,
        )]);
        let policy = ResourcePolicy {
            block: vec!["noisy-ext".to_owned()],
            ..Default::default()
        };
        let plan = policy.evaluate(&catalog);

        assert!(plan.extensions.is_empty());
        assert_eq!(plan.blocked.len(), 1);
        assert!(plan.blocked[0].reason.contains("user policy"));
    }

    #[test]
    fn renderer_heuristic_blocks_by_name() {
        let catalog = make_catalog(vec![make_resource(
            "/home/user/.pi/agent/extensions/some-tool-display/index.ts",
            "npm:some-tool-display",
            PiResourceType::Extensions,
            true,
        )]);
        let policy = ResourcePolicy::default();
        let plan = policy.evaluate(&catalog);

        assert!(plan.extensions.is_empty());
        assert!(plan.blocked[0].reason.contains("heuristic"));
    }

    #[test]
    fn disabled_heuristics_allows_name_match() {
        let catalog = make_catalog(vec![make_resource(
            "/home/user/.pi/agent/extensions/some-tool-display/index.ts",
            "npm:some-tool-display",
            PiResourceType::Extensions,
            true,
        )]);
        let policy = ResourcePolicy {
            disable_heuristics: true,
            ..Default::default()
        };
        let plan = policy.evaluate(&catalog);

        assert_eq!(plan.extensions.len(), 1);
    }

    #[test]
    fn disabled_resource_is_blocked() {
        let catalog = make_catalog(vec![make_resource(
            "/home/user/.pi/agent/extensions/good-ext/index.ts",
            "local",
            PiResourceType::Extensions,
            false, // disabled in Pi settings
        )]);
        let policy = ResourcePolicy::default();
        let plan = policy.evaluate(&catalog);

        assert!(plan.extensions.is_empty());
        assert!(plan.blocked[0].reason.contains("disabled in Pi settings"));
    }

    #[test]
    fn allow_does_not_override_pi_disabled() {
        // A resource disabled in Pi settings must stay blocked even if the
        // host allow-list matches it. Pi owns its own enable/disable state.
        let catalog = make_catalog(vec![make_resource(
            "/home/user/.pi/agent/extensions/some-ext/index.ts",
            "npm:some-ext",
            PiResourceType::Extensions,
            false, // disabled in Pi settings
        )]);
        let policy = ResourcePolicy {
            allow: vec!["npm:some-ext".to_owned()],
            ..Default::default()
        };
        let plan = policy.evaluate(&catalog);

        assert!(plan.extensions.is_empty());
        assert_eq!(plan.blocked.len(), 1);
        assert!(plan.blocked[0].reason.contains("disabled in Pi settings"));
    }

    #[test]
    fn skills_prompts_themes_pass_through() {
        let catalog = make_catalog(vec![
            make_resource(
                "/home/user/.pi/agent/skills/my-skill/SKILL.md",
                "auto",
                PiResourceType::Skills,
                true,
            ),
            make_resource(
                "/home/user/.pi/agent/prompts/my-prompt.md",
                "auto",
                PiResourceType::Prompts,
                true,
            ),
            make_resource(
                "/home/user/.pi/agent/themes/dark.json",
                "auto",
                PiResourceType::Themes,
                true,
            ),
        ]);
        let policy = ResourcePolicy::default();
        let plan = policy.evaluate(&catalog);

        assert_eq!(plan.skills.len(), 1);
        assert_eq!(plan.prompts.len(), 1);
        assert_eq!(plan.themes.len(), 1);
        assert!(plan.blocked.is_empty());
    }

    #[test]
    fn append_args_keeps_skills_pi_owned() {
        let mut plan = PiLaunchPlan::default();
        plan.extensions.push(PathBuf::from("/ext/a.ts"));
        plan.skills.push(PathBuf::from("/skills/b/SKILL.md"));

        let mut args = Vec::new();
        plan.append_args(&mut args);

        assert!(args.contains(&"--no-extensions".to_string()));
        assert!(args.contains(&"--no-prompt-templates".to_string()));
        assert!(args.contains(&"--no-themes".to_string()));
        assert!(args.contains(&"--extension".to_string()));
        assert!(args.contains(&"/ext/a.ts".to_string()));
        assert!(!args.contains(&"--no-skills".to_string()));
        assert!(!args.contains(&"--skill".to_string()));
        assert!(!args.contains(&"/skills/b/SKILL.md".to_string()));
    }

    #[test]
    fn blocked_summary_is_none_when_empty() {
        let plan = PiLaunchPlan::default();
        assert!(plan.blocked_summary().is_none());
    }

    #[test]
    fn blocked_summary_lists_resources() {
        let mut plan = PiLaunchPlan::default();
        plan.blocked.push(BlockedResource {
            path: PathBuf::from("/ext/bad.ts"),
            source: "npm:bad".to_owned(),
            resource_type: PiResourceType::Extensions,
            reason: "test reason".to_owned(),
        });
        let summary = plan.blocked_summary().unwrap();
        assert!(summary.contains("bad.ts"));
        assert!(summary.contains("test reason"));
    }

    // ── check_explicit_path tests ─────────────────────────────────────────

    #[test]
    fn explicit_path_blocks_pi_tool_display() {
        let policy = ResourcePolicy::default();
        let reason = policy
            .check_explicit_path("/home/user/.pi/agent/npm/node_modules/pi-tool-display/index.ts");
        assert!(reason.is_some());
        assert!(reason.unwrap().contains("pi-tool-display"));
    }

    #[test]
    fn explicit_path_blocks_pi_pretty() {
        let policy = ResourcePolicy::default();
        let reason = policy.check_explicit_path(
            "/home/user/.pi/agent/npm/node_modules/@heyhuynhgiabuu/pi-pretty/index.ts",
        );
        assert!(reason.is_some());
        assert!(reason.unwrap().contains("pi-pretty"));
    }

    #[test]
    fn explicit_path_allows_normal_extension() {
        let policy = ResourcePolicy::default();
        assert!(
            policy
                .check_explicit_path("/home/user/my-ext/index.ts")
                .is_none()
        );
    }

    #[test]
    fn explicit_path_allow_overrides_block() {
        let policy = ResourcePolicy {
            allow: vec!["pi-tool-display".to_owned()],
            ..Default::default()
        };
        assert!(
            policy
                .check_explicit_path(
                    "/home/user/.pi/agent/npm/node_modules/pi-tool-display/index.ts"
                )
                .is_none()
        );
    }

    #[test]
    fn explicit_path_user_block() {
        let policy = ResourcePolicy {
            block: vec!["my-custom-renderer".to_owned()],
            ..Default::default()
        };
        let reason = policy.check_explicit_path("/ext/my-custom-renderer/index.ts");
        assert!(reason.is_some());
        assert!(reason.unwrap().contains("user policy"));
    }

    #[test]
    fn explicit_path_heuristic_blocks_renderer_name() {
        let policy = ResourcePolicy::default();
        let reason = policy.check_explicit_path("/ext/custom-header-widget/index.ts");
        assert!(reason.is_some());
        assert!(reason.unwrap().contains("custom-header"));
    }

    #[test]
    fn explicit_path_disable_heuristics_allows_renderer_name() {
        let policy = ResourcePolicy {
            disable_heuristics: true,
            ..Default::default()
        };
        assert!(
            policy
                .check_explicit_path("/ext/custom-header-widget/index.ts")
                .is_none()
        );
    }

    // ── TOML persistence tests ────────────────────────────────────────────

    #[test]
    fn toml_round_trip_preserves_policy() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");

        let policy = ResourcePolicy {
            allow: vec!["npm:pi-tool-display".to_owned()],
            block: vec!["my-bad-ext".to_owned()],
            disable_heuristics: true,
            enabled_native_features: Vec::new(),
            forced_native_features: Vec::new(),
        };
        policy.save_to_path(&path).unwrap();

        let loaded = ResourcePolicy::load_from_path(&path);
        assert_eq!(loaded.allow, policy.allow);
        assert_eq!(loaded.block, policy.block);
        assert!(loaded.disable_heuristics);
    }

    #[test]
    fn toml_load_missing_file_returns_default() {
        let policy = ResourcePolicy::load_from_path(Path::new("/nonexistent/config.toml"));
        assert!(policy.allow.is_empty());
        assert!(policy.block.is_empty());
        assert!(!policy.disable_heuristics);
    }

    #[test]
    fn toml_save_preserves_sibling_tables() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[ui]\ntheme = \"dark\"\n").unwrap();

        let policy = ResourcePolicy {
            block: vec!["bad-ext".to_owned()],
            ..Default::default()
        };
        policy.save_to_path(&path).unwrap();

        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("theme"), "sibling [ui] should be preserved");
        assert!(body.contains("bad-ext"));
    }

    #[test]
    fn toml_load_partial_table_fills_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[pi.resources]\nallow = [\"npm:foo\"]\n").unwrap();

        let policy = ResourcePolicy::load_from_path(&path);
        assert_eq!(policy.allow, vec!["npm:foo"]);
        assert!(policy.block.is_empty());
        assert!(!policy.disable_heuristics);
    }

    #[test]
    fn sidecar_flat_toml_loads_block_list() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(PROJECT_SIDECAR_FILE);
        std::fs::write(
            &path,
            "# project sidecar\nblock = [\"npm:@heyhuynhgiabuu/pi-pretty\", \"npm:pi-pretty\"]\nallow = []\n",
        )
        .unwrap();

        let policy = ResourcePolicy::load_from_path(&path);
        assert_eq!(
            policy.block,
            vec![
                "npm:@heyhuynhgiabuu/pi-pretty".to_owned(),
                "npm:pi-pretty".to_owned()
            ]
        );
        assert!(policy.allow.is_empty());
        assert!(!policy.disable_heuristics);
    }

    #[test]
    fn merge_partial_unions_lists_and_preserves_heuristics() {
        let mut policy = ResourcePolicy {
            allow: vec!["npm:a".to_owned()],
            block: vec!["npm:b".to_owned()],
            disable_heuristics: true,
            enabled_native_features: Vec::new(),
            forced_native_features: Vec::new(),
        };
        // Layer without disable_heuristics key must not reset the flag.
        policy.merge_partial(&PartialPolicy {
            allow: vec!["npm:c".to_owned()],
            block: vec!["npm:b".to_owned(), "npm:d".to_owned()],
            disable_heuristics: None,
        });
        assert_eq!(policy.allow, vec!["npm:a".to_owned(), "npm:c".to_owned()]);
        assert_eq!(policy.block, vec!["npm:b".to_owned(), "npm:d".to_owned()]);
        assert!(policy.disable_heuristics);

        policy.merge_partial(&PartialPolicy {
            allow: vec![],
            block: vec![],
            disable_heuristics: Some(false),
        });
        assert!(!policy.disable_heuristics);
    }

    #[test]
    fn project_sidecar_blocks_via_user_policy() {
        let catalog = make_catalog(vec![make_resource(
            "/home/user/.pi/agent/extensions/noisy/index.ts",
            "npm:noisy",
            PiResourceType::Extensions,
            true,
        )]);
        let policy = ResourcePolicy {
            block: vec!["npm:noisy".to_owned()],
            ..Default::default()
        };
        let plan = policy.evaluate(&catalog);
        assert!(plan.extensions.is_empty());
        assert!(plan.blocked[0].reason.contains("user policy"));
    }
}
