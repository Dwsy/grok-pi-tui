//! Declarative host-feature registration for external agent compositions.
//!
//! A host feature owns the metadata and config binding needed by the pager
//! settings UI plus the startup materialization metadata used by grok-pi.
//! Core Grok does not opt into these specs; an external composition passes a
//! [`HostFeatureManifest`] explicitly.

use crate::agent::config::UiConfig;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostFeatureCategory {
    Agent,
}

#[derive(Debug, Clone, Copy)]
pub struct HostFeatureSpec {
    pub key: HostFeatureKey,
    pub label: &'static str,
    pub description: &'static str,
    pub keywords: &'static [&'static str],
    pub category: HostFeatureCategory,
    pub section: &'static str,
    pub default_enabled: bool,
    pub restart_required: bool,
    /// TOML path used for layered startup resolution. The persisted writer is
    /// the typed UiConfig setter below, so this path is never used to mutate a
    /// raw TOML tree.
    pub config_path: &'static [&'static str],
    /// Key used by `native_feature_conflicts.toml` resource admission.
    pub native_feature_key: Option<&'static str>,
    /// Child-process environment marker set when this feature is enabled.
    pub startup_env: Option<&'static str>,
    /// Optional single-file Pi extension source materialized by the host.
    pub extension_source: Option<&'static str>,
    get_bool: fn(&UiConfig) -> bool,
    set_bool: fn(&mut UiConfig, bool),
}

impl HostFeatureSpec {
    pub fn current_bool(self, ui: &UiConfig) -> bool {
        (self.get_bool)(ui)
    }

