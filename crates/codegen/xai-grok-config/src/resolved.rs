//! A resolved config value tagged with the layer it came from.

use xai_grok_env::env_bool;

/// Where a resolved config value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::Display)]
#[strum(serialize_all = "snake_case")]
pub enum ConfigSource {
    Requirement,
    Cli,
    Env,
    SystemManagedConfig,
    ManagedConfig,
    UserConfig,
    /// A value injected via the `GROK_CONFIG` / `GROK_CONFIG_PATH` overlay.
    EnvOverlay,
    Config,
    Remote,
    Default,
}

/// A resolved config value with its source for diagnostics.
#[derive(Debug, Clone)]
pub struct Resolved<T> {
    pub value: T,
    pub source: ConfigSource,
}

impl<T> Resolved<T> {
    pub fn new(value: T, source: ConfigSource) -> Self {
        Self { value, source }
    }
}

impl<T: std::fmt::Display> std::fmt::Display for Resolved<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.value, self.source)
    }
}

/// Resolve a boolean feature flag; the highest set tier wins: requirement, cli, env, config, managed, feature flag, default.
pub struct BoolFlag {
    requirement: Option<bool>,
    cli: Option<bool>,
    env: Option<bool>,
    config: Option<bool>,
    managed: Option<bool>,
    feature_flag: Option<bool>,
    default: bool,
}

impl BoolFlag {
    pub fn env(env_var: &str) -> Self {
        Self::env_value(env_bool(env_var))
    }

    /// Takes the env tier already read, so a caller handed every tier as data never reads the process environment here.
    pub fn env_value(env: Option<bool>) -> Self {
        Self {
            requirement: None,
            cli: None,
            env,
            config: None,
            managed: None,
            feature_flag: None,
            default: false,
        }
    }

    pub fn requirement(mut self, v: Option<bool>) -> Self {
        self.requirement = v;
        self
    }
    pub fn cli(mut self, v: Option<bool>) -> Self {
        self.cli = v;
        self
    }
    pub fn config(mut self, v: Option<bool>) -> Self {
        self.config = v;
        self
    }
    pub fn managed(mut self, v: Option<bool>) -> Self {
        self.managed = v;
        self
    }
    pub fn feature_flag(mut self, v: Option<bool>) -> Self {
        self.feature_flag = v;
        self
    }
    pub fn default(mut self, v: bool) -> Self {
        self.default = v;
        self
    }

    pub fn resolve(self) -> Resolved<bool> {
        resolve_bool_flag(
            self.requirement,
            self.cli,
            self.env,
            self.config,
            self.managed,
            self.feature_flag,
            self.default,
        )
    }
}

fn resolve_bool_flag(
    requirement: Option<bool>,
    cli_arg: Option<bool>,
    env_val: Option<bool>,
    config_val: Option<bool>,
    managed_val: Option<bool>,
    feature_flag_val: Option<bool>,
    default: bool,
) -> Resolved<bool> {
    if let Some(val) = requirement {
        return Resolved::new(val, ConfigSource::Requirement);
    }
    if let Some(val) = cli_arg {
        return Resolved::new(val, ConfigSource::Cli);
    }
    if let Some(val) = env_val {
        return Resolved::new(val, ConfigSource::Env);
    }
    if let Some(val) = config_val {
        return Resolved::new(val, ConfigSource::Config);
    }
    if let Some(val) = managed_val {
        return Resolved::new(val, ConfigSource::ManagedConfig);
    }
    if let Some(val) = feature_flag_val {
        return Resolved::new(val, ConfigSource::Remote);
    }
    Resolved::new(default, ConfigSource::Default)
}

/// Resolve a string setting: cli > env > config > feature flag.
/// `None` if no source provides a value.
pub fn resolve_string_flag(
    cli_arg: Option<&str>,
    env_var: &str,
    config_val: Option<&str>,
    feature_flag_val: Option<&str>,
) -> Option<Resolved<String>> {
    if let Some(val) = cli_arg.filter(|s| !s.is_empty()) {
        return Some(Resolved::new(val.to_owned(), ConfigSource::Cli));
    }
    if let Some(val) = xai_grok_env::env_string(env_var) {
        return Some(Resolved::new(val, ConfigSource::Env));
    }
    if let Some(val) = config_val.filter(|s| !s.is_empty()) {
        return Some(Resolved::new(val.to_owned(), ConfigSource::Config));
    }
    if let Some(val) = feature_flag_val.filter(|s| !s.is_empty()) {
        return Some(Resolved::new(val.to_owned(), ConfigSource::Remote));
    }
    None
}
