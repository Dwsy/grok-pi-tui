use super::*;
use crate::agent::session_registry_client::RegisterRequest;
use crate::agent::session_registry_client::RegistryStatusError;
use crate::agent::session_registry_client::SessionRegistryClient;
use crate::agent::session_registry_client::UpdateRequest;
use tracing::Instrument;

pub(super) struct TurnResultArgs {
    pub(super) request_id: String,
    pub(super) completed: bool,
    pub(super) stop_reason: String,
    pub(super) total_tokens: Option<u64>,
    pub(super) error: Option<String>,
    pub(super) finished_at: String,
    pub(super) turn_snapshot: Option<crate::session::signals::TurnDeltaSnapshot>,
    pub(super) prompt_mode: String,
    pub(super) subagents_spawned: Vec<crate::upload::trace::SubagentSpawnedRef>,
}

impl TurnResultArgs {
    pub(super) fn into_metadata(self, resolved_model: Option<String>) -> TurnResultMetadata {
        let snapshot = self.turn_snapshot;
        TurnResultMetadata {
            schema_version: GCS_SCHEMA_VERSION,
            request_id: self.request_id,
            completed: self.completed,
            stop_reason: Some(self.stop_reason),
            total_tokens: self.total_tokens,
            input_tokens: snapshot.as_ref().map(|s| s.turn_input_tokens),
            cached_input_tokens: snapshot.as_ref().map(|s| s.turn_cached_input_tokens),
            output_tokens: snapshot.as_ref().map(|s| s.turn_output_tokens),
            error: self.error,
            finished_at: self.finished_at,
            signals: snapshot.as_ref().map(|s| s.current.clone()),
            turn_delta: snapshot.as_ref().map(|s| s.delta.clone()),
            start_prompt_mode: snapshot
                .as_ref()
                .and_then(|s| s.start_prompt_mode.clone())
                .or(Some(self.prompt_mode)),
            end_prompt_mode: snapshot.as_ref().and_then(|s| s.end_prompt_mode.clone()),
            resolved_model,
            subagents_spawned: self.subagents_spawned,
        }
    }
}

struct SessionRegistration {
    model_id: String,
    hostname: String,
    device_id: Option<String>,
    /// The session's first prompt: built on turn 0 only, and never for a ZDR team.
    first_prompt: Option<String>,
    suppress: bool,
}

pub(super) struct RegistryTurnEndArgs {
    client: Option<SessionRegistryClient>,
    session_id: String,
    turn: i32,
    cwd: String,
    cmd_tx: tokio::sync::mpsc::UnboundedSender<SessionCommand>,
    register: Option<SessionRegistration>,
    order: crate::session::handle::RegistryWriteOrder,
    claim: crate::session::handle::RegistryTurnClaim,
    head: GitRead,
    head_branch: Option<String>,
}

impl MvpAgent {
    pub(super) fn build_registry_turn_end_args(
        &self,
        session_id: &acp::SessionId,
        turn_number: u64,
        handle: &SessionHandle,
        prompt: &[acp::ContentBlock],
        claim: crate::session::handle::RegistryTurnClaim,
        head: GitRead,
        head_branch: Option<String>,
    ) -> RegistryTurnEndArgs {
        let client = self.session_registry_client();
        let register = client
            .is_some()
            .then(|| self.session_registration(turn_number, prompt));
        RegistryTurnEndArgs {
            client,
            session_id: session_id.to_string(),
            turn: i32::try_from(turn_number).unwrap_or(i32::MAX),
            cwd: handle.info.cwd.clone(),
            cmd_tx: handle.cmd_tx.clone(),
            register,
            order: handle.registry_write_order.clone(),
            claim,
            head,
            head_branch,
        }
    }