    pub fn set_bool(self, ui: &mut UiConfig, value: bool) {
        (self.set_bool)(ui, value);
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
}

#[derive(Debug, Clone, Default)]
pub struct HostFeatureManifest {
    specs: Vec<&'static HostFeatureSpec>,
}

impl HostFeatureManifest {
    pub fn new(specs: impl IntoIterator<Item = &'static HostFeatureSpec>) -> Self {
        let specs: Vec<_> = specs.into_iter().collect();
        let mut seen = std::collections::HashSet::with_capacity(specs.len());
        for spec in &specs {
            assert!(
                seen.insert(spec.key),
                "duplicate host feature key in manifest: {}",
                spec.key.as_str()
            );
        }
        Self { specs }
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
}

pub const PI_WORKFLOWS: HostFeatureKey = HostFeatureKey::new("pi_workflows");
pub const PI_HERDR: HostFeatureKey = HostFeatureKey::new("pi_herdr");
pub const PI_SUBAGENTS: HostFeatureKey = HostFeatureKey::new("pi_subagents");
pub const PI_TODO: HostFeatureKey = HostFeatureKey::new("pi_todo");
pub const PI_GOAL: HostFeatureKey = HostFeatureKey::new("pi_goal");
pub const PI_LOOP: HostFeatureKey = HostFeatureKey::new("pi_loop");
pub const PI_ASK_USER_QUESTION: HostFeatureKey = HostFeatureKey::new("pi_ask_user_question");
pub const PI_BTW: HostFeatureKey = HostFeatureKey::new("pi_btw");

fn pi_workflows_get(ui: &UiConfig) -> bool {
    ui.pi_workflows
}

fn pi_workflows_set(ui: &mut UiConfig, value: bool) {
    ui.pi_workflows = value;
}

pub static PI_WORKFLOWS_SPEC: HostFeatureSpec = HostFeatureSpec {
    key: PI_WORKFLOWS,
    label: "Pi workflows",
    description: "Enable upstream Rhai workflows in grok-pi (tool `workflow`, deep-research, .grok/workflows). Takes effect for new grok-pi sessions.",
    keywords: &[
        "pi",
        "workflow",
        "workflows",
        "rhai",
        "deep-research",
        "agent",
        "orchestration",
    ],
    category: HostFeatureCategory::Agent,
    section: "Pi features",
    default_enabled: false,
    restart_required: true,
    config_path: &["ui", "pi_workflows"],
    native_feature_key: Some("pi_workflows"),
    startup_env: Some("PI_GROK_WORKFLOWS"),
    extension_source: Some(include_str!(
        "../../../../extensions/pi-grok-workflows/index.ts"
    )),
    get_bool: pi_workflows_get,
    set_bool: pi_workflows_set,
};

fn pi_herdr_get(ui: &UiConfig) -> bool {
    ui.pi_herdr
}

fn pi_herdr_set(ui: &mut UiConfig, value: bool) {
    ui.pi_herdr = value;
}

pub static PI_HERDR_SPEC: HostFeatureSpec = HostFeatureSpec {
    key: PI_HERDR,
    label: "Pi Herdr integration",
    description: "Report grok-pi lifecycle and native Pi session state to Herdr. Disabled by default; enable only when Herdr is in use. Takes effect for new grok-pi sessions.",
    keywords: &[
        "pi",
        "herdr",
        "agent",
        "lifecycle",
        "workspace",
        "pane",
        "status",
        "integration",
    ],
    category: HostFeatureCategory::Agent,
    section: "Pi features",
    default_enabled: false,
    restart_required: true,
    config_path: &["ui", "pi_herdr"],
    native_feature_key: None,
    startup_env: None,
    extension_source: None,
    get_bool: pi_herdr_get,
    set_bool: pi_herdr_set,
};

fn pi_subagents_get(ui: &UiConfig) -> bool {
    ui.pi_subagents
}

fn pi_subagents_set(ui: &mut UiConfig, value: bool) {
    ui.pi_subagents = value;
}

pub static PI_SUBAGENTS_SPEC: HostFeatureSpec = HostFeatureSpec {
    key: PI_SUBAGENTS,
    label: "Pi subagents",
    description: "Enable the built-in Pi child-session subagents (`spawn_subagent`, native Tasks/child views). Default on; takes effect for new grok-pi sessions.",
    keywords: &[
        "pi",
        "subagent",
        "subagents",
        "child",
        "agent",
        "spawn_subagent",
        "tasks",
        "delegate",
    ],
    category: HostFeatureCategory::Agent,
    section: "Pi features",
    default_enabled: true,
    restart_required: true,
    config_path: &["ui", "pi_subagents"],
    native_feature_key: Some("pi_subagents"),
    startup_env: None,
    extension_source: None,
    get_bool: pi_subagents_get,
    set_bool: pi_subagents_set,
};

fn pi_todo_get(ui: &UiConfig) -> bool {
    ui.pi_todo
}

fn pi_todo_set(ui: &mut UiConfig, value: bool) {
    ui.pi_todo = value;
}

pub static PI_TODO_SPEC: HostFeatureSpec = HostFeatureSpec {
    key: PI_TODO,
    label: "Pi todo",
    description: "Enable grok-pi's built-in structured `todo` tool and native TodoPane projection. Default on; takes effect for new grok-pi sessions.",
    keywords: &["pi", "todo", "todos", "task", "tasks", "plan", "progress"],
    category: HostFeatureCategory::Agent,
    section: "Pi features",
    default_enabled: true,
    restart_required: true,
    config_path: &["ui", "pi_todo"],
    native_feature_key: Some("pi_todo"),
    startup_env: None,
    extension_source: None,
    get_bool: pi_todo_get,
    set_bool: pi_todo_set,
};

fn pi_goal_get(ui: &UiConfig) -> bool {
    ui.pi_goal
}

fn pi_goal_set(ui: &mut UiConfig, value: bool) {
    ui.pi_goal = value;
}

pub static PI_GOAL_SPEC: HostFeatureSpec = HostFeatureSpec {
    key: PI_GOAL,
    label: "Pi goal mode",
    description: "Enable Grok-style /goal autonomous loop in grok-pi (status bar + update_goal). Takes effect for new grok-pi sessions.",
    keywords: &["pi", "goal", "/goal", "autonomous", "update_goal", "agent"],
    category: HostFeatureCategory::Agent,
    section: "Pi features",
    default_enabled: false,
    restart_required: true,
    config_path: &["ui", "pi_goal"],
    native_feature_key: Some("pi_goal"),
    startup_env: None,
    extension_source: None,
    get_bool: pi_goal_get,
    set_bool: pi_goal_set,
};

fn pi_loop_get(ui: &UiConfig) -> bool {
    ui.pi_loop
}

fn pi_loop_set(ui: &mut UiConfig, value: bool) {
    ui.pi_loop = value;
}

pub static PI_LOOP_SPEC: HostFeatureSpec = HostFeatureSpec {
    key: PI_LOOP,
    label: "Pi /loop scheduler",
    description: "Enable Grok-style /loop recurring prompts in grok-pi (tasks pane + scheduler_create). Takes effect for new grok-pi sessions.",
    keywords: &[
        "pi",
        "loop",
        "/loop",
        "scheduler",
        "cron",
        "recurring",
        "agent",
    ],
    category: HostFeatureCategory::Agent,
    section: "Pi features",
    default_enabled: false,
    restart_required: true,
    config_path: &["ui", "pi_loop"],
    native_feature_key: None,
    startup_env: None,
    extension_source: None,
    get_bool: pi_loop_get,
    set_bool: pi_loop_set,
};

fn pi_ask_user_question_get(ui: &UiConfig) -> bool {
    ui.pi_ask_user_question
}

fn pi_ask_user_question_set(ui: &mut UiConfig, value: bool) {
    ui.pi_ask_user_question = value;
}

pub static PI_ASK_USER_QUESTION_SPEC: HostFeatureSpec = HostFeatureSpec {
    key: PI_ASK_USER_QUESTION,
    label: "Q&A",
    description: "Grok Build asks the right questions to nail the details. Enable native ask_user_question → QuestionView. Takes effect for new grok-pi sessions.",
    keywords: &[
        "pi",
        "qa",
        "q&a",
        "ask",
        "question",
        "ask_user_question",
        "questionnaire",
        "interview",
    ],
    category: HostFeatureCategory::Agent,
    section: "Pi features",
    default_enabled: false,
    restart_required: true,
    config_path: &["ui", "pi_ask_user_question"],
    native_feature_key: Some("pi_ask_user_question"),
    startup_env: None,
    extension_source: None,
    get_bool: pi_ask_user_question_get,
    set_bool: pi_ask_user_question_set,
};

fn pi_btw_get(ui: &UiConfig) -> bool {
    ui.pi_btw
}

fn pi_btw_set(ui: &mut UiConfig, value: bool) {
    ui.pi_btw = value;
}

pub static PI_BTW_SPEC: HostFeatureSpec = HostFeatureSpec {
    key: PI_BTW,
    label: "Pi /btw side questions",
    description: "Enable native /btw side questions in grok-pi (overlay + Pi complete()). Takes effect for new grok-pi sessions.",
    keywords: &["pi", "btw", "side", "question", "side-question", "overlay"],
    category: HostFeatureCategory::Agent,
    section: "Pi features",
    default_enabled: false,
    restart_required: true,
    config_path: &["ui", "pi_btw"],
    native_feature_key: Some("pi_btw"),
    startup_env: None,
    extension_source: None,
    get_bool: pi_btw_get,
    set_bool: pi_btw_set,
};

pub static ALL_HOST_FEATURE_SPECS: &[&HostFeatureSpec] = &[
    &PI_WORKFLOWS_SPEC,
    &PI_HERDR_SPEC,
    &PI_SUBAGENTS_SPEC,
    &PI_TODO_SPEC,
    &PI_GOAL_SPEC,
    &PI_LOOP_SPEC,
    &PI_ASK_USER_QUESTION_SPEC,
    &PI_BTW_SPEC,
];

pub fn feature_spec(key: HostFeatureKey) -> Option<&'static HostFeatureSpec> {
    ALL_HOST_FEATURE_SPECS
        .iter()
        .copied()
        .find(|spec| spec.key == key)
}

pub fn feature_spec_by_setting_key(key: &str) -> Option<&'static HostFeatureSpec> {
    ALL_HOST_FEATURE_SPECS
        .iter()
        .copied()
        .find(|spec| spec.key.as_str() == key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pi_workflows_spec_matches_ui_default_and_path() {
        let ui = UiConfig::default();
        assert!(!PI_WORKFLOWS_SPEC.current_bool(&ui));
        assert!(!PI_WORKFLOWS_SPEC.default_enabled);
        assert_eq!(PI_WORKFLOWS_SPEC.config_path, &["ui", "pi_workflows"]);
        assert_eq!(PI_WORKFLOWS_SPEC.native_feature_key, Some("pi_workflows"));
        assert!(PI_WORKFLOWS_SPEC.extension_source.is_some());
    }

    #[test]
    fn pi_workflows_resolves_layered_toml_with_default_off() {
        let missing = toml::Value::Table(Default::default());
        assert!(!PI_WORKFLOWS_SPEC.resolve_enabled(Some(&missing)));
        let enabled = toml::Value::Table("[ui]\npi_workflows = true\n".parse().unwrap());
        assert!(PI_WORKFLOWS_SPEC.resolve_enabled(Some(&enabled)));
        let disabled = toml::Value::Table("[ui]\npi_workflows = false\n".parse().unwrap());
        assert!(!PI_WORKFLOWS_SPEC.resolve_enabled(Some(&disabled)));
    }

    /// Every declared spec must agree with `UiConfig::default()` and carry a
    /// config path whose leaf equals the feature key — the manifest is the
    /// single source of truth, so silent drift here would desync F2 defaults
    /// from persisted state.
    #[test]
    fn all_specs_match_ui_config_defaults_and_paths() {
        let ui = UiConfig::default();
        for spec in ALL_HOST_FEATURE_SPECS {
            assert_eq!(
                spec.current_bool(&ui),
                spec.default_enabled,
                "host feature `{}` default drifts from UiConfig::default()",
                spec.key.as_str(),
            );
            let last_segment = spec.config_path.last().unwrap_or_else(|| {
                panic!("host feature `{}` has empty config_path", spec.key.as_str())
            });
            assert_eq!(
                *last_segment,
                spec.key.as_str(),
                "host feature `{}` config_path leaf must equal the key",
                spec.key.as_str(),
            );
        }
        assert!(
            ALL_HOST_FEATURE_SPECS.len() >= 8,
            "Pi feature specs went missing from the manifest table"
        );
    }
}
