//! Vendor compatibility configuration for third-party agent surfaces
//! (skills, rules, agents, MCPs, hooks, sessions).
//!
//! This module owns the canonical cell registry used by runtime resolution and diagnostics
//! (env var → config TOML → remote setting → default ON).
//!
//! Two forms:
//! - [`CompatConfigToml`] — as parsed from the `[compat]` TOML section. Each
//!   cell is `Option<bool>` so `None` falls through to the resolution chain.
//! - [`CompatConfig`] — resolved plain bools consumed at runtime. Every cell
//!   defaults on.
//!
//! The session config, `grok inspect`, the session picker, and any other process that loads
//! vendor settings resolve through this module, one cell at a time.

use serde::{Deserialize, Serialize};

use crate::{ConfigSource, RemoteSettings, Resolved};

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::AsRefStr, strum::IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub enum CompatVendor {
    Cursor,
    Claude,
    Codex,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::AsRefStr, strum::IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub enum CompatSurface {
    Skills,
    Rules,
    Agents,
    Mcps,
    Hooks,
    Sessions,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompatRemoteKey {
    CursorSkills,
    CursorRules,
    CursorAgents,
    CursorMcps,
    CursorHooks,
    CursorSessions,
    ClaudeSkills,
    ClaudeRules,
    ClaudeAgents,
    ClaudeMcps,
    ClaudeHooks,
    ClaudeSessions,
    CodexSessions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompatCell {
    vendor: CompatVendor,
    surface: CompatSurface,
    env_var: &'static str,
    remote_key: Option<CompatRemoteKey>,
}

impl CompatCell {
    const fn new(
        vendor: CompatVendor,
        surface: CompatSurface,
        env_var: &'static str,
        remote_key: Option<CompatRemoteKey>,
    ) -> Self {
        Self {
            vendor,
            surface,
            env_var,
            remote_key,
        }
    }

    pub const fn vendor(self) -> CompatVendor {
        self.vendor
    }

    pub const fn surface(self) -> CompatSurface {
        self.surface
    }

    pub const fn env_var(self) -> &'static str {
        self.env_var
    }

    pub const fn remote_key(self) -> Option<CompatRemoteKey> {
        self.remote_key
    }

    /// Whether Grok currently implements this compatibility surface. Codex non-session cells remain
    /// reserved in the registry so their config shape is stable, but runtime discovery does not
    /// consume them.
    pub const fn is_runtime_supported(self) -> bool {
        match self.vendor {
            CompatVendor::Cursor | CompatVendor::Claude => true,
            CompatVendor::Codex => matches!(self.surface, CompatSurface::Sessions),
        }
    }
}

pub const COMPAT_CELLS: [CompatCell; 18] = [
    CompatCell::new(
        CompatVendor::Cursor,
        CompatSurface::Skills,
        "GROK_CURSOR_SKILLS_ENABLED",
        Some(CompatRemoteKey::CursorSkills),
    ),
    CompatCell::new(
        CompatVendor::Cursor,
        CompatSurface::Rules,
        "GROK_CURSOR_RULES_ENABLED",
        Some(CompatRemoteKey::CursorRules),
    ),
    CompatCell::new(
        CompatVendor::Cursor,
        CompatSurface::Agents,
        "GROK_CURSOR_AGENTS_ENABLED",
        Some(CompatRemoteKey::CursorAgents),
    ),
    CompatCell::new(
        CompatVendor::Cursor,
        CompatSurface::Mcps,
        "GROK_CURSOR_MCPS_ENABLED",
        Some(CompatRemoteKey::CursorMcps),
    ),
    CompatCell::new(
        CompatVendor::Cursor,
        CompatSurface::Hooks,
        "GROK_CURSOR_HOOKS_ENABLED",
        Some(CompatRemoteKey::CursorHooks),
    ),
    CompatCell::new(
        CompatVendor::Cursor,
        CompatSurface::Sessions,
        "GROK_CURSOR_SESSIONS_ENABLED",
        Some(CompatRemoteKey::CursorSessions),
    ),
    CompatCell::new(
        CompatVendor::Claude,
        CompatSurface::Skills,
        "GROK_CLAUDE_SKILLS_ENABLED",
        Some(CompatRemoteKey::ClaudeSkills),
    ),
    CompatCell::new(
        CompatVendor::Claude,
        CompatSurface::Rules,
        "GROK_CLAUDE_RULES_ENABLED",
        Some(CompatRemoteKey::ClaudeRules),
    ),
    CompatCell::new(
        CompatVendor::Claude,
        CompatSurface::Agents,
        "GROK_CLAUDE_AGENTS_ENABLED",
        Some(CompatRemoteKey::ClaudeAgents),
    ),
    CompatCell::new(
        CompatVendor::Claude,
        CompatSurface::Mcps,
        "GROK_CLAUDE_MCPS_ENABLED",
        Some(CompatRemoteKey::ClaudeMcps),
    ),
    CompatCell::new(
        CompatVendor::Claude,
        CompatSurface::Hooks,
        "GROK_CLAUDE_HOOKS_ENABLED",
        Some(CompatRemoteKey::ClaudeHooks),
    ),
    CompatCell::new(
        CompatVendor::Claude,
        CompatSurface::Sessions,
        "GROK_CLAUDE_SESSIONS_ENABLED",
        Some(CompatRemoteKey::ClaudeSessions),
    ),
    CompatCell::new(
        CompatVendor::Codex,
        CompatSurface::Skills,
        "GROK_CODEX_SKILLS_ENABLED",
        None,
    ),
    CompatCell::new(
        CompatVendor::Codex,
        CompatSurface::Rules,
        "GROK_CODEX_RULES_ENABLED",
        None,
    ),
    CompatCell::new(
        CompatVendor::Codex,
        CompatSurface::Agents,
        "GROK_CODEX_AGENTS_ENABLED",
        None,
    ),
    CompatCell::new(
        CompatVendor::Codex,
        CompatSurface::Mcps,
        "GROK_CODEX_MCPS_ENABLED",
        None,
    ),
    CompatCell::new(
        CompatVendor::Codex,
        CompatSurface::Hooks,
        "GROK_CODEX_HOOKS_ENABLED",
        None,
    ),
    CompatCell::new(
        CompatVendor::Codex,
        CompatSurface::Sessions,
        "GROK_CODEX_SESSIONS_ENABLED",
        Some(CompatRemoteKey::CodexSessions),
    ),
];

/// Per-vendor compat cells as parsed from `[compat.<vendor>]` TOML.
///
/// Resolution order is env override, this value, remote flag, default ON.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct VendorCompatToml {
    pub skills: Option<bool>,
    pub rules: Option<bool>,
    pub agents: Option<bool>,
    pub mcps: Option<bool>,
    pub hooks: Option<bool>,
    pub sessions: Option<bool>,
}

impl VendorCompatToml {
    fn value(&self, surface: CompatSurface) -> Option<bool> {
        match surface {
            CompatSurface::Skills => self.skills,
            CompatSurface::Rules => self.rules,
            CompatSurface::Agents => self.agents,
            CompatSurface::Mcps => self.mcps,
            CompatSurface::Hooks => self.hooks,
            CompatSurface::Sessions => self.sessions,
        }
    }
}

/// The `[compat]` TOML section as parsed.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CompatConfigToml {
    #[serde(default)]
    pub cursor: VendorCompatToml,
    #[serde(default)]
    pub claude: VendorCompatToml,
    #[serde(default)]
    pub codex: VendorCompatToml,
}

