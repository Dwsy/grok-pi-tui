use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::validation::{CacheAuthMethod, ModelsCacheScope};
use xai_grok_config::cache_file::{
    CacheFileMode, CacheLoadError, is_fresh, read_capped, write_atomic,
};

pub const MODELS_CACHE_FILE: &str = "models_cache.json";
pub const CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(300);
const MODELS_CACHE_MAX_BYTES: u64 = 4 << 20;

// One lock covers read, check, and write so an older fetch cannot win.
static WRITE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(serde::Serialize, serde::Deserialize)]
pub struct ModelsCache<E> {
    pub fetched_at: DateTime<Utc>,

    /// A TTL renewal writes this field and leaves `fetched_at` unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renewed_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grok_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_method: Option<CacheAuthMethod>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,

    /// A legacy `None` misses and refetches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    pub models: IndexMap<String, E>,
}

pub struct CacheResult<E> {
    pub models: IndexMap<String, E>,
    pub etag: Option<String>,
}

#[derive(Clone)]
pub struct ModelsCacheManager<E> {
    pub path: std::path::PathBuf,
    pub ttl: std::time::Duration,
    entry: std::marker::PhantomData<E>,
}

impl<E> ModelsCacheManager<E> {
    pub fn at(path: std::path::PathBuf, ttl: std::time::Duration) -> Self {
        Self {
            path,
            ttl,
            entry: std::marker::PhantomData,
        }
    }

    pub fn new() -> Self {
        Self::default()
    }
}

impl<E> Default for ModelsCacheManager<E> {
    fn default() -> Self {
        Self::at(xai_dirs::grok_home().join(MODELS_CACHE_FILE), CACHE_TTL)
    }
}

impl<E> ModelsCacheManager<E>
where
    E: Serialize + DeserializeOwned + Clone,
{
    pub fn load_fresh(&self, scope: &ModelsCacheScope) -> Option<CacheResult<E>> {
        match self.try_load_fresh(scope) {
            Ok(hit) => Some(hit),
            Err(status) => {
                status.log(&self.path);
                None
            }
        }
    }

    fn try_load_fresh(&self, scope: &ModelsCacheScope) -> Result<CacheResult<E>, CacheLoadError> {
        let data =
            read_capped(&self.path, MODELS_CACHE_MAX_BYTES).ok_or(CacheLoadError::NotFound)?;
        let cache: ModelsCache<E> =
            serde_json::from_slice(&data).map_err(|_| CacheLoadError::ParseFailed)?;
        if cache.grok_version.as_deref() != Some(xai_grok_version::VERSION) {
            return Err(CacheLoadError::VersionMismatch);
        }
        if cache.auth_method.as_ref() != Some(&scope.auth_method) {
            return Err(CacheLoadError::ScopeMismatch("auth method"));
        }
        if cache.origin.as_deref() != Some(scope.origin.as_str()) {
            return Err(CacheLoadError::ScopeMismatch("origin"));
        }
        if cache.identity.as_deref() != Some(scope.identity.as_str()) {
            return Err(CacheLoadError::ScopeMismatch("identity"));
        }
        if !is_fresh(cache.renewed_at.unwrap_or(cache.fetched_at), self.ttl) {
            return Err(CacheLoadError::Stale);
        }
        tracing::debug!(count = cache.models.len(), "loaded models from disk cache");
        Ok(CacheResult {
            models: cache.models,
            etag: cache.etag,
        })
    }

    pub fn persist(
        &self,
        models: &IndexMap<String, E>,
        etag: Option<&str>,
        scope: &ModelsCacheScope,
        fetched_at: DateTime<Utc>,
    ) {
        let _guard = WRITE_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if self
            .disk_fetched_at(scope)
            .is_some_and(|disk| disk > fetched_at)
        {
            tracing::debug!("models cache commit skipped: a newer fetch is already stored");
            return;
        }
        let cache = ModelsCache {
            fetched_at,
            renewed_at: None,
            grok_version: Some(xai_grok_version::VERSION.to_string()),
            auth_method: Some(scope.auth_method.clone()),
            origin: Some(scope.origin.clone()),
            identity: Some(scope.identity.clone()),
            etag: etag.map(str::to_string),
            models: models.clone(),
        };
        self.atomic_write(&cache);
    }

    fn disk_fetched_at(&self, scope: &ModelsCacheScope) -> Option<DateTime<Utc>> {
        let data = read_capped(&self.path, MODELS_CACHE_MAX_BYTES)?;
        let cache: ModelsCache<E> = serde_json::from_slice(&data).ok()?;
        if cache.auth_method.as_ref() != Some(&scope.auth_method)
            || cache.origin.as_deref() != Some(scope.origin.as_str())
            || cache.identity.as_deref() != Some(scope.identity.as_str())
        {
            return None;
        }
        (cache.fetched_at <= Utc::now()).then_some(cache.fetched_at)
    }

    /// Bump `renewed_at` when the on-disk catalog is unchanged, extending its TTL
    /// without touching the `fetched_at` content key. The read-modify-write holds
    /// `WRITE_LOCK` and re-reads inside it, so a concurrent in-process commit is
    /// never clobbered. `WRITE_LOCK` is process-local, so a cross-process commit
    /// can still race; that self-heals on the next etag-driven refresh.
    pub async fn renew_ttl(&self, scope: &ModelsCacheScope) {
        let path = self.path.clone();
        let ttl = self.ttl;
        let scope = scope.clone();
        let _ = tokio::task::spawn_blocking(move || {
            let _guard = WRITE_LOCK
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            let Some(data) = read_capped(&path, MODELS_CACHE_MAX_BYTES) else {
                return;
            };
            let Ok(mut cache) = serde_json::from_slice::<ModelsCache<E>>(&data) else {
                return;
            };
            if cache.auth_method.as_ref() != Some(&scope.auth_method)
                || cache.origin.as_deref() != Some(scope.origin.as_str())
                || cache.identity.as_deref() != Some(scope.identity.as_str())
            {
                tracing::debug!("models cache TTL renewal skipped: scope mismatch");
                return;
            }
            cache.renewed_at = Some(Utc::now());
            ModelsCacheManager {
                path,
                ttl,
                entry: std::marker::PhantomData,
            }
            .atomic_write(&cache);
            tracing::debug!("models cache TTL renewed");
        })
        .await;
    }

    pub fn invalidate(&self) {
        let _guard = WRITE_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        match std::fs::remove_file(&self.path) {
            Ok(()) => tracing::info!("models disk cache invalidated"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                tracing::warn!(error = %error, "failed to invalidate models disk cache")
            }
        }
    }

    fn atomic_write(&self, cache: &ModelsCache<E>) {
        let Ok(json) = serde_json::to_vec_pretty(cache) else {
            return;
        };
        if let Err(error) = write_atomic(&self.path, self.ttl, &json, CacheFileMode::Default) {
            tracing::debug!(%error, path = %self.path.display(), "models cache write failed");
        }
    }
}
