mod backend;
mod cache;
mod metrics;
mod prefetch;
mod validation;

#[cfg(test)]
#[path = "validation_tests.rs"]
mod validation_tests;

pub use backend::{ModelsPrefetch, commit_catalog_fetch, fetch_catalog, prefetch_catalog_blocking};
pub use cache::{CACHE_TTL, CacheResult, MODELS_CACHE_FILE, ModelsCache, ModelsCacheManager};
pub use metrics::{DegradedStartCause, degraded_log_level, record_degraded_start};
pub use prefetch::{
    InitialModelsLoad, fetch_catalog_blocking, run_catalog_prefetch, start_catalog_load,
};
pub use validation::{
    CacheAuthMethod, ModelGlobSet, ModelsCacheScope, evaluate_models_commit, is_campaign_only_flip,
    scope_hash,
};
