//! Project Pi background-Bash custom messages into existing Grok task updates.

use serde_json::{Value, json};
use std::{collections::HashMap, time::SystemTime};

const BRIDGE_TYPE: &str = "pi-grok-background-bash/v1";

/// Pager's marker for a task that died with a previous process lifetime. It
/// finalizes the row without pushing a fresh failure block into scrollback.
const ORPHANED_SIGNAL: &str = "session_restart";

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum BackgroundBashProjection {
    Started {
        task_id: String,
        tool_call_id: String,
        command: String,
        cwd: String,
        output_file: String,
        description: Option<String>,
    },
    Completed {
        tool_call_id: String,
        task_snapshot: Value,
    },
}

/// Adapter-side mirror of what Pager has been told about a Pi-owned background
/// task. Pi's extension owns the child process; this only exists so a terminal
/// state is projected exactly once, and so tasks orphaned by a Pi restart can
/// still be settled.
#[derive(Debug)]
pub(crate) struct BackgroundBashTask {
    pub(crate) tool_call_id: String,
    pub(crate) command: String,
    pub(crate) cwd: String,
    pub(crate) output_file: String,
    pub(crate) started_at: SystemTime,
    pub(crate) completed: bool,
}

impl BackgroundBashTask {
    /// A task whose terminal state landed before its start was ever projected —
    /// a shell short enough to beat its own `tool_execution_end`. Recorded so a
    /// duplicate terminal state on the other channel is still dropped.
    pub(crate) fn completed_from_snapshot(tool_call_id: &str, task_snapshot: &Value) -> Self {
        Self {
            tool_call_id: tool_call_id.to_string(),
            command: optional_string(task_snapshot, "command").unwrap_or_default(),
            cwd: optional_string(task_snapshot, "cwd").unwrap_or_default(),
            output_file: optional_string(task_snapshot, "output_file").unwrap_or_default(),
            started_at: SystemTime::now(),
            completed: true,
        }
    }
}

/// Parse a Pi `message_end` custom message emitted by the private grok-pi Bash
/// extension. Unknown custom messages intentionally return `None` so they keep
/// their normal Pi message handling.
pub(crate) fn parse_background_bash_message(event: &Value) -> Option<BackgroundBashProjection> {
    let message = event.get("message").unwrap_or(event);
    if field_str(message, "role") != Some("custom")
        || field_str(message, "customType") != Some(BRIDGE_TYPE)
    {
        return None;
    }
    parse_background_bash_details(message.get("details").unwrap_or(message))
}

/// Parse the private `__pi_grok_bash_task__` status payload.
///
/// The extension publishes terminal state on this channel *in addition* to the
/// bridge message: `ui.setStatus` is fire-and-forget in Pi's RPC mode, so it
/// survives streaming, aborts and a cleared follow-up queue — none of which the
/// conversation message does.
pub(crate) fn parse_background_bash_status(payload: &Value) -> Option<BackgroundBashProjection> {
    parse_background_bash_details(payload)
}

fn parse_background_bash_details(details: &Value) -> Option<BackgroundBashProjection> {
    match field_str(details, "event")? {
        "started" => Some(BackgroundBashProjection::Started {
            task_id: required_string(details, "taskId")?,
            tool_call_id: required_string(details, "toolCallId")?,
            command: required_string(details, "command")?,
            cwd: required_string(details, "cwd")?,
            output_file: required_string(details, "outputFile")?,
            description: optional_string(details, "description"),
        }),
        "completed" => {
            let task_snapshot = details.get("taskSnapshot")?.clone();
            if task_snapshot
                .get("task_id")
                .and_then(Value::as_str)
                .is_none()
            {
                return None;
            }
            Some(BackgroundBashProjection::Completed {
                tool_call_id: required_string(details, "toolCallId")?,
                task_snapshot,
            })
        }
        _ => None,
    }
}

