use chrono::DateTime;
use chrono::Duration;
use chrono::Utc;
use xai_grok_config::CampaignOverride;
use xai_grok_login::GrokAuth;
use xai_grok_login::model::AuthStore;

use super::*;
use crate::settings_cache::SettingsCacheManager;
use crate::settings_cache::SettingsCacheScope;

const LOCAL_PROXY: &str = "https://proxy.example/v1";
const CAMPAIGN_PROXY: &str = "https://campaign.example/v1";

struct Case<'a> {
    label: &'static str,
    writer_config: toml::Value,
    token_expires_at: DateTime<Utc>,
    reader_layers: &'a ConfigLayers,
    cache_mode: SettingsCacheMode,
    leader_mode: Option<Option<bool>>,
    proxy: &'static str,
}

fn layers(user: &str) -> ConfigLayers {
    ConfigLayers {
        user: toml::from_str(user).unwrap(),
        ..ConfigLayers::default()
    }
}

fn sign_in(
    grok_home: &Path,
    writer_config: &toml::Value,
    token_expires_at: DateTime<Utc>,
) -> SettingsCacheScope {
    let auth = GrokAuth {
        user_id: "user-1".to_owned(),
        expires_at: Some(token_expires_at),
        ..GrokAuth::test_default()
    };

    let login_config = GrokComConfig::from_effective_config(writer_config)
        .expect("writer config has a readable [grok_com_config]");
    std::fs::write(
        xai_grok_login::auth_json_path(grok_home),
        serde_json::to_vec(&AuthStore::from([(
            login_config.auth_scope(),
            auth.clone(),
        )]))
        .expect("auth store serializes"),
    )
    .expect("write auth.json");

    SettingsEndpoint::from(&EndpointsConfig::from_config_value(writer_config))
        .cache_scope(&auth, "grok-shell".to_owned())
}

#[test]
fn reader_uses_only_the_entry_fetched_from_the_endpoint_its_campaigns_name() {
    let endpoint = format!("[endpoints]\ncli_chat_proxy_base_url = \"{LOCAL_PROXY}\"\n");
    let fetch_on = layers(&endpoint);
    let fetch_off = layers(&format!("{endpoint}[features]\nremote_fetch = false\n"));
    let settings = RemoteSettings {
        leader_mode: Some(true),
        campaigns: vec![CampaignOverride {
            id: Some("move-the-proxy".to_owned()),
            patch: serde_json::from_value(
                serde_json::json!({"endpoints": {"cli_chat_proxy_base_url": CAMPAIGN_PROXY}}),
            )
            .unwrap(),
        }],
        ..RemoteSettings::default()
    };
    let before_campaign = CampaignOverlay::new(&fetch_on, &CampaignSources::default()).effective;
    let after_campaign = CampaignOverlay::new(
        &fetch_on,
        &CampaignSources {
            remote: remote_campaigns_from_settings(Some(&settings)),
            ..CampaignSources::default()
        },
    )
    .effective;
    let live = Utc::now() + Duration::hours(1);
    let cases = [
        Case {
            label: "fetched after the campaign moved the proxy",
            writer_config: after_campaign.clone(),
            token_expires_at: live,
            reader_layers: &fetch_on,
            cache_mode: SettingsCacheMode::Enabled,
            leader_mode: Some(Some(true)),
            proxy: CAMPAIGN_PROXY,
        },
        Case {
            label: "an expired token still names its account",
            writer_config: after_campaign.clone(),
            token_expires_at: Utc::now() - Duration::hours(1),
            reader_layers: &fetch_on,
            cache_mode: SettingsCacheMode::Enabled,
            leader_mode: Some(Some(true)),
            proxy: CAMPAIGN_PROXY,
        },
        Case {
            label: "fetched before the campaign moved the proxy",
            writer_config: before_campaign,
            token_expires_at: live,
            reader_layers: &fetch_on,
            cache_mode: SettingsCacheMode::Enabled,
            leader_mode: None,
            proxy: LOCAL_PROXY,
        },
        Case {
            label: "the given mode keeps the cache off",
            writer_config: after_campaign.clone(),
            token_expires_at: live,
            reader_layers: &fetch_on,
            cache_mode: SettingsCacheMode::Disabled,
            leader_mode: None,
            proxy: LOCAL_PROXY,
        },
        Case {
            label: "remote_fetch = false keeps the cached entry out",
            writer_config: after_campaign,
            token_expires_at: live,
            reader_layers: &fetch_off,
            cache_mode: SettingsCacheMode::Enabled,
            leader_mode: None,
            proxy: LOCAL_PROXY,
        },
    ];
    for case in cases {
        let grok_home = tempfile::tempdir().unwrap();
        let scope = sign_in(grok_home.path(), &case.writer_config, case.token_expires_at);
        SettingsCacheManager::new(grok_home.path(), SettingsCacheMode::Enabled)
            .write_through(&scope, &settings, Utc::now())
            .unwrap();

        let config = load_config_with_cached_remote(CachedConfigInputs {
            grok_home: Some(grok_home.path()),
            layers: case.reader_layers,
            campaign_env: &CampaignEnv::default(),
            cache_mode: case.cache_mode,
        });

        let leader_mode = match config.remote {
            CachedRemote::Entry(remote) => Some(remote.leader_mode),
            CachedRemote::NotApplicable | CachedRemote::Unavailable => None,
        };
        assert_eq!(case.leader_mode, leader_mode, "{}", case.label);
        let proxy = config
            .effective
            .get("endpoints")
            .and_then(|endpoints| endpoints.get("cli_chat_proxy_base_url"))
            .and_then(toml::Value::as_str);
        assert_eq!(Some(case.proxy), proxy, "{}", case.label);
    }
}

