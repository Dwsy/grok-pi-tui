use std::cell::Cell;

use indexmap::IndexMap;

use super::run_catalog_prefetch;
use crate::remote_settings::{
    CacheAuthMethod, ModelsCacheManager, ModelsCacheScope, ModelsPrefetch, fetch_catalog,
};

fn scope(origin: &str, identity: &str) -> ModelsCacheScope {
    ModelsCacheScope {
        auth_method: CacheAuthMethod::Session,
        origin: origin.to_string(),
        identity: identity.to_string(),
    }
}

#[test]
fn disabled_policy_after_fetch_abandons_without_writing() {
    let dir = tempfile::tempdir().unwrap();
    let cache = ModelsCacheManager::at(
        dir.path().join("models.json"),
        std::time::Duration::from_secs(300),
    );
    let expected = scope("https://origin.example/v1/models", "account-a");
    let mut models = IndexMap::new();
    models.insert("grok".to_string(), "model".to_string());
    let fetched = Cell::new(false);
    let served = run_catalog_prefetch(
        &tokio_util::sync::CancellationToken::new(),
        &expected,
        || {
            let prefetch = fetch_catalog(&cache, &expected, true, || Some((models, None)));
            fetched.set(matches!(prefetch, ModelsPrefetch::Fetched(_)));
            prefetch
        },
        || (expected.clone(), !fetched.get()),
    );
    assert!(fetched.get());
    assert_eq!(None, served);
    assert!(!cache.path.exists());
}
