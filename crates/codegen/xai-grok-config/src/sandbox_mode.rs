use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize};

/// Environment override for the mode, above every config file.
pub const SANDBOX_MODE_ENV: &str = "GROK_SANDBOX_MODE";

/// Rollout mode of the per-command sandbox. Resolved per workspace by [`SandboxMode::resolve`]:
/// `GROK_SANDBOX_MODE`, then the remote rollout switch, then `<grok_home>/workspaced.toml`, then
/// the default (`Off`); the workspace's `.grok/workspaced.toml` may only tighten the result. On a
/// host with no sandbox backend the result is then subject to [`ResolvedSandboxMode::on_host`].
///
/// - `Off` (the default): no wrapper, no proxy env, no decode — the spawn is byte-identical to a
///   build without the sandbox; the per-spawn network filter applies.
/// - `Observe`: behaviour-preserving. The command runs with its usual permissions; the egress
///   proxy allows every host and records what `Enforce` would have asked about. Only network
///   would-be violations are recorded. Never blocks, never re-runs, never prompts.
/// - `Enforce`: wrap, run, decode, ask the session owner, replay under the widened policy.
///
/// The variants are ordered `Off < Observe < Enforce`, which is what "tighten" means. Every
/// spelling is parsed through [`FromStr`], case-insensitively and trimmed (`enforce`, `Enforce`,
/// `ENFORCE`), from the environment, a `workspaced.toml` or the remote settings alike.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Serialize,
    strum::IntoStaticStr,
    strum::EnumString,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case", ascii_case_insensitive)]
pub enum SandboxMode {
    #[default]
    Off,
    Observe,
    Enforce,
}

/// The accepted spellings, for the parse errors; the rejected text itself is never echoed, since
/// it may be anything a file holds.
const EXPECTED_MODES: &str = "expected off, observe or enforce";

/// Through [`FromStr`], so a file or the remote settings accept what the environment accepts.
impl<'de> Deserialize<'de> for SandboxMode {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        SandboxMode::from_str(text.trim()).map_err(|_| serde::de::Error::custom(EXPECTED_MODES))
    }
}

/// Deserialize a [`SandboxMode`], consuming a wrong-typed or unrecognised value as an unset field
/// (with one warning saying whether it was a string, never its text) instead of failing the table.
pub fn optional_sandbox_mode<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<SandboxMode>, D::Error> {
    /// A string names the mode and any other value is skipped: neither fails the settings or
    /// table around it.
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Text(String),
        Other(serde::de::IgnoredAny),
    }
    let (value_type, mode) = match Option::<Raw>::deserialize(deserializer)? {
        None => return Ok(None),
        Some(Raw::Text(text)) => ("string", SandboxMode::from_str(text.trim()).ok()),
        Some(Raw::Other(_)) => ("non-string", None),
    };
    if mode.is_none() {
        tracing::warn!(
            value_type,
            "ignoring invalid sandbox mode; expected off, observe or enforce"
        );
    }
    Ok(mode)
}

impl SandboxMode {
    pub fn is_wrapped(self) -> bool {
        matches!(self, SandboxMode::Enforce)
    }

    /// Whether the proxy environment is injected: observe records through it, enforce routes
    /// through it.
    pub fn uses_proxy(self) -> bool {
        matches!(self, SandboxMode::Observe | SandboxMode::Enforce)
    }

    /// Env over remote over user file over the default, then the workspace file as a
    /// tighten-only layer, on a host with a sandbox backend; the daemon resolves through
    /// [`SandboxMode::resolve_on_host`], which is this where there is one.
    ///
    /// - The env value is a string; one that does not name a mode is logged (never echoed) and
    ///   skipped, so a typo cannot turn the sandbox off (or on) through a lower layer's silence.
    /// - The workspace layer applies only when neither the env nor the remote layer decided: a
    ///   cloned repository's `.grok/workspaced.toml` may raise the mode above the user's, never
    ///   lower it (a lower value is ignored with one warning), and never override a developer's
    ///   env or the fleet's rollout switch. It is read whether or not the folder is trusted: a
    ///   layer that can only tighten needs no trust, and gating it on trust would let a
    ///   repository's own `.envrc` (which makes the folder untrusted) drop the mode from
    ///   `enforce` to the user's default.
    /// - Over the default, a workspace value decides even when it names the default mode, so an
    ///   explicit `off` in the folder file is reported as the folder's choice.
    /// - A file layer that is there but refused or unreadable ([`RefusedLayers`]) counts as
    ///   `enforce`, whatever it holds: a file the daemon cannot trust never lowers the mode. It
    ///   applies where its file would, and only when it raises the mode the readable layers
    ///   resolve to; the mode is then marked [`ModeDegradation::ConfigRefused`], attributed to the
    ///   highest refused layer (the managed file, else the user's). When a readable layer already
    ///   names `enforce`, that layer decided, unmarked: a refusal never wins a tie, so it can
    ///   never lower a readable layer's mode on a host with no backend.
    pub fn resolve(layers: SandboxModeLayers<'_>) -> ResolvedSandboxMode {
        SandboxMode::resolve_on_host(layers, true)
    }