fn read_cached_remote(grok_home: &Path, reader_layers: &ConfigLayers) -> CachedRemote {
    load_config_with_cached_remote(CachedConfigInputs {
        grok_home: Some(grok_home),
        layers: reader_layers,
        campaign_env: &CampaignEnv::default(),
        cache_mode: SettingsCacheMode::Enabled,
    })
    .remote
}

#[test]
fn stale_entry_can_only_turn_vendor_hooks_off() {
    let reader_layers = layers(&format!(
        "[endpoints]\ncli_chat_proxy_base_url = \"{LOCAL_PROXY}\"\n"
    ));
    let writer_config = CampaignOverlay::new(&reader_layers, &CampaignSources::default()).effective;
    let token_expires_at = Utc::now() + Duration::hours(1);
    let stale = Utc::now() - Duration::hours(2);
    let cases = [
        ("fresh enable", Utc::now(), true, Some(true)),
        ("stale enable", stale, true, None),
        ("stale disable", stale, false, Some(false)),
    ];
    for (label, fetched_at, claude_hooks_enabled, expected) in cases {
        let grok_home = tempfile::tempdir().expect("create grok home");
        let scope = sign_in(grok_home.path(), &writer_config, token_expires_at);
        let settings = RemoteSettings {
            claude_hooks_enabled: Some(claude_hooks_enabled),
            ..RemoteSettings::default()
        };
        SettingsCacheManager::new(grok_home.path(), SettingsCacheMode::Enabled)
            .write_through(&scope, &settings, fetched_at)
            .expect("cache the entry");

        let claude_hooks_enabled = match read_cached_remote(grok_home.path(), &reader_layers) {
            CachedRemote::Entry(remote) => Some(remote.claude_hooks_enabled),
            CachedRemote::NotApplicable | CachedRemote::Unavailable => None,
        };
        assert_eq!(Some(expected), claude_hooks_enabled, "{label}");
    }
}

#[test]
fn signed_in_account_without_an_entry_is_unavailable() {
    let reader_layers = layers(&format!(
        "[endpoints]\ncli_chat_proxy_base_url = \"{LOCAL_PROXY}\"\n"
    ));
    let writer_config = CampaignOverlay::new(&reader_layers, &CampaignSources::default()).effective;
    for (signed_in, expected) in [(true, "unavailable"), (false, "not applicable")] {
        let grok_home = tempfile::tempdir().expect("create grok home");
        if signed_in {
            sign_in(
                grok_home.path(),
                &writer_config,
                Utc::now() + Duration::hours(1),
            );
        }

        let remote = match read_cached_remote(grok_home.path(), &reader_layers) {
            CachedRemote::Entry(_) => "entry",
            CachedRemote::NotApplicable => "not applicable",
            CachedRemote::Unavailable => "unavailable",
        };
        assert_eq!(expected, remote, "signed in: {signed_in}");
    }
}
