//! The typed `agent-client-protocol` connection a test client holds to an agent child: the pipes, the
//! [`ScriptedClient`] that answers the agent, and the reader, io, and request handler tasks, which stop with
//! the connection. Every request shape a test client sends lives here once, untimed; [`timed`],
//! [`timed_ok`], and [`timed_result`] are the budget each client puts on top.

use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use agent_client_protocol::{self as acp, Agent as _};
use futures_util::future::LocalBoxFuture;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use tokio_util::sync::{CancellationToken, DropGuard};
use tokio_util::task::AbortOnDropHandle;
use xai_acp_lib::LineBufferedRead;

use crate::acp_policy::{ClientPolicy, Interactivity};

use crate::acp_scripted_client::ScriptedClient;
use crate::process::TestProcess;
use crate::scaled;

const API_KEY_AUTH_METHOD: &str = "xai.api_key";
/// The non-interactive session-login method the agent advertises when a cached `auth.json` login is
/// present and no first-party env key is set.
const CACHED_TOKEN_AUTH_METHOD: &str = "cached_token";
/// The `_meta` flag on the login method the agent runs through `auth_provider_command`.
const EXTERNAL_PROVIDER_META: &str = "external_provider";

/// Prefer the env-key method, then the cached login, then a login backed by the operator's
/// `auth_provider_command` (`_meta.external_provider`); each authenticates without a browser. Returns
/// `None` when the agent offers only interactive methods, which a headless test cannot complete.
fn select_auth_method(methods: &[acp::AuthMethod]) -> Option<acp::AuthMethodId> {
    let by_id = |wanted: &str| {
        methods
            .iter()
            .find(|method| method.id().0.as_ref() == wanted)
            .map(|method| method.id().clone())
    };
    let external_provider = || {
        methods
            .iter()
            .find(|method| {
                method
                    .meta()
                    .and_then(|meta| meta.get(EXTERNAL_PROVIDER_META))
                    .and_then(serde_json::Value::as_bool)
                    == Some(true)
            })
            .map(|method| method.id().clone())
    };
    by_id(API_KEY_AUTH_METHOD)
        .or_else(|| by_id(CACHED_TOKEN_AUTH_METHOD))
        .or_else(external_provider)
}

/// The `client_capabilities.meta` that advertises `x.ai/folderTrust.interactive`, so the agent knows
/// this client can answer an interactive folder-trust prompt.
fn interactive_trust_capability() -> serde_json::Map<String, serde_json::Value> {
    serde_json::json!({ "x.ai/folderTrust": { "interactive": true } })
        .as_object()
        .cloned()
        .expect("object literal is a JSON object")
}

/// What every `session/new` and `session/load` this connection sends carries, the way an embedding
/// client sends one `_meta` and one `mcpServers` list on both requests. Empty by default.
#[derive(Debug, Clone, Default)]
pub struct SessionParams {
    /// Merged into the request's `_meta`; a key the request sets itself, such as `modelId`, wins.
    pub meta: serde_json::Map<String, serde_json::Value>,
    pub mcp_servers: Vec<acp::McpServer>,
}

impl SessionParams {
    fn meta_with(
        &self,
        own: Option<serde_json::Map<String, serde_json::Value>>,
    ) -> Option<serde_json::Map<String, serde_json::Value>> {
        let mut meta = self.meta.clone();
        meta.extend(own.unwrap_or_default());
        (!meta.is_empty()).then_some(meta)
    }
}

/// Built inside a `tokio::task::LocalSet`: the connection is not `Send`, so its tasks are spawned locally.
pub(crate) struct AgentConnection {
    conn: acp::ClientSideConnection,
    handler: ScriptedClient,
    session_params: SessionParams,
    /// Aborted on drop so the connection loop and the pipe writer stop with the connection.
    _io_task: AbortOnDropHandle<acp::Result<()>>,
    /// Cancels, on drop, the line reader and every request handler spawned through the connection's spawn
    /// callback, so a handler still waiting on a hold stops with the connection.
    _stop_spawned_tasks: DropGuard,
}

