use chrono::Duration as ChronoDuration;

use super::*;

const ORIGIN: &str = "https://proxy.example";
const CLIENT: &str = "grok-shell";

fn account(identity: &str) -> SettingsCacheAccount {
    SettingsCacheAccount {
        identity: identity.to_owned(),
        origin: ORIGIN.to_owned(),
    }
}

fn scope(identity: &str) -> SettingsCacheScope {
    SettingsCacheScope {
        account: account(identity),
        client: CLIENT.to_owned(),
    }
}

fn temp_manager(ttl: std::time::Duration) -> (tempfile::TempDir, SettingsCacheManager) {
    let dir = tempfile::tempdir().unwrap();
    let manager = SettingsCacheManager {
        ttl,
        ..SettingsCacheManager::new(dir.path(), SettingsCacheMode::Enabled)
    };
    (dir, manager)
}

fn settings() -> RemoteSettings {
    RemoteSettings {
        leader_mode: Some(true),
        ..RemoteSettings::default()
    }
}

#[test]
fn load_or_fetch_skips_the_fetch_on_a_cache_hit() {
    let (_dir, manager) = temp_manager(SETTINGS_CACHE_TTL);
    let SettingsLoad::Fetched {
        write: Some(write), ..
    } = manager.load_or_fetch(scope("id"), || Some(settings()))
    else {
        panic!("a cold load fetches and returns a write");
    };
    write.commit().unwrap();

    let SettingsLoad::Cached(cached) =
        manager.load_or_fetch(scope("id"), || panic!("warm hit must not fetch"))
    else {
        panic!("a warm load is served from the cache");
    };

    assert_eq!(Some(true), cached.leader_mode);
}

#[test]
fn load_or_fetch_defers_the_write_until_commit() {
    let (dir, manager) = temp_manager(SETTINGS_CACHE_TTL);
    let SettingsLoad::Fetched {
        write: Some(write), ..
    } = manager.load_or_fetch(scope("id"), || Some(settings()))
    else {
        panic!("a cold load fetches and returns a write");
    };
    assert!(!dir.path().join(SETTINGS_CACHE_FILE).exists());

    write.commit().unwrap();
    assert!(dir.path().join(SETTINGS_CACHE_FILE).exists());
}

#[test]
fn disabled_cache_fetches_every_load_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let manager = SettingsCacheManager::new(dir.path(), SettingsCacheMode::Disabled);

    manager
        .write_through(&scope("id"), &settings(), Utc::now())
        .unwrap();
    let load = manager.load_or_fetch(scope("id"), || Some(settings()));

    assert!(matches!(load, SettingsLoad::Fetched { write: None, .. }));
    assert!(!dir.path().join(SETTINGS_CACHE_FILE).exists());
}

#[test]
fn load_or_fetch_does_not_persist_a_failed_fetch() {
    let (dir, manager) = temp_manager(SETTINGS_CACHE_TTL);
    assert!(matches!(
        manager.load_or_fetch(scope("id"), || None),
        SettingsLoad::FetchFailed
    ));
    assert!(!dir.path().join(SETTINGS_CACHE_FILE).exists());
}

#[test]
fn misses_on_identity_origin_or_ttl_mismatch() {
    let (dir, manager) = temp_manager(SETTINGS_CACHE_TTL);
    manager
        .persist(&scope("id"), &settings(), Utc::now())
        .unwrap();

    assert!(manager.load_fresh(&scope("other")).is_none());

    let other_origin = SettingsCacheScope {
        account: SettingsCacheAccount {
            origin: "https://other".to_owned(),
            ..account("id")
        },
        ..scope("id")
    };
    assert!(manager.load_fresh(&other_origin).is_none());

    let expired = SettingsCacheManager {
        ttl: std::time::Duration::ZERO,
        ..SettingsCacheManager::new(dir.path(), SettingsCacheMode::Enabled)
    };
    assert!(expired.load_fresh(&scope("id")).is_none());
}

fn cache_file(fetched_at: DateTime<Utc>) -> SettingsCache {
    SettingsCache {
        fetched_at,
        grok_version: xai_grok_version::VERSION.to_string(),
        identity: "id".to_string(),
        origin: ORIGIN.to_string(),
        client: CLIENT.to_owned(),
        settings: settings(),
    }
}

