//! [`GrokStdioClient`] drives `grok agent stdio`: it owns the child, the [`AgentConnection`] over its pipes,
//! and runs every request under a scaled budget. A request the test treats as infallible panics, with the
//! child's stderr, when it times out or the agent refuses it. [`GrokStdioClient::try_initialize`] and
//! [`GrokStdioClient::try_load_session`] return that failure.

use std::path::Path;
use std::pin::pin;
use std::sync::Mutex;
use std::time::Duration;

use agent_client_protocol as acp;

use crate::acp_agent_connection::{AgentConnection, SessionParams, timed, timed_ok, timed_result};
use crate::acp_agent_process::{AgentProcessOptions, SpawnedAgent};
use crate::acp_policy::ClientPolicy;
use crate::acp_transcript::TranscriptEntry;
use crate::mock_server::MockInferenceServer;
use crate::process::TestProcess;
use crate::sandbox::TestSandbox;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
/// A prompt runs a whole turn against the mock server, as does the wait for a request held inside one.
const PROMPT_TIMEOUT: Duration = Duration::from_secs(30);
/// `session/load` replays history and is slower under Rosetta (macos-x86_64 lifecycle CI), where 20s flaked.
const LOAD_SESSION_TIMEOUT: Duration = Duration::from_secs(60);

/// How [`GrokStdioClient::spawn_with_options`] starts the agent. [`SpawnOptions::new`] is the sandbox alone,
/// with no overrides and a policy that allows every permission and dismisses every question.
pub struct SpawnOptions {
    agent: AgentProcessOptions,
    policy: ClientPolicy,
    turn_budget: Option<Duration>,
    session_params: SessionParams,
}

impl SpawnOptions {
    #[must_use]
    pub fn new(sandbox: TestSandbox) -> Self {
        SpawnOptions {
            agent: AgentProcessOptions::new(sandbox),
            policy: ClientPolicy::default(),
            turn_budget: None,
            session_params: SessionParams::default(),
        }
    }

    /// Applied to the sandbox after its hermetic baseline, in order, so a later pair overrides an earlier one
    /// with the same key.
    #[must_use]
    pub fn with_extra_env(
        mut self,
        extra_env: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
    ) -> Self {
        self.agent.extra_env.extend(
            extra_env
                .into_iter()
                .map(|(key, value)| (key.into(), value.into())),
        );
        self
    }

    /// Keys removed from the sandbox baseline after the mock URL and `with_extra_env` are applied.
    /// Use this to drop a baseline variable a scenario must run without, such as the mock's
    /// `XAI_API_KEY`, so a login-only or missing-credential case is exercised faithfully.
    #[must_use]
    pub fn with_removed_env(
        mut self,
        removed_env: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.agent
            .removed_env
            .extend(removed_env.into_iter().map(Into::into));
        self
    }

    /// Flags placed between `agent` and `stdio`, such as `--yolo`.
    #[must_use]
    pub fn with_agent_args(
        mut self,
        agent_args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.agent
            .agent_args
            .extend(agent_args.into_iter().map(Into::into));
        self
    }

    /// Global flags placed before the `agent stdio` subcommand.
    #[must_use]
    pub fn with_leading_args(
        mut self,
        leading_args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.agent
            .leading_args
            .extend(leading_args.into_iter().map(Into::into));
        self
    }

