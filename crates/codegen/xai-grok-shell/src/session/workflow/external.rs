//! External / Pi-facing workflow runtime surface.
//!
//! Reuses the same `WorkflowManager` + registry as the native Grok session path.
//! Callers supply a [`WorkflowAgentBackend`] (Grok channel or Pi spawn bridge).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::mpsc;
use xai_workflow::WorkflowOutcome;

use super::backend::WorkflowAgentBackend;
use super::manager::{LaunchError, LaunchSpec, WorkflowManager};
use super::notify::WorkflowNotifySender;
use super::registry::{
    ResolveError, ResolvedWorkflow, WorkflowListing, WorkflowRegistry, resolve_by_name,
    resolve_inline, workflow_snapshot,
};
use super::store::WorkflowRunStore;
use super::tracker::{WorkflowRunState, WorkflowTracker};

/// Bundle of manager + tracker for process-local External (grok-pi) use.
pub struct ExternalWorkflowRuntime {
    manager: Arc<tokio::sync::Mutex<WorkflowManager>>,
    tracker: Arc<parking_lot::Mutex<WorkflowTracker>>,
    cwd: PathBuf,
}

pub struct ExternalWorkflowRuntimeConfig {
    pub session_id: String,
    pub session_dir: Option<PathBuf>,
    pub cwd: PathBuf,
    pub backend: Arc<dyn WorkflowAgentBackend>,
    pub notify: WorkflowNotifySender,
    pub store: WorkflowRunStore,
    pub session_cmd_tx: mpsc::UnboundedSender<crate::session::commands::SessionCommand>,
    pub templates: HashMap<String, String>,
}

impl ExternalWorkflowRuntime {
    pub fn new(config: ExternalWorkflowRuntimeConfig) -> Self {
        let tracker = Arc::new(parking_lot::Mutex::new(WorkflowTracker::default()));
        let manager = Arc::new(tokio::sync::Mutex::new(WorkflowManager::new(
            config.session_id,
            config.session_dir,
            config.cwd.clone(),
            tracker.clone(),
            config.store,
            config.notify,
            config.backend,
            Arc::new(|_, _, _| {}),
            config.session_cmd_tx,
            config.templates,
            super::host_service::DEFAULT_WORKFLOW_MAX_CONCURRENT_AGENTS,
        )));
        Self {
            manager,
            tracker,
            cwd: config.cwd,
        }
    }

    pub(crate) fn tracker(&self) -> Arc<parking_lot::Mutex<WorkflowTracker>> {
        self.tracker.clone()
    }

    pub(crate) fn manager(&self) -> Arc<tokio::sync::Mutex<WorkflowManager>> {
        self.manager.clone()
    }

    pub(crate) fn registry_snapshot(&self) -> (WorkflowRegistry, Vec<WorkflowListing>) {
        workflow_snapshot(Some(self.cwd.as_path()))
    }

    pub(crate) fn resolve_named(&self, name: &str) -> Result<ResolvedWorkflow, ResolveError> {
        resolve_by_name(name, Some(self.cwd.as_path()))
    }

    pub async fn launch_named(
        &self,
        name: &str,
        objective: String,
        args: serde_json::Value,
        agent_budget: Option<u64>,
    ) -> Result<(String, tokio::sync::oneshot::Receiver<WorkflowOutcome>), LaunchError> {
        let resolved = self
            .resolve_named(name)
            .map_err(|e| LaunchError::Store(e.to_string()))?;
        let mut manager = self.manager.lock().await;
        manager.launch(
            resolved,
            LaunchSpec {
                objective,
                args,
                agent_budget,
                resume_run_id: None,
            },
        )
    }

    pub async fn launch_inline(
        &self,
        script: String,
        objective: String,
        args: serde_json::Value,
        agent_budget: Option<u64>,
    ) -> Result<(String, tokio::sync::oneshot::Receiver<WorkflowOutcome>), LaunchError> {
        let resolved = resolve_inline(script).map_err(|e| LaunchError::Store(e.to_string()))?;
        let mut manager = self.manager.lock().await;
        manager.launch(
            resolved,
            LaunchSpec {
                objective,
                args,
                agent_budget,
                resume_run_id: None,
            },
        )
    }

    pub async fn pause(&self, run_id: &str) -> bool {
        self.manager.lock().await.pause(run_id)
    }

    pub async fn cancel(&self, run_id: &str) -> bool {
        self.manager.lock().await.cancel(run_id)
    }

    pub fn list_runs(&self) -> Vec<WorkflowRunState> {
        self.tracker.lock().list()
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub fn elapsed_ms(&self, run_id: &str) -> u64 {
        self.tracker.lock().elapsed_ms(run_id)
    }
}

/// Build a test-only runtime with mock agent backend (no Grok subagent channel).
#[cfg(test)]
pub fn test_runtime(session_dir: Option<PathBuf>) -> ExternalWorkflowRuntime {
    use super::backend::MockWorkflowAgentBackend;
    use super::notify::WorkflowNotifySender;

    let (persist_tx, mut persist_rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Some(message) = persist_rx.recv().await {
            if let crate::session::persistence::PersistenceMsg::WorkflowRunStateAndAck {
                respond_to,
                ..
            } = message
            {
                let _ = respond_to.send(Ok(()));
            }
        }
    });
    let (gateway_tx, _gateway_rx) = mpsc::unbounded_channel();
    let store = WorkflowRunStore::new(session_dir.clone(), persist_tx.clone());
    let notify = WorkflowNotifySender::new(
        agent_client_protocol::SessionId::new("external-test"),
        xai_acp_lib::AcpAgentGatewaySender::new(gateway_tx),
        persist_tx,
        store.clone(),
    );
    ExternalWorkflowRuntime::new(ExternalWorkflowRuntimeConfig {
        session_id: "external-test".into(),
        session_dir,
        cwd: std::env::temp_dir(),
        backend: Arc::new(MockWorkflowAgentBackend {
            output: Arc::from("external-mock"),
        }),
        notify,
        store,
        session_cmd_tx: mpsc::unbounded_channel().0,
        templates: HashMap::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use xai_workflow::WorkflowOutcome;

    #[tokio::test]
    async fn external_runtime_launch_inline_with_mock_backend() {
        let dir = tempfile::tempdir().unwrap();
        let rt = test_runtime(Some(dir.path().to_path_buf()));
        let (run_id, outcome_rx) = rt
            .launch_inline(
                r#"
let meta = #{ name: "ext-inline", description: "d" };
let r = agent("hi");
complete(r.output);
"#
                .into(),
                "obj".into(),
                serde_json::json!({}),
                None,
            )
            .await
            .unwrap();
        let outcome = outcome_rx.await.unwrap();
        match outcome {
            WorkflowOutcome::Completed { result } => {
                assert_eq!(result.as_str().unwrap_or_default(), "external-mock");
            }
            other => panic!("expected completed, got {other:?}"),
        }
        assert!(rt.list_runs().iter().any(|r| r.run_id == run_id));
    }
}
