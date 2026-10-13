//! Each function here calls the `xai-grok-cloud-config` startup settings getter with the shell's `policy_repair_pending` check.

use std::time::Duration;

use tokio_util::sync::CancellationToken;
use xai_grok_login::{GrokAuth, GrokComConfig};

use crate::util::config::RemoteSettings;
use xai_grok_cloud_config::managed_config::policy_repair_pending;

#[cfg(any(test, feature = "test-support"))]
pub use xai_grok_cloud_config::settings_get::reset_startup_settings_for_tests;

pub use xai_grok_cloud_config::settings_get::{SettingsOutcome, SettingsQuery, SettingsWait};

pub fn is_eligible(query: &SettingsQuery) -> bool {
    xai_grok_cloud_config::settings_get::is_eligible(query, policy_repair_pending)
}

#[cfg(any(test, feature = "test-support"))]
pub async fn get_settings(query: SettingsQuery) -> SettingsOutcome {
    xai_grok_cloud_config::settings_get::get_settings(query, policy_repair_pending).await
}

pub async fn await_startup_settings(
    query: SettingsQuery,
    deadline: Duration,
    cancel: &CancellationToken,
) -> SettingsWait {
    xai_grok_cloud_config::settings_get::await_startup_settings(
        query,
        deadline,
        cancel,
        policy_repair_pending,
    )
    .await
}

pub fn consume_wait(
    wait: SettingsWait,
    auth: Option<&GrokAuth>,
    grok_com_config: &GrokComConfig,
) -> Option<RemoteSettings> {
    xai_grok_cloud_config::settings_get::consume_wait(
        wait,
        auth,
        grok_com_config,
        policy_repair_pending,
    )
}

#[cfg(any(test, feature = "test-support"))]
pub async fn get_startup_settings(query: SettingsQuery) -> SettingsOutcome {
    xai_grok_cloud_config::settings_get::get_startup_settings(query, policy_repair_pending).await
}

pub fn warm_startup_settings(query: SettingsQuery) {
    xai_grok_cloud_config::settings_get::warm_startup_settings(query, policy_repair_pending);
}

pub fn block_on_startup_settings(
    query: SettingsQuery,
    deadline: Duration,
    cancel: &CancellationToken,
) -> SettingsWait {
    xai_grok_cloud_config::settings_get::block_on_startup_settings(
        query,
        deadline,
        cancel,
        policy_repair_pending,
    )
}