    #[must_use]
    pub fn with_policy(mut self, policy: ClientPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Budget for `prompt` and `ext_method`, so a caller that wraps the turn in its own deadline
    /// records the timeout instead of the client panicking at the shorter default first. Setup
    /// requests keep the default request budget.
    #[must_use]
    pub fn with_turn_budget(mut self, budget: Duration) -> Self {
        self.turn_budget = Some(budget);
        self
    }

    #[must_use]
    pub fn with_binary(mut self, binary: impl Into<std::path::PathBuf>) -> Self {
        self.agent.binary = Some(binary.into());
        self
    }

    /// The `_meta` and `mcpServers` every `session/new` and `session/load` carries.
    #[must_use]
    pub fn with_session_params(mut self, params: SessionParams) -> Self {
        self.session_params = params;
        self
    }
}

/// Spawn and drive it inside a `tokio::task::LocalSet`; the connection's tasks are spawned locally. The child
/// is killed on drop, before its sandbox goes.
pub struct GrokStdioClient {
    connection: AgentConnection,
    process: TestProcess,
    sandbox: TestSandbox,
    turn_budget: Option<Duration>,
    /// Aborted when the client drops, so a FIFO open cannot outlive the turn.
    fifo_release: Mutex<Option<FifoRelease>>,
}

/// Join handle that aborts its task on drop. Dropping a bare `JoinHandle` detaches instead.
struct FifoRelease {
    task: tokio::task::JoinHandle<()>,
}

impl FifoRelease {
    fn spawn(task: impl std::future::Future<Output = ()> + Send + 'static) -> Self {
        Self {
            task: tokio::spawn(task),
        }
    }
}

impl Drop for FifoRelease {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[derive(Clone, Copy)]
enum FifoEnd {
    Read,
    Write,
}

pub async fn open_fifo_writer_until(
    path: &Path,
    deadline: tokio::time::Instant,
) -> Option<std::fs::File> {
    open_fifo_until(path, FifoEnd::Write, deadline).await
}

/// Opens the write end without blocking when no reader is attached, and retries until the prompt budget.
pub async fn write_fifo_release(fifo: &Path) {
    let deadline = tokio::time::Instant::now() + crate::scaled(PROMPT_TIMEOUT);
    let Some(mut file) = open_fifo_writer_until(fifo, deadline).await else {
        return;
    };
    let _ = tokio::task::spawn_blocking(move || {
        use std::io::Write;
        let _ = file.write_all(b"\n");
    })
    .await;
}

/// Read end stays non-blocking, so a missing writer cannot stall the caller. Retries until `deadline`.
pub async fn read_fifo_until(path: &Path, deadline: tokio::time::Instant) -> std::io::Result<()> {
    let mut file = open_fifo(path, FifoEnd::Read)?;
    loop {
        match read_fifo_byte(&mut file) {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(error) => return Err(error),
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "done fifo",
            ));
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// `None` when the budget runs out or the open fails for a reason other than "no peer yet".
async fn open_fifo_until(
    path: &Path,
    end: FifoEnd,
    deadline: tokio::time::Instant,
) -> Option<std::fs::File> {
    loop {
        let opened = match end {
            FifoEnd::Read => open_fifo(path, end).map_err(Some),
            FifoEnd::Write => {
                let path = path.to_path_buf();
                match tokio::task::spawn_blocking(move || open_fifo(&path, FifoEnd::Write)).await {
                    Ok(result) => result.map_err(Some),
                    Err(_) => Err(None),
                }
            }
        };
        match opened {
            Ok(file) => return Some(file),
            Err(Some(error)) if matches!(end, FifoEnd::Write) && fifo_has_no_reader(&error) => {}
            Err(_) => return None,
        }
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn read_fifo_byte(file: &mut std::fs::File) -> std::io::Result<bool> {
    use std::io::Read as _;
    let mut buf = [0_u8; 64];
    match file.read(&mut buf) {
        Ok(0) => Ok(false),
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(false),
        Err(error) => Err(error),
    }
}

fn fifo_has_no_reader(error: &std::io::Error) -> bool {
    #[cfg(unix)]
    {
        error.raw_os_error() == Some(libc::ENXIO)
    }
    #[cfg(not(unix))]
    {
        let _ = error;
        false
    }
}

/// Syscall wrappers for the sibling-hold FIFO. Protocol code stays safe.
#[cfg(unix)]
mod fifo_syscall {
    use std::os::unix::io::AsRawFd;

    pub(super) fn mkfifo(path: &std::path::Path) -> std::io::Result<()> {
        let bytes = std::os::unix::ffi::OsStrExt::as_bytes(path.as_os_str());
        let c_path = std::ffi::CString::new(bytes)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
        // SAFETY: `c_path` is a fresh NUL-terminated path and outlives the call.
        let rc = unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) };
        if rc == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }

    pub(super) fn set_blocking(file: &std::fs::File) -> std::io::Result<()> {
        let fd = file.as_raw_fd();
        // SAFETY: `fd` belongs to `file` and stays open for this call.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `fd` belongs to `file`, and `flags` is the result of F_GETFL on that fd.
        let rc = unsafe { libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK) };
        if rc == -1 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

/// Non-blocking open. The write end then becomes blocking so the release write waits for the reader.
/// The read end stays non-blocking: a blocking read open hangs until a writer appears.
fn open_fifo(path: &Path, end: FifoEnd) -> std::io::Result<std::fs::File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut options = std::fs::OpenOptions::new();
        match end {
            FifoEnd::Read => {
                options.read(true);
            }
            FifoEnd::Write => {
                options.write(true);
            }
        }
        let file = options.custom_flags(libc::O_NONBLOCK).open(path)?;
        if matches!(end, FifoEnd::Write) {
            fifo_syscall::set_blocking(&file)?;
        }
        Ok(file)
    }
    #[cfg(not(unix))]
    {
        let mut options = std::fs::OpenOptions::new();
        match end {
            FifoEnd::Read => {
                options.read(true);
            }
            FifoEnd::Write => {
                options.write(true);
            }
        }
        options.open(path)
    }
}

/// Creates `path` as a FIFO. A direct syscall, so the caller does not spawn `mkfifo` or block on a process.
pub fn make_fifo(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        fifo_syscall::mkfifo(path)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "mkfifo",
        ))
    }
}