/// Record a lifecycle transition in the adapter mirror and report whether it
/// should be projected to Pager.
///
/// Terminal state arrives on two independent channels — the private status
/// channel and the bridge message — so only the first one may be forwarded.
/// Pager would otherwise render two completion blocks for a single task.
pub(crate) fn record_background_bash(
    tasks: &mut HashMap<String, BackgroundBashTask>,
    projection: &BackgroundBashProjection,
) -> bool {
    match projection {
        BackgroundBashProjection::Started {
            task_id,
            tool_call_id,
            command,
            cwd,
            output_file,
            ..
        } => {
            tasks
                .entry(task_id.clone())
                .or_insert_with(|| BackgroundBashTask {
                    tool_call_id: tool_call_id.clone(),
                    command: command.clone(),
                    cwd: cwd.clone(),
                    output_file: output_file.clone(),
                    started_at: SystemTime::now(),
                    completed: false,
                });
            true
        }
        BackgroundBashProjection::Completed {
            tool_call_id,
            task_snapshot,
        } => {
            let Some(task_id) = task_snapshot.get("task_id").and_then(Value::as_str) else {
                return false;
            };
            if tasks.get(task_id).is_some_and(|task| task.completed) {
                return false;
            }
            tasks
                .entry(task_id.to_string())
                .and_modify(|task| task.completed = true)
                // A shell short enough to beat its own `tool_execution_end`
                // settles before it was ever registered; Pager renders that as
                // a tombstone.
                .or_insert_with(|| {
                    BackgroundBashTask::completed_from_snapshot(tool_call_id, task_snapshot)
                });
            true
        }
    }
}

/// Settle every task still mirrored as running and hand back their terminal
/// projections. Used when the Pi process that owned the shells is gone.
pub(crate) fn drain_running_background_bash(
    tasks: &mut HashMap<String, BackgroundBashTask>,
) -> Vec<BackgroundBashProjection> {
    tasks
        .iter_mut()
        .filter(|(_, task)| !task.completed)
        .map(|(task_id, task)| {
            task.completed = true;
            orphaned_background_bash_completion(task_id, task)
        })
        .collect()
}

/// Terminal projection for a task whose owning Pi process is gone.
///
/// The shells are children of that process, so they died with it. Reporting
/// them as `session_restart` keeps the resumed transcript clean: Pager stops
/// the row's running animation without claiming the task failed *now*.
fn orphaned_background_bash_completion(
    task_id: &str,
    task: &BackgroundBashTask,
) -> BackgroundBashProjection {
    BackgroundBashProjection::Completed {
        tool_call_id: task.tool_call_id.clone(),
        task_snapshot: json!({
            "task_id": task_id,
            "command": task.command,
            "cwd": task.cwd,
            "start_time": system_time_wire(task.started_at),
            "end_time": system_time_wire(SystemTime::now()),
            "output": "",
            "output_file": task.output_file,
            "truncated": false,
            "exit_code": Value::Null,
            "signal": ORPHANED_SIGNAL,
            "completed": true,
            "kind": "bash",
            "block_waited": false,
            "explicitly_killed": false,
        }),
    }
}

/// serde's wire shape for `std::time::SystemTime`, matching what the extension
/// emits for its own snapshots.
fn system_time_wire(time: SystemTime) -> Value {
    let since_epoch = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    json!({
        "secs_since_epoch": since_epoch.as_secs(),
        "nanos_since_epoch": since_epoch.subsec_nanos(),
    })
}

/// Extract the immediate background-task registration from the private Bash
/// tool result. Tool lifecycle events are emitted synchronously, unlike a
/// custom message sent after a child process completes while Pi is streaming.
pub(crate) fn parse_background_bash_tool_result(
    tool_name: &str,
    tool_call_id: &str,
    args: Option<&Value>,
    result: &Value,
) -> Option<BackgroundBashProjection> {
    if tool_name != "bash" {
        return None;
    }
    let details = result.get("details")?;
    if details.get("background").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    // Prefer details.description; fall back to pi-grok-bash task_name / description args.
    let description = optional_string(details, "description").or_else(|| {
        args.and_then(|a| {
            optional_string(a, "description").or_else(|| optional_string(a, "task_name"))
        })
    });
    Some(BackgroundBashProjection::Started {
        task_id: required_string(details, "taskId")?,
        tool_call_id: tool_call_id.to_string(),
        command: required_string(details, "command")?,
        cwd: required_string(details, "cwd")?,
        output_file: required_string(details, "outputFile")?,
        description,
    })
}

