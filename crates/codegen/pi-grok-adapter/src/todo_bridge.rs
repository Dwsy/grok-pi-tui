//! Project Pi todo-plugin tool results onto ACP `Plan` updates.
//!
//! Grok's native TodoPane is driven exclusively by `SessionUpdate::Plan`.
//! Pi plugins such as `@juicesharp/rpiv-todo` expose a full task snapshot under
//! tool-result `details`; this module translates those snapshots without
//! rendering any UI (adapter stays headless).
//!
//! Sources are registered in a small registry so additional todo plugins can
//! be added later without touching the tool-end / history main path.

use agent_client_protocol as acp;
use serde_json::{Map, Value};

/// Match a Pi tool name and, when successful, build a full-replace Plan.
pub trait TodoSource: Send + Sync {
    fn matches(&self, tool_name: &str) -> bool;
    /// `None` means "do not refresh the native TodoPane" (wrong shape, error, etc.).
    fn plan_from_result(&self, result: &Value, is_error: bool) -> Option<acp::Plan>;
}

/// `@juicesharp/rpiv-todo` — tool name `"todo"`, snapshot at `details.tasks`.
#[derive(Debug, Default, Clone, Copy)]
pub struct RpivTodoSource;

impl TodoSource for RpivTodoSource {
    fn matches(&self, tool_name: &str) -> bool {
        tool_name == "todo"
    }

    fn plan_from_result(&self, result: &Value, is_error: bool) -> Option<acp::Plan> {
        if is_error {
            return None;
        }
        let details = rpiv_details(result)?;
        if details.get("error").is_some_and(|e| !e.is_null()) {
            // Validation / transition failures keep the previous pane snapshot.
            return None;
        }
        let tasks = details
            .get("tasks")
            .or_else(|| details.get("todos"))?
            .as_array()?;
        let entries: Vec<acp::PlanEntry> =
            tasks.iter().filter_map(rpiv_task_to_plan_entry).collect();
        Some(acp::Plan::new(entries))
    }
}

/// Registry of todo sources. Default: only `RpivTodoSource`.
#[derive(Default)]
pub struct TodoSourceRegistry {
    sources: Vec<Box<dyn TodoSource>>,
}

impl TodoSourceRegistry {
    pub fn with_defaults() -> Self {
        let mut registry = Self::default();
        registry.register(RpivTodoSource);
        registry
    }

    pub fn register<S: TodoSource + 'static>(&mut self, source: S) {
        self.sources.push(Box::new(source));
    }

    /// First matching source wins.
    pub fn plan_from_tool(
        &self,
        tool_name: &str,
        result: &Value,
        is_error: bool,
    ) -> Option<acp::Plan> {
        self.sources
            .iter()
            .find(|source| source.matches(tool_name))
            .and_then(|source| source.plan_from_result(result, is_error))
    }
}

/// Convenience for call sites that do not hold a long-lived registry.
pub fn plan_update_for_tool(tool_name: &str, result: &Value, is_error: bool) -> Option<acp::Plan> {
    TodoSourceRegistry::with_defaults().plan_from_tool(tool_name, result, is_error)
}

/// Live `tool_execution_end` stores the full tool envelope under `result`
/// (`{ content, details }`). History `ToolEnd.raw_output` is often just
/// `details`. Accept both shapes.
fn rpiv_details(result: &Value) -> Option<&Value> {
    if let Some(details) = result.get("details") {
        return Some(details);
    }
    // Already a details object (history path).
    if result.get("tasks").is_some() || result.get("todos").is_some() {
        return Some(result);
    }
    None
}

