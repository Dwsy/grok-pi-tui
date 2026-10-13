//! Signed on-disk cache for remote settings, scoped to the account and client that fetched them.

use std::path::Path;

use chrono::{DateTime, Utc};

use xai_grok_config::RemoteSettings;
use xai_grok_config::cache_file::{
    CacheFileMode, CacheLoadError, is_fresh, read_capped, write_atomic,
};

const SETTINGS_CACHE_FILE: &str = "settings_cache.json";
/// One hour lets an offline start use the last good settings.
/// `managed_policy_gate` and each session still load fresh settings after startup.
const SETTINGS_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(3600);
const SETTINGS_CACHE_MAX_BYTES: u64 = 1 << 20;

/// Whether a process reads and writes the settings cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsCacheMode {
    Enabled,
    /// The cache file is never read or written.
    Disabled,
}

impl SettingsCacheMode {
    /// `GROK_SETTINGS_CACHE=false` disables the cache.
    pub fn from_process() -> SettingsCacheMode {
        if xai_grok_config::env_bool("GROK_SETTINGS_CACHE") == Some(false) {
            SettingsCacheMode::Disabled
        } else {
            SettingsCacheMode::Enabled
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SettingsCache {
    fetched_at: DateTime<Utc>,
    grok_version: String,
    identity: String,
    origin: String,
    client: String,
    settings: RemoteSettings,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SignedSettingsCache {
    payload: String,
    signature: Vec<u8>,
}

struct CachedSettings(RemoteSettings);

impl CachedSettings {
    /// Only a live fetch may set `managed_config_signature_verification` or `accept_request_encodings`.
    /// Either value may have changed on the server or the proxy since the cache was written.
    fn into_settings(self) -> RemoteSettings {
        let mut settings = self.0;
        settings.managed_config_signature_verification = None;
        settings.accept_request_encodings.clear();
        settings
    }
}

/// The account and proxy origin a cache entry belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsCacheAccount {
    /// A one-way hash of the account's stable identity.
    pub identity: String,
    pub origin: String,
}

/// The account and client a cache entry belongs to.
#[derive(Debug, Clone)]
pub struct SettingsCacheScope {
    pub account: SettingsCacheAccount,
    pub client: String,
}

#[derive(Debug, Clone, Copy)]
enum ClientScope<'a> {
    Only(&'a str),
    AnyClient,
}

#[derive(Debug, Clone)]
pub struct SettingsCacheManager {
    path: std::path::PathBuf,
    ttl: std::time::Duration,
    mode: SettingsCacheMode,
}

impl SettingsCacheManager {
    pub fn new(grok_home: &Path, mode: SettingsCacheMode) -> SettingsCacheManager {
        SettingsCacheManager {
            path: grok_home.join(SETTINGS_CACHE_FILE),
            ttl: SETTINGS_CACHE_TTL,
            mode,
        }
    }

    pub fn load_or_fetch(
        &self,
        scope: SettingsCacheScope,
        fetch: impl FnOnce() -> Option<RemoteSettings>,
    ) -> SettingsLoad {
        match self.mode {
            SettingsCacheMode::Enabled => {}
            SettingsCacheMode::Disabled => {
                return match fetch() {
                    Some(settings) => SettingsLoad::Fetched {
                        settings,
                        write: None,
                    },
                    None => SettingsLoad::FetchFailed,
                };
            }
        }

        if let Some(cached) = self.load_fresh(&scope) {
            tracing::info!("settings cache hit");
            return SettingsLoad::Cached(cached.into_settings());
        }

        tracing::info!("settings cache miss; fetching");
        let fetched_at = Utc::now();
        let Some(settings) = fetch() else {
            return SettingsLoad::FetchFailed;
        };

        let write = SettingsCacheWrite {
            manager: self.clone(),
            scope,
            settings: settings.clone(),
            fetched_at,
        };
        SettingsLoad::Fetched {
            settings,
            write: Some(Box::new(write)),
        }
    }

    /// Writes a live fetch, unless a newer fetch for the same scope is already stored.
    /// `fetched_at` is the time the fetch started.
    pub fn write_through(
        &self,
        scope: &SettingsCacheScope,
        settings: &RemoteSettings,
        fetched_at: DateTime<Utc>,
    ) -> std::io::Result<()> {
        match self.mode {
            SettingsCacheMode::Enabled => self.persist(scope, settings, fetched_at),
            SettingsCacheMode::Disabled => Ok(()),
        }
    }

    fn load_fresh(&self, scope: &SettingsCacheScope) -> Option<CachedSettings> {
        match self.try_load_fresh(scope) {
            Ok(hit) => Some(hit),
            Err(status) => {
                status.log(&self.path);
                None
            }
        }
    }

    fn try_load_fresh(&self, scope: &SettingsCacheScope) -> Result<CachedSettings, CacheLoadError> {
        let cache = self.load_verified()?;
        Self::check_scope(&cache, &scope.account, ClientScope::Only(&scope.client))?;
        if !is_fresh(cache.fetched_at, self.ttl) {
            return Err(CacheLoadError::Stale);
        }
        Ok(CachedSettings(cache.settings))
    }

    /// Accepts any client's entry, because this process gets its client name from whoever started it.
    /// `account_for` names the account the entry's own campaigns resolve to; the entry is used only if
    /// it was fetched for that account.
    pub(crate) fn load_for_any_client(
        &self,
        account_for: impl FnOnce(&RemoteSettings) -> Option<SettingsCacheAccount>,
    ) -> Option<(RemoteSettings, Freshness)> {
        match self.mode {
            SettingsCacheMode::Enabled => {}
            SettingsCacheMode::Disabled => return None,
        }
        let loaded = self.load_verified().and_then(|cache| {
            Self::check_version(&cache)?;
            let Some(account) = account_for(&cache.settings) else {
                return Ok(None);
            };
            Self::check_scope(&cache, &account, ClientScope::AnyClient)?;

            let freshness = if is_fresh(cache.fetched_at, self.ttl) {
                Freshness::Fresh
            } else {
                Freshness::Stale
            };
            Ok(Some((
                CachedSettings(cache.settings).into_settings(),
                freshness,
            )))
        });
        loaded.unwrap_or_else(|miss| {
            miss.log(&self.path);
            None
        })
    }

    /// Reads the cache file and checks its signature, but not its scope or freshness.
    fn load_verified(&self) -> Result<SettingsCache, CacheLoadError> {
        let data =
            read_capped(&self.path, SETTINGS_CACHE_MAX_BYTES).ok_or(CacheLoadError::NotFound)?;
        let signed: SignedSettingsCache =
            serde_json::from_slice(&data).map_err(|_| CacheLoadError::ParseFailed)?;
        if !verify_cache_signature(signed.payload.as_bytes(), &signed.signature) {
            return Err(CacheLoadError::SignatureInvalid);
        }
        serde_json::from_str(&signed.payload).map_err(|_| CacheLoadError::ParseFailed)
    }

    fn check_version(cache: &SettingsCache) -> Result<(), CacheLoadError> {
        if cache.grok_version != xai_grok_version::VERSION {
            return Err(CacheLoadError::VersionMismatch);
        }
        Ok(())
    }

    /// Rejects an entry written by another build or for another scope.
    fn check_scope(
        cache: &SettingsCache,
        account: &SettingsCacheAccount,
        client: ClientScope<'_>,
    ) -> Result<(), CacheLoadError> {
        Self::check_version(cache)?;
        if cache.identity != account.identity {
            return Err(CacheLoadError::ScopeMismatch("identity"));
        }
        if cache.origin != account.origin {
            return Err(CacheLoadError::ScopeMismatch("origin"));
        }
        let client_matches = match client {
            ClientScope::Only(client) => cache.client == client,
            ClientScope::AnyClient => true,
        };
        if !client_matches {
            return Err(CacheLoadError::ScopeMismatch("client"));
        }
        Ok(())
    }

    /// Keeps the stored entry for the same scope when its fetch started at or after `fetched_at`.
    /// A stored time in the future (a corrupt file or a clock rollback) never blocks the write.
    fn persist(
        &self,
        scope: &SettingsCacheScope,
        settings: &RemoteSettings,
        fetched_at: DateTime<Utc>,
    ) -> std::io::Result<()> {
        // One thread at a time reads, checks, and writes the cache file
        static WRITE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = WRITE_LOCK.lock().unwrap_or_else(|p| p.into_inner());

        if let Ok(existing) = self.load_verified()
            && Self::check_scope(&existing, &scope.account, ClientScope::Only(&scope.client))
                .is_ok()
            && existing.fetched_at <= Utc::now()
            && existing.fetched_at >= fetched_at
        {
            tracing::debug!("settings cache write skipped: a newer fetch is already stored");
            return Ok(());
        }

        let cache = SettingsCache {
            fetched_at,
            grok_version: xai_grok_version::VERSION.to_string(),
            identity: scope.account.identity.clone(),
            origin: scope.account.origin.clone(),
            client: scope.client.clone(),
            settings: settings.clone(),
        };
        let payload = serde_json::to_string(&cache)?;
        let signature = sign_cache_payload(payload.as_bytes());
        let json = serde_json::to_vec_pretty(&SignedSettingsCache { payload, signature })?;
        write_atomic(&self.path, self.ttl, &json, CacheFileMode::OwnerOnly)
    }
}

#[derive(Debug)]
pub(crate) enum Freshness {
    Fresh,
    Stale,
}

/// Where [`SettingsCacheManager::load_or_fetch`] got its settings.
#[must_use]
pub enum SettingsLoad {
    /// A fresh cache entry for the scope, returned without a fetch.
    Cached(RemoteSettings),
    /// A live fetch and the write that caches it when committed.
    /// `write` is `None` when the cache is disabled.
    Fetched {
        settings: RemoteSettings,
        write: Option<Box<SettingsCacheWrite>>,
    },
    FetchFailed,
}

#[must_use]
pub struct SettingsCacheWrite {
    manager: SettingsCacheManager,
    scope: SettingsCacheScope,
    settings: RemoteSettings,
    fetched_at: DateTime<Utc>,
}

impl SettingsCacheWrite {
    /// Writes the fetch, unless a newer fetch for the same scope is already stored.
    pub fn commit(self) -> std::io::Result<()> {
        self.manager
            .persist(&self.scope, &self.settings, self.fetched_at)
    }
}

type HmacSha256 = hmac::Hmac<sha2::Sha256>;

/// The key that signs new cache files.
/// It rejects corrupt or foreign cache files. It is not a secret: every client binary carries it.
/// To rotate it, put the new key here and keep the old key in `SETTINGS_CACHE_READ_HMAC_KEYS`.
const SETTINGS_CACHE_WRITE_HMAC_KEY: &[u8] =
    b"grok-shell-settings-cache-hmac-v1-ba6c43d3-404f-4b5c-b0cd-df09b2f5bdf4";
/// The keys a read accepts, newest first.
const SETTINGS_CACHE_READ_HMAC_KEYS: &[&[u8]] = &[SETTINGS_CACHE_WRITE_HMAC_KEY];

fn sign_cache_payload(payload: &[u8]) -> Vec<u8> {
    use hmac::Mac;
    #[expect(clippy::expect_used)]
    let mut mac = HmacSha256::new_from_slice(SETTINGS_CACHE_WRITE_HMAC_KEY)
        .expect("HMAC takes a key of any length");
    mac.update(payload);
    mac.finalize().into_bytes().to_vec()
}

fn verify_cache_signature(payload: &[u8], signature: &[u8]) -> bool {
    verify_with_keys(SETTINGS_CACHE_READ_HMAC_KEYS, payload, signature)
}

fn verify_with_keys(keys: &[&[u8]], payload: &[u8], signature: &[u8]) -> bool {
    use hmac::Mac;
    keys.iter().any(|key| {
        HmacSha256::new_from_slice(key).is_ok_and(|mut m| {
            m.update(payload);
            m.verify_slice(signature).is_ok()
        })
    })
}

#[cfg(test)]
#[path = "settings_cache_tests.rs"]
mod tests;