    /// [`SandboxMode::resolve`] with no backend ([`ResolvedSandboxMode::on_host`]): the switch's
    /// `enforce` yields to the user's or folder's own, which refuses; a refused file's (chosen by
    /// no one) leaves the readable layers' mode, marked [`ModeDegradation::ConfigRefused`].
    pub fn resolve_on_host(
        layers: SandboxModeLayers<'_>,
        backend_available: bool,
    ) -> ResolvedSandboxMode {
        let RefusedLayers {
            remote,
            user,
            workspace,
        } = layers.refused;
        let values = SandboxModeLayers {
            remote: layers.remote.filter(|_| !remote),
            user: layers.user.filter(|_| !user),
            workspace: layers.workspace.filter(|_| !workspace),
            ..layers
        };
        let mut readable = SandboxMode::resolve_values(values);
        if !backend_available
            && readable == ResolvedSandboxMode::new(SandboxMode::Enforce, SandboxModeSource::Remote)
            && let Some((own, _)) = [
                (SandboxModeSource::UserConfig, values.user),
                (SandboxModeSource::WorkspaceConfig, values.workspace),
            ]
            .into_iter()
            .find(|(_, mode)| *mode == Some(SandboxMode::Enforce))
        {
            readable = ResolvedSandboxMode::new(SandboxMode::Enforce, own);
        }
        let readable = readable.on_host(backend_available);
        let refusal_applies = match readable.source {
            SandboxModeSource::Env | SandboxModeSource::Remote => false,
            SandboxModeSource::UserConfig
            | SandboxModeSource::WorkspaceConfig
            | SandboxModeSource::Default => remote || user || workspace,
        };
        if !refusal_applies || readable.mode == SandboxMode::Enforce {
            return readable;
        }
        ResolvedSandboxMode {
            mode: if backend_available {
                SandboxMode::Enforce
            } else {
                readable.mode
            },
            source: if remote {
                SandboxModeSource::Remote
            } else if user {
                SandboxModeSource::UserConfig
            } else {
                SandboxModeSource::WorkspaceConfig
            },
            degraded: Some(ModeDegradation::ConfigRefused),
        }
    }

    /// [`SandboxMode::resolve`] over the readable layers' values, a refused layer's taken out.
    fn resolve_values(layers: SandboxModeLayers<'_>) -> ResolvedSandboxMode {
        let env = layers.env.and_then(|value| {
            SandboxMode::from_str(value.trim())
                .inspect_err(|_| {
                    tracing::warn!(
                        source = <&str>::from(SandboxModeSource::Env),
                        "ignoring unrecognised sandbox mode; {EXPECTED_MODES}"
                    )
                })
                .ok()
        });
        let candidates = [
            (SandboxModeSource::Env, env),
            (SandboxModeSource::Remote, layers.remote),
            (SandboxModeSource::UserConfig, layers.user),
        ];
        let effective = candidates
            .into_iter()
            .find_map(|(source, mode)| mode.map(|mode| ResolvedSandboxMode::new(mode, source)))
            .unwrap_or(ResolvedSandboxMode::new(
                SandboxMode::default(),
                SandboxModeSource::Default,
            ));
        let Some(workspace) = layers.workspace else {
            return effective;
        };
        match effective.source {
            SandboxModeSource::Env | SandboxModeSource::Remote => {
                tracing::debug!(
                    requested = <&str>::from(workspace),
                    decided_by = <&str>::from(effective.source),
                    "workspace [sandbox] mode ignored: a higher layer decided"
                );
                effective
            }
            SandboxModeSource::UserConfig | SandboxModeSource::Default
                if workspace < effective.mode =>
            {
                tracing::warn!(
                    requested = <&str>::from(workspace),
                    effective = <&str>::from(effective.mode),
                    "workspace [sandbox] mode ignored: a workspace may only tighten the user's mode"
                );
                effective
            }
            SandboxModeSource::UserConfig if workspace == effective.mode => effective,
            SandboxModeSource::UserConfig | SandboxModeSource::Default => {
                ResolvedSandboxMode::new(workspace, SandboxModeSource::WorkspaceConfig)
            }
            SandboxModeSource::WorkspaceConfig => effective,
        }
    }
}

