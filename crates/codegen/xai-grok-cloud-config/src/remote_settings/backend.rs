use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::{ModelsCacheManager, ModelsCacheScope, evaluate_models_commit};
use crate::Commit;

pub enum ModelsPrefetch<E> {
    Cached(IndexMap<String, E>),
    Fetched(ModelsCacheWrite<E>),
    Unavailable,
}

impl<E> ModelsPrefetch<E>
where
    E: Serialize + DeserializeOwned + Clone,
{
    fn commit(self) -> Option<IndexMap<String, E>> {
        match self {
            Self::Cached(models) => Some(models),
            Self::Fetched(write) => Some(write.commit()),
            Self::Unavailable => None,
        }
    }
}

pub struct ModelsCacheWrite<E> {
    models: IndexMap<String, E>,
    etag: Option<String>,
    scope: ModelsCacheScope,
    fetched_at: DateTime<Utc>,
    cache: ModelsCacheManager<E>,
}

impl<E> ModelsCacheWrite<E> {
    /// The fetched catalog without persisting it.
    pub fn into_models(self) -> IndexMap<String, E> {
        self.models
    }
}

impl<E> ModelsCacheWrite<E>
where
    E: Serialize + DeserializeOwned + Clone,
{
    fn commit(self) -> IndexMap<String, E> {
        self.cache.persist(
            &self.models,
            self.etag.as_deref(),
            &self.scope,
            self.fetched_at,
        );
        self.models
    }
}

pub fn fetch_catalog<E>(
    cache: &ModelsCacheManager<E>,
    scope: &ModelsCacheScope,
    remote_fetch_enabled: bool,
    fetch: impl FnOnce() -> Option<(IndexMap<String, E>, Option<String>)>,
) -> ModelsPrefetch<E>
where
    E: Serialize + DeserializeOwned + Clone,
{
    if let Some(cached) = cache.load_fresh(scope) {
        return ModelsPrefetch::Cached(cached.models);
    }

    if !remote_fetch_enabled {
        tracing::info!("models fetch skipped: remote_fetch disabled");
        return ModelsPrefetch::Unavailable;
    }

    let fetched_at = Utc::now();
    match fetch() {
        Some((models, etag)) => ModelsPrefetch::Fetched(ModelsCacheWrite {
            models,
            etag,
            scope: scope.clone(),
            fetched_at,
            cache: cache.clone(),
        }),
        None => ModelsPrefetch::Unavailable,
    }
}

/// Persist without the policy/identity gate because the caller resolved auth synchronously before any session exists.
/// Catalog loads go through `commit_catalog_fetch`.
pub fn prefetch_catalog_blocking<E>(
    manager: &ModelsCacheManager<E>,
    scope: &ModelsCacheScope,
    remote_fetch_enabled: bool,
    fetch: impl FnOnce() -> Option<(IndexMap<String, E>, Option<String>)>,
) -> Option<IndexMap<String, E>>
where
    E: Serialize + DeserializeOwned + Clone,
{
    fetch_catalog(manager, scope, remote_fetch_enabled, fetch).commit()
}

/// Apply the commit gate. `live` runs only after a real fetch, so a cache hit does not re-read disk auth.
/// Its fetch policy is read after the fetch so a mid-fetch disable abandons the result.
pub fn commit_catalog_fetch<E>(
    expected: &ModelsCacheScope,
    fetched: ModelsPrefetch<E>,
    live: impl FnOnce() -> (ModelsCacheScope, bool),
) -> Option<IndexMap<String, E>>
where
    E: Serialize + DeserializeOwned + Clone,
{
    match fetched {
        ModelsPrefetch::Cached(models) => Some(models),
        ModelsPrefetch::Fetched(write) => {
            let (live_scope, fetch_enabled) = live();
            apply_models_commit(
                evaluate_models_commit(expected, &live_scope, fetch_enabled),
                write,
            )
        }
        ModelsPrefetch::Unavailable => None,
    }
}

fn apply_models_commit<E>(
    decision: Commit,
    write: ModelsCacheWrite<E>,
) -> Option<IndexMap<String, E>>
where
    E: Serialize + DeserializeOwned + Clone,
{
    match decision {
        Commit::CacheAndServe => Some(write.commit()),
        Commit::ServeInMemory => {
            tracing::info!("models fetch served in memory; not cached until the session persists");
            Some(write.models)
        }
        Commit::Retry | Commit::Abandon => {
            tracing::info!("models load discarded fetch: policy or origin changed");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote_settings::CacheAuthMethod;

    fn scope(origin: &str, identity: &str) -> ModelsCacheScope {
        ModelsCacheScope {
            auth_method: CacheAuthMethod::Session,
            origin: origin.to_string(),
            identity: identity.to_string(),
        }
    }

    fn fetched_write(
        cache: &ModelsCacheManager<String>,
        scope: &ModelsCacheScope,
    ) -> ModelsCacheWrite<String> {
        let mut models = IndexMap::new();
        models.insert("grok".to_string(), "model".to_string());
        match fetch_catalog(cache, scope, true, || Some((models, None))) {
            ModelsPrefetch::Fetched(write) => write,
            ModelsPrefetch::Cached(_) | ModelsPrefetch::Unavailable => {
                panic!("expected a fetched catalog")
            }
        }
    }

    #[test]
    fn commit_gate_writes_only_on_cache_and_serve() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ModelsCacheManager::at(
            dir.path().join("models.json"),
            std::time::Duration::from_secs(300),
        );
        let expected = scope("https://origin.example/v1/models", "account-a");
        let cases = [
            (Commit::Retry, false, false),
            (Commit::Abandon, false, false),
            (Commit::ServeInMemory, true, false),
            (Commit::CacheAndServe, true, true),
        ];
        for (decision, serves, writes) in cases {
            let _ = std::fs::remove_file(&cache.path);
            let write = fetched_write(&cache, &expected);
            let served = apply_models_commit(decision, write);
            assert_eq!(served.is_some(), serves);
            assert_eq!(cache.path.exists(), writes);
        }
    }
}