fn completed_tool_seen(entries: &[TranscriptEntry], needle: &str) -> bool {
    let mut tool_id = None;
    for entry in entries {
        let TranscriptEntry::SessionUpdate(notification) = entry else {
            continue;
        };
        let Ok(value) = serde_json::to_value(&notification.update) else {
            continue;
        };
        let rendered = value.to_string();
        let id = value
            .get("toolCallId")
            .or_else(|| value.get("tool_call_id"))
            .and_then(|id| id.as_str())
            .map(str::to_owned);
        if tool_id.is_none() && rendered.contains(needle) {
            tool_id = id;
        }
        if value.get("status").and_then(|status| status.as_str()) == Some("completed")
            && tool_id
                .as_ref()
                .is_some_and(|id| rendered.contains(id.as_str()))
        {
            return true;
        }
    }
    false
}

impl GrokStdioClient {
    pub async fn spawn(server: &MockInferenceServer, cwd: &Path) -> Self {
        Self::spawn_with_options(server, cwd, SpawnOptions::new(TestSandbox::new())).await
    }

    pub async fn spawn_with_sandbox(
        server: &MockInferenceServer,
        cwd: &Path,
        sandbox: TestSandbox,
    ) -> Self {
        Self::spawn_with_options(server, cwd, SpawnOptions::new(sandbox)).await
    }

    pub async fn spawn_with_options(
        server: &MockInferenceServer,
        cwd: &Path,
        options: SpawnOptions,
    ) -> Self {
        let SpawnOptions {
            agent,
            policy,
            turn_budget,
            session_params,
        } = options;
        let SpawnedAgent {
            mut process,
            sandbox,
        } = agent.spawn(server, cwd);
        let connection =
            AgentConnection::connect(&mut process, policy).with_session_params(session_params);
        GrokStdioClient {
            connection,
            process,
            sandbox,
            turn_budget,
            fifo_release: Mutex::new(None),
        }
    }

    /// Initialize, then authenticate with the `xai.api_key` method.
    pub async fn initialize(&self) -> acp::InitializeResponse {
        timed_ok(
            &self.process,
            "initialize",
            REQUEST_TIMEOUT,
            self.connection.initialize_and_authenticate(),
        )
        .await
    }

    /// [`Self::initialize`] that returns the agent's refusal or a missed budget instead of panicking.
    /// A cold process after logout offers no cached credential, and the caller records that refusal.
    pub async fn try_initialize(&self) -> acp::Result<acp::InitializeResponse> {
        timed_result(
            "initialize",
            REQUEST_TIMEOUT,
            self.connection.initialize_and_authenticate(),
        )
        .await
    }

    pub async fn create_session(&self, cwd: &Path) -> acp::SessionId {
        self.create_session_response(cwd).await.session_id
    }

    pub async fn create_session_response(&self, cwd: &Path) -> acp::NewSessionResponse {
        timed_ok(
            &self.process,
            "session/new",
            REQUEST_TIMEOUT,
            self.connection.new_session_response(cwd),
        )
        .await
    }