    fn session_registration(
        &self,
        turn_number: u64,
        prompt: &[acp::ContentBlock],
    ) -> SessionRegistration {
        let suppress = self
            .auth_manager
            .current_or_expired()
            .is_some_and(|a| a.is_zdr_team());
        SessionRegistration {
            model_id: self.models_manager.current_model_id().0.to_string(),
            hostname: gethostname::gethostname().to_string_lossy().to_string(),
            device_id: if suppress { None } else { Some(agent_id()) },
            first_prompt: if suppress || turn_number != 0 {
                None
            } else {
                prompt.iter().find_map(|b| {
                    if let acp::ContentBlock::Text(t) = b {
                        Some(t.text.clone())
                    } else {
                        None
                    }
                })
            },
            suppress,
        }
    }
}

const GIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

const REGISTRY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

const SUMMARY_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

async fn bounded_registry<T>(
    op: impl std::future::Future<Output = anyhow::Result<T>>,
) -> anyhow::Result<T> {
    tokio::time::timeout(REGISTRY_TIMEOUT, op)
        .await
        .unwrap_or_else(|elapsed| Err(anyhow::Error::from(elapsed)))
}

pub(super) enum GitRead {
    Value(String),
    Empty,
    Failed,
}

async fn git_read(cwd: &str, args: &[&str]) -> GitRead {
    let mut cmd = tokio::process::Command::from(xai_tty_utils::git_command());
    cmd.current_dir(cwd)
        .args(args)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let Ok(Ok(output)) = tokio::time::timeout(GIT_TIMEOUT, cmd.output()).await else {
        return GitRead::Failed;
    };
    match output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
    {
        Some(value) => GitRead::Value(value),
        None => GitRead::Empty,
    }
}

async fn git_out(cwd: &str, args: &[&str]) -> Option<String> {
    match git_read(cwd, args).await {
        GitRead::Value(value) => Some(value),
        GitRead::Empty | GitRead::Failed => None,
    }
}

async fn sample_git_head(cwd: &str) -> (GitRead, Option<String>) {
    (
        git_read(cwd, &["rev-parse", "HEAD"]).await,
        git_out(cwd, &["branch", "--show-current"]).await,
    )
}

#[must_use = "dropping a TurnEndCapture without finish() skips this turn's registry writes"]
pub(super) struct TurnEndCapture {
    registry_claim: crate::session::handle::RegistryTurnClaim,
    git_sample: tokio_util::task::AbortOnDropHandle<(GitRead, Option<String>)>,
    session_copy_rx:
        oneshot::Receiver<anyhow::Result<crate::session::persistence::SessionStateCopy>>,
}

impl TurnEndCapture {
    pub(super) fn begin(handle: &SessionHandle, head_cwd: String) -> Self {
        let registry_claim = handle.registry_write_order.begin_turn_end();
        // Enqueue CopyFile before any await so the actor snapshots this turn.
        let (session_copy_tx, session_copy_rx) = oneshot::channel();
        if handle
            .cmd_tx
            .send(crate::session::SessionCommand::CopyFile {
                respond_to: session_copy_tx,
            })
            .is_err()
        {
            tracing::warn!("Failed to send CopyFile command, skipping session state upload");
        }
        let git_sample = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
            sample_git_head(&head_cwd).await
        }));
        Self {
            registry_claim,
            git_sample,
            session_copy_rx,
        }
    }

    pub(super) async fn finish(
        self,
    ) -> (
        GitRead,
        Option<String>,
        crate::session::handle::RegistryTurnClaim,
        oneshot::Receiver<anyhow::Result<crate::session::persistence::SessionStateCopy>>,
    ) {
        let (head, head_branch) = self.git_sample.await.unwrap_or((GitRead::Failed, None));
        (head, head_branch, self.registry_claim, self.session_copy_rx)
    }
}

fn plan_git_head(
    commit: GitRead,
    branch: Option<String>,
) -> Option<(Option<String>, Option<String>)> {
    match commit {
        GitRead::Value(sha) => Some((Some(sha), branch)),
        GitRead::Empty => Some((None, branch)),
        GitRead::Failed => None,
    }
}

enum RegisterOutcome {
    Registered,
    /// The server rejected the request with a 4xx that a retry would get again.
    Refused,
    /// Timeout, transport error, 401, throttling, or 5xx: a later turn may still land it.
    Transient,
}

