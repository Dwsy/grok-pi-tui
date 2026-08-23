//! Declarative host-feature registration for external agent compositions.
//!
//! A host feature owns the metadata and config binding needed by the pager
//! settings UI plus the startup materialization metadata used by grok-pi.
//! Core Grok does not opt into these specs; an external composition passes a
//! [`HostFeatureManifest`] explicitly.

use crate::agent::config::UiConfig;
use serde::Deserialize;
use std::sync::{OnceLock, RwLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HostFeatureKey(&'static str);

impl HostFeatureKey {
    pub const fn new(value: &'static str) -> Self {
        Self(value)
    }

    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct HostFeatureEnv {
    pub key: &'static str,
    /// Value pushed when the feature is enabled.
    pub on: &'static str,
    /// Override pushed when disabled so inherited shell values cannot leak
    /// into the Pi child; None leaves the variable untouched.
    pub off: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum HostFeatureCategory {
    Appearance,
    Popups,
    Mouse,
    Editor,
    Agent,
    Privacy,
    Models,
    Session,
    Advanced,
}

#[derive(Debug, Clone, Copy)]
pub struct HostFeatureSpec {
    pub key: HostFeatureKey,
    pub label: &'static str,
    pub description: &'static str,
    pub keywords: &'static [&'static str],
    pub category: HostFeatureCategory,
    pub section: &'static str,
    /// Stable order inside the declared F2 section.
    pub order: i32,
    pub default_enabled: bool,
    pub restart_required: bool,
    /// TOML path used for layered startup resolution.
    pub config_path: &'static [&'static str],
    /// Key used by `native_feature_conflicts.toml` resource admission.
    pub native_feature_key: Option<&'static str>,
    /// Child-process environment variable driven by this toggle.
    pub startup_env: Option<HostFeatureEnv>,
}

/// Cmd+P placement for one real ACP slash command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostPaletteSpec {
    pub command: &'static str,
    pub section: &'static str,
    pub section_order: i32,
    pub order: i32,
    pub label: Option<&'static str>,
    pub shortcut: Option<&'static str>,
    pub source: &'static str,
}

impl HostFeatureSpec {
    pub fn current_bool(self, ui: &UiConfig) -> bool {
        serde_json::to_value(ui)
            .ok()
            .and_then(|value| value.get(self.key.as_str()).cloned())
            .and_then(|value| value.as_bool())
            .unwrap_or(self.default_enabled)
    }

    pub fn set_bool(self, ui: &mut UiConfig, enabled: bool) {
        let mut value = serde_json::to_value(&*ui).expect("UiConfig must serialize");
        let object = value
            .as_object_mut()
            .expect("UiConfig must serialize as an object");
        let previous = object.insert(
            self.key.as_str().to_string(),
            serde_json::Value::Bool(enabled),
        );
        assert!(
            previous.is_some(),
            "unknown UiConfig host-feature key: {}",
            self.key.as_str()
        );
        *ui =
            serde_json::from_value(value).expect("validated host-feature update must deserialize");
    }

    pub fn resolve_enabled(self, root: Option<&toml::Value>) -> bool {
        let Some(mut value) = root else {
            return self.default_enabled;
        };
        for segment in self.config_path {
            let Some(next) = value.get(*segment) else {
                return self.default_enabled;
            };
            value = next;
        }
        value.as_bool().unwrap_or(self.default_enabled)
    }

    /// `(key, value)` child-env override for the given state. Returns None
    /// when the feature binds no env or the disabled state carries no override.
    pub fn startup_env_override(self, enabled: bool) -> Option<(&'static str, &'static str)> {
        let env = self.startup_env?;
        if enabled {
            Some((env.key, env.on))
        } else {
            Some((env.key, env.off?))
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct HostFeatureManifest {
    specs: Vec<&'static HostFeatureSpec>,
    palette: Vec<HostPaletteSpec>,
}

impl HostFeatureManifest {
    pub fn new(specs: impl IntoIterator<Item = &'static HostFeatureSpec>) -> Self {
        Self::with_palette(specs.into_iter().collect(), Vec::new())
    }

    fn with_palette(
        mut specs: Vec<&'static HostFeatureSpec>,
        mut palette: Vec<HostPaletteSpec>,
    ) -> Self {
        specs.sort_by_key(|spec| (spec.category, spec.section, spec.order, spec.key.as_str()));
        let mut seen = std::collections::HashSet::with_capacity(specs.len());
        for spec in &specs {
            assert!(
                seen.insert(spec.key),
                "duplicate host feature key in manifest: {}",
                spec.key.as_str()
            );
        }
        let mut registered = specs_registry()
            .write()
            .expect("host feature registry poisoned");
        for spec in &specs {
            if !registered.iter().any(|existing| existing.key == spec.key) {
                registered.push(*spec);
            }
        }
        drop(registered);
        palette.sort_by_key(|entry| {
            (
                entry.section_order,
                entry.section,
                entry.order,
                entry.command,
            )
        });
        Self { specs, palette }
    }

    /// Parse extension-owned grok-pi JSON descriptors. All UI registration
    /// metadata is process-lifetime data because the Pager registry stores
    /// `&'static str` for its immutable metadata catalog.
    pub fn from_json_sources(sources: &[(&str, &str)]) -> Result<Self, String> {
        let mut specs = Vec::new();
        let mut palette = Vec::new();
        for (source, json) in sources {
            let document: RawHostUiDocument = serde_json::from_str(json)
                .map_err(|err| format!("{source}: invalid grok-pi.json: {err}"))?;
            if document.schema_version != 1 {
                return Err(format!(
                    "{source}: unsupported schemaVersion {}",
                    document.schema_version
                ));
            }
            for raw in document.settings {
                specs.push(raw.into_spec(source)?);
            }
            for raw in document.palette {
                palette.push(raw.into_spec(source)?);
            }
        }
        let mut seen = std::collections::HashSet::new();
        for spec in &specs {
            if !seen.insert(spec.key) {
                return Err(format!("duplicate host feature key: {}", spec.key.as_str()));
            }
        }
        let mut seen_palette = std::collections::HashSet::new();
        for entry in &palette {
            if !seen_palette.insert(entry.command) {
                return Err(format!("duplicate palette command: {}", entry.command));
            }
        }
        Ok(Self::with_palette(specs, palette))
    }

    pub fn iter(&self) -> impl Iterator<Item = &'static HostFeatureSpec> + '_ {
        self.specs.iter().copied()
    }

    pub fn find(&self, key: HostFeatureKey) -> Option<&'static HostFeatureSpec> {
        self.iter().find(|spec| spec.key == key)
    }

    pub fn contains(&self, key: HostFeatureKey) -> bool {
        self.find(key).is_some()
    }

    pub fn palette(&self) -> &[HostPaletteSpec] {
        &self.palette
    }

    pub fn validate_palette_targets<'a>(
        &self,
        commands: impl IntoIterator<Item = &'a str>,
    ) -> Result<(), String> {
        let commands = commands.into_iter().collect::<std::collections::HashSet<_>>();
        for entry in &self.palette {
            if !commands.contains(entry.command) {
                return Err(format!(
                    "{}: palette target /{} is not available from ACP",
                    entry.source, entry.command
                ));
            }
        }
        Ok(())
    }
}

pub const PI_WORKFLOWS: HostFeatureKey = HostFeatureKey::new("pi_workflows");
pub const PI_HERDR: HostFeatureKey = HostFeatureKey::new("pi_herdr");
pub const PI_SUBAGENTS: HostFeatureKey = HostFeatureKey::new("pi_subagents");
pub const PI_SUBAGENTS_V2: HostFeatureKey = HostFeatureKey::new("pi_subagents_v2");
pub const PI_TODO: HostFeatureKey = HostFeatureKey::new("pi_todo");
pub const PI_TODO_V2: HostFeatureKey = HostFeatureKey::new("pi_todo_v2");
pub const PI_GOAL: HostFeatureKey = HostFeatureKey::new("pi_goal");
pub const PI_LOOP: HostFeatureKey = HostFeatureKey::new("pi_loop");
pub const PI_ASK_USER_QUESTION: HostFeatureKey = HostFeatureKey::new("pi_ask_user_question");
pub const PI_BTW: HostFeatureKey = HostFeatureKey::new("pi_btw");

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawHostUiDocument {
    schema_version: u32,
    #[serde(default)]
    settings: Vec<RawHostFeature>,
    #[serde(default)]
    palette: Vec<RawPaletteEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawHostFeature {
    key: String,
    label: String,
    description: String,
    #[serde(default)]
    keywords: Vec<String>,
    f2: RawF2Placement,
    kind: String,
    default: bool,
    restart_required: bool,
    config_path: Vec<String>,
    native_feature_key: Option<String>,
    startup_env: Option<RawHostFeatureEnv>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawF2Placement {
    category: String,
    section: String,
    order: i32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawHostFeatureEnv {
    key: String,
    on: String,
    off: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawPaletteEntry {
    command: String,
    section: String,
    section_order: i32,
    order: i32,
    label: Option<String>,
    shortcut: Option<String>,
}

fn leak_string(value: String) -> &'static str {
    Box::leak(value.into_boxed_str())
}

fn leak_strings(values: Vec<String>) -> &'static [&'static str] {
    Box::leak(
        values
            .into_iter()
            .map(leak_string)
            .collect::<Vec<_>>()
            .into_boxed_slice(),
    )
}

impl RawHostFeature {
    fn into_spec(self, source: &str) -> Result<&'static HostFeatureSpec, String> {
        let key = self.key.trim();
        if key.is_empty() {
            return Err(format!("{source}: setting key must not be empty"));
        }
        let category = match self.f2.category.as_str() {
            "appearance" => HostFeatureCategory::Appearance,
            "popups" => HostFeatureCategory::Popups,
            "mouse" => HostFeatureCategory::Mouse,
            "editor" => HostFeatureCategory::Editor,
            "agent" => HostFeatureCategory::Agent,
            "privacy" => HostFeatureCategory::Privacy,
            "models" => HostFeatureCategory::Models,
            "session" => HostFeatureCategory::Session,
            "advanced" => HostFeatureCategory::Advanced,
            other => {
                return Err(format!(
                    "{source}: {key}: unsupported F2 category `{other}`"
                ));
            }
        };
        if self.f2.section.trim().is_empty() || self.f2.order < 0 {
            return Err(format!("{source}: {key}: invalid F2 section/order"));
        }
        if self.kind != "bool" {
            return Err(format!(
                "{source}: {key}: unsupported setting kind `{}` (expected `bool`)",
                self.kind
            ));
        }
        if self.config_path.as_slice() != ["ui", key] {
            return Err(format!(
                "{source}: {key}: configPath must be [\"ui\", \"{key}\"]"
            ));
        }
        let default_ui = serde_json::to_value(UiConfig::default())
            .map_err(|err| format!("serialize UiConfig defaults: {err}"))?;
        let Some(ui_default) = default_ui.get(key).and_then(serde_json::Value::as_bool) else {
            return Err(format!("{source}: {key}: not a boolean UiConfig field"));
        };
        if ui_default != self.default {
            return Err(format!(
                "{source}: {key}: JSON default {} != UiConfig default {ui_default}",
                self.default
            ));
        }

        let key = leak_string(key.to_string());
        let startup_env = self.startup_env.map(|env| HostFeatureEnv {
            key: leak_string(env.key),
            on: leak_string(env.on),
            off: env.off.map(leak_string),
        });
        let spec = HostFeatureSpec {
            key: HostFeatureKey::new(key),
            label: leak_string(self.label),
            description: leak_string(self.description),
            keywords: leak_strings(self.keywords),
            category,
            section: leak_string(self.f2.section),
            order: self.f2.order,
            default_enabled: self.default,
            restart_required: self.restart_required,
            config_path: leak_strings(self.config_path),
            native_feature_key: self.native_feature_key.map(leak_string),
            startup_env,
        };
        Ok(Box::leak(Box::new(spec)))
    }
}

impl RawPaletteEntry {
    fn into_spec(self, source: &str) -> Result<HostPaletteSpec, String> {
        let command = self.command.trim().trim_start_matches('/');
        if command.is_empty() || command.chars().any(char::is_whitespace) {
            return Err(format!(
                "{source}: invalid palette command `{}`",
                self.command
            ));
        }
        if self.section.trim().is_empty() || self.section_order < 0 || self.order < 0 {
            return Err(format!(
                "{source}: /{command}: invalid palette section/order"
            ));
        }
        Ok(HostPaletteSpec {
            command: leak_string(command.to_string()),
            section: leak_string(self.section),
            section_order: self.section_order,
            order: self.order,
            label: self.label.map(leak_string),
            shortcut: self.shortcut.map(leak_string),
            source: leak_string(source.to_string()),
        })
    }
}

fn specs_registry() -> &'static RwLock<Vec<&'static HostFeatureSpec>> {
    static SPECS: OnceLock<RwLock<Vec<&'static HostFeatureSpec>>> = OnceLock::new();
    SPECS.get_or_init(|| RwLock::new(Vec::new()))
}

pub fn feature_spec(key: HostFeatureKey) -> Option<&'static HostFeatureSpec> {
    specs_registry()
        .read()
        .expect("host feature registry poisoned")
        .iter()
        .copied()
        .find(|spec| spec.key == key)
}

pub fn feature_spec_by_setting_key(key: &str) -> Option<&'static HostFeatureSpec> {
    specs_registry()
        .read()
        .expect("host feature registry poisoned")
        .iter()
        .copied()
        .find(|spec| spec.key.as_str() == key)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = r#"{
      "schemaVersion": 1,
      "settings": [{
        "key": "pi_workflows",
        "label": "Pi workflows",
        "description": "Workflows",
        "keywords": ["pi", "workflow"],
        "f2": {"category": "agent", "section": "Pi features", "order": 10},
        "kind": "bool",
        "default": false,
        "restartRequired": true,
        "configPath": ["ui", "pi_workflows"],
        "nativeFeatureKey": "pi_workflows",
        "startupEnv": {"key": "PI_GROK_WORKFLOWS", "on": "1"}
      }],
      "palette": [{"command": "workflow", "section": "Pi / Extensions", "sectionOrder": 900, "order": 10}]
    }"#;

    #[test]
    fn parses_extension_owned_ui_registration() {
        let manifest = HostFeatureManifest::from_json_sources(&[("test/grok-pi.json", VALID)])
            .expect("manifest");
        let spec = manifest.find(PI_WORKFLOWS).expect("workflow setting");
        assert_eq!(spec.section, "Pi features");
        assert_eq!(spec.order, 10);
        assert_eq!(
            spec.current_bool(&UiConfig::default()),
            spec.default_enabled
        );
        assert_eq!(spec.config_path, &["ui", "pi_workflows"]);
        assert_eq!(spec.native_feature_key, Some("pi_workflows"));
        assert_eq!(
            spec.startup_env_override(true),
            Some(("PI_GROK_WORKFLOWS", "1"))
        );
        assert_eq!(manifest.palette()[0].command, "workflow");
        manifest
            .validate_palette_targets(["workflow"])
            .expect("declared palette target exists");
        let err = manifest
            .validate_palette_targets(["other"])
            .expect_err("missing target must fail fast");
        assert!(err.contains("test/grok-pi.json"));
        assert!(err.contains("/workflow"));
    }

    #[test]
    fn generic_bool_binding_updates_ui_config() {
        let manifest = HostFeatureManifest::from_json_sources(&[("test/grok-pi.json", VALID)])
            .expect("manifest");
        let spec = manifest.find(PI_WORKFLOWS).expect("workflow setting");
        let mut ui = UiConfig::default();
        spec.set_bool(&mut ui, true);
        assert!(ui.pi_workflows);
        assert!(spec.current_bool(&ui));
    }

    #[test]
    fn rejects_bad_coordinate_and_default_drift() {
        let bad_coordinate = VALID.replace("\"order\": 10", "\"order\": -1");
        assert!(HostFeatureManifest::from_json_sources(&[("bad.json", &bad_coordinate)]).is_err());

        let bad_default = VALID.replace("\"default\": false", "\"default\": true");
        let err = HostFeatureManifest::from_json_sources(&[("bad.json", &bad_default)])
            .expect_err("default drift must fail");
        assert!(err.contains("UiConfig default"));

        let bad_kind = VALID.replace("\"kind\": \"bool\"", "\"kind\": \"string\"");
        let err = HostFeatureManifest::from_json_sources(&[("bad.json", &bad_kind)])
            .expect_err("unsupported kind must fail");
        assert!(err.contains("unsupported setting kind"));
    }
}