impl CompatConfigToml {
    pub fn value(&self, cell: CompatCell) -> Option<bool> {
        match cell.vendor() {
            CompatVendor::Cursor => self.cursor.value(cell.surface()),
            CompatVendor::Claude => self.claude.value(cell.surface()),
            CompatVendor::Codex => self.codex.value(cell.surface()),
        }
    }
}

/// Resolved per-vendor compat cells. Plain bools — the runtime source of truth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VendorCompat {
    pub skills: bool,
    pub rules: bool,
    pub agents: bool,
    pub mcps: bool,
    pub hooks: bool,
    pub sessions: bool,
}

impl VendorCompat {
    fn value(&self, surface: CompatSurface) -> bool {
        match surface {
            CompatSurface::Skills => self.skills,
            CompatSurface::Rules => self.rules,
            CompatSurface::Agents => self.agents,
            CompatSurface::Mcps => self.mcps,
            CompatSurface::Hooks => self.hooks,
            CompatSurface::Sessions => self.sessions,
        }
    }

    fn set(&mut self, surface: CompatSurface, value: bool) {
        match surface {
            CompatSurface::Skills => self.skills = value,
            CompatSurface::Rules => self.rules = value,
            CompatSurface::Agents => self.agents = value,
            CompatSurface::Mcps => self.mcps = value,
            CompatSurface::Hooks => self.hooks = value,
            CompatSurface::Sessions => self.sessions = value,
        }
    }
}

