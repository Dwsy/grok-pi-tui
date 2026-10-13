use std::ops::Deref;

use serde::Deserialize;
use strum::IntoEnumIterator;

use crate::validation::RequirementsSource;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AllowlistPin {
    List(Vec<String>),
    FailClosed,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    strum::EnumIter,
    strum::EnumString,
    strum::IntoStaticStr,
)]
#[strum(serialize_all = "snake_case")]
pub enum ToolFeature {
    AskUserQuestion,
    ImageEdit,
    ImageGen,
    LspTools,
    VideoGen,
    WebFetch,
    WriteFile,
}

impl ToolFeature {
    #[must_use]
    pub fn tool_names(self) -> &'static [&'static str] {
        match self {
            ToolFeature::AskUserQuestion => &["ask_user_question"],
            ToolFeature::ImageEdit => &["image_edit"],
            ToolFeature::ImageGen => &["image_gen"],
            ToolFeature::LspTools => &["lsp"],
            ToolFeature::VideoGen => &["image_to_video", "reference_to_video"],
            ToolFeature::WebFetch => &["web_fetch"],
            ToolFeature::WriteFile => &["write"],
        }
    }

    fn pin(self, parsed: &RequirementsToml) -> Option<bool> {
        match self {
            ToolFeature::AskUserQuestion => parsed.features.ask_user_question,
            ToolFeature::ImageEdit => parsed.features.image_edit,
            ToolFeature::ImageGen => parsed.features.image_gen,
            ToolFeature::LspTools => parsed.features.lsp_tools,
            ToolFeature::VideoGen => parsed.features.video_gen,
            ToolFeature::WebFetch => parsed.features.web_fetch,
            ToolFeature::WriteFile => parsed.features.write_file,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct ModelRequirements {
    #[serde(default, deserialize_with = "de_allowlist")]
    pub allowed_models: Option<AllowlistPin>,
    #[serde(default, rename = "default", deserialize_with = "de_string")]
    pub default_model: Option<String>,
    #[serde(default, deserialize_with = "de_string")]
    pub web_search: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct FeatureRequirements {
    #[serde(default, deserialize_with = "de_ask_user_question")]
    pub ask_user_question: Option<bool>,
    #[serde(default, deserialize_with = "de_image_edit")]
    pub image_edit: Option<bool>,
    #[serde(default, deserialize_with = "de_image_gen")]
    pub image_gen: Option<bool>,
    #[serde(default, deserialize_with = "de_lsp_tools")]
    pub lsp_tools: Option<bool>,
    #[serde(default, deserialize_with = "de_video_gen")]
    pub video_gen: Option<bool>,
    #[serde(default, deserialize_with = "de_web_fetch")]
    pub web_fetch: Option<bool>,
    #[serde(default, deserialize_with = "de_write_file")]
    pub write_file: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct CliRequirements {
    #[serde(default, deserialize_with = "de_auto_update")]
    pub auto_update: Option<bool>,
    #[serde(default, deserialize_with = "de_use_leader")]
    pub use_leader: Option<bool>,
    #[serde(default, deserialize_with = "de_show_tips")]
    pub show_tips: Option<bool>,
    #[serde(default, deserialize_with = "de_string")]
    pub channel: Option<String>,
    #[serde(default, deserialize_with = "de_string")]
    pub minimum_version: Option<String>,
    #[serde(default, deserialize_with = "de_string")]
    pub maximum_version: Option<String>,
    #[serde(default, deserialize_with = "de_string")]
    pub required_minimum_version: Option<String>,
    #[serde(default, deserialize_with = "de_string")]
    pub required_maximum_version: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct MemoryRequirements {
    #[serde(default, deserialize_with = "de_memory_enabled")]
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct SubagentsRequirements {
    #[serde(default, deserialize_with = "de_subagents_enabled")]
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct ManagedMcpsRequirements {
    #[serde(default, deserialize_with = "de_managed_mcps_enabled")]
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct EndpointRequirements {
    #[serde(default, deserialize_with = "de_string")]
    pub models_base_url: Option<String>,
    #[serde(default, deserialize_with = "de_string")]
    pub models_list_url: Option<String>,
    #[serde(default, deserialize_with = "de_string")]
    pub trace_upload_url: Option<String>,
    #[serde(default, deserialize_with = "de_string")]
    pub feedback_base_url: Option<String>,
    #[serde(default, deserialize_with = "de_string")]
    pub deployment_key: Option<String>,
    #[serde(default, deserialize_with = "de_string")]
    pub trace_upload_bucket: Option<String>,
    #[serde(default, deserialize_with = "de_string")]
    pub trace_upload_region: Option<String>,
    #[serde(default, deserialize_with = "de_string")]
    pub trace_upload_credentials_file: Option<String>,
    #[serde(default, deserialize_with = "de_string")]
    pub trace_upload_endpoint_url: Option<String>,
    #[serde(default, deserialize_with = "de_string")]
    pub trace_upload_credentials: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct TelemetryRequirements {
    #[serde(default, deserialize_with = "de_string")]
    pub events_url: Option<String>,
    #[serde(default, deserialize_with = "de_string")]
    pub events_api_key: Option<String>,
    #[serde(default, deserialize_with = "de_mixpanel_enabled")]
    pub mixpanel_enabled: Option<bool>,
    #[serde(default, deserialize_with = "de_string")]
    pub mixpanel_token: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct RequirementsToml {
    #[serde(default, deserialize_with = "de_section")]
    pub models: ModelRequirements,
    #[serde(default, deserialize_with = "de_section")]
    pub features: FeatureRequirements,
    #[serde(default, deserialize_with = "de_section")]
    pub cli: CliRequirements,
    #[serde(default, deserialize_with = "de_section")]
    pub memory: MemoryRequirements,
    #[serde(default, deserialize_with = "de_section")]
    pub subagents: SubagentsRequirements,
    #[serde(default, deserialize_with = "de_section")]
    pub managed_mcps: ManagedMcpsRequirements,
    #[serde(default, deserialize_with = "de_section")]
    pub endpoints: EndpointRequirements,
    #[serde(default, deserialize_with = "de_section")]
    pub telemetry: TelemetryRequirements,
}

impl RequirementsToml {
    #[must_use]
    pub fn from_value(layer: &toml::Value) -> Self {
        Self::deserialize(layer.clone()).unwrap_or_default()
    }

    pub fn tool_pins(&self) -> impl Iterator<Item = (ToolFeature, bool)> + '_ {
        ToolFeature::iter().filter_map(|tool| tool.pin(self).map(|val| (tool, val)))
    }

    pub fn enforce_cli_toggles(
        &self,
        auto_update: &mut Option<bool>,
        use_leader: &mut Option<bool>,
        show_tips: &mut Option<bool>,
        push: &mut impl FnMut(&'static str, String),
    ) {
        write_opt(auto_update, self.cli.auto_update, "cli.auto_update", push);
        write_opt(use_leader, self.cli.use_leader, "cli.use_leader", push);
        write_opt(show_tips, self.cli.show_tips, "cli.show_tips", push);
    }

    pub fn enforce_service_toggles(
        &self,
        pins: &mut ServiceTogglePins<'_>,
        push: &mut impl FnMut(&'static str, String),
    ) {
        write_opt(
            pins.memory_enabled,
            self.memory.enabled,
            "memory.enabled",
            push,
        );
        write_flag(
            pins.subagents_enabled,
            self.subagents.enabled,
            "subagents.enabled",
            push,
        );
        write_flag(
            pins.managed_mcps_enabled,
            self.managed_mcps.enabled,
            "managed_mcps.enabled",
            push,
        );
    }

    pub fn enforce_web_search(
        &self,
        web_search: &mut Option<String>,
        push: &mut impl FnMut(&'static str, String),
    ) {
        write_text(
            web_search,
            self.models.web_search.as_deref(),
            "models.web_search",
            false,
            push,
        );
    }

    pub fn enforce_cli_strings(
        &self,
        pins: &mut CliStringPins<'_>,
        push: &mut impl FnMut(&'static str, String),
    ) {
        write_text(
            pins.channel,
            self.cli.channel.as_deref(),
            "cli.channel",
            false,
            push,
        );
        write_text(
            pins.minimum_version,
            self.cli.minimum_version.as_deref(),
            "cli.minimum_version",
            false,
            push,
        );
        write_text(
            pins.maximum_version,
            self.cli.maximum_version.as_deref(),
            "cli.maximum_version",
            false,
            push,
        );
        write_text(
            pins.required_minimum_version,
            self.cli.required_minimum_version.as_deref(),
            "cli.required_minimum_version",
            false,
            push,
        );
        write_text(
            pins.required_maximum_version,
            self.cli.required_maximum_version.as_deref(),
            "cli.required_maximum_version",
            false,
            push,
        );
    }

    pub fn enforce_model_urls(
        &self,
        models_base_url: &mut Option<String>,
        models_list_url: &mut Option<String>,
        push: &mut impl FnMut(&'static str, String),
    ) {
        write_text(
            models_base_url,
            self.endpoints.models_base_url.as_deref(),
            "endpoints.models_base_url",
            false,
            push,
        );
        write_text(
            models_list_url,
            self.endpoints.models_list_url.as_deref(),
            "endpoints.models_list_url",
            false,
            push,
        );
    }

    pub fn enforce_upload_and_telemetry(
        &self,
        pins: &mut UploadTelemetryPins<'_>,
        push: &mut impl FnMut(&'static str, String),
    ) {
        write_text(
            pins.trace_upload_url,
            self.endpoints.trace_upload_url.as_deref(),
            "endpoints.trace_upload_url",
            false,
            push,
        );
        write_text(
            pins.feedback_base_url,
            self.endpoints.feedback_base_url.as_deref(),
            "endpoints.feedback_base_url",
            false,
            push,
        );
        write_text(
            pins.deployment_key,
            self.endpoints.deployment_key.as_deref(),
            "endpoints.deployment_key",
            true,
            push,
        );
        write_text(
            pins.events_url,
            self.telemetry.events_url.as_deref(),
            "telemetry.events_url",
            false,
            push,
        );
        write_text(
            pins.events_api_key,
            self.telemetry.events_api_key.as_deref(),
            "telemetry.events_api_key",
            true,
            push,
        );
        write_flag(
            pins.mixpanel_enabled,
            self.telemetry.mixpanel_enabled,
            "telemetry.mixpanel_enabled",
            push,
        );
        write_text(
            pins.mixpanel_token,
            self.telemetry.mixpanel_token.as_deref(),
            "telemetry.mixpanel_token",
            true,
            push,
        );
        write_text(
            pins.trace_upload_bucket,
            self.endpoints.trace_upload_bucket.as_deref(),
            "endpoints.trace_upload_bucket",
            false,
            push,
        );
        write_text(
            pins.trace_upload_region,
            self.endpoints.trace_upload_region.as_deref(),
            "endpoints.trace_upload_region",
            false,
            push,
        );
        write_text(
            pins.trace_upload_credentials_file,
            self.endpoints.trace_upload_credentials_file.as_deref(),
            "endpoints.trace_upload_credentials_file",
            false,
            push,
        );
        write_text(
            pins.trace_upload_endpoint_url,
            self.endpoints.trace_upload_endpoint_url.as_deref(),
            "endpoints.trace_upload_endpoint_url",
            false,
            push,
        );
        write_text(
            pins.trace_upload_credentials,
            self.endpoints.trace_upload_credentials.as_deref(),
            "endpoints.trace_upload_credentials",
            true,
            push,
        );
    }
}

/// Destinations for the CLI string requirement pins.
pub struct CliStringPins<'a> {
    /// Receives `cli.channel`.
    pub channel: &'a mut Option<String>,
    /// Receives `cli.minimum_version`.
    pub minimum_version: &'a mut Option<String>,
    /// Receives `cli.maximum_version`.
    pub maximum_version: &'a mut Option<String>,
    /// Receives `cli.required_minimum_version`.
    pub required_minimum_version: &'a mut Option<String>,
    /// Receives `cli.required_maximum_version`.
    pub required_maximum_version: &'a mut Option<String>,
}

/// Destinations for the memory, subagent, and managed-MCP requirement pins.
pub struct ServiceTogglePins<'a> {
    /// Receives `memory.enabled`.
    pub memory_enabled: &'a mut Option<bool>,
    /// Receives `subagents.enabled`.
    pub subagents_enabled: &'a mut bool,
    /// Receives `managed_mcps.enabled`.
    pub managed_mcps_enabled: &'a mut bool,
}

pub struct UploadTelemetryPins<'a> {
    pub trace_upload_url: &'a mut Option<String>,
    pub feedback_base_url: &'a mut Option<String>,
    pub deployment_key: &'a mut Option<String>,
    pub events_url: &'a mut Option<String>,
    pub events_api_key: &'a mut Option<String>,
    pub mixpanel_enabled: &'a mut bool,
    pub mixpanel_token: &'a mut Option<String>,
    pub trace_upload_bucket: &'a mut Option<String>,
    pub trace_upload_region: &'a mut Option<String>,
    pub trace_upload_credentials_file: &'a mut Option<String>,
    pub trace_upload_endpoint_url: &'a mut Option<String>,
    pub trace_upload_credentials: &'a mut Option<String>,
}

fn write_opt(
    field: &mut Option<bool>,
    next: Option<bool>,
    path: &'static str,
    push: &mut impl FnMut(&'static str, String),
) {
    let Some(next) = next else { return };
    if *field == Some(next) {
        return;
    }
    *field = Some(next);
    push(path, next.to_string());
}

fn write_flag(
    field: &mut bool,
    next: Option<bool>,
    path: &'static str,
    push: &mut impl FnMut(&'static str, String),
) {
    let Some(next) = next else { return };
    if *field == next {
        return;
    }
    *field = next;
    push(path, next.to_string());
}

fn write_text(
    field: &mut Option<String>,
    next: Option<&str>,
    path: &'static str,
    redacted: bool,
    push: &mut impl FnMut(&'static str, String),
) {
    let Some(next) = next else { return };
    if field.as_deref() == Some(next) {
        return;
    }
    let shown = if redacted {
        "[redacted]".to_owned()
    } else {
        next.to_owned()
    };
    *field = Some(next.to_owned());
    push(path, shown);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sourced<T> {
    pub value: T,
    pub source: RequirementsSource,
}

impl<T> Sourced<T> {
    #[must_use]
    pub fn new(value: T, source: RequirementsSource) -> Self {
        Self { value, source }
    }
}

impl<T> Deref for Sourced<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

// An Err from de_allowlist drops the rest of the models section
fn de_allowlist<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<AllowlistPin>, D::Error> {
    let value = toml::Value::deserialize(deserializer)?;
    let Some(entries) = value.as_array() else {
        tracing::error!(
            section = "models",
            key = "allowed_models",
            kind = value.type_str(),
            "requirements value is not an array; the constraint fail-closes"
        );
        return Ok(Some(AllowlistPin::FailClosed));
    };
    let mut patterns = Vec::with_capacity(entries.len());
    for entry in entries {
        let Some(pattern) = entry.as_str() else {
            tracing::error!(
                section = "models",
                key = "allowed_models",
                kind = entry.type_str(),
                "requirements array entry is not a string; the constraint fail-closes"
            );
            return Ok(Some(AllowlistPin::FailClosed));
        };
        patterns.push(pattern.to_owned());
    }
    Ok(Some(AllowlistPin::List(patterns)))
}

fn de_string<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    let value = toml::Value::deserialize(deserializer)?;
    Ok(value.as_str().map(str::to_owned))
}

fn de_section<'de, T, D>(deserializer: D) -> Result<T, D::Error>
where
    T: Deserialize<'de> + Default,
    D: serde::Deserializer<'de>,
{
    let value = toml::Value::deserialize(deserializer)?;
    if value.is_table() {
        Ok(T::deserialize(value).unwrap_or_default())
    } else {
        Ok(T::default())
    }
}

macro_rules! de_bool {
    ($name:ident, $section:literal, $key:literal, $message:literal) => {
        fn $name<'de, D: serde::Deserializer<'de>>(
            deserializer: D,
        ) -> Result<Option<bool>, D::Error> {
            let value = toml::Value::deserialize(deserializer)?;
            match value.as_bool() {
                Some(pin) => Ok(Some(pin)),
                None => {
                    tracing::error!(
                        section = $section,
                        key = $key,
                        kind = value.type_str(),
                        $message
                    );
                    Ok(None)
                }
            }
        }
    };
}

macro_rules! de_pin {
    ($name:ident, $key:literal) => {
        de_bool!(
            $name,
            "features",
            $key,
            "requirements pin is not a boolean; the pin is ignored until the next launch, which \
             will refuse to start"
        );
    };
}

de_pin!(de_ask_user_question, "ask_user_question");
de_pin!(de_image_edit, "image_edit");
de_pin!(de_image_gen, "image_gen");
de_pin!(de_lsp_tools, "lsp_tools");
de_pin!(de_video_gen, "video_gen");
de_pin!(de_web_fetch, "web_fetch");
de_pin!(de_write_file, "write_file");
de_bool!(
    de_auto_update,
    "cli",
    "auto_update",
    "requirements value is not a boolean; the constraint is not applied"
);
de_bool!(
    de_use_leader,
    "cli",
    "use_leader",
    "requirements value is not a boolean; the constraint is not applied"
);
de_bool!(
    de_show_tips,
    "cli",
    "show_tips",
    "requirements value is not a boolean; the constraint is not applied"
);
de_bool!(
    de_memory_enabled,
    "memory",
    "enabled",
    "requirements value is not a boolean; the constraint is not applied"
);
de_bool!(
    de_subagents_enabled,
    "subagents",
    "enabled",
    "requirements value is not a boolean; the constraint is not applied"
);
de_bool!(
    de_managed_mcps_enabled,
    "managed_mcps",
    "enabled",
    "requirements value is not a boolean; the constraint is not applied"
);
de_bool!(
    de_mixpanel_enabled,
    "telemetry",
    "mixpanel_enabled",
    "requirements value is not a boolean; the constraint is not applied"
);

#[cfg(test)]
#[path = "config_requirements_tests.rs"]
mod tests;
