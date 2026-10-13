//! Presentation policy for the model-facing `model` and `subagent_type` arguments of the task tool.

use std::sync::Arc;

use xai_tool_types::SubagentDescriptor;

use crate::types::definition::ToolDefinition;
use crate::types::template_renderer::TemplateRenderer;

/// Canonical name of the model argument, before any client parameter remapping.
pub const MODEL_PARAM: &str = "model";

/// Canonical name of the subagent type argument, before any client parameter remapping.
pub const SUBAGENT_TYPE_PARAM: &str = "subagent_type";

/// The schema enum lists at most this many types. The tool still accepts any valid type.
const MAX_SELECTABLE_SUBAGENT_TYPES: usize = 64;

/// Each type's description is cut to this many bytes in the schema.
const MAX_SUBAGENT_TYPE_DESCRIPTION_BYTES: usize = 200;

/// Types with longer names are left out of the schema, since a cut name would not match the agent.
const MAX_SUBAGENT_TYPE_NAME_BYTES: usize = 128;

/// Whether the model may pick a child model explicitly or every child inherits the defaults.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskModelSelection {
    #[default]
    Selectable,
    Inherited,
}

/// Task tool configuration, stored as `Params<TaskParams>`.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TaskParams {
    #[serde(default)]
    pub model_selection: TaskModelSelection,
    /// Fresh spawns that omit a type use this when general-purpose is not spawnable
    /// and the parent allowlist names exactly one other type. Not a model argument.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub implicit_subagent_type: Option<String>,
    /// Plugin and user-defined types the schema offers as an optional `subagent_type` enum.
    /// Empty leaves the argument out of the schema.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selectable_subagent_types: Vec<SubagentDescriptor>,
}

crate::register_resource!("grok_build", "TaskParams", TaskParams);

/// `ToolMetadata::versioned_definition` for a task tool, over the schema filtered by `TaskParams`.
pub fn task_versioned_definition(
    client_name: &str,
    description_override: Option<&str>,
    description_template: &str,
    renderer: &TemplateRenderer,
    param_map: &std::collections::HashMap<String, String>,
    input_schema: &serde_json::Value,
    effective_params: &serde_json::Value,
) -> ToolDefinition {
    let raw_desc = description_override.unwrap_or(description_template);
    let description = renderer.render(raw_desc).unwrap_or_else(|e| {
        crate::types::template_renderer::strip_markers_on_render_failure(raw_desc, &e)
    });
    let exported = exported_input_schema(input_schema, &task_params_from(effective_params));
    let remapped = if param_map.is_empty() {
        exported
    } else {
        crate::util::remap::remap_schema_properties(&exported, param_map)
    };
    ToolDefinition::function(client_name, Some(&description), remapped)
}

/// Finalize validated the params before merging, so a parse failure here only logs.
fn task_params_from(effective_params: &serde_json::Value) -> TaskParams {
    match serde_json::from_value::<TaskParams>(effective_params.clone()) {
        Ok(params) => params,
        Err(error) => {
            tracing::warn!(%error, "task params did not parse; keeping the default task schema");
            TaskParams::default()
        }
    }
}

/// Runs before parameter remapping. Its keys are the canonical names.
fn exported_input_schema(
    input_schema: &serde_json::Value,
    params: &TaskParams,
) -> serde_json::Value {
    let mut schema = input_schema.clone();
    let Some(obj) = schema.as_object_mut() else {
        return schema;
    };

    if params.model_selection == TaskModelSelection::Inherited {
        if let Some(props) = obj.get_mut("properties").and_then(|p| p.as_object_mut()) {
            props.remove(MODEL_PARAM);
        }
        if let Some(required) = obj.get_mut("required").and_then(|r| r.as_array_mut()) {
            required.retain(|name| name.as_str() != Some(MODEL_PARAM));
        }
    }

    let listed = listed_subagent_types(&params.selectable_subagent_types);
    if !listed.is_empty()
        && let Some(props) = obj.get_mut("properties").and_then(|p| p.as_object_mut())
    {
        props.insert(
            SUBAGENT_TYPE_PARAM.to_owned(),
            subagent_type_property(&listed),
        );
    }
    schema
}

/// The types the schema lists, within the name, count, and order limits.
fn listed_subagent_types(types: &[SubagentDescriptor]) -> Vec<&SubagentDescriptor> {
    let fitting: Vec<_> = types
        .iter()
        .filter(|t| t.name.len() <= MAX_SUBAGENT_TYPE_NAME_BYTES)
        .collect();
    if fitting.len() < types.len() {
        tracing::warn!(
            skipped = types.len() - fitting.len(),
            limit = MAX_SUBAGENT_TYPE_NAME_BYTES,
            "subagent type names over the byte limit are not listed in the schema"
        );
    }
    if fitting.len() > MAX_SELECTABLE_SUBAGENT_TYPES {
        tracing::warn!(
            count = fitting.len(),
            limit = MAX_SELECTABLE_SUBAGENT_TYPES,
            "too many selectable subagent types; the schema lists the first ones only"
        );
    }
    fitting
        .into_iter()
        .take(MAX_SELECTABLE_SUBAGENT_TYPES)
        .collect()
}

/// An optional enum over `types`. An omitted value keeps the general-purpose default.
fn subagent_type_property(types: &[&SubagentDescriptor]) -> serde_json::Value {
    let names: Vec<&str> = types.iter().map(|t| t.name.as_str()).collect();
    let lines: Vec<String> = types.iter().map(|t| subagent_type_line(t)).collect();
    serde_json::json!({
        "type": "string",
        "enum": names,
        "description": format!(
            "Omit for a general-purpose subagent. Set it to hand the task to one of these agents:\n{}",
            lines.join("\n")
        ),
    })
}

/// One `- name: description` line. The description loses its line breaks and is cut to the byte limit.
fn subagent_type_line(t: &SubagentDescriptor) -> String {
    let description = t
        .description
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let description =
        crate::util::truncate_str_with_marker(&description, MAX_SUBAGENT_TYPE_DESCRIPTION_BYTES);
    format!("- {}: {description}", t.name)
}

/// `param_name` is the client-facing spelling; the catalog is never enumerated.
pub fn hidden_selection_message(param_name: &str) -> String {
    format!(
        "Explicit subagent model selection is unavailable for this catalog. Retry without the \
         {param_name} argument; configured defaults will apply."
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskModelRejection {
    HiddenSelection,
}

/// Host-injected observer; this crate emits no telemetry itself.
#[derive(Clone)]
pub struct TaskModelRejectionSink(Arc<dyn Fn(TaskModelRejection) + Send + Sync>);

impl TaskModelRejectionSink {
    pub fn new(observe: impl Fn(TaskModelRejection) + Send + Sync + 'static) -> Self {
        Self(Arc::new(observe))
    }

    pub fn notify(&self, rejection: TaskModelRejection) {
        (self.0)(rejection)
    }
}

impl std::fmt::Debug for TaskModelRejectionSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskModelRejectionSink").finish()
    }
}

crate::register_resource!(
    "grok_build",
    "TaskModelRejectionSink",
    TaskModelRejectionSink
);

#[cfg(test)]
#[path = "model_policy_tests.rs"]
mod tests;