fn signed_cache_bytes(cache: &SettingsCache) -> Vec<u8> {
    let payload = serde_json::to_string(cache).unwrap();
    let signature = sign_cache_payload(payload.as_bytes());
    serde_json::to_vec(&SignedSettingsCache { payload, signature }).unwrap()
}

#[test]
fn hand_written_file_with_stale_version_or_client_misses() {
    let (dir, manager) = temp_manager(SETTINGS_CACHE_TTL);
    let path = dir.path().join(SETTINGS_CACHE_FILE);
    let base = || cache_file(Utc::now());
    let write = |c: &SettingsCache| std::fs::write(&path, signed_cache_bytes(c)).unwrap();

    write(&SettingsCache {
        grok_version: format!("{}-stale", xai_grok_version::VERSION),
        ..base()
    });
    assert!(manager.load_fresh(&scope("id")).is_none());

    write(&SettingsCache {
        client: "other-client".to_string(),
        ..base()
    });
    assert!(manager.load_fresh(&scope("id")).is_none());
}

#[test]
fn load_rejects_a_tampered_cache() {
    let (dir, manager) = temp_manager(SETTINGS_CACHE_TTL);
    let path = dir.path().join(SETTINGS_CACHE_FILE);
    manager
        .persist(&scope("id"), &settings(), Utc::now())
        .unwrap();
    assert!(manager.load_fresh(&scope("id")).is_some());

    let mut signed: SignedSettingsCache =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let mut payload: SettingsCache = serde_json::from_str(&signed.payload).unwrap();
    payload.settings.folder_trust_enabled = Some(false);
    signed.payload = serde_json::to_string(&payload).unwrap();
    std::fs::write(&path, serde_json::to_vec(&signed).unwrap()).unwrap();
    assert!(manager.load_fresh(&scope("id")).is_none());
}

#[test]
fn persist_replaces_a_newer_entry_this_process_cannot_read() {
    let newer = Utc::now() - ChronoDuration::seconds(30);
    let unreadable = || SettingsCache {
        settings: RemoteSettings::default(),
        ..cache_file(newer)
    };
    let cases = [
        (
            "another client",
            SettingsCache {
                client: format!("{CLIENT}-other"),
                ..unreadable()
            },
        ),
        (
            "another grok version",
            SettingsCache {
                grok_version: format!("{}-other", xai_grok_version::VERSION),
                ..unreadable()
            },
        ),
    ];
    for (label, entry) in cases {
        let (dir, manager) = temp_manager(SETTINGS_CACHE_TTL);
        std::fs::write(
            dir.path().join(SETTINGS_CACHE_FILE),
            signed_cache_bytes(&entry),
        )
        .unwrap();

        manager
            .persist(
                &scope("id"),
                &settings(),
                newer - ChronoDuration::seconds(60),
            )
            .unwrap();

        assert_eq!(
            Some(true),
            manager
                .load_fresh(&scope("id"))
                .unwrap()
                .into_settings()
                .leader_mode,
            "{label}",
        );
    }
}

#[test]
fn persist_does_not_regress_to_an_older_fetch() {
    let (dir, manager) = temp_manager(SETTINGS_CACHE_TTL);
    let path = dir.path().join(SETTINGS_CACHE_FILE);
    let alt = || RemoteSettings {
        leader_mode: Some(false),
        ..RemoteSettings::default()
    };

    let newer = Utc::now() - ChronoDuration::seconds(30);
    manager.persist(&scope("id"), &settings(), newer).unwrap();
    let stored = std::fs::read(&path).unwrap();

    manager
        .persist(&scope("id"), &alt(), newer - ChronoDuration::seconds(60))
        .unwrap();
    assert_eq!(
        stored,
        std::fs::read(&path).unwrap(),
        "an older fetch must not overwrite a newer entry",
    );

    manager.persist(&scope("id"), &alt(), Utc::now()).unwrap();
    assert_eq!(
        Some(false),
        manager
            .load_fresh(&scope("id"))
            .unwrap()
            .into_settings()
            .leader_mode,
    );
}

