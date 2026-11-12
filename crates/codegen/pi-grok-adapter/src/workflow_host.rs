//! Session-scoped upstream workflow host for grok-pi (External ACP).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

// Host is shared via Arc so PiAgent can await launch/pause without RefCell borrow.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use xai_grok_shell::session::workflow::{
    ExternalWorkflowRuntime, ExternalWorkflowRuntimeConfig, WorkflowNotifySender, WorkflowRunStore,
    workflow_session_notification_json,
};
use xai_workflow::WorkflowOutcome;

use crate::pi_workflow_backend::{BridgeCommandTx, PiWorkflowAgentBackend};

pub struct WorkflowHost {
    runtime: ExternalWorkflowRuntime,
    session_id: String,
}

// `Arc` for await-friendly sharing from PiAgent without holding RefCell borrows.

impl WorkflowHost {
    pub fn new(
        session_id: String,
        cwd: PathBuf,
        session_dir: Option<PathBuf>,
        bridge_tx: BridgeCommandTx,
    ) -> Self {
        let (persist_tx, mut persist_rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Some(message) = persist_rx.recv().await {
                use xai_grok_shell::session::persistence::PersistenceMsg;
                if let PersistenceMsg::WorkflowRunStateAndAck { respond_to, .. } = message {
                    let _ = respond_to.send(Ok(()));
                }
            }
        });
        let (gateway_tx, mut gateway_rx) = mpsc::unbounded_channel();
        tokio::spawn(async move { while gateway_rx.recv().await.is_some() {} });

        let store = WorkflowRunStore::new(session_dir.clone(), persist_tx.clone());
        let notify = WorkflowNotifySender::new(
            agent_client_protocol::SessionId::new(session_id.clone()),
            xai_acp_lib::AcpAgentGatewaySender::new(gateway_tx),
            persist_tx,
            store.clone(),
        );
        let scratch = session_dir
            .clone()
            .unwrap_or_else(std::env::temp_dir)
            .join("pi-workflow-spawn");
        let backend = Arc::new(PiWorkflowAgentBackend::new(bridge_tx, scratch));
        let runtime = ExternalWorkflowRuntime::new(ExternalWorkflowRuntimeConfig {
            session_id: session_id.clone(),
            session_dir,
            cwd,
            backend,
            notify,
            store,
            session_cmd_tx: mpsc::unbounded_channel().0,
            templates: HashMap::new(),
        });
        Self {
            runtime,
            session_id,
        }
    }

    pub async fn launch_named(
        &self,
        name: &str,
        objective: String,
        args: Value,
    ) -> Result<(String, tokio::sync::oneshot::Receiver<WorkflowOutcome>)> {
        self.runtime
            .launch_named(name, objective, args, None)
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))
    }

    pub async fn pause(&self, run_id: &str) -> bool {
        self.runtime.pause(run_id).await
    }

    pub async fn cancel(&self, run_id: &str) -> bool {
        self.runtime.cancel(run_id).await
    }

    pub fn notification_payloads(&self) -> Vec<Value> {
        self.runtime
            .list_runs()
            .into_iter()
            .map(|state| {
                let elapsed = self.runtime.elapsed_ms(&state.run_id);
                workflow_session_notification_json(&self.session_id, &state, elapsed)
            })
            .collect()
    }

    pub async fn drive_until_outcome(
        &self,
        mut outcome_rx: tokio::sync::oneshot::Receiver<WorkflowOutcome>,
        mut emit: impl FnMut(Value),
    ) -> Result<WorkflowOutcome> {
        let mut interval = tokio::time::interval(Duration::from_millis(150));
        loop {
            tokio::select! {
                outcome = &mut outcome_rx => {
                    for payload in self.notification_payloads() {
                        emit(payload);
                    }
                    return outcome.context("workflow outcome channel closed");
                }
                _ = interval.tick() => {
                    for payload in self.notification_payloads() {
                        emit(payload);
                    }
                }
            }
        }
    }
}

pub fn outcome_to_json(outcome: &WorkflowOutcome) -> Value {
    match outcome {
        WorkflowOutcome::Completed { result } => json!({
            "status": "completed",
            "result": result,
        }),
        WorkflowOutcome::Paused { kind, message } => json!({
            "status": "paused",
            "kind": format!("{kind:?}"),
            "message": message,
        }),
        WorkflowOutcome::BudgetExceeded { message } => json!({
            "status": "budget_exceeded",
            "message": message,
        }),
        WorkflowOutcome::Cancelled => json!({ "status": "cancelled" }),
        WorkflowOutcome::Failed { error } => json!({
            "status": "failed",
            "error": error,
        }),
    }
}

pub fn format_outcome_for_tool(run_id: &str, outcome: &WorkflowOutcome) -> String {
    match outcome {
        WorkflowOutcome::Completed { result } => {
            let body = match result {
                Value::String(s) => s.clone(),
                other => serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string()),
            };
            format!("Workflow run `{run_id}` completed.\n\n{body}")
        }
        WorkflowOutcome::Paused { kind, message } => {
            format!("Workflow run `{run_id}` paused ({kind:?}): {message}")
        }
        WorkflowOutcome::BudgetExceeded { message } => {
            format!("Workflow run `{run_id}` hit agent budget: {message}")
        }
        WorkflowOutcome::Cancelled => format!("Workflow run `{run_id}` was cancelled."),
        WorkflowOutcome::Failed { error } => {
            format!("Workflow run `{run_id}` failed: {error}")
        }
    }
}

pub fn parse_workflow_request(name: &str, args: &str) -> Result<WorkflowRequest> {
    let name = name.trim();
    let args = args.trim();
    if name.is_empty() {
        bail!("workflow name is required");
    }
    match name {
        "pause" | "stop" | "resume" | "save" => Ok(WorkflowRequest::Manage {
            op: name.to_string(),
            target: args.to_string(),
        }),
        _ if matches!(args, "pause" | "stop" | "resume" | "save") => Ok(WorkflowRequest::Manage {
            op: args.to_string(),
            target: name.to_string(),
        }),
        _ => {
            let json_args = if args.is_empty() {
                json!({})
            } else if let Ok(v) = serde_json::from_str::<Value>(args) {
                v
            } else {
                json!({ "objective": args })
            };
            let objective = json_args
                .get("objective")
                .and_then(Value::as_str)
                .unwrap_or(args)
                .to_string();
            Ok(WorkflowRequest::Launch {
                name: name.to_string(),
                objective: if objective.is_empty() {
                    name.to_string()
                } else {
                    objective
                },
                args: json_args,
            })
        }
    }
}

#[derive(Debug, Clone)]
pub enum WorkflowRequest {
    Launch {
        name: String,
        objective: String,
        args: Value,
    },
    Manage {
        op: String,
        target: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_launch_and_manage() {
        match parse_workflow_request("deep-research", "compare postgres").unwrap() {
            WorkflowRequest::Launch {
                name, objective, ..
            } => {
                assert_eq!(name, "deep-research");
                assert!(objective.contains("postgres"));
            }
            other => panic!("{other:?}"),
        }
        match parse_workflow_request("pause", "deep-research").unwrap() {
            WorkflowRequest::Manage { op, target } => {
                assert_eq!(op, "pause");
                assert_eq!(target, "deep-research");
            }
            other => panic!("{other:?}"),
        }
    }
}
