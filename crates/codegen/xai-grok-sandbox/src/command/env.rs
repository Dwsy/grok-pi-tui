//! Applies an [`EnvPolicy`] to a command before spawn: excluded names are removed from both the
//! inherited environment and the tool's explicit overrides, then the policy's `set` entries are
//! added. Done once here for every backend, so the wrapper (`sandbox-exec`) inherits an
//! already-filtered environment and no backend has to re-implement the globs.

use std::ffi::{OsStr, OsString};

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::command::policy::EnvPolicy;

/// Exclusion globs over environment variable names, ASCII case-insensitive, compiled when the
/// list is made: a list that exists always applies, so no reader has a failure to handle.
#[derive(Clone, Debug)]
pub struct EnvGlobs {
    globs: Vec<String>,
    matcher: GlobSet,
}

#[derive(Debug, thiserror::Error)]
#[error("invalid environment exclusion glob {glob:?}: {reason}")]
pub struct InvalidEnvGlob {
    pub glob: String,
    pub reason: String,
}

impl EnvGlobs {
    /// # Errors
    /// [`InvalidEnvGlob`] naming the first glob that does not parse.
    pub fn new(globs: Vec<String>) -> Result<Self, InvalidEnvGlob> {
        let mut builder = GlobSetBuilder::new();
        for glob in &globs {
            let compiled = GlobBuilder::new(&glob.to_ascii_uppercase())
                .literal_separator(false)
                .build()
                .map_err(|error| InvalidEnvGlob {
                    glob: glob.clone(),
                    reason: error.to_string(),
                })?;
            builder.add(compiled);
        }
        let matcher = builder.build().map_err(|error| InvalidEnvGlob {
            glob: globs.join(" "),
            reason: error.to_string(),
        })?;
        Ok(Self { globs, matcher })
    }

    /// Whether `name` matches a glob; a name that is not UTF-8 is read with its invalid bytes
    /// replaced (`API\xffKEY` is still a key).
    pub fn is_match(&self, name: &OsStr) -> bool {
        self.matcher
            .is_match(name.to_string_lossy().to_ascii_uppercase())
    }

    pub fn len(&self) -> usize {
        self.globs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.globs.is_empty()
    }

    pub fn iter(&self) -> std::slice::Iter<'_, String> {
        self.globs.iter()
    }
}

impl Default for EnvGlobs {
    fn default() -> Self {
        Self {
            globs: Vec::new(),
            matcher: GlobSet::empty(),
        }
    }
}

impl PartialEq for EnvGlobs {
    fn eq(&self, other: &Self) -> bool {
        self.globs == other.globs
    }
}

impl Eq for EnvGlobs {}

impl<'a> IntoIterator for &'a EnvGlobs {
    type Item = &'a String;
    type IntoIter = std::slice::Iter<'a, String>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl Serialize for EnvGlobs {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.globs.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for EnvGlobs {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        EnvGlobs::new(Vec::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

/// The environment names `apply_env_policy` will remove: every inherited or explicitly set
/// variable whose name matches an exclusion glob.
pub fn excluded_names(
    policy: &EnvPolicy,
    inherited: impl IntoIterator<Item = OsString>,
    explicit: impl IntoIterator<Item = OsString>,
) -> Vec<OsString> {
    let mut names: Vec<OsString> = inherited
        .into_iter()
        .chain(explicit)
        .filter(|name| policy.exclude_globs.is_match(name))
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Rewrite `cmd`'s environment per `policy`.
pub fn apply_env_policy(cmd: &mut tokio::process::Command, policy: &EnvPolicy) {
    let explicit: Vec<OsString> = cmd
        .as_std()
        .get_envs()
        .map(|(name, _)| name.to_os_string())
        .collect();
    let inherited = std::env::vars_os().map(|(name, _)| name);
    for name in excluded_names(policy, inherited, explicit) {
        cmd.env_remove(name);
    }
    for (name, value) in &policy.set {
        cmd.env(name, value);
    }
}

/// Only the `set` entries (the proxy pointers), for observe mode where the inherited environment
/// must stay as it is.
pub fn apply_env_set_only(cmd: &mut tokio::process::Command, policy: &EnvPolicy) {
    for (name, value) in &policy.set {
        cmd.env(name, value);
    }
}

#[cfg(test)]
#[path = "env_tests.rs"]
mod tests;