async fn register_session(
    client: &SessionRegistryClient,
    session_id: &str,
    cwd: &str,
    reg: SessionRegistration,
) -> RegisterOutcome {
    let repo_remote_url = git_out(cwd, &["remote", "get-url", "origin"]).await;
    let repo_branch = git_out(cwd, &["rev-parse", "--abbrev-ref", "HEAD"]).await;
    let repo_head_at_start = git_out(cwd, &["rev-parse", "HEAD"]).await;
    let reg_req = RegisterRequest {
        session_id: session_id.to_owned(),
        cwd: cwd.to_owned(),
        gcs_trace_prefix: session_id.to_owned(),
        model_id: Some(reg.model_id),
        repo_remote_url,
        repo_branch,
        repo_head_at_start,
        hostname: Some(reg.hostname),
        device_id: reg.device_id,
        parent_session_id: None,
        subagent_type: None,
        subagent_persona: None,
        subagent_role: None,
        fork_context_source: None,
        subagent_depth: None,
    };
    let outcome = match bounded_registry(client.register(&reg_req)).await {
        Ok(()) => RegisterOutcome::Registered,
        Err(e) => {
            let refused = e
                .downcast_ref::<RegistryStatusError>()
                .is_some_and(RegistryStatusError::is_permanent_refusal);
            tracing::warn!(
                error = %e,
                refused,
                "session registry register failed (non-fatal)"
            );
            if refused {
                RegisterOutcome::Refused
            } else {
                RegisterOutcome::Transient
            }
        }
    };
    let info = crate::session::info::Info {
        id: acp::SessionId::new(session_id.to_owned()),
        cwd: cwd.to_owned(),
    };
    let summary_path = crate::session::persistence::session_dir(&info).join("summary.json");
    let summary = if reg.suppress {
        None
    } else {
        tokio::time::timeout(SUMMARY_READ_TIMEOUT, tokio::fs::read(&summary_path))
            .await
            .ok()
            .and_then(Result::ok)
            .and_then(|bytes| {
                serde_json::from_slice::<crate::session::persistence::Summary>(&bytes).ok()
            })
            .map(|s| s.session_summary)
            .filter(|s| !s.is_empty())
    };
    if reg.first_prompt.is_some() || summary.is_some() {
        let upd_req = UpdateRequest {
            summary,
            first_prompt: reg.first_prompt,
            last_turn_number: None,
            repo_head_at_end: None,
            restorable_turn_number: None,
        };
        tracing::debug!(
            session_id = %session_id,
            has_summary = upd_req.summary.is_some(),
            "session registry post-register update"
        );
        if let Err(e) = bounded_registry(client.update(session_id, &upd_req)).await {
            tracing::warn!(
                error = %e,
                "session registry first-prompt update failed (non-fatal)"
            );
        }
    }
    outcome
}

async fn advance_last_turn(
    client: &SessionRegistryClient,
    session_id: &str,
    turn: i32,
    repo_head_at_end: Option<String>,
) -> bool {
    let req = UpdateRequest {
        summary: None,
        first_prompt: None,
        last_turn_number: Some(turn),
        repo_head_at_end,
        restorable_turn_number: None,
    };
    match bounded_registry(client.update(session_id, &req)).await {
        Ok(_) => true,
        Err(e) => {
            tracing::warn!(
                error = %e,
                "session registry last_turn_number update failed (non-fatal)"
            );
            false
        }
    }
}

async fn advance_restorable_turn(
    client: &SessionRegistryClient,
    session_id: &str,
    turn: i32,
) -> bool {
    let req = UpdateRequest {
        summary: None,
        first_prompt: None,
        last_turn_number: None,
        repo_head_at_end: None,
        restorable_turn_number: Some(turn),
    };
    match bounded_registry(client.update(session_id, &req)).await {
        Ok(_) => true,
        Err(e) => {
            tracing::warn!(
                error = %e,
                "session registry restorable_turn_number update failed (non-fatal)"
            );
            false
        }
    }
}