fn rpiv_task_to_plan_entry(task: &Value) -> Option<acp::PlanEntry> {
    let status = task
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("pending");
    // Tombstones and cancelled entries stay out of the native pane.
    if matches!(status, "deleted" | "cancelled") {
        return None;
    }
    let subject = task
        .get("subject")
        .or_else(|| task.get("content"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    let plan_status = match status {
        "in_progress" => acp::PlanEntryStatus::InProgress,
        "completed" => acp::PlanEntryStatus::Completed,
        // pending and any unknown future status
        _ => acp::PlanEntryStatus::Pending,
    };

    let mut meta = Map::new();
    if let Some(id) = task.get("id").and_then(Value::as_i64) {
        meta.insert("rpivId".into(), Value::Number(id.into()));
    } else if let Some(id) = task.get("id").and_then(Value::as_u64) {
        meta.insert("rpivId".into(), Value::Number(id.into()));
    }
    if let Some(active) = task
        .get("activeForm")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        meta.insert("activeForm".into(), Value::String(active.to_string()));
    }
    if let Some(owner) = task
        .get("owner")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        meta.insert("owner".into(), Value::String(owner.to_string()));
    }
    if let Some(blocked) = task.get("blockedBy").and_then(Value::as_array) {
        meta.insert("blockedBy".into(), Value::Array(blocked.clone()));
    }

    let entry = acp::PlanEntry::new(subject, acp::PlanEntryPriority::Medium, plan_status);
    if meta.is_empty() {
        Some(entry)
    } else {
        Some(entry.meta(Some(meta)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_tasks() -> Value {
        json!([
            {
                "id": 1,
                "subject": "Wire adapter",
                "status": "completed",
            },
            {
                "id": 2,
                "subject": "Ship tests",
                "status": "in_progress",
                "activeForm": "writing tests",
            },
            {
                "id": 3,
                "subject": "Polish docs",
                "status": "pending",
            },
            {
                "id": 4,
                "subject": "Gone",
                "status": "deleted",
            }
        ])
    }

    #[test]
    fn rpiv_live_envelope_projects_plan_and_drops_deleted() {
        let result = json!({
            "content": [{ "type": "text", "text": "Updated #2" }],
            "details": {
                "action": "update",
                "params": {},
                "tasks": sample_tasks(),
                "nextId": 5,
            }
        });
        let plan = plan_update_for_tool("todo", &result, false).expect("plan");
        assert_eq!(plan.entries.len(), 3);
        assert_eq!(plan.entries[0].content, "Wire adapter");
        assert_eq!(plan.entries[0].status, acp::PlanEntryStatus::Completed);
        assert_eq!(plan.entries[1].status, acp::PlanEntryStatus::InProgress);
        assert_eq!(plan.entries[2].status, acp::PlanEntryStatus::Pending);
        assert_eq!(
            plan.entries[1]
                .meta
                .as_ref()
                .and_then(|m| m.get("activeForm"))
                .and_then(Value::as_str),
            Some("writing tests")
        );
        assert_eq!(
            plan.entries[0]
                .meta
                .as_ref()
                .and_then(|m| m.get("rpivId"))
                .and_then(Value::as_i64),
            Some(1)
        );
    }

    #[test]
    fn rpiv_history_details_shape_also_projects() {
        let details = json!({
            "action": "list",
            "params": {},
            "tasks": sample_tasks(),
            "nextId": 5,
        });
        let plan = plan_update_for_tool("todo", &details, false).expect("plan");
        assert_eq!(plan.entries.len(), 3);
    }

    #[test]
    fn clear_emits_empty_plan() {
        let result = json!({
            "content": [{ "type": "text", "text": "Cleared 2 tasks" }],
            "details": {
                "action": "clear",
                "params": {},
                "tasks": [],
                "nextId": 1,
            }
        });
        let plan = plan_update_for_tool("todo", &result, false).expect("plan");
        assert!(plan.entries.is_empty());
    }

    #[test]
    fn error_flag_or_details_error_skips_refresh() {
        let result = json!({
            "content": [{ "type": "text", "text": "Error: bad" }],
            "details": {
                "action": "update",
                "params": {},
                "tasks": sample_tasks(),
                "nextId": 5,
                "error": "invalid transition",
            }
        });
        assert!(plan_update_for_tool("todo", &result, false).is_none());
        assert!(
            plan_update_for_tool(
                "todo",
                &json!({ "details": { "tasks": sample_tasks() } }),
                true
            )
            .is_none()
        );
    }

    #[test]
    fn v1_todos_fallback_projects_content_and_drops_cancelled() {
        let result = json!({
            "details": {
                "version": 1,
                "todos": [
                    { "id": "t1", "content": "Login", "status": "in_progress" },
                    { "id": "t2", "content": "Old step", "status": "cancelled" },
                    { "id": "t3", "content": "Verify", "status": "pending" }
                ]
            }
        });
        let plan = plan_update_for_tool("todo", &result, false).expect("plan");
        assert_eq!(plan.entries.len(), 2);
        assert_eq!(plan.entries[0].content, "Login");
        assert_eq!(plan.entries[0].status, acp::PlanEntryStatus::InProgress);
        assert_eq!(plan.entries[1].content, "Verify");
        assert_eq!(plan.entries[1].status, acp::PlanEntryStatus::Pending);
    }

    #[test]
    fn non_todo_tools_are_ignored() {
        let result = json!({
            "details": { "tasks": sample_tasks() }
        });
        assert!(plan_update_for_tool("bash", &result, false).is_none());
        assert!(plan_update_for_tool("TodoWrite", &result, false).is_none());
    }

    #[test]
    fn registry_can_host_additional_sources() {
        struct AlwaysEmpty;
        impl TodoSource for AlwaysEmpty {
            fn matches(&self, tool_name: &str) -> bool {
                tool_name == "other_todo"
            }
            fn plan_from_result(&self, _result: &Value, is_error: bool) -> Option<acp::Plan> {
                if is_error {
                    None
                } else {
                    Some(acp::Plan::new(vec![]))
                }
            }
        }
        let mut registry = TodoSourceRegistry::with_defaults();
        registry.register(AlwaysEmpty);
        assert!(
            registry
                .plan_from_tool("other_todo", &json!({}), false)
                .is_some()
        );
        assert!(
            registry
                .plan_from_tool("todo", &json!({ "details": { "tasks": [] } }), false)
                .is_some()
        );
    }
}