impl Default for VendorCompat {
    fn default() -> Self {
        Self {
            skills: true,
            rules: true,
            agents: true,
            mcps: true,
            hooks: true,
            sessions: true,
        }
    }
}

/// Bare file names, no path separators: read_file matches them against `Path::file_name()`; `agent_filenames()` prepends them to the vendor-gated `.claude/` paths.
pub const INSTRUCTION_FILENAMES: &[&str] = &[
    "Agents.md",
    "Claude.md",
    "CLAUDE.md",
    "CLAUDE.local.md",
    "AGENT.md",
    "AGENTS.md",
];

/// The resolved hook cell of each vendor whose hooks discovery loads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompatHooks {
    pub cursor: bool,
    pub claude: bool,
}

/// The resolved session cell of each vendor whose sessions the picker lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompatSessions {
    pub cursor: bool,
    pub claude: bool,
    pub codex: bool,
}

/// The `GROK_<VENDOR>_<SURFACE>_ENABLED` values set in the environment, which outrank every other
/// source of their cell.
#[derive(Debug, Clone, Default)]
pub struct CompatEnv {
    values: Vec<(CompatCell, bool)>,
}

impl CompatEnv {
    pub fn from_process() -> CompatEnv {
        CompatEnv::read(crate::env_bool)
    }

    pub fn read(lookup: impl Fn(&str) -> Option<bool>) -> CompatEnv {
        let values = COMPAT_CELLS
            .into_iter()
            .filter_map(|cell| Some((cell, lookup(cell.env_var())?)))
            .collect();
        CompatEnv { values }
    }

    fn value(&self, cell: CompatCell) -> Option<bool> {
        self.values
            .iter()
            .find_map(|(set, value)| (*set == cell).then_some(*value))
    }
}

/// Resolved `[compat]` configuration threaded into compatibility consumers. Every cell defaults on.
/// Codex's non-session cells are reserved and are not consumed by discovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CompatConfig {
    pub cursor: VendorCompat,
    pub claude: VendorCompat,
    pub codex: VendorCompat,
}

impl CompatConfig {
    pub fn value(&self, cell: CompatCell) -> bool {
        match cell.vendor() {
            CompatVendor::Cursor => self.cursor.value(cell.surface()),
            CompatVendor::Claude => self.claude.value(cell.surface()),
            CompatVendor::Codex => self.codex.value(cell.surface()),
        }
    }

    pub fn hooks(&self) -> CompatHooks {
        CompatHooks {
            cursor: self.cursor.hooks,
            claude: self.claude.hooks,
        }
    }

    fn sessions(&self) -> CompatSessions {
        CompatSessions {
            cursor: self.cursor.sessions,
            claude: self.claude.sessions,
            codex: self.codex.sessions,
        }
    }

    pub fn set(&mut self, cell: CompatCell, value: bool) {
        match cell.vendor() {
            CompatVendor::Cursor => self.cursor.set(cell.surface(), value),
            CompatVendor::Claude => self.claude.set(cell.surface(), value),
            CompatVendor::Codex => self.codex.set(cell.surface(), value),
        }
    }