struct OrderedTurnWrites {
    client: Option<SessionRegistryClient>,
    session_id: String,
    turn: i32,
    cwd: String,
    cmd_tx: tokio::sync::mpsc::UnboundedSender<SessionCommand>,
    order: crate::session::handle::RegistryWriteOrder,
    register: Option<SessionRegistration>,
}

impl OrderedTurnWrites {
    async fn write_git_head(&self, head: GitRead, head_branch: Option<String>) {
        async {
            if let Some((commit, branch)) = plan_git_head(head, head_branch) {
                let _ = self
                    .cmd_tx
                    .send(crate::session::SessionCommand::PersistGitHead { commit, branch });
            }
        }
        .instrument(tracing::debug_span!("turn_end.persist_git_head"))
        .await;
    }

    async fn write_register(&mut self) {
        let (Some(client), Some(reg)) = (self.client.as_ref(), self.register.take()) else {
            return;
        };
        if !self.order.claim_registration() {
            return;
        }
        match register_session(client, &self.session_id, &self.cwd, reg).await {
            RegisterOutcome::Registered | RegisterOutcome::Refused => {}
            RegisterOutcome::Transient => self.order.release_registration(),
        }
    }

    async fn write_last_turn(&self, repo_head_at_end: Option<String>) {
        let Some(client) = self.client.as_ref() else {
            return;
        };
        if self.order.should_write_last_turn(self.turn)
            && advance_last_turn(client, &self.session_id, self.turn, repo_head_at_end).await
        {
            self.order.commit_last_turn(self.turn);
        }
    }

    async fn write_restorable(&self) {
        let Some(client) = self.client.as_ref() else {
            return;
        };
        let _apply = self.order.lock_restorable_apply().await;
        if self.order.should_write_restorable(self.turn)
            && advance_restorable_turn(client, &self.session_id, self.turn).await
        {
            self.order.commit_restorable(self.turn);
        }
    }
}

pub(super) async fn run_registry_turn_end(
    args: RegistryTurnEndArgs,
    archive_confirmed: oneshot::Receiver<bool>,
) {
    let RegistryTurnEndArgs {
        client,
        session_id,
        turn,
        cwd,
        cmd_tx,
        register,
        order,
        mut claim,
        head,
        head_branch,
    } = args;
    claim.wait_predecessor().await;
    let repo_head_at_end = match &head {
        GitRead::Value(sha) => Some(sha.clone()),
        GitRead::Empty | GitRead::Failed => None,
    };
    let mut writes = OrderedTurnWrites {
        client,
        session_id,
        turn,
        cwd,
        cmd_tx,
        order,
        register,
    };
    writes.write_git_head(head, head_branch).await;
    writes.write_register().await;
    writes.write_last_turn(repo_head_at_end).await;
    drop(claim);
    if let Ok(true) = archive_confirmed.await {
        writes.write_restorable().await;
    }
}

pub(super) struct TraceCaptures {
    pub(super) permission_events: Vec<PermissionEvent>,
    pub(super) session_copy_rx:
        oneshot::Receiver<anyhow::Result<crate::session::persistence::SessionStateCopy>>,
    pub(super) turn_messages: Option<xai_chat_state::TurnCapture>,
    pub(super) streaming_partial: Option<crate::session::acp_session::StreamingTurnCapture>,
}

