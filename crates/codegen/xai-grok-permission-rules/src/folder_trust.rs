use toml::Value as TomlValue;

use xai_grok_config::{BoolFlag, RemoteSettings};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustLevel {
    Trusted,
    Untrusted,
}

impl TrustLevel {
    #[must_use]
    pub fn is_trusted(self) -> bool {
        matches!(self, Self::Trusted)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustOutcome {
    Trusted,
    Untrusted,
    Prompt,
}

#[derive(Debug, Clone, Copy)]
pub struct DecideInputs {
    pub store_trusted: bool,
    pub repo_configs_present: bool,
    pub is_interactive: bool,
    /// False for the home directory, the filesystem root, and relative paths.
    pub key_recordable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptPolicy {
    MayPrompt,
    FailClosed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustDurability {
    Durable,
    Provisional,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrustDecision {
    pub level: TrustLevel,
    pub durability: TrustDurability,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustResolution {
    Decided(TrustDecision),
    NeedsPrompt,
}

/// Precedence:
/// 1. Feature flag off → trusted (no gating).
/// 2. Store (this workspace recorded trusted) → trusted.
///    An explicit `--trust` grant is persisted to the store up front (`grant_folder_trust`), so it is honored here.
/// 3. Key unrecordable (the user's own `$HOME`, the filesystem root, or a non-absolute path) → trusted.
///    The store refuses to persist such an over-broad root (`is_unsafe_trust_root`), so gating would re-prompt forever on a key that can never persist.
/// 4. No trust-sensitive configs present → trusted (nothing to gate).
/// 5. Interactive TTY → [`TrustOutcome::Prompt`]. [`resolve_trust`] turns that into [`TrustResolution::NeedsPrompt`] when the caller may prompt.
/// 6. Otherwise (headless, or prompting is unsafe) → untrusted.
///
/// Rules 3 and 4 are provisional. [`resolve_trust`] marks them [`TrustDurability::Provisional`].
pub fn decide(feature_enabled: bool, i: &DecideInputs) -> TrustOutcome {
    if is_durably_trusted(feature_enabled, i) || !i.key_recordable || !i.repo_configs_present {
        return TrustOutcome::Trusted;
    }
    if i.is_interactive {
        return TrustOutcome::Prompt;
    }
    TrustOutcome::Untrusted
}

/// Rules 3 and 4 stay [`TrustDurability::Provisional`].
/// The no-configs allow must not be cached: repo-local config can appear after this resolve (git pull or an agent write), and caching it would let a later `/hooks reload` run that config with no trust decision (TOCTOU).
/// The unrecordable-key allow must not be cached either: the store can never persist `$HOME`, the filesystem root, or a non-absolute path.
/// Store-trusted, feature-off, and an accepted prompt are durable.
pub fn resolve_trust(
    feature_enabled: bool,
    inputs: &DecideInputs,
    prompt: PromptPolicy,
) -> TrustResolution {
    match decide(feature_enabled, inputs) {
        TrustOutcome::Trusted => {
            let durability = if is_durably_trusted(feature_enabled, inputs) {
                TrustDurability::Durable
            } else {
                TrustDurability::Provisional
            };
            TrustResolution::Decided(TrustDecision {
                level: TrustLevel::Trusted,
                durability,
            })
        }
        TrustOutcome::Prompt => match prompt {
            PromptPolicy::MayPrompt => TrustResolution::NeedsPrompt,
            PromptPolicy::FailClosed => TrustResolution::Decided(TrustDecision {
                level: TrustLevel::Untrusted,
                durability: TrustDurability::Durable,
            }),
        },
        TrustOutcome::Untrusted => TrustResolution::Decided(TrustDecision {
            level: TrustLevel::Untrusted,
            durability: TrustDurability::Durable,
        }),
    }
}

/// Feature-off and a store grant are the durable allows. The unrecordable-key and no-configs allows are not.
fn is_durably_trusted(feature_enabled: bool, inputs: &DecideInputs) -> bool {
    !feature_enabled || inputs.store_trusted
}

/// True for a binary built without a `GROK_VERSION` stamp.
/// That binary trusts every folder without reading the trust store.
pub fn folder_trust_inert() -> bool {
    if std::env::var(xai_grok_version::TEST_VERSION_ENV).is_ok() {
        return false;
    }
    option_env!("GROK_VERSION").is_none()
}

pub fn feature_enabled(remote: Option<&RemoteSettings>) -> bool {
    feature_enabled_for_build(remote, folder_trust_inert())
}

fn feature_enabled_for_build(remote: Option<&RemoteSettings>, is_local_build: bool) -> bool {
    if is_local_build {
        return false;
    }

    let user = xai_grok_config::load_from_disk().ok();
    let managed = xai_grok_config::load_managed_config().ok();
    let flag = |root: Option<&TomlValue>| {
        root.and_then(|v| v.get("folder_trust"))
            .and_then(|v| v.get("enabled"))
            .and_then(|v| v.as_bool())
    };

    BoolFlag::env("GROK_FOLDER_TRUST")
        .config(flag(user.as_ref()))
        .managed(flag(managed.as_ref()))
        .feature_flag(remote.and_then(|r| r.folder_trust_enabled))
        .default(true)
        .resolve()
        .value
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs() -> DecideInputs {
        DecideInputs {
            store_trusted: false,
            repo_configs_present: true,
            is_interactive: false,
            key_recordable: true,
        }
    }

    #[test]
    fn decide_trusts_an_empty_folder_until_a_config_appears() {
        let open = DecideInputs {
            repo_configs_present: false,
            ..inputs()
        };
        let configured = DecideInputs {
            repo_configs_present: true,
            store_trusted: false,
            ..inputs()
        };

        assert_eq!(TrustOutcome::Trusted, decide(true, &open));
        assert_eq!(TrustOutcome::Untrusted, decide(true, &configured));
    }

    #[test]
    fn resolve_trust_durability_and_no_tty() {
        let interactive = DecideInputs {
            is_interactive: true,
            ..inputs()
        };
        let no_configs = DecideInputs {
            repo_configs_present: false,
            is_interactive: true,
            ..inputs()
        };
        let unrecordable = DecideInputs {
            is_interactive: true,
            key_recordable: false,
            ..inputs()
        };
        let stored = DecideInputs {
            store_trusted: true,
            ..inputs()
        };

        let decided =
            |level, durability| TrustResolution::Decided(TrustDecision { level, durability });
        let cases = [
            (
                false,
                inputs(),
                PromptPolicy::MayPrompt,
                decided(TrustLevel::Trusted, TrustDurability::Durable),
            ),
            (
                true,
                stored,
                PromptPolicy::FailClosed,
                decided(TrustLevel::Trusted, TrustDurability::Durable),
            ),
            (
                true,
                no_configs,
                PromptPolicy::MayPrompt,
                decided(TrustLevel::Trusted, TrustDurability::Provisional),
            ),
            (
                true,
                unrecordable,
                PromptPolicy::MayPrompt,
                decided(TrustLevel::Trusted, TrustDurability::Provisional),
            ),
            (
                true,
                interactive,
                PromptPolicy::MayPrompt,
                TrustResolution::NeedsPrompt,
            ),
            (
                true,
                interactive,
                PromptPolicy::FailClosed,
                decided(TrustLevel::Untrusted, TrustDurability::Durable),
            ),
            (
                true,
                inputs(),
                PromptPolicy::MayPrompt,
                decided(TrustLevel::Untrusted, TrustDurability::Durable),
            ),
        ];

        for (feature, inputs, prompt, expected) in cases {
            assert_eq!(expected, resolve_trust(feature, &inputs, prompt));
        }
    }

    struct EnvVar {
        key: &'static str,
        prev: Option<std::ffi::OsString>,
    }

    impl EnvVar {
        fn set(key: &'static str, val: &str) -> Self {
            let prev = std::env::var_os(key);
            unsafe { std::env::set_var(key, val) };
            Self { key, prev }
        }

        fn unset(key: &'static str) -> Self {
            let prev = std::env::var_os(key);
            unsafe { std::env::remove_var(key) };
            Self { key, prev }
        }
    }

    impl Drop for EnvVar {
        fn drop(&mut self) {
            match self.prev.take() {
                Some(prev) => unsafe { std::env::set_var(self.key, prev) },
                None => unsafe { std::env::remove_var(self.key) },
            }
        }
    }

    struct TempHome(std::path::PathBuf);

    impl TempHome {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "grok-folder-trust-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("clock")
                    .as_nanos()
            ));
            std::fs::create_dir_all(&path).expect("temp home");
            Self(path)
        }
    }

    impl Drop for TempHome {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    use crate::ENV_TEST_LOCK as ENV_LOCK;

    fn isolated_home() -> (std::sync::MutexGuard<'static, ()>, TempHome, EnvVar, EnvVar) {
        let lock = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        let home = TempHome::new();
        let home_var = EnvVar::set("GROK_HOME", home.0.to_str().expect("temp home is utf-8"));
        let flag = EnvVar::unset("GROK_FOLDER_TRUST");
        (lock, home, home_var, flag)
    }

    #[test]
    fn local_build_ignores_remote_rollout() {
        let (_lock, _home, _home_var, _flag) = isolated_home();
        let remote = xai_grok_config::RemoteSettings {
            folder_trust_enabled: Some(true),
            ..Default::default()
        };
        let feature = super::feature_enabled_for_build(Some(&remote), true);
        assert!(!feature);
        let configured = DecideInputs {
            is_interactive: true,
            ..inputs()
        };
        assert_eq!(TrustOutcome::Trusted, decide(feature, &configured));
    }

    #[test]
    fn release_build_keeps_gate_when_enabled() {
        let (_lock, _home, _home_var, _flag) = isolated_home();
        let remote = xai_grok_config::RemoteSettings {
            folder_trust_enabled: Some(true),
            ..Default::default()
        };
        let feature = super::feature_enabled_for_build(Some(&remote), false);
        assert!(feature);
        let configured = DecideInputs {
            is_interactive: true,
            ..inputs()
        };
        assert_eq!(TrustOutcome::Prompt, decide(feature, &configured));
    }

    #[test]
    fn local_build_ignores_explicit_env_optin() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        let home = TempHome::new();
        let _home_var = EnvVar::set("GROK_HOME", home.0.to_str().expect("temp home is utf-8"));
        let _flag = EnvVar::set("GROK_FOLDER_TRUST", "1");
        assert!(!super::feature_enabled_for_build(None, true));
    }

    #[test]
    fn release_build_defaults_on() {
        let (_lock, _home, _home_var, _flag) = isolated_home();
        assert!(super::feature_enabled_for_build(None, false));
    }

    #[test]
    fn is_local_build_honors_test_version_override() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        {
            let _sim = EnvVar::set(xai_grok_version::TEST_VERSION_ENV, "0.0.0-sim");
            assert!(!folder_trust_inert());
        }
        let _unset = EnvVar::unset(xai_grok_version::TEST_VERSION_ENV);
        if option_env!("GROK_VERSION").is_none() {
            assert!(folder_trust_inert());
        }
    }
}
