//! Pluggable agent spawn backend for workflow host.
//!
//! Grok default: `SubagentEvent` coordinator.
//! grok-pi: implement this trait to route `SpawnAgent` to Pi child sessions.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use xai_grok_tools::implementations::grok_build::task::types::{
    ModelOverrideProvenance, SubagentCancelRequest, SubagentCancelTarget, SubagentEvent,
    SubagentOwner, SubagentRequest, SubagentResult, SubagentRuntimeOverrides, SubagentSpawnRequest,
};
use xai_tool_types::{SubagentCapabilityMode, SubagentIsolationMode};
use xai_workflow::HostError;

/// Outcome of draining workflow child agents after cancel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostDrainOutcome {
    Drained,
    TimedOut,
}

/// One host-level agent execution request (after host validation / contract wrap).
#[derive(Debug, Clone)]
pub struct WorkflowAgentSpawnRequest {
    pub id: String,
    pub prompt: String,
    pub description: String,
    pub subagent_type: String,
    pub parent_session_id: String,
    pub resume_from: Option<String>,
    pub model: Option<String>,
    pub capability_mode: Option<SubagentCapabilityMode>,
    pub isolation: Option<SubagentIsolationMode>,
    pub fork_context: bool,
    pub run_id: String,
    pub cancel_token: CancellationToken,
}

/// Result shape expected by `xai_workflow` host completion mapping.
#[derive(Debug, Clone)]
pub struct WorkflowAgentSpawnResult {
    pub success: bool,
    pub output: Arc<str>,
    pub error: Option<String>,
    pub cancelled: bool,
    pub child_session_id: String,
    pub total_tokens_used: u64,
    pub duration_ms: u64,
    pub backgrounded: bool,
}

impl From<SubagentResult> for WorkflowAgentSpawnResult {
    fn from(result: SubagentResult) -> Self {
        Self {
            success: result.success,
            output: result.output,
            error: result.error,
            cancelled: result.cancelled,
            child_session_id: result.child_session_id,
            total_tokens_used: result.total_tokens_used,
            duration_ms: result.duration_ms,
            backgrounded: result.backgrounded,
        }
    }
}

/// Spawn / cancel children for a workflow run.
#[async_trait]
pub trait WorkflowAgentBackend: Send + Sync {
    async fn spawn_and_await(
        &self,
        request: WorkflowAgentSpawnRequest,
    ) -> Result<WorkflowAgentSpawnResult, HostError>;

    async fn cancel_run_children(&self, run_id: &str) -> HostDrainOutcome;

    /// Best-effort fire-and-forget cancel (pause/stop paths).
    fn request_cancel_run_children(&self, _run_id: &str) -> bool {
        false
    }
}

/// Upstream Grok path: funnel through the existing subagent coordinator channel.
pub struct GrokSubagentBackend {
    pub subagent_event_tx: mpsc::UnboundedSender<SubagentEvent>,
    pub parent_session_id: String,
}

#[async_trait]
impl WorkflowAgentBackend for GrokSubagentBackend {
    async fn spawn_and_await(
        &self,
        request: WorkflowAgentSpawnRequest,
    ) -> Result<WorkflowAgentSpawnResult, HostError> {
        let (result_tx, result_rx) = oneshot::channel();
        let subagent_request = SubagentRequest {
            id: request.id,
            prompt: request.prompt,
            description: request.description,
            subagent_type: request.subagent_type,
            parent_session_id: request.parent_session_id,
            parent_prompt_id: None,
            resume_from: request.resume_from,
            cwd: None,
            runtime_overrides: SubagentRuntimeOverrides {
                model: request.model,
                output_token_budget: None,
                model_override_provenance: ModelOverrideProvenance::Tool,
                capability_mode: request.capability_mode,
                isolation: request.isolation,
                output_schema: None,
                ..Default::default()
            },
            run_in_background: false,
            surface_completion: false,
            await_to_completion: true,
            fork_context: request.fork_context,
            owner: SubagentOwner::workflow(&request.run_id),
            cancel_token: request.cancel_token,
            spawn_root: Default::default(),
        };

        if self
            .subagent_event_tx
            .send(SubagentEvent::Spawn(SubagentSpawnRequest {
                request: Box::new(subagent_request),
                result_tx,
                registered_tx: None,
            }))
            .is_err()
        {
            return Err(HostError::Failed(
                "subagent coordinator channel closed".into(),
            ));
        }

        let result = result_rx.await.map_err(|_| {
            HostError::Failed("subagent result channel closed before completion".into())
        })?;
        Ok(WorkflowAgentSpawnResult::from(result))
    }

    async fn cancel_run_children(&self, run_id: &str) -> HostDrainOutcome {
        let (respond_to, response) = oneshot::channel();
        if self
            .subagent_event_tx
            .send(SubagentEvent::Cancel(SubagentCancelRequest {
                parent_session_id: Some(self.parent_session_id.clone()),
                target: SubagentCancelTarget::WorkflowRunId(run_id.to_owned()),
                respond_to,
            }))
            .is_err()
        {
            tracing::warn!(%run_id, "workflow child cancellation channel closed");
            return HostDrainOutcome::TimedOut;
        }
        const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);
        if matches!(tokio::time::timeout(TIMEOUT, response).await, Ok(Ok(_))) {
            HostDrainOutcome::Drained
        } else {
            tracing::warn!(
                %run_id,
                timeout_ms = TIMEOUT.as_millis() as u64,
                "workflow child cancel/drain timed out"
            );
            HostDrainOutcome::TimedOut
        }
    }

    fn request_cancel_run_children(&self, run_id: &str) -> bool {
        let (respond_to, _response) = oneshot::channel();
        self.subagent_event_tx
            .send(SubagentEvent::Cancel(SubagentCancelRequest {
                parent_session_id: Some(self.parent_session_id.clone()),
                target: SubagentCancelTarget::WorkflowRunId(run_id.to_owned()),
                respond_to,
            }))
            .is_ok()
    }
}

/// Test / Pi-stub backend: returns canned success without Grok subagents.
pub struct MockWorkflowAgentBackend {
    pub output: Arc<str>,
}

#[async_trait]
impl WorkflowAgentBackend for MockWorkflowAgentBackend {
    async fn spawn_and_await(
        &self,
        request: WorkflowAgentSpawnRequest,
    ) -> Result<WorkflowAgentSpawnResult, HostError> {
        if request.cancel_token.is_cancelled() {
            return Err(HostError::Cancelled);
        }
        Ok(WorkflowAgentSpawnResult {
            success: true,
            output: self.output.clone(),
            error: None,
            cancelled: false,
            child_session_id: request.id,
            total_tokens_used: 0,
            duration_ms: 1,
            backgrounded: false,
        })
    }

    async fn cancel_run_children(&self, _run_id: &str) -> HostDrainOutcome {
        HostDrainOutcome::Drained
    }

    fn request_cancel_run_children(&self, _run_id: &str) -> bool {
        true
    }
}