    pub async fn create_session_with_model(&self, cwd: &Path, model_id: &str) -> acp::SessionId {
        timed_ok(
            &self.process,
            &format!("session/new with modelId={model_id}"),
            REQUEST_TIMEOUT,
            self.connection.new_session_with_model(cwd, model_id),
        )
        .await
    }

    pub async fn set_model(
        &self,
        session_id: &acp::SessionId,
        model_id: &str,
    ) -> acp::Result<acp::SetSessionModelResponse> {
        self.set_model_with_effort(session_id, model_id, None).await
    }

    /// `_meta.reasoningEffort` is how the pager sends `/effort` on the current model.
    pub async fn set_model_with_effort(
        &self,
        session_id: &acp::SessionId,
        model_id: &str,
        effort: Option<&str>,
    ) -> acp::Result<acp::SetSessionModelResponse> {
        timed(
            &self.process,
            &format!("session/set_model({model_id})"),
            REQUEST_TIMEOUT,
            self.connection
                .set_model_with_effort(session_id, model_id, effort),
        )
        .await
    }

    pub async fn set_mode(
        &self,
        session_id: &acp::SessionId,
        mode_id: &str,
    ) -> acp::Result<acp::SetSessionModeResponse> {
        timed(
            &self.process,
            &format!("session/set_mode({mode_id})"),
            REQUEST_TIMEOUT,
            self.connection.set_mode(session_id, mode_id),
        )
        .await
    }

    /// Sends `authenticate`, which an interactive login answers only once the login ends.
    pub async fn authenticate(
        &self,
        method_id: &str,
        meta: serde_json::Value,
    ) -> acp::Result<acp::AuthenticateResponse> {
        timed(
            &self.process,
            &format!("authenticate({method_id})"),
            REQUEST_TIMEOUT,
            self.connection.authenticate(method_id, meta),
        )
        .await
    }

    pub async fn set_config_option(
        &self,
        session_id: &acp::SessionId,
        config_id: &str,
        value: &str,
    ) -> acp::Result<acp::SetSessionConfigOptionResponse> {
        timed(
            &self.process,
            &format!("session/set_config_option({config_id}={value})"),
            REQUEST_TIMEOUT,
            self.connection
                .set_config_option(session_id, config_id, value),
        )
        .await
    }

    pub async fn prompt(
        &self,
        session_id: &acp::SessionId,
        text: &str,
    ) -> acp::Result<acp::PromptResponse> {
        timed(
            &self.process,
            "prompt",
            self.turn_budget.unwrap_or(PROMPT_TIMEOUT),
            self.connection.prompt(session_id, text),
        )
        .await
    }

    pub async fn prompt_blocks(
        &self,
        session_id: &acp::SessionId,
        blocks: Vec<acp::ContentBlock>,
    ) -> acp::Result<acp::PromptResponse> {
        timed(
            &self.process,
            "prompt blocks",
            self.turn_budget.unwrap_or(PROMPT_TIMEOUT),
            self.connection.prompt_blocks(session_id, blocks),
        )
        .await
    }

    pub async fn prompt_send_now(
        &self,
        session_id: &acp::SessionId,
        text: &str,
    ) -> acp::Result<acp::PromptResponse> {
        timed(
            &self.process,
            "prompt send-now",
            self.turn_budget.unwrap_or(PROMPT_TIMEOUT),
            self.connection.prompt_with_meta(
                session_id,
                text,
                serde_json::json!({ "sendNow": true }),
            ),
        )
        .await
    }

    pub async fn load_session(
        &self,
        session_id: &acp::SessionId,
        cwd: &Path,
    ) -> acp::LoadSessionResponse {
        timed_ok(
            &self.process,
            "session/load",
            LOAD_SESSION_TIMEOUT,
            self.connection.load_session(session_id, cwd),
        )
        .await
    }

    /// [`Self::load_session`] that returns the agent's refusal or a missed budget instead of panicking.
    pub async fn try_load_session(
        &self,
        session_id: &acp::SessionId,
        cwd: &Path,
    ) -> acp::Result<acp::LoadSessionResponse> {
        timed_result(
            "session/load",
            LOAD_SESSION_TIMEOUT,
            self.connection.load_session(session_id, cwd),
        )
        .await
    }

