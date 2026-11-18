//! Pi `WorkflowAgentBackend` for upstream `xai-workflow` host.
//!
//! Spawn protocol: write request JSON → hidden `/__pi_workflow_spawn` → read response JSON.
//! Uses a channel so the backend is `Send` while command execution stays on the Pi LocalSet.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};
use xai_grok_shell::session::workflow::{
    HostDrainOutcome, WorkflowAgentBackend, WorkflowAgentSpawnRequest, WorkflowAgentSpawnResult,
};
use xai_workflow::HostError;

pub const WORKFLOW_SPAWN_COMMAND: &str = "__pi_workflow_spawn";
pub const WORKFLOW_CANCEL_COMMAND: &str = "__pi_workflow_cancel";

/// Request executed by the Pi LocalSet owner (`PiAgent`).
pub struct BridgeCommandRequest {
    pub command: String,
    pub args: String,
    pub reply: oneshot::Sender<Result<(), String>>,
}

pub type BridgeCommandTx = mpsc::UnboundedSender<BridgeCommandRequest>;

#[derive(Debug, Serialize)]
struct SpawnRequestFile {
    id: String,
    prompt: String,
    description: String,
    subagent_type: String,
    parent_session_id: String,
    resume_from: Option<String>,
    model: Option<String>,
    capability_mode: Option<String>,
    isolation_worktree: bool,
    fork_context: bool,
    run_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct SpawnResponseFile {
    success: bool,
    output: String,
    error: Option<String>,
    cancelled: bool,
    child_session_id: String,
    total_tokens_used: u64,
    duration_ms: u64,
    backgrounded: bool,
}

pub struct PiWorkflowAgentBackend {
    bridge_tx: BridgeCommandTx,
    scratch_dir: PathBuf,
}

impl PiWorkflowAgentBackend {
    pub fn new(bridge_tx: BridgeCommandTx, scratch_dir: PathBuf) -> Self {
        Self {
            bridge_tx,
            scratch_dir,
        }
    }

    async fn run_bridge(&self, command: &str, args: String) -> Result<(), HostError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.bridge_tx
            .send(BridgeCommandRequest {
                command: command.to_string(),
                args,
                reply: reply_tx,
            })
            .map_err(|_| HostError::Failed("workflow bridge command channel closed".into()))?;
        reply_rx
            .await
            .map_err(|_| HostError::Failed("workflow bridge command dropped".into()))?
            .map_err(HostError::Failed)
    }
}

#[async_trait]
impl WorkflowAgentBackend for PiWorkflowAgentBackend {
    async fn spawn_and_await(
        &self,
        request: WorkflowAgentSpawnRequest,
    ) -> Result<WorkflowAgentSpawnResult, HostError> {
        if request.cancel_token.is_cancelled() {
            return Err(HostError::Cancelled);
        }
        std::fs::create_dir_all(&self.scratch_dir)
            .map_err(|e| HostError::Failed(format!("workflow spawn scratch dir: {e}")))?;
        let id = uuid::Uuid::now_v7().simple().to_string();
        let req_path = self.scratch_dir.join(format!("spawn-{id}.req.json"));
        let resp_path = self.scratch_dir.join(format!("spawn-{id}.resp.json"));

        let capability_mode = request.capability_mode.map(|_| "all".to_string());

        let body = SpawnRequestFile {
            id: request.id.clone(),
            prompt: request.prompt,
            description: request.description,
            subagent_type: request.subagent_type,
            parent_session_id: request.parent_session_id,
            resume_from: request.resume_from,
            model: request.model,
            capability_mode,
            isolation_worktree: request.isolation.is_some(),
            fork_context: request.fork_context,
            run_id: request.run_id.clone(),
        };
        std::fs::write(
            &req_path,
            serde_json::to_vec(&body).map_err(|e| HostError::Failed(e.to_string()))?,
        )
        .map_err(|e| HostError::Failed(format!("write spawn request: {e}")))?;

        let args = format!(
            "--request {} --response {}",
            req_path.display(),
            resp_path.display()
        );
        let cancel = request.cancel_token.clone();
        let bridge_fut = self.run_bridge(WORKFLOW_SPAWN_COMMAND, args);
        tokio::pin!(bridge_fut);
        tokio::select! {
            result = &mut bridge_fut => {
                result?;
            }
            _ = cancel.cancelled() => {
                let _ = self
                    .run_bridge(
                        WORKFLOW_CANCEL_COMMAND,
                        format!("--run-id {}", request.run_id),
                    )
                    .await;
                return Err(HostError::Cancelled);
            }
        }

        let raw = std::fs::read_to_string(&resp_path).map_err(|e| {
            HostError::Failed(format!("read spawn response {}: {e}", resp_path.display()))
        })?;
        let resp: SpawnResponseFile = serde_json::from_str(&raw)
            .map_err(|e| HostError::Failed(format!("parse spawn response: {e}; body={raw}")))?;
        let _ = std::fs::remove_file(&req_path);
        let _ = std::fs::remove_file(&resp_path);

        Ok(WorkflowAgentSpawnResult {
            success: resp.success,
            output: Arc::from(resp.output.as_str()),
            error: resp.error,
            cancelled: resp.cancelled,
            child_session_id: resp.child_session_id,
            total_tokens_used: resp.total_tokens_used,
            duration_ms: resp.duration_ms,
            backgrounded: resp.backgrounded,
        })
    }

    async fn cancel_run_children(&self, run_id: &str) -> HostDrainOutcome {
        match self
            .run_bridge(WORKFLOW_CANCEL_COMMAND, format!("--run-id {run_id}"))
            .await
        {
            Ok(()) => HostDrainOutcome::Drained,
            Err(_) => HostDrainOutcome::TimedOut,
        }
    }

    fn request_cancel_run_children(&self, run_id: &str) -> bool {
        let (reply_tx, _reply_rx) = oneshot::channel();
        self.bridge_tx
            .send(BridgeCommandRequest {
                command: WORKFLOW_CANCEL_COMMAND.to_string(),
                args: format!("--run-id {run_id}"),
                reply: reply_tx,
            })
            .is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn pi_backend_reads_spawn_response_file() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path().to_path_buf();
        let (tx, mut rx) = mpsc::unbounded_channel::<BridgeCommandRequest>();
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                let parts: Vec<_> = req.args.split_whitespace().collect();
                let resp = parts
                    .windows(2)
                    .find(|w| w[0] == "--response")
                    .map(|w| w[1]);
                if let Some(resp) = resp {
                    let body = SpawnResponseFile {
                        success: true,
                        output: "pi-child-ok".into(),
                        error: None,
                        cancelled: false,
                        child_session_id: "child-1".into(),
                        total_tokens_used: 3,
                        duration_ms: 5,
                        backgrounded: false,
                    };
                    let _ = std::fs::write(resp, serde_json::to_vec(&body).unwrap());
                }
                let _ = req.reply.send(Ok(()));
            }
        });
        let backend = PiWorkflowAgentBackend::new(tx, dir_path);
        let result = backend
            .spawn_and_await(WorkflowAgentSpawnRequest {
                id: "a1".into(),
                prompt: "hi".into(),
                description: "d".into(),
                subagent_type: "general-purpose".into(),
                parent_session_id: "p".into(),
                resume_from: None,
                model: None,
                reasoning_effort: None,
                capability_mode: None,
                isolation: None,
                fork_context: false,
                run_id: "wf_1".into(),
                cancel_token: CancellationToken::new(),
            })
            .await
            .unwrap();
        assert!(result.success);
        assert_eq!(&*result.output, "pi-child-ok");
        assert_eq!(result.child_session_id, "child-1");
    }
}