#[test]
fn read_keys_accept_a_superseded_signing_key() {
    use hmac::Mac;

    let old_key: &[u8] = b"grok-shell-settings-cache-hmac-v0-superseded";
    let payload = b"cache-payload";
    let mut mac = HmacSha256::new_from_slice(old_key).unwrap();
    mac.update(payload);
    let sig = mac.finalize().into_bytes().to_vec();

    assert!(
        !verify_with_keys(&[SETTINGS_CACHE_WRITE_HMAC_KEY], payload, &sig),
        "a signature from an untrusted key must not verify",
    );
    assert!(
        verify_with_keys(&[SETTINGS_CACHE_WRITE_HMAC_KEY, old_key], payload, &sig),
        "a signature from a retained read key must verify",
    );
}

#[test]
fn load_clears_fields_only_a_live_fetch_may_set() {
    let (dir, manager) = temp_manager(SETTINGS_CACHE_TTL);
    let mut cache = cache_file(Utc::now());
    cache.settings.managed_config_signature_verification = Some(false);
    cache.settings.accept_request_encodings = vec![xai_grok_config::RemoteRequestEncoding::Zstd];
    std::fs::write(
        dir.path().join(SETTINGS_CACHE_FILE),
        signed_cache_bytes(&cache),
    )
    .unwrap();

    let loaded = manager.load_fresh(&scope("id")).unwrap().into_settings();

    assert_eq!(None, loaded.managed_config_signature_verification);
    assert!(loaded.accept_request_encodings.is_empty());
}

#[test]
fn corrupt_cache_file_is_ignored() {
    let (dir, manager) = temp_manager(SETTINGS_CACHE_TTL);
    std::fs::write(dir.path().join(SETTINGS_CACHE_FILE), b"{ not json").unwrap();
    assert!(manager.load_fresh(&scope("id")).is_none());
}

fn padded_cache_file(len: usize) -> Vec<u8> {
    let mut json = signed_cache_bytes(&cache_file(Utc::now()));
    assert!(
        json.len() <= len,
        "base envelope already exceeds the target size"
    );
    json.resize(len, b' ');
    json
}

#[test]
fn load_enforces_the_size_cap() {
    let (dir, manager) = temp_manager(SETTINGS_CACHE_TTL);
    let path = dir.path().join(SETTINGS_CACHE_FILE);

    std::fs::write(
        &path,
        padded_cache_file(SETTINGS_CACHE_MAX_BYTES as usize + 1),
    )
    .unwrap();
    assert!(
        manager.load_fresh(&scope("id")).is_none(),
        "over the cap is rejected"
    );

    std::fs::write(&path, padded_cache_file(SETTINGS_CACHE_MAX_BYTES as usize)).unwrap();
    assert!(
        manager.load_fresh(&scope("id")).is_some(),
        "at the cap is accepted"
    );
}

#[test]
fn load_rejects_a_future_dated_cache() {
    let (dir, manager) = temp_manager(SETTINGS_CACHE_TTL);
    let future = cache_file(Utc::now() + ChronoDuration::hours(1));
    std::fs::write(
        dir.path().join(SETTINGS_CACHE_FILE),
        signed_cache_bytes(&future),
    )
    .unwrap();
    assert!(manager.load_fresh(&scope("id")).is_none());
}

#[test]
fn write_into_a_grok_home_that_is_a_file_reports_the_failure() {
    let dir = tempfile::tempdir().unwrap();
    let grok_home = dir.path().join("grok-home");
    std::fs::write(&grok_home, b"").unwrap();

    let error = SettingsCacheManager::new(&grok_home, SettingsCacheMode::Enabled)
        .write_through(&scope("id"), &settings(), Utc::now())
        .unwrap_err();

    assert_eq!(std::io::ErrorKind::AlreadyExists, error.kind());
}

#[cfg(unix)]
#[test]
fn persist_writes_an_owner_only_file() {
    use std::os::unix::fs::PermissionsExt;

    let (dir, manager) = temp_manager(SETTINGS_CACHE_TTL);

    manager
        .persist(&scope("id"), &settings(), Utc::now())
        .unwrap();

    let mode = std::fs::metadata(dir.path().join(SETTINGS_CACHE_FILE))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(0o600, mode & 0o777);
}