    /// A `session/load` that adds `meta`, such as `{"noReplay": true}`, to the session params.
    pub async fn load_session_with_meta(
        &self,
        session_id: &acp::SessionId,
        cwd: &Path,
        meta: serde_json::Value,
    ) -> acp::LoadSessionResponse {
        timed_ok(
            &self.process,
            "session/load",
            LOAD_SESSION_TIMEOUT,
            self.connection
                .load_session_with_meta(session_id, cwd, meta),
        )
        .await
    }

    pub async fn ext_method(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> acp::Result<acp::ExtResponse> {
        timed(
            &self.process,
            &format!("ext {method}"),
            self.turn_budget.unwrap_or(REQUEST_TIMEOUT),
            self.connection.ext_method(method, params),
        )
        .await
    }

    pub async fn ext_notification(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> acp::Result<()> {
        timed(
            &self.process,
            &format!("ext notify {method}"),
            self.turn_budget.unwrap_or(REQUEST_TIMEOUT),
            self.connection.ext_notification(method, params),
        )
        .await
    }

    pub async fn probe(
        &self,
        method: &str,
        live_session: &acp::SessionId,
        cwd: &Path,
    ) -> acp::Result<()> {
        timed(
            &self.process,
            &format!("probe {method}"),
            REQUEST_TIMEOUT,
            self.connection.probe(method, live_session, cwd),
        )
        .await
    }

    /// Send `session/cancel`, then answer every request this session holds under `HoldUntilCancel` with
    /// `cancelled`. Resolves once every released reply is recorded in the transcript; panics with the agent's
    /// stderr if the notification cannot be sent or a held request is not answered within the request budget.
    pub async fn cancel(&self, session_id: &acp::SessionId) {
        timed_ok(
            &self.process,
            "session/cancel",
            REQUEST_TIMEOUT,
            self.connection.cancel_and_release_holds(session_id),
        )
        .await;
    }

    /// Run a prompt and `cancel` the session the moment one of its requests is held under `HoldUntilCancel`.
    /// Returns the prompt's response, so a test asserts its stop reason without sleeping. Panics at once if
    /// the turn ends before any request is held, and with the agent's stderr if none is held within the
    /// prompt budget.
    /// Resolves once this session holds a request under `HoldUntilCancel`.
    pub async fn wait_until_a_request_is_held(&self, session_id: &acp::SessionId) {
        timed(
            &self.process,
            "held request",
            PROMPT_TIMEOUT,
            self.connection
                .handler()
                .holds()
                .wait_for_held_request(session_id),
        )
        .await;
    }

    pub async fn prompt_then_cancel_at_held_request(
        &self,
        session_id: &acp::SessionId,
        text: &str,
    ) -> acp::Result<acp::PromptResponse> {
        let mut prompt = pin!(self.prompt(session_id, text));
        let held = timed(
            &self.process,
            "held request",
            PROMPT_TIMEOUT,
            self.connection
                .handler()
                .holds()
                .wait_for_held_request(session_id),
        );
        tokio::select! {
            () = held => {}
            response = &mut prompt => panic!(
                "prompt finished before any request was held: {response:?}\nstderr:\n{}",
                self.stderr()
            ),
        }
        self.cancel(session_id).await;
        prompt.await
    }

    /// Every message the agent sent so far, in the order the client finished handling them; a held request
    /// lands once `cancel` releases it.
    pub fn transcript(&self) -> Vec<TranscriptEntry> {
        self.connection.handler().transcript().entries()
    }

    pub fn wire_lines(&self) -> Vec<String> {
        self.connection.handler().wire_lines()
    }

    pub fn wire_exchange(&self) -> Vec<crate::acp_wire::WireLine> {
        self.connection.handler().wire_exchange()
    }

    /// Resolves once the transcript holds an entry `is_match` accepts, so a test waits for a notification
    /// instead of running another turn to flush it. Panics with the agent's stderr after the request budget.
    pub fn release_fifo_when_tool_completes(&self, needle: &str, fifo: std::path::PathBuf) {
        let handler = self.connection.handler().clone();
        let needle = needle.to_owned();
        let release = FifoRelease::spawn(async move {
            handler
                .transcript()
                .wait_until(|entries| completed_tool_seen(entries, &needle))
                .await;
            handler
                .transcript()
                .record(TranscriptEntry::ExtNotification {
                    method: "harness.sibling_released".to_owned(),
                    params: serde_json::json!({}),
                });
            write_fifo_release(&fifo).await;
        });
        *self
            .fifo_release
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(release);
    }

    pub async fn wait_for_transcript_entry(&self, is_match: impl Fn(&TranscriptEntry) -> bool) {
        timed(
            &self.process,
            "transcript entry",
            REQUEST_TIMEOUT,
            self.connection
                .handler()
                .transcript()
                .wait_until(|entries| entries.iter().any(&is_match)),
        )
        .await;
    }

    pub async fn wait_for_transcript_after(
        &self,
        already: usize,
        is_match: impl Fn(&TranscriptEntry) -> bool,
    ) {
        timed(
            &self.process,
            "transcript entry",
            REQUEST_TIMEOUT,
            self.connection
                .handler()
                .transcript()
                .wait_until(|entries| entries.iter().skip(already).any(&is_match)),
        )
        .await;
    }

    /// A missed deadline is an error whose message is `missing` and whose source is the elapsed timeout.
    pub async fn wait_for_transcript_entry_until(
        &self,
        deadline: tokio::time::Instant,
        missing: &str,
        is_match: impl Fn(&TranscriptEntry) -> bool,
    ) -> anyhow::Result<()> {
        self.connection
            .handler()
            .transcript()
            .wait_until_deadline(deadline, missing, |entries| entries.iter().any(&is_match))
            .await?;
        Ok(())
    }

    /// Same deadline as [`Self::wait_for_transcript_entry_until`], ignoring entries already recorded.
    pub async fn wait_for_transcript_entry_after_until(
        &self,
        already: usize,
        deadline: tokio::time::Instant,
        missing: &str,
        is_match: impl Fn(&TranscriptEntry) -> bool,
    ) -> anyhow::Result<()> {
        self.connection
            .handler()
            .transcript()
            .wait_until_deadline(deadline, missing, |entries| {
                entries.iter().skip(already).any(&is_match)
            })
            .await?;
        Ok(())
    }

    pub fn captured_text(&self) -> String {
        self.connection.handler().transcript().agent_text()
    }

    pub fn notification_count(&self) -> usize {
        self.connection
            .handler()
            .transcript()
            .session_update_count()
    }

    pub fn stderr(&self) -> String {
        self.process.stderr_tail().text
    }

    pub fn child_pid(&self) -> Option<u32> {
        self.process.pid()
    }

    pub fn process_diagnostics(&self) -> String {
        self.process.diagnostic_summary()
    }

    pub fn start_terminate(&mut self) -> std::io::Result<()> {
        self.process.start_terminate()
    }

    pub fn start_kill(&mut self) {
        self.process.start_kill();
    }

    pub async fn close(&mut self) -> std::io::Result<std::process::ExitStatus> {
        self.process.close().await
    }

    pub fn sandbox(&self) -> &TestSandbox {
        &self.sandbox
    }

    /// Kills the child and hands its sandbox back, so a restart on the same sandbox never overlaps the old
    /// process.
    pub fn into_sandbox(self) -> TestSandbox {
        self.sandbox
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use super::{FifoRelease, PROMPT_TIMEOUT, make_fifo, write_fifo_release};

    struct FlagOnDrop(Arc<AtomicBool>);

    impl Drop for FlagOnDrop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn dropped_fifo_release_ends_without_a_reader() {
        let dir = tempfile::tempdir().expect("temp dir");
        let fifo = dir.path().join("sibling.hold");
        make_fifo(&fifo).expect("mkfifo");
        let ended = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&ended);
        let release = FifoRelease::spawn(async move {
            let _ended = FlagOnDrop(flag);
            write_fifo_release(&fifo).await;
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        drop(release);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while !ended.load(Ordering::SeqCst) {
            if tokio::time::Instant::now() >= deadline {
                panic!("fifo release still running after drop, with no reader");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[tokio::test]
    async fn write_fifo_release_returns_without_a_reader() {
        let dir = tempfile::tempdir().expect("temp dir");
        let fifo = dir.path().join("sibling.hold");
        make_fifo(&fifo).expect("mkfifo");
        let bound = crate::scaled(PROMPT_TIMEOUT) + Duration::from_secs(2);
        tokio::time::timeout(bound, write_fifo_release(&fifo))
            .await
            .expect("fifo release returned without a reader");
    }
}