pub(super) async fn run_trace_completion(
    ctx: &PromptTraceContext,
    captures: TraceCaptures,
    wait: UploadWait,
) -> bool {
    let TraceCaptures {
        permission_events,
        session_copy_rx,
        turn_messages,
        streaming_partial,
    } = captures;
    match complete_prompt_trace(
        ctx.clone(),
        permission_events,
        session_copy_rx,
        turn_messages.into(),
        streaming_partial,
        wait,
    )
    .await
    {
        Ok(true) => true,
        Ok(false) => {
            match wait {
                UploadWait::Defer { .. } => tracing::debug!(
                    "session state unconfirmed within the flush budget; \
                     skipping restorable_turn_number advance"
                ),
                UploadWait::Confirm => tracing::warn!(
                    "session state upload failed; skipping restorable_turn_number advance"
                ),
            }
            false
        }
        Err(e) => {
            tracing::warn!("Failed to complete prompt trace: {e:?}");
            match wait {
                UploadWait::Confirm => write_error_manifest(ctx).await,
                UploadWait::Defer { deadline } => {
                    crate::upload::trace::flush_then_write_error_manifest(ctx, deadline).await
                }
            }
            false
        }
    }
}

pub(super) struct ErrorTurnArtifacts {
    pub(super) turn_messages: Option<xai_chat_state::TurnCapture>,
    pub(super) streaming_partial: Option<crate::session::acp_session::StreamingTurnCapture>,
    pub(super) upload_unified: bool,
}

pub(super) async fn upload_error_turn_artifacts(
    ctx: &PromptTraceContext,
    result: &TurnResultMetadata,
    artifacts: ErrorTurnArtifacts,
    wait: UploadWait,
) {
    let ErrorTurnArtifacts {
        turn_messages,
        streaming_partial,
        upload_unified,
    } = artifacts;
    upload_turn_result(ctx, result, wait).await;
    if let Some(capture) = turn_messages {
        upload_turn_messages(ctx, capture, wait).await;
    }
    if let Some(ref capture) = streaming_partial {
        crate::upload::trace::upload_streaming_partial(ctx, capture, wait).await;
    }
    if upload_unified {
        upload_unified_log(ctx, wait).await;
    }
    match wait {
        UploadWait::Confirm => write_error_manifest(ctx).await,
        UploadWait::Defer { deadline } => {
            crate::upload::trace::flush_then_write_error_manifest(ctx, deadline).await
        }
    }
}

pub(super) enum TurnEndOutcome {
    Completed {
        captures: TraceCaptures,
        registry: Box<RegistryTurnEndArgs>,
    },
    Failed(ErrorTurnArtifacts),
}