/// One `[sandbox] mode` value per layer, already read from its source so the precedence is
/// testable without touching the environment or the filesystem. `env` is the raw variable; the
/// file layers are typed because their loaders already parsed them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SandboxModeLayers<'a> {
    pub env: Option<&'a str>,
    /// `RemoteSettings.sandbox_mode`: the fleet's rollout switch, moving every host at once, above
    /// the user's file and below a developer's env. With no backend its `enforce` runs `off` unless
    /// the user's or folder's own names it ([`SandboxMode::resolve_on_host`]).
    pub remote: Option<SandboxMode>,
    pub user: Option<SandboxMode>,
    /// The folder's own `.grok/workspaced.toml`.
    pub workspace: Option<SandboxMode>,
    /// The file layers that are there but were refused or could not be read; their values above
    /// are not used.
    pub refused: RefusedLayers,
}

/// File layers there but refused or unreadable (a symlink into a writable place, not a regular
/// file, another user's, too large, not TOML): each counts as `enforce`, never as absent (the
/// default `off`), so a file the daemon cannot trust never lowers the mode it would have set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RefusedLayers {
    /// The managed config file the remote layer is read from, when it is read from a file.
    pub remote: bool,
    pub user: bool,
    pub workspace: bool,
}

/// Which layer decided the mode, for the daemon log line and the desktop's Settings badge
/// ("set by …").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, strum::IntoStaticStr)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum SandboxModeSource {
    Env,
    Remote,
    WorkspaceConfig,
    UserConfig,
    Default,
}

/// Why the mode is not the one the layers' values name, kept beside the source so
/// `sandbox.status` and the Settings badge can say so.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, strum::IntoStaticStr)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum ModeDegradation {
    /// The rollout switch set `enforce`; this host has no sandbox backend, so it runs `off`.
    EnforceWithoutBackend,
    /// A layer's file that was refused or could not be read ([`RefusedLayers`]) raised the
    /// mode above what the readable layers resolve to: `enforce`, or on a host with no sandbox
    /// backend the readable layers' own mode ([`SandboxMode::resolve_on_host`]).
    ConfigRefused,
}

/// The mode a workspace runs in and where it came from. `degraded` is set when the mode is not
/// the one the layers' values name: a refused layer raised it ([`SandboxMode::resolve`]), or
/// the host runs a lower one ([`ResolvedSandboxMode::on_host`]); it is left out of the
/// serialized form when it is not.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct ResolvedSandboxMode {
    pub mode: SandboxMode,
    pub source: SandboxModeSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub degraded: Option<ModeDegradation>,
}

impl ResolvedSandboxMode {
    /// A mode as its layers resolved it, not degraded.
    pub fn new(mode: SandboxMode, source: SandboxModeSource) -> ResolvedSandboxMode {
        ResolvedSandboxMode {
            mode,
            source,
            degraded: None,
        }
    }

    /// The rule on a host with no backend: the rollout switch's `enforce`, or one a refused file
    /// counts as, runs as `off` ([`ModeDegradation`] says which); an `enforce` the developer chose
    /// is kept and refuses every command, naming the missing backend.
    pub fn on_host(self, backend_available: bool) -> ResolvedSandboxMode {
        if backend_available || self.mode != SandboxMode::Enforce {
            return self;
        }
        if self.degraded == Some(ModeDegradation::ConfigRefused) {
            return ResolvedSandboxMode {
                mode: SandboxMode::Off,
                ..self
            };
        }
        if self.source != SandboxModeSource::Remote {
            return self;
        }
        tracing::warn!(
            requested = <&str>::from(self.mode),
            source = <&str>::from(self.source),
            "the rollout switch asks for enforce but this host has no sandbox backend: running \
             off; a developer's own enforce (GROK_SANDBOX_MODE or workspaced.toml) would refuse \
             every command instead"
        );
        ResolvedSandboxMode {
            mode: SandboxMode::Off,
            source: self.source,
            degraded: Some(ModeDegradation::EnforceWithoutBackend),
        }
    }
}

#[cfg(test)]
#[path = "sandbox_mode_tests.rs"]
mod tests;
