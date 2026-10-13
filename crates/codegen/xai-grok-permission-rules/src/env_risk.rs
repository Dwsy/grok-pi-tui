/// Env var KEYs safe to set for a routine command: cosmetic / logging only, with no effect on which binary runs or how it resolves code.
/// Anything else (LD_PRELOAD, DYLD_*, PATH, NODE_OPTIONS, PYTHONPATH, GIT_SSH_COMMAND, FOO, ...) is treated as exec-affecting and blocks.
/// Case-sensitive exact match.
const SAFE_ENV_KEYS: &[&str] = &[
    "CARGO_TERM_COLOR",
    "CARGO_TERM_PROGRESS_WHEN",
    "RUST_LOG",
    "RUST_LOG_STYLE",
    "RUST_BACKTRACE",
    "RUST_TEST_THREADS",
    "RUST_MIN_STACK",
    "NO_COLOR",
    "CLICOLOR",
    "CLICOLOR_FORCE",
    "FORCE_COLOR",
    "COLORTERM",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EnvRisk {
    Safe,
    Unvetted,
    Injection,
}

const INJECTION_ENV_KEYS: &[&str] = &[
    "LD_PRELOAD",
    "LD_AUDIT",
    "BASH_ENV",
    "ENV",
    "IFS",
    "PATH",
    "GIT_EXTERNAL_DIFF",
    "GIT_PROXY_COMMAND",
    "PROMPT_COMMAND",
];

const INJECTION_ENV_KEY_PREFIXES: &[&str] = &["DYLD_", "GIT_CONFIG"];

pub fn env_key_risk(key: &str) -> EnvRisk {
    if is_safe_env_key(key) {
        EnvRisk::Safe
    } else if INJECTION_ENV_KEYS.contains(&key)
        || INJECTION_ENV_KEY_PREFIXES
            .iter()
            .any(|p| key.starts_with(p))
    {
        EnvRisk::Injection
    } else {
        EnvRisk::Unvetted
    }
}

fn is_safe_env_key(key: &str) -> bool {
    SAFE_ENV_KEYS.contains(&key)
}