    /// Config directories that may contain `skills/` subdirectories, in priority order. `.grok` and `.agents` are always included; `.claude` and
    /// `.cursor` are gated on their respective `skills` cell.
    pub fn skill_config_dirs(&self) -> Vec<&'static str> {
        let mut dirs = vec![".grok", ".agents"];
        if self.claude.skills {
            dirs.push(".claude");
        }
        if self.cursor.skills {
            dirs.push(".cursor");
        }
        dirs
    }

    /// Subdirectories scanned for `*.md` rules files. `.grok/rules` is always included;
    /// `.claude/rules` and `.cursor/rules` are gated on their respective `rules` cell.
    pub fn rules_dirs(&self) -> Vec<&'static str> {
        let mut dirs = vec![".grok/rules"];
        if self.claude.rules {
            dirs.push(".claude/rules");
        }
        if self.cursor.rules {
            dirs.push(".cursor/rules");
        }
        dirs
    }

    /// Filenames (and relative paths) recognized as project-instruction files. The generic names are always included; the
    /// `.claude/`-prefixed entries are gated on `claude.agents`.
    pub fn agent_filenames(&self) -> Vec<&'static str> {
        let mut names = INSTRUCTION_FILENAMES.to_vec();
        if self.claude.agents {
            names.push(".claude/CLAUDE.md");
            names.push(".claude/CLAUDE.local.md");
        }
        names
    }

    /// Home-level vendor directories scanned for AGENTS.md / rules files (e.g. `~/.claude`, `~/.cursor`). `.claude` is
    /// gated on `claude.agents` and `.cursor` on `cursor.agents`.
    pub fn agents_home_dirs(&self) -> Vec<&'static str> {
        let mut dirs = Vec::new();
        if self.claude.agents {
            dirs.push(".claude");
        }
        if self.cursor.agents {
            dirs.push(".cursor");
        }
        dirs
    }
}

pub fn resolve_compat_cell_with_env(
    env: Option<bool>,
    configured: Option<bool>,
    remote: Option<bool>,
    default: bool,
) -> Resolved<bool> {
    if let Some(value) = env {
        Resolved::new(value, ConfigSource::Env)
    } else if let Some(value) = configured {
        Resolved::new(value, ConfigSource::Config)
    } else if let Some(value) = remote {
        Resolved::new(value, ConfigSource::Remote)
    } else {
        Resolved::new(default, ConfigSource::Default)
    }
}

fn remote_compat_value(
    remote: Option<&RemoteSettings>,
    key: Option<CompatRemoteKey>,
) -> Option<bool> {
    let remote = remote?;
    match key? {
        CompatRemoteKey::CursorSkills => remote.cursor_skills_enabled,
        CompatRemoteKey::CursorRules => remote.cursor_rules_enabled,
        CompatRemoteKey::CursorAgents => remote.cursor_agents_enabled,
        CompatRemoteKey::CursorMcps => remote.cursor_mcps_enabled,
        CompatRemoteKey::CursorHooks => remote.cursor_hooks_enabled,
        CompatRemoteKey::CursorSessions => remote.cursor_sessions_enabled,
        CompatRemoteKey::ClaudeSkills => remote.claude_skills_enabled,
        CompatRemoteKey::ClaudeRules => remote.claude_rules_enabled,
        CompatRemoteKey::ClaudeAgents => remote.claude_agents_enabled,
        CompatRemoteKey::ClaudeMcps => remote.claude_mcps_enabled,
        CompatRemoteKey::ClaudeHooks => remote.claude_hooks_enabled,
        CompatRemoteKey::ClaudeSessions => remote.claude_sessions_enabled,
        CompatRemoteKey::CodexSessions => remote.codex_sessions_enabled,
    }
}

pub fn resolve_compat_config(
    config: &CompatConfigToml,
    env: &CompatEnv,
    remote: Option<&RemoteSettings>,
) -> CompatConfig {
    resolve_compat_cells(|cell| config.value(cell), env, remote)
}