impl AgentConnection {
    /// Takes the child's piped stdin and stdout.
    pub(crate) fn connect(process: &mut TestProcess, policy: ClientPolicy) -> Self {
        let handler = ScriptedClient::new(policy);
        let wire = handler.wire();
        let outgoing = wire
            .record_client_lines(process.take_stdin().expect("child stdin missing"))
            .compat_write();
        let incoming = wire
            .record_agent_lines(process.take_stdout().expect("child stdout missing"))
            .compat();

        let stop = CancellationToken::new();
        let spawn_until_stopped = {
            let stop = stop.clone();
            move |future: LocalBoxFuture<'static, ()>| {
                tokio::task::spawn_local(stop.clone().run_until_cancelled_owned(future));
            }
        };
        let incoming = LineBufferedRead::new(incoming, &spawn_until_stopped);
        let (conn, handle_io) = acp::ClientSideConnection::new(
            handler.clone(),
            outgoing,
            incoming,
            spawn_until_stopped,
        );
        AgentConnection {
            conn,
            handler,
            session_params: SessionParams::default(),
            _io_task: AbortOnDropHandle::new(tokio::task::spawn_local(handle_io)),
            _stop_spawned_tasks: stop.drop_guard(),
        }
    }

    pub(crate) fn with_session_params(mut self, params: SessionParams) -> Self {
        self.session_params = params;
        self
    }

    pub(crate) fn handler(&self) -> &ScriptedClient {
        &self.handler
    }

    /// `initialize` as a test client, then `authenticate` with the `xai.api_key` method in headless
    /// mode. The client advertises `nonInteractive` per its [`Interactivity`]: `Headless` (the default)
    /// stays non-interactive; `Interactive` opts in so the agent forwards reverse interactions such as
    /// MCP elicitation instead of auto-cancelling them. An agent that offers no non-interactive auth
    /// method is an error naming the methods it offered.
    pub(crate) async fn initialize_and_authenticate(&self) -> acp::Result<acp::InitializeResponse> {
        let mut capabilities = acp::ClientCapabilities::new()
            .fs(acp::FileSystemCapabilities::new())
            .terminal(false);
        if self.handler.advertises_interactive_trust() {
            capabilities = capabilities.meta(interactive_trust_capability());
        }
        let non_interactive = matches!(self.handler.interactivity(), Interactivity::Headless);
        let response = self
            .conn
            .initialize(
                acp::InitializeRequest::new(acp::ProtocolVersion::V1)
                    .client_capabilities(capabilities)
                    .meta(
                        serde_json::json!({
                            "startupHints": {
                                "nonInteractive": non_interactive
                            },
                            "clientType": "test-client",
                            "clientVersion": "0.0.0-test"
                        })
                        .as_object()
                        .cloned(),
                    ),
            )
            .await?;

        let chosen = select_auth_method(&response.auth_methods).ok_or_else(|| {
            let offered: Vec<_> = response
                .auth_methods
                .iter()
                .map(|method| &method.id().0)
                .collect();
            acp::Error::new(
                i32::from(acp::ErrorCode::AuthRequired),
                format!(
                    "no non-interactive auth method ({API_KEY_AUTH_METHOD} or {CACHED_TOKEN_AUTH_METHOD}); the agent offered {offered:?}"
                ),
            )
        })?;
        self.authenticate(chosen.0.as_ref(), serde_json::json!({ "headless": true }))
            .await?;
        Ok(response)
    }

    pub(crate) async fn new_session(&self, cwd: &Path) -> acp::Result<acp::SessionId> {
        Ok(self.new_session_response(cwd).await?.session_id)
    }

    pub(crate) async fn new_session_with_model(
        &self,
        cwd: &Path,
        model_id: &str,
    ) -> acp::Result<acp::SessionId> {
        self.new_session_with(cwd, Some(model_id), Vec::new()).await
    }

    pub(crate) async fn new_session_with(
        &self,
        cwd: &Path,
        model_id: Option<&str>,
        mcp_servers: Vec<acp::McpServer>,
    ) -> acp::Result<acp::SessionId> {
        Ok(self
            .new_session_with_response(cwd, model_id, mcp_servers)
            .await?
            .session_id)
    }

    async fn new_session_with_response(
        &self,
        cwd: &Path,
        model_id: Option<&str>,
        mcp_servers: Vec<acp::McpServer>,
    ) -> acp::Result<acp::NewSessionResponse> {
        let mut request = acp::NewSessionRequest::new(cwd.to_path_buf());
        if let Some(model_id) = model_id {
            request = request.meta(
                serde_json::json!({ "modelId": model_id })
                    .as_object()
                    .cloned(),
            );
        }
        let mut servers = mcp_servers;
        servers.extend(self.session_params.mcp_servers.clone());
        self.send_new_session(request.mcp_servers(servers)).await
    }

    async fn send_new_session(
        &self,
        mut request: acp::NewSessionRequest,
    ) -> acp::Result<acp::NewSessionResponse> {
        if let Some((key, hooks)) = self.handler.client_hook_registration() {
            request
                .meta
                .get_or_insert_with(acp::Meta::new)
                .insert(key, hooks);
        }
        let meta = self.session_params.meta_with(request.meta.take());
        self.conn.new_session(request.meta(meta)).await
    }

    pub(crate) async fn new_session_response(
        &self,
        cwd: &Path,
    ) -> acp::Result<acp::NewSessionResponse> {
        self.new_session_with_response(cwd, None, Vec::new()).await
    }

    pub(crate) async fn load_session(
        &self,
        session_id: &acp::SessionId,
        cwd: &Path,
    ) -> acp::Result<acp::LoadSessionResponse> {
        self.conn
            .load_session(
                acp::LoadSessionRequest::new(session_id.clone(), cwd.to_path_buf())
                    .mcp_servers(self.session_params.mcp_servers.clone())
                    .meta(self.session_params.meta_with(None)),
            )
            .await
    }

    pub(crate) async fn load_session_with_meta(
        &self,
        session_id: &acp::SessionId,
        cwd: &Path,
        meta: serde_json::Value,
    ) -> acp::Result<acp::LoadSessionResponse> {
        self.conn
            .load_session(
                acp::LoadSessionRequest::new(session_id.clone(), cwd.to_path_buf())
                    .mcp_servers(self.session_params.mcp_servers.clone())
                    .meta(self.session_params.meta_with(meta.as_object().cloned())),
            )
            .await
    }

    pub(crate) async fn prompt(
        &self,
        session_id: &acp::SessionId,
        text: &str,
    ) -> acp::Result<acp::PromptResponse> {
        self.conn
            .prompt(acp::PromptRequest::new(
                session_id.clone(),
                vec![acp::ContentBlock::Text(acp::TextContent::new(
                    text.to_owned(),
                ))],
            ))
            .await
    }

    pub(crate) async fn prompt_with_meta(
        &self,
        session_id: &acp::SessionId,
        text: &str,
        meta: serde_json::Value,
    ) -> acp::Result<acp::PromptResponse> {
        self.conn
            .prompt(
                acp::PromptRequest::new(
                    session_id.clone(),
                    vec![acp::ContentBlock::Text(acp::TextContent::new(
                        text.to_owned(),
                    ))],
                )
                .meta(meta.as_object().cloned()),
            )
            .await
    }

    pub(crate) async fn prompt_blocks(
        &self,
        session_id: &acp::SessionId,
        blocks: Vec<acp::ContentBlock>,
    ) -> acp::Result<acp::PromptResponse> {
        self.conn
            .prompt(acp::PromptRequest::new(session_id.clone(), blocks))
            .await
    }

    pub(crate) async fn set_model(
        &self,
        session_id: &acp::SessionId,
        model_id: &str,
    ) -> acp::Result<acp::SetSessionModelResponse> {
        self.set_model_with_effort(session_id, model_id, None).await
    }

    /// `session/set_model`, with `_meta.reasoningEffort` when `effort` is set. That is how the pager
    /// sends `/effort`: the current model id plus the effort, and `None` on a switch that keeps it.
    pub(crate) async fn set_model_with_effort(
        &self,
        session_id: &acp::SessionId,
        model_id: &str,
        effort: Option<&str>,
    ) -> acp::Result<acp::SetSessionModelResponse> {
        let mut request =
            acp::SetSessionModelRequest::new(session_id.clone(), acp::ModelId::new(model_id));
        if let Some(effort) = effort {
            let mut meta = acp::Meta::new();
            meta.insert(
                "reasoningEffort".to_owned(),
                serde_json::Value::String(effort.to_owned()),
            );
            request = request.meta(Some(meta));
        }
        self.conn.set_session_model(request).await
    }

    pub(crate) async fn set_mode(
        &self,
        session_id: &acp::SessionId,
        mode_id: &str,
    ) -> acp::Result<acp::SetSessionModeResponse> {
        self.conn
            .set_session_mode(acp::SetSessionModeRequest::new(
                session_id.clone(),
                acp::SessionModeId::new(mode_id),
            ))
            .await
    }

    pub(crate) async fn authenticate(
        &self,
        method_id: &str,
        meta: serde_json::Value,
    ) -> acp::Result<acp::AuthenticateResponse> {
        self.conn
            .authenticate(
                acp::AuthenticateRequest::new(method_id.to_owned()).meta(meta.as_object().cloned()),
            )
            .await
    }

    pub(crate) async fn set_config_option(
        &self,
        session_id: &acp::SessionId,
        config_id: &str,
        value: &str,
    ) -> acp::Result<acp::SetSessionConfigOptionResponse> {
        self.conn
            .set_session_config_option(acp::SetSessionConfigOptionRequest::new(
                session_id.clone(),
                config_id.to_owned(),
                acp::SessionConfigValueId::new(value),
            ))
            .await
    }

    pub(crate) async fn ext_method(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> acp::Result<acp::ExtResponse> {
        let raw = serde_json::value::to_raw_value(&params).expect("serialize ext params");
        self.conn
            .ext_method(acp::ExtRequest::new(method, Arc::from(raw)))
            .await
    }

    pub(crate) async fn ext_notification(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> acp::Result<()> {
        let encoded =
            serde_json::value::to_raw_value(&params).expect("serialize ext notification params");
        self.conn
            .ext_notification(acp::ExtNotification::new(method, Arc::from(encoded)))
            .await
    }

    /// `session/set_mode` uses `live_session` and the mode it already has; the shell never answers a mode change for a session it does not hold.
    pub(crate) async fn probe(
        &self,
        method: &str,
        live_session: &acp::SessionId,
        cwd: &Path,
    ) -> acp::Result<()> {
        const UNOPENED_SESSION_ID: &str = "probe-unopened-session";
        const DEFAULT_MODE_ID: &str = "default";

        let names = acp::AGENT_METHOD_NAMES;
        let session_id = &acp::SessionId::new(UNOPENED_SESSION_ID);
        match method {
            m if m == names.initialize => self
                .conn
                .initialize(acp::InitializeRequest::new(acp::ProtocolVersion::V1))
                .await
                .map(drop),
            m if m == names.authenticate => self
                .conn
                .authenticate(acp::AuthenticateRequest::new(API_KEY_AUTH_METHOD))
                .await
                .map(drop),
            m if m == names.session_new => self.new_session(cwd).await.map(drop),
            m if m == names.session_load => self.load_session(session_id, cwd).await.map(drop),
            m if m == names.session_list => self
                .conn
                .list_sessions(acp::ListSessionsRequest::new())
                .await
                .map(drop),
            m if m == names.session_resume => self
                .conn
                .resume_session(acp::ResumeSessionRequest::new(
                    session_id.clone(),
                    cwd.to_path_buf(),
                ))
                .await
                .map(drop),
            m if m == names.session_close => self
                .conn
                .close_session(acp::CloseSessionRequest::new(session_id.clone()))
                .await
                .map(drop),
            m if m == names.session_prompt => self.prompt(session_id, "").await.map(drop),
            m if m == names.session_set_mode => {
                self.set_mode(live_session, DEFAULT_MODE_ID).await.map(drop)
            }
            m if m == names.session_set_model => self.set_model(session_id, "").await.map(drop),
            m if m == names.session_set_config_option => self
                .conn
                .set_session_config_option(acp::SetSessionConfigOptionRequest::new(
                    session_id.clone(),
                    String::new(),
                    acp::SessionConfigValueId::new(""),
                ))
                .await
                .map(drop),
            _ => self
                .ext_method(method, serde_json::json!({}))
                .await
                .map(drop),
        }
    }

    /// Sends `session/cancel`, then answers every request the session holds under `HoldUntilCancel` with
    /// `cancelled`. The notification goes first because that is the order the protocol asks of a cancelling
    /// client; resolves once every released reply is recorded.
    pub(crate) async fn cancel_and_release_holds(
        &self,
        session_id: &acp::SessionId,
    ) -> acp::Result<()> {
        self.conn
            .cancel(acp::CancelNotification::new(session_id.clone()))
            .await?;
        self.handler.holds().release_held_requests(session_id).await;
        Ok(())
    }
}

/// Runs `request` under the scaled `budget`, panicking with the request name and the budget on
/// timeout, and logs the elapsed time for tuning CI budgets (visible with --nocapture).
pub(crate) async fn timed<T>(
    _process: &TestProcess,
    what: &str,
    budget: Duration,
    request: impl Future<Output = T>,
) -> T {
    let started = Instant::now();
    let result = tokio::time::timeout(scaled(budget), request)
        .await
        .unwrap_or_else(|_| panic!("{what} timed out after {budget:?}"));
    eprintln!("[harness-timing] {what}: {:?}", started.elapsed());
    result
}

/// [`timed`] for a request no test expects to fail: an error panics with the child's stderr too.
pub(crate) async fn timed_ok<T>(
    process: &TestProcess,
    what: &str,
    budget: Duration,
    request: impl Future<Output = acp::Result<T>>,
) -> T {
    timed(process, what, budget, request)
        .await
        .unwrap_or_else(|error| {
            panic!(
                "{what} failed: {error}\nstderr:\n{}",
                process.stderr_tail().text
            )
        })
}

/// [`timed`] for a request the caller records. The agent's error and a missed budget both come back.
pub(crate) async fn timed_result<T>(
    what: &str,
    budget: Duration,
    request: impl Future<Output = acp::Result<T>>,
) -> acp::Result<T> {
    let started = Instant::now();
    let result = match tokio::time::timeout(scaled(budget), request).await {
        Ok(result) => result,
        Err(_elapsed) => Err(acp::Error::new(
            i32::from(acp::ErrorCode::InternalError),
            format!("{what} timed out after {budget:?}"),
        )),
    };
    eprintln!("[harness-timing] {what}: {:?}", started.elapsed());
    result
}

#[cfg(test)]
mod tests {
    use std::future::ready;
    use std::time::Duration;

    use agent_client_protocol as acp;

    use super::timed_result;

    #[tokio::test]
    async fn timed_result_returns_the_budget_or_the_agent_error() {
        let timed_out = timed_result(
            "session/load",
            Duration::from_millis(1),
            std::future::pending::<acp::Result<()>>(),
        )
        .await
        .expect_err("budget");
        let refused = acp::Error::new(i32::from(acp::ErrorCode::InternalError), "no session");
        let agent = timed_result(
            "session/load",
            Duration::from_secs(5),
            ready::<acp::Result<()>>(Err(refused.clone())),
        )
        .await
        .expect_err("refusal");
        assert_eq!(
            ("session/load timed out after 1ms".to_owned(), refused),
            (timed_out.message, agent)
        );
    }
}