/// Render a projection as the exact session-notification envelope consumed by
/// the existing Pager background-task handlers.
pub(crate) fn background_bash_notification(
    session_id: &str,
    projection: &BackgroundBashProjection,
) -> (&'static str, Value) {
    match projection {
        BackgroundBashProjection::Started {
            task_id,
            tool_call_id,
            command,
            cwd,
            output_file,
            description,
        } => (
            "x.ai/task_backgrounded",
            json!({
                "sessionId": session_id,
                "update": {
                    "sessionUpdate": "task_backgrounded",
                    "tool_call_id": tool_call_id,
                    "task_id": task_id,
                    "command": command,
                    "cwd": cwd,
                    "output_file": output_file,
                    "description": description,
                }
            }),
        ),
        BackgroundBashProjection::Completed { task_snapshot, .. } => (
            "x.ai/task_completed",
            json!({
                "sessionId": session_id,
                "update": {
                    "sessionUpdate": "task_completed",
                    "task_snapshot": task_snapshot,
                    "will_wake": false,
                }
            }),
        ),
    }
}

fn field_str<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn required_string(value: &Value, key: &str) -> Option<String> {
    field_str(value, key)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn optional_string(value: &Value, key: &str) -> Option<String> {
    required_string(value, key)
}

/// Build the cumulative Bash output update consumed by Pager's existing
/// background-task stdout router before the terminal completion notification.
///
/// An empty buffer carries nothing: Pager refuses to overwrite stdout with it,
/// and emitting one would only put a stray in-progress tool update on the wire.
pub(crate) fn background_bash_output_update(
    projection: &BackgroundBashProjection,
) -> Option<Value> {
    let BackgroundBashProjection::Completed {
        tool_call_id,
        task_snapshot,
    } = projection
    else {
        return None;
    };
    let output = task_snapshot
        .get("output")
        .and_then(Value::as_str)
        .filter(|output| !output.is_empty())?;
    Some(json!({
        "toolCallId": tool_call_id,
        "rawOutput": {
            "type": "Bash",
            "output_for_prompt": output,
            "truncated": task_snapshot.get("truncated").and_then(Value::as_bool).unwrap_or(false),
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_started_task() {
        let event = json!({
            "message": {
                "role": "custom",
                "customType": BRIDGE_TYPE,
                "details": {
                    "event": "started",
                    "taskId": "bash-1",
                    "toolCallId": "call-1",
                    "command": "cargo test",
                    "cwd": "/repo",
                    "outputFile": "/tmp/task.log",
                    "description": "Run tests",
                }
            }
        });
        assert_eq!(
            parse_background_bash_message(&event),
            Some(BackgroundBashProjection::Started {
                task_id: "bash-1".into(),
                tool_call_id: "call-1".into(),
                command: "cargo test".into(),
                cwd: "/repo".into(),
                output_file: "/tmp/task.log".into(),
                description: Some("Run tests".into()),
            })
        );
    }

    #[test]
    fn parses_background_tool_result_for_initial_or_promoted_bash() {
        let result = json!({
            "details": {
                "background": true,
                "taskId": "bash-1",
                "command": "cargo test",
                "cwd": "/repo",
                "outputFile": "/tmp/task.log",
            }
        });
        assert_eq!(
            parse_background_bash_tool_result("bash", "call-1", None, &result),
            Some(BackgroundBashProjection::Started {
                task_id: "bash-1".into(),
                tool_call_id: "call-1".into(),
                command: "cargo test".into(),
                cwd: "/repo".into(),
                output_file: "/tmp/task.log".into(),
                description: None,
            })
        );

        // When details omit description, fall back to pi-grok-bash task_name.
        assert_eq!(
            parse_background_bash_tool_result(
                "bash",
                "call-1",
                Some(&json!({ "command": "cargo test", "task_name": "运行测试" })),
                &result,
            ),
            Some(BackgroundBashProjection::Started {
                task_id: "bash-1".into(),
                tool_call_id: "call-1".into(),
                command: "cargo test".into(),
                cwd: "/repo".into(),
                output_file: "/tmp/task.log".into(),
                description: Some("运行测试".into()),
            })
        );
    }

    #[test]
    fn parses_completed_task() {
        let event = json!({
            "message": {
                "role": "custom",
                "customType": BRIDGE_TYPE,
                "details": {
                    "event": "completed",
                    "toolCallId": "call-1",
                    "taskSnapshot": { "task_id": "bash-1", "completed": true }
                }
            }
        });
        assert_eq!(
            parse_background_bash_message(&event),
            Some(BackgroundBashProjection::Completed {
                tool_call_id: "call-1".into(),
                task_snapshot: json!({ "task_id": "bash-1", "completed": true }),
            })
        );
    }

    #[test]
    fn ignores_non_bridge_messages() {
        assert!(
            parse_background_bash_message(&json!({
                "message": { "role": "custom", "customType": "pi-grok-recap/v1" }
            }))
            .is_none()
        );
    }

    #[test]
    fn builds_pager_task_notifications() {
        let (method, started) = background_bash_notification(
            "session-1",
            &BackgroundBashProjection::Started {
                task_id: "bash-1".into(),
                tool_call_id: "call-1".into(),
                command: "cargo test".into(),
                cwd: "/repo".into(),
                output_file: "/tmp/task.log".into(),
                description: None,
            },
        );
        assert_eq!(method, "x.ai/task_backgrounded");
        assert_eq!(started["sessionId"], "session-1");
        assert_eq!(started["update"]["sessionUpdate"], "task_backgrounded");
        assert_eq!(started["update"]["task_id"], "bash-1");

        let (method, completed) = background_bash_notification(
            "session-1",
            &BackgroundBashProjection::Completed {
                tool_call_id: "call-1".into(),
                task_snapshot: json!({ "task_id": "bash-1", "completed": true }),
            },
        );
        assert_eq!(method, "x.ai/task_completed");
        assert_eq!(completed["update"]["sessionUpdate"], "task_completed");
        assert_eq!(completed["update"]["will_wake"], false);
    }

    #[test]
    fn builds_cumulative_output_update_before_completion() {
        let update = background_bash_output_update(&BackgroundBashProjection::Completed {
            tool_call_id: "call-1".into(),
            task_snapshot: json!({
                "task_id": "bash-1",
                "output": "test output\n",
                "truncated": true,
            }),
        })
        .expect("completed projection creates output update");
        assert_eq!(update["toolCallId"], "call-1");
        assert_eq!(update["rawOutput"]["type"], "Bash");
        assert_eq!(update["rawOutput"]["output_for_prompt"], "test output\n");
        assert_eq!(update["rawOutput"]["truncated"], true);
    }

    #[test]
    fn skips_output_update_when_the_task_produced_nothing() {
        assert!(
            background_bash_output_update(&BackgroundBashProjection::Completed {
                tool_call_id: "call-1".into(),
                task_snapshot: json!({ "task_id": "bash-1", "output": "" }),
            })
            .is_none()
        );
    }

    #[test]
    fn parses_the_out_of_band_status_payload() {
        let payload = json!({
            "version": 1,
            "event": "completed",
            "taskId": "bash-1",
            "toolCallId": "call-1",
            "taskSnapshot": { "task_id": "bash-1", "completed": true, "exit_code": 0 },
        });
        assert_eq!(
            parse_background_bash_status(&payload),
            Some(BackgroundBashProjection::Completed {
                tool_call_id: "call-1".into(),
                task_snapshot: json!({ "task_id": "bash-1", "completed": true, "exit_code": 0 }),
            })
        );
        assert!(parse_background_bash_status(&json!({ "event": "unknown" })).is_none());
    }

    fn started(task_id: &str) -> BackgroundBashProjection {
        BackgroundBashProjection::Started {
            task_id: task_id.into(),
            tool_call_id: "call-1".into(),
            command: "just desktop-test".into(),
            cwd: "/repo".into(),
            output_file: "/tmp/task.log".into(),
            description: Some("运行完整桌面回归".into()),
        }
    }

    fn completed(task_id: &str) -> BackgroundBashProjection {
        BackgroundBashProjection::Completed {
            tool_call_id: "call-1".into(),
            task_snapshot: json!({ "task_id": task_id, "completed": true, "exit_code": 0 }),
        }
    }

    #[test]
    fn terminal_state_is_projected_once_across_both_channels() {
        let mut tasks = HashMap::new();
        assert!(record_background_bash(&mut tasks, &started("bash-1")));
        // Pi re-announces the same start on tool_execution_end and the bridge
        // message; the row must not be recreated as running.
        assert!(record_background_bash(&mut tasks, &started("bash-1")));
        assert!(!tasks["bash-1"].completed);

        assert!(record_background_bash(&mut tasks, &completed("bash-1")));
        assert!(tasks["bash-1"].completed);
        assert!(!record_background_bash(&mut tasks, &completed("bash-1")));
    }

    #[test]
    fn a_completion_without_a_start_is_still_deduplicated() {
        let mut tasks = HashMap::new();
        assert!(record_background_bash(&mut tasks, &completed("bash-1")));
        assert!(!record_background_bash(&mut tasks, &completed("bash-1")));
        // A start arriving late must not resurrect the running state.
        assert!(record_background_bash(&mut tasks, &started("bash-1")));
        assert!(tasks["bash-1"].completed);
    }

    #[test]
    fn draining_settles_only_the_still_running_tasks() {
        let mut tasks = HashMap::new();
        record_background_bash(&mut tasks, &started("bash-1"));
        record_background_bash(&mut tasks, &started("bash-2"));
        record_background_bash(&mut tasks, &completed("bash-2"));

        let orphans = drain_running_background_bash(&mut tasks);
        assert_eq!(orphans.len(), 1);
        let BackgroundBashProjection::Completed { task_snapshot, .. } = &orphans[0] else {
            panic!("orphan projection must be a completion");
        };
        assert_eq!(task_snapshot["task_id"], "bash-1");
        assert_eq!(task_snapshot["signal"], ORPHANED_SIGNAL);
        assert!(drain_running_background_bash(&mut tasks).is_empty());
    }

    #[test]
    fn orphan_completion_carries_every_required_snapshot_field() {
        let task = BackgroundBashTask {
            tool_call_id: "call-1".into(),
            command: "just desktop-test".into(),
            cwd: "/repo".into(),
            output_file: "/tmp/task.log".into(),
            started_at: SystemTime::now(),
            completed: false,
        };
        let BackgroundBashProjection::Completed {
            tool_call_id,
            task_snapshot,
        } = orphaned_background_bash_completion("bash-1", &task)
        else {
            panic!("orphan projection must be a completion");
        };
        assert_eq!(tool_call_id, "call-1");
        assert_eq!(task_snapshot["task_id"], "bash-1");
        assert_eq!(task_snapshot["command"], "just desktop-test");
        assert_eq!(task_snapshot["signal"], ORPHANED_SIGNAL);
        assert_eq!(task_snapshot["exit_code"], Value::Null);
        assert_eq!(task_snapshot["completed"], true);
        // `TaskSnapshot` has no serde defaults for these, so Pager's decode
        // fails outright if the synthetic snapshot drops one.
        for field in [
            "task_id",
            "command",
            "cwd",
            "start_time",
            "end_time",
            "output",
            "output_file",
            "truncated",
            "exit_code",
            "signal",
            "completed",
        ] {
            assert!(
                task_snapshot.get(field).is_some(),
                "orphan snapshot must carry {field}"
            );
        }
        assert!(task_snapshot["start_time"]["secs_since_epoch"].is_u64());
    }
}