fn resolve_compat_cells(
    configured: impl Fn(CompatCell) -> Option<bool>,
    env: &CompatEnv,
    remote: Option<&RemoteSettings>,
) -> CompatConfig {
    let defaults = CompatConfig::default();
    let mut resolved = defaults;
    for cell in COMPAT_CELLS {
        resolved.set(
            cell,
            resolve_compat_cell_with_env(
                env.value(cell),
                configured(cell),
                remote_compat_value(remote, cell.remote_key()),
                defaults.value(cell),
            )
            .value,
        );
    }
    resolved
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompatConfigCellError {
    Unavailable,
    Malformed,
}

/// One `[compat.<vendor>] <surface>` value, read on its own so a malformed sibling cell cannot erase it.
/// `None` for `effective_config` means it could not be loaded.
pub fn compat_config_cell(
    effective_config: Option<&toml::Value>,
    cell: CompatCell,
) -> Result<Option<bool>, CompatConfigCellError> {
    let effective_config = effective_config.ok_or(CompatConfigCellError::Unavailable)?;
    let Some(compat) = effective_config.get("compat") else {
        return Ok(None);
    };
    let compat = compat.as_table().ok_or(CompatConfigCellError::Malformed)?;
    let Some(vendor) = compat.get(cell.vendor().as_ref()) else {
        return Ok(None);
    };
    let vendor = vendor.as_table().ok_or(CompatConfigCellError::Malformed)?;
    let Some(value) = vendor.get(cell.surface().as_ref()) else {
        return Ok(None);
    };
    value
        .as_bool()
        .map(Some)
        .ok_or(CompatConfigCellError::Malformed)
}

/// What a surface's cell falls back to when its `[compat]` value cannot be read.
#[derive(Debug, Clone, Copy)]
enum UnreadableCellPolicy {
    /// Fail closed: the cell is off unless the env turns it on.
    Disable,
    /// Treat the cell as unset, so the remote setting or the default decides.
    TreatAsUnset,
}

/// The `[compat]` value of each `surface` cell in `effective_config`, read one cell at a time.
fn configured_surface(
    effective_config: Option<&toml::Value>,
    surface: CompatSurface,
    policy: UnreadableCellPolicy,
) -> impl Fn(CompatCell) -> Option<bool> {
    move |cell| {
        if cell.surface() != surface {
            return None;
        }
        compat_config_cell(effective_config, cell).unwrap_or_else(|error| match policy {
            UnreadableCellPolicy::Disable => {
                tracing::warn!(
                    vendor = cell.vendor().as_ref(),
                    surface = surface.as_ref(),
                    ?error,
                    "invalid compat config; disabling the cell"
                );
                Some(false)
            }
            UnreadableCellPolicy::TreatAsUnset => {
                tracing::warn!(
                    vendor = cell.vendor().as_ref(),
                    surface = surface.as_ref(),
                    ?error,
                    "invalid compat config; treating the cell as unset"
                );
                None
            }
        })
    }
}

/// The session picker's cells, each read independently; an unreadable one disables that vendor's sessions.
pub fn resolve_compat_sessions(
    effective_config: Option<&toml::Value>,
    env: &CompatEnv,
    remote: Option<&RemoteSettings>,
) -> CompatSessions {
    let configured = configured_surface(
        effective_config,
        CompatSurface::Sessions,
        UnreadableCellPolicy::Disable,
    );
    resolve_compat_cells(configured, env, remote).sessions()
}

/// The hook cells for a loader with no session config; an unreadable one is treated as unset.
pub fn resolve_compat_hooks(
    effective_config: Option<&toml::Value>,
    env: &CompatEnv,
    remote: Option<&RemoteSettings>,
) -> CompatHooks {
    let configured = configured_surface(
        effective_config,
        CompatSurface::Hooks,
        UnreadableCellPolicy::TreatAsUnset,
    );
    resolve_compat_cells(configured, env, remote).hooks()
}

#[cfg(test)]
#[path = "compat_tests.rs"]
mod tests;