pub(super) async fn run_detached_turn_end(
    ctx: PromptTraceContext,
    turn_result: TurnResultArgs,
    resolved_model: Option<String>,
    outcome: TurnEndOutcome,
) {
    let result = turn_result.into_metadata(resolved_model);
    match outcome {
        TurnEndOutcome::Completed { captures, registry } => {
            let (archive_confirmed_tx, archive_confirmed_rx) = oneshot::channel();
            futures::join!(
                upload_turn_result(&ctx, &result, UploadWait::Confirm)
                    .instrument(tracing::debug_span!("turn_end.turn_result_upload")),
                run_registry_turn_end(*registry, archive_confirmed_rx)
                    .instrument(tracing::debug_span!("turn_end.registry")),
                async {
                    let confirmed = run_trace_completion(&ctx, captures, UploadWait::Confirm).await;
                    let _ = archive_confirmed_tx.send(confirmed);
                }
                .instrument(tracing::debug_span!("turn_end.trace_completion")),
            );
        }
        TurnEndOutcome::Failed(artifacts) => {
            upload_error_turn_artifacts(&ctx, &result, artifacts, UploadWait::Confirm).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::GitRead;
    use super::acp;
    use super::plan_git_head;

    #[test]
    fn plan_git_head_decides_what_to_store() {
        assert_eq!(
            plan_git_head(GitRead::Value("abc123".into()), Some("main".into())),
            Some((Some("abc123".into()), Some("main".into())))
        );
        assert_eq!(
            plan_git_head(GitRead::Value("abc123".into()), None),
            Some((Some("abc123".into()), None))
        );
        assert_eq!(
            plan_git_head(GitRead::Empty, Some("main".into())),
            Some((None, Some("main".into())))
        );
        assert_eq!(plan_git_head(GitRead::Failed, Some("main".into())), None);
    }

    #[tokio::test]
    async fn dropped_claim_keeps_later_turn_ordered() {
        let order = crate::session::handle::RegistryWriteOrder::default();
        let running = order.begin_turn_end();
        let mut dropped = order.begin_turn_end();
        let mut next = order.begin_turn_end();
        {
            let mut waiting = std::pin::pin!(dropped.wait_predecessor());
            assert!(futures::poll!(waiting.as_mut()).is_pending());
        }
        drop(dropped);
        let mut waiting = std::pin::pin!(next.wait_predecessor());
        assert!(futures::poll!(waiting.as_mut()).is_pending());
        drop(running);
        waiting.await;
    }

    /// Registry double with the server's row rule: an update 404s until a register has created the row.
    #[derive(Default)]
    struct RegistryRows {
        registers: usize,
        /// Statuses the next registers answer with, in order, before registers start creating the row.
        register_failures: std::collections::VecDeque<axum::http::StatusCode>,
        rejected_updates: usize,
        rows: std::collections::HashMap<String, serde_json::Map<String, serde_json::Value>>,
    }

    impl RegistryRows {
        fn row(&self, session_id: &acp::SessionId) -> Option<serde_json::Value> {
            self.rows
                .get(&session_id.to_string())
                .cloned()
                .map(serde_json::Value::Object)
        }
    }

    async fn serve_registry_rows() -> (String, std::sync::Arc<parking_lot::Mutex<RegistryRows>>) {
        use axum::{Json, Router, extract::Path, http::StatusCode, routing::post};

        let state = std::sync::Arc::new(parking_lot::Mutex::new(RegistryRows::default()));
        let on_register = state.clone();
        let on_update = state.clone();
        let router = Router::new()
            .route(
                "/sessions/register",
                post(move |Json(body): Json<serde_json::Value>| {
                    let state = on_register.clone();
                    async move {
                        let mut registry = state.lock();
                        registry.registers += 1;
                        if let Some(status) = registry.register_failures.pop_front() {
                            return status;
                        }
                        let id = body.get("sessionId").and_then(serde_json::Value::as_str);
                        registry
                            .rows
                            .entry(id.unwrap_or_default().to_owned())
                            .or_default();
                        StatusCode::OK
                    }
                }),
            )
            .route(
                "/sessions/{id}/replicas/update",
                post(
                    move |Path(id): Path<String>, Json(body): Json<serde_json::Value>| {
                        let state = on_update.clone();
                        async move {
                            let mut registry = state.lock();
                            let Some(row) = registry.rows.get_mut(&id) else {
                                registry.rejected_updates += 1;
                                return StatusCode::NOT_FOUND;
                            };
                            if let serde_json::Value::Object(fields) = body {
                                row.extend(fields);
                            }
                            StatusCode::OK
                        }
                    },
                ),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        (format!("http://{addr}"), state)
    }

    /// A registry-enabled agent and a session handle whose summary path cannot pre-exist: fresh session id, fresh cwd.
    struct TurnEndHarness {
        agent: super::MvpAgent,
        handle: super::SessionHandle,
        registry: std::sync::Arc<parking_lot::Mutex<RegistryRows>>,
        _gateway_rx: tokio::sync::mpsc::UnboundedReceiver<xai_acp_lib::AcpClientMessage>,
        _cwd: tempfile::TempDir,
    }

    impl TurnEndHarness {
        async fn start() -> TurnEndHarness {
            let (base_url, registry) = serve_registry_rows().await;
            let (agent, gateway_rx) =
                crate::agent::mvp_agent::tests::build_agent_with_auth_and_proxy(
                    xai_grok_login::GrokAuth {
                        oidc_issuer: Some(xai_grok_login::XAI_OAUTH2_ISSUER.to_owned()),
                        ..xai_grok_login::GrokAuth::test_default()
                    },
                    base_url,
                    crate::agent::config::AgentMode::Generic,
                );
            agent.cfg.borrow_mut().remote_settings = Some(crate::util::config::RemoteSettings {
                session_registry_enabled: Some(true),
                ..crate::util::config::RemoteSettings::default()
            });
            let cwd = tempfile::tempdir().unwrap();
            let mut handle = crate::agent::mvp_agent::tests::make_test_handle("grok", false, None);
            handle.info = crate::session::info::Info {
                id: acp::SessionId::new(uuid::Uuid::new_v4().to_string()),
                cwd: cwd.path().to_string_lossy().into_owned(),
            };
            TurnEndHarness {
                agent,
                handle,
                registry,
                _gateway_rx: gateway_rx,
                _cwd: cwd,
            }
        }

        fn turn_end_args(&self, turn: u64) -> super::RegistryTurnEndArgs {
            self.agent.build_registry_turn_end_args(
                &self.handle.info.id,
                turn,
                &self.handle,
                &[acp::ContentBlock::from(format!("prompt of turn {turn}"))],
                self.handle.registry_write_order.begin_turn_end(),
                GitRead::Failed,
                None,
            )
        }

        async fn end_turn(&self, turn: u64) {
            run_turn_end(self.turn_end_args(turn)).await;
        }
    }

    async fn run_turn_end(args: super::RegistryTurnEndArgs) {
        let (archive_confirmed_tx, archive_confirmed_rx) = tokio::sync::oneshot::channel();
        archive_confirmed_tx.send(true).unwrap();
        super::run_registry_turn_end(args, archive_confirmed_rx).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn restored_child_registers_on_its_first_turn_past_zero() {
        let harness = TurnEndHarness::start().await;

        harness.end_turn(3).await;
        harness.end_turn(4).await;

        let registry = harness.registry.lock();
        assert_eq!(1, registry.registers);
        assert_eq!(0, registry.rejected_updates);
        assert_eq!(
            Some(serde_json::json!({ "lastTurnNumber": 4, "restorableTurnNumber": 4 })),
            registry.row(&harness.handle.info.id)
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn only_turn_zero_sends_the_first_prompt() {
        let harness = TurnEndHarness::start().await;

        harness.end_turn(0).await;

        assert_eq!(
            Some(serde_json::json!({
                "firstPrompt": "prompt of turn 0",
                "lastTurnNumber": 0,
                "restorableTurnNumber": 0,
            })),
            harness.registry.lock().row(&harness.handle.info.id)
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn transient_register_failure_is_retried_on_next_turn() {
        let harness = TurnEndHarness::start().await;
        harness
            .registry
            .lock()
            .register_failures
            .push_back(axum::http::StatusCode::SERVICE_UNAVAILABLE);

        harness.end_turn(3).await;
        harness.end_turn(4).await;

        let registry = harness.registry.lock();
        assert_eq!(2, registry.registers);
        assert_eq!(2, registry.rejected_updates);
        assert_eq!(
            Some(serde_json::json!({ "lastTurnNumber": 4, "restorableTurnNumber": 4 })),
            registry.row(&harness.handle.info.id)
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn refused_register_is_not_retried() {
        let harness = TurnEndHarness::start().await;
        harness
            .registry
            .lock()
            .register_failures
            .push_back(axum::http::StatusCode::CONFLICT);

        harness.end_turn(3).await;
        harness.end_turn(4).await;

        let registry = harness.registry.lock();
        assert_eq!(1, registry.registers);
        assert_eq!(4, registry.rejected_updates);
        assert_eq!(None, registry.row(&harness.handle.info.id));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn overlapping_turn_ends_register_once_before_any_update() {
        let harness = TurnEndHarness::start().await;
        let fifth = harness.turn_end_args(5);
        let sixth = harness.turn_end_args(6);

        tokio::join!(run_turn_end(sixth), run_turn_end(fifth));

        let registry = harness.registry.lock();
        assert_eq!(1, registry.registers);
        assert_eq!(0, registry.rejected_updates);
        assert_eq!(
            Some(serde_json::json!({ "lastTurnNumber": 6, "restorableTurnNumber": 6 })),
            registry.row(&harness.handle.info.id)
        );
    }
}
