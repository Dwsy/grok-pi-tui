use globset::{Glob, GlobSet, GlobSetBuilder};

use crate::Commit;

#[derive(serde::Serialize, serde::Deserialize, PartialEq, Eq, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub enum CacheAuthMethod {
    Session,
    ApiKey,
    Deployment,
}

#[derive(Clone, PartialEq, Eq)]
pub struct ModelsCacheScope {
    pub auth_method: CacheAuthMethod,
    pub origin: String,
    pub identity: String,
}

/// The hash uses the same scheme as `settings_cache_identity`.
/// The raw credential stays off disk.
pub fn scope_hash(parts: &[&str]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update([0u8]);
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn evaluate_models_commit(
    expected: &ModelsCacheScope,
    live: &ModelsCacheScope,
    models_fetch_enabled: bool,
) -> Commit {
    if crate::managed_config::policy_repair_pending() {
        return Commit::Retry;
    }
    if !models_fetch_enabled || live.origin != expected.origin {
        return Commit::Abandon;
    }
    if live.identity == expected.identity {
        Commit::CacheAndServe
    } else {
        Commit::ServeInMemory
    }
}

pub fn is_campaign_only_flip(
    old_preferred: &Option<String>,
    new_preferred: &Option<String>,
    campaign_defaults: &std::collections::HashSet<String>,
) -> bool {
    if new_preferred == old_preferred || new_preferred.is_none() {
        return false;
    }
    new_preferred
        .as_ref()
        .is_some_and(|preferred| campaign_defaults.contains(preferred))
        || old_preferred
            .as_ref()
            .is_some_and(|preferred| campaign_defaults.contains(preferred))
}

/// Compiled glob matcher shared by `allowed_models`, `disabled_models`, and `hidden_models` (matched against catalog key or model id).
pub struct ModelGlobSet(GlobSet);

impl ModelGlobSet {
    /// Compile a filter list (`Ok(None)` for `None`/empty). Fails **closed**: an invalid pattern returns `Err` listing every bad one.
    pub fn compile(patterns: Option<&[String]>) -> Result<Option<Self>, Vec<String>> {
        let patterns = match patterns {
            Some(patterns) if !patterns.is_empty() => patterns,
            Some(_) | None => return Ok(None),
        };

        let mut builder = GlobSetBuilder::new();
        let mut invalid = Vec::new();
        for pattern in patterns {
            match Glob::new(pattern) {
                Ok(glob) => {
                    builder.add(glob);
                }
                Err(_) => invalid.push(pattern.clone()),
            }
        }

        if !invalid.is_empty() {
            return Err(invalid);
        }
        builder
            .build()
            .map(|set| Some(Self(set)))
            .map_err(|error| vec![error.to_string()])
    }

    pub fn matches(&self, key: &str, model: &str) -> bool {
        self.0.is_match(key) || self.0.is_match(model)
    }

    pub fn matches_model(&self, model: &str) -> bool {
        self.0.is_match(model)
    }
}
