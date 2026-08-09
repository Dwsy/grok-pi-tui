use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::HashMap,
    env,
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
    sync::{mpsc, oneshot},
};

#[derive(Debug, Clone)]
pub struct SpawnConfig {
    pub program: String,
    pub prefix_args: Vec<String>,
    pub cwd: PathBuf,
    pub pi_args: Vec<String>,
    /// Environment additions scoped to the spawned Pi process.
    pub env: Vec<(String, String)>,
}

#[derive(Clone)]
pub struct PiRpc {
    writer: mpsc::UnboundedSender<Value>,
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<Result<Value, String>>>>>,
    next_id: Arc<AtomicU64>,
    /// Control channel for the exit coordinator, which is the sole owner of
    /// the child process. This avoids holding a mutex across `Child::wait()`.
    child_control: mpsc::UnboundedSender<ChildControl>,
}

pub struct PiProcess {
    pub rpc: PiRpc,
    pub events: mpsc::UnboundedReceiver<Value>,
}

enum ChildControl {
    Kill(oneshot::Sender<()>),
}

impl PiRpc {
    pub async fn spawn(config: SpawnConfig) -> Result<PiProcess> {
        let rpc_entry = rpc_entry_for_cli(&config.program, &config.prefix_args);
        let program = rpc_entry
            .as_deref()
            .unwrap_or_else(|| Path::new(&config.program));

        // Windows: CreateProcess cannot launch .cmd/.bat as the image; route via cmd.exe.
        // Node CLIs (.js/.mjs/.cjs) need an explicit node host (shebang is not honored).
        let mut command = spawn_command_for_program(program);
        if looks_like_js_cli(program) {
            command.arg(program);
        }
        command.args(&config.prefix_args);
        if rpc_entry.is_none() {
            tracing::debug!(program = %config.program, "Pi RPC entrypoint unavailable; passing --mode rpc to CLI");
            command.arg("--mode").arg("rpc");
        }
        command
            .args(&config.pi_args)
            .current_dir(&config.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for (key, value) in &config.env {
            command.env(key, value);
        }

        let mut child = command.spawn().with_context(|| {
            format!(
                "failed to start Pi RPC process: {} {:?}",
                config.program, config.prefix_args
            )
        })?;
        let mut stdin = child.stdin.take().context("Pi RPC stdin is unavailable")?;
        let stdout = child
            .stdout
            .take()
            .context("Pi RPC stdout is unavailable")?;
        let stderr = child
            .stderr
            .take()
            .context("Pi RPC stderr is unavailable")?;

        let (writer_tx, mut writer_rx) = mpsc::unbounded_channel::<Value>();
        let (event_tx, event_rx) = mpsc::unbounded_channel::<Value>();
        let pending: Arc<Mutex<HashMap<String, oneshot::Sender<Result<Value, String>>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let stderr_ring: Arc<Mutex<StderrRingBuffer>> =
            Arc::new(Mutex::new(StderrRingBuffer::new(32)));

        tokio::spawn(async move {
            while let Some(value) = writer_rx.recv().await {
                let line = match serde_json::to_vec(&value) {
                    Ok(line) => line,
                    Err(error) => {
                        tracing::error!(%error, "failed to serialize Pi RPC command");
                        continue;
                    }
                };
                if stdin.write_all(&line).await.is_err()
                    || stdin.write_all(b"\n").await.is_err()
                    || stdin.flush().await.is_err()
                {
                    break;
                }
            }
        });

        // Stdout reader: dispatches responses and events. On EOF it does NOT
        // fail pending requests — that is the exit task's job, so the error
        // message always includes the exit code and fully-drained stderr.
        let pending_stdout = pending.clone();
        let event_stdout = event_tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => match parse_pi_rpc_json(&line) {
                        Ok(value) => {
                            let response_id = value
                                .get("id")
                                .and_then(Value::as_str)
                                .map(ToOwned::to_owned);
                            let is_response =
                                value.get("type").and_then(Value::as_str) == Some("response");
                            if is_response
                                && let Some(id) = response_id
                                && let Some(sender) = pending_stdout
                                    .lock()
                                    .expect("Pi pending map poisoned")
                                    .remove(&id)
                            {
                                let _ = sender.send(Ok(value));
                                continue;
                            }
                            let _ = event_stdout.send(value);
                        }
                        Err(error) => {
                            tracing::warn!(%error, bytes = line.len(), "invalid JSON on Pi RPC stdout");
                            let _ = event_stdout.send(serde_json::json!({
                                "type": "adapter_diagnostic",
                                "message": format!(
                                    "Invalid Pi RPC JSON ({} bytes): {error}",
                                    line.len()
                                ),
                            }));
                        }
                    },
                    Ok(None) => break,
                    Err(error) => {
                        tracing::warn!(%error, "failed reading Pi RPC stdout");
                        break;
                    }
                }
            }
            // stdout closed — do NOT fail_pending here. The exit task owns
            // that so the error includes exit code + drained stderr.
        });

        // Stderr reader: buffers lines and signals completion.
        let stderr_ring_for_reader = stderr_ring.clone();
        let (stderr_done_tx, stderr_done_rx) = oneshot::channel::<()>();
        let mut stderr_log = open_pi_stderr_log();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                stderr_ring_for_reader
                    .lock()
                    .expect("stderr ring poisoned")
                    .push(line.clone());
                if let Some(log) = stderr_log.as_mut()
                    && let Err(error) = writeln!(log, "{line}")
                {
                    tracing::warn!(%error, "failed writing Pi stderr log");
                    stderr_log = None;
                }
                tracing::warn!(target: "pi_rpc", "{line}");
            }
            let _ = stderr_done_tx.send(());
        });

        // Exit coordinator: the single owner of fail_pending. Waits for the
        // child to exit and stderr to drain, then assembles the diagnostic.
        let (child_control_tx, child_control_rx) = mpsc::unbounded_channel();
        let pending_exit = pending.clone();
        let stderr_ring_for_exit = stderr_ring.clone();
        tokio::spawn(async move {
            let base = wait_for_child_exit(child, child_control_rx).await;
            // Wait for the stderr reader to finish (bounded so we never hang).
            let _ = tokio::time::timeout(Duration::from_secs(2), stderr_done_rx).await;
            let stderr_context = stderr_ring_for_exit
                .lock()
                .expect("stderr ring poisoned")
                .snapshot();
            let message = if stderr_context.is_empty() {
                base
            } else {
                format!(
                    "{base}\n\nPi stderr (last {} lines):\n{stderr_context}",
                    stderr_context.lines().count()
                )
            };
            fail_pending(&pending_exit, &message);
            let _ = event_tx.send(serde_json::json!({
                "type": "adapter_process_exit",
                "message": message,
            }));
        });

        Ok(PiProcess {
            rpc: PiRpc {
                writer: writer_tx,
                pending,
                next_id: Arc::new(AtomicU64::new(1)),
                child_control: child_control_tx,
            },
            events: event_rx,
        })
    }

    pub async fn request(&self, mut command: Value) -> Result<Value> {
        let command_type = command
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("Pi RPC command is missing its type"))?;
        let timeout = request_timeout(command_type);
        let object = command
            .as_object_mut()
            .ok_or_else(|| anyhow!("Pi RPC command must be a JSON object"))?;
        let id = format!("pi-grok-{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        object.insert("id".to_string(), Value::String(id.clone()));
        let (response_tx, response_rx) = oneshot::channel();
        self.pending
            .lock()
            .expect("Pi pending map poisoned")
            .insert(id.clone(), response_tx);
        if self.writer.send(command).is_err() {
            self.pending
                .lock()
                .expect("Pi pending map poisoned")
                .remove(&id);
            bail!("Pi RPC writer is closed");
        }
        let response = if let Some(timeout) = timeout {
            match tokio::time::timeout(timeout, response_rx).await {
                Ok(response) => response
                    .map_err(|_| anyhow!("Pi RPC response channel closed for {id}"))?
                    .map_err(anyhow::Error::msg)?,
                Err(_) => {
                    self.pending
                        .lock()
                        .expect("Pi pending map poisoned")
                        .remove(&id);
                    bail!(
                        "Pi RPC request timed out after {} seconds: {id}",
                        timeout.as_secs()
                    );
                }
            }
        } else {
            response_rx
                .await
                .map_err(|_| anyhow!("Pi RPC response channel closed for {id}"))?
                .map_err(anyhow::Error::msg)?
        };
        if response.get("success").and_then(Value::as_bool) == Some(false) {
            let error = response
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("Pi RPC command failed");
            bail!("{error}");
        }
        Ok(response.get("data").cloned().unwrap_or(Value::Null))
    }

    pub fn notify(&self, command: Value) -> Result<()> {
        self.writer
            .send(command)
            .map_err(|_| anyhow!("Pi RPC writer is closed"))
    }

    /// Kill the Pi child process. Used during extension bisection probes so
    /// each probe tears down cleanly instead of leaking an OS process.
    pub async fn kill(&self) {
        let (done_tx, done_rx) = oneshot::channel();
        if self.child_control.send(ChildControl::Kill(done_tx)).is_ok() {
            let _ = done_rx.await;
        }
    }
}

async fn wait_for_child_exit(
    mut child: tokio::process::Child,
    mut control: mpsc::UnboundedReceiver<ChildControl>,
) -> String {
    loop {
        tokio::select! {
            command = control.recv() => match command {
                Some(ChildControl::Kill(done)) => {
                    let _ = child.start_kill();
                    let result = child.wait().await;
                    let _ = done.send(());
                    return describe_child_exit(result);
                }
                None => return describe_child_exit(child.wait().await),
            },
            _ = tokio::time::sleep(Duration::from_millis(25)) => {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        return format!("Pi RPC process exited with {status}");
                    }
                    Ok(None) => {}
                    Err(error) => {
                        return format!("failed waiting for Pi RPC process: {error}");
                    }
                }
            }
        }
    }
}

fn describe_child_exit(result: std::io::Result<std::process::ExitStatus>) -> String {
    match result {
        Ok(status) => format!("Pi RPC process exited with {status}"),
        Err(error) => format!("failed waiting for Pi RPC process: {error}"),
    }
}

fn request_timeout(command_type: &str) -> Option<Duration> {
    match command_type {
        // Pi keeps these requests open until the operation completes; the
        // existing ACP cancel path sends abort_bash/abort without waiting.
        "bash" | "compact" => None,
        _ => Some(Duration::from_secs(300)),
    }
}

/// Parse Pi RPC JSONL, tolerating deeply nested `get_tree` payloads.
///
/// `get_tree` returns a recursively nested `{entry, children:[...]}` graph.
/// serde_json's default recursion limit (~128) rejects those lines, and even
/// with `unbounded_depth` the recursive `Value` visitor can overflow the
/// default thread stack. Large/deep lines are therefore parsed on a dedicated
/// large-stack thread so `/tree` cannot hang forever on "Fetching…".
fn parse_pi_rpc_json(line: &str) -> Result<Value, String> {
    // Fast path: normal sessions fit default limits.
    match serde_json::from_str::<Value>(line) {
        Ok(value) => return Ok(value),
        Err(error) => {
            let msg = error.to_string();
            let needs_deep = line.len() > 64 * 1024 || msg.contains("recursion limit exceeded");
            if !needs_deep {
                return Err(msg);
            }
        }
    }
    parse_pi_rpc_json_deep(line.to_string())
}

fn parse_pi_rpc_json_deep(line: String) -> Result<Value, String> {
    with_large_stack(move || {
        let mut de = serde_json::Deserializer::from_str(&line);
        de.disable_recursion_limit();
        let value = Value::deserialize(&mut de).map_err(|e| e.to_string())?;
        de.end().map_err(|e| e.to_string())?;
        Ok(value)
    })
}

/// Run `f` on a thread with a 64 MiB stack.
///
/// Used for deep Pi tree JSON parse/flatten/drop. Keep the critical section
/// short — only the recursive JSON work belongs here.
pub(crate) fn with_large_stack<F, T>(f: F) -> T
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    std::thread::Builder::new()
        .name("pi-json-deep".into())
        .stack_size(64 * 1024 * 1024)
        .spawn(f)
        .expect("spawn pi-json-deep thread")
        .join()
        .expect("pi-json-deep thread panicked")
}

/// Opens the complete Pi stderr log under the product-isolated Grok home.
///
/// Pi stderr is intentionally mirrored to a file because narrow terminal
/// surfaces can truncate multi-line Node stack traces.
fn open_pi_stderr_log() -> Option<File> {
    let grok_home = env::var_os("GROK_HOME")?;
    match open_pi_stderr_log_at(Path::new(&grok_home)) {
        Ok(file) => Some(file),
        Err(error) => {
            tracing::warn!(%error, home = %Path::new(&grok_home).display(), "failed opening Pi stderr log");
            None
        }
    }
}

fn open_pi_stderr_log_at(grok_home: &Path) -> std::io::Result<File> {
    let path = grok_home.join("logs/pi-rpc-stderr.log");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    OpenOptions::new().create(true).append(true).open(path)
}

/// Ring buffer that keeps the last N stderr lines from the Pi child process.
/// Used to surface meaningful diagnostics when the RPC connection drops.
struct StderrRingBuffer {
    lines: Vec<String>,
    capacity: usize,
}

impl StderrRingBuffer {
    fn new(capacity: usize) -> Self {
        Self {
            lines: Vec::with_capacity(capacity),
            capacity,
        }
    }

    fn push(&mut self, line: String) {
        if self.lines.len() >= self.capacity {
            self.lines.remove(0);
        }
        self.lines.push(line);
    }

    fn snapshot(&self) -> String {
        self.lines.join("\n")
    }
}

/// Build the OS process host for a Pi program path.
///
/// - `.js`/`.mjs`/`.cjs` → `node` / `node.exe` (shebang is not honored by CreateProcess)
/// - `.cmd`/`.bat` → `cmd.exe /D /C <path>` (CreateProcess cannot run batch as image)
/// - otherwise → direct executable path / name
fn spawn_command_for_program(program: &Path) -> Command {
    if looks_like_js_cli(program) {
        return Command::new(if cfg!(windows) { "node.exe" } else { "node" });
    }
    if uses_cmd_wrapper(program) {
        let mut c = Command::new("cmd.exe");
        c.arg("/D");
        c.arg("/C");
        c.arg(program);
        return c;
    }
    Command::new(program)
}

fn uses_cmd_wrapper(program: &Path) -> bool {
    matches!(
        program
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat")),
        Some(true)
    )
}

fn looks_like_js_cli(program: &Path) -> bool {
    matches!(
        program.extension().and_then(|e| e.to_str()),
        Some("js" | "mjs" | "cjs")
    )
}

/// Locate the official npm Pi RPC entrypoint next to a canonical `dist/cli.js`.
///
/// `rpc-entry.js` inserts `--mode rpc` only when it calls Pi's `main()`, so
/// third-party extensions do not see that transport detail in `process.argv`.
/// Prefix arguments describe a custom launcher (`node cli.js`, `bun`, etc.);
/// those launchers intentionally remain on the documented CLI fallback.
fn rpc_entry_for_cli(program: &str, prefix_args: &[String]) -> Option<PathBuf> {
    if !prefix_args.is_empty() {
        return None;
    }
    let cli = canonical_cli_path(program)?;
    if cli.file_name().and_then(|name| name.to_str()) != Some("cli.js") {
        return None;
    }
    let entry = cli.with_file_name("rpc-entry.js");
    entry.is_file().then_some(entry)
}

fn canonical_cli_path(program: &str) -> Option<PathBuf> {
    let direct = Path::new(program);
    if direct.components().count() > 1 {
        return std::fs::canonicalize(direct).ok();
    }
    env::var_os("PATH")
        .and_then(|paths| {
            env::split_paths(&paths)
                .map(|directory| directory.join(program))
                .find(|candidate| candidate.is_file())
        })
        .and_then(|candidate| std::fs::canonicalize(candidate).ok())
}

fn fail_pending(
    pending: &Arc<Mutex<HashMap<String, oneshot::Sender<Result<Value, String>>>>>,
    message: &str,
) {
    let drained: Vec<_> = pending
        .lock()
        .expect("Pi pending map poisoned")
        .drain()
        .map(|(_, sender)| sender)
        .collect();
    for sender in drained {
        let _ = sender.send(Err(message.to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persists_pi_stderr_to_product_log_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("logs/pi-rpc-stderr.log");
        let mut file = open_pi_stderr_log_at(temp.path()).expect("open stderr log");
        writeln!(file, "Error: stale extension context").expect("write stderr");
        drop(file);

        assert_eq!(
            std::fs::read_to_string(path).expect("read stderr log"),
            "Error: stale extension context\n"
        );
    }

    #[test]
    fn only_long_running_pi_operations_skip_the_default_deadline() {
        assert_eq!(request_timeout("bash"), None);
        assert_eq!(request_timeout("compact"), None);
        assert_eq!(request_timeout("get_state"), Some(Duration::from_secs(300)));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn kill_does_not_block_behind_exit_wait() {
        let mut command = Command::new("sleep");
        command.arg("30").kill_on_drop(true);
        let child = command.spawn().expect("spawn sleeping child");
        let (child_control, child_control_rx) = mpsc::unbounded_channel();
        let (writer, _writer_rx) = mpsc::unbounded_channel();
        let rpc = PiRpc {
            writer,
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(AtomicU64::new(1)),
            child_control,
        };

        let waiter = tokio::spawn(wait_for_child_exit(child, child_control_rx));

        let result = tokio::time::timeout(Duration::from_millis(100), rpc.kill()).await;
        assert!(result.is_ok(), "kill blocked behind the exit wait mutex");
        let exit = tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("killed child should be reaped promptly")
            .expect("exit coordinator task");
        assert!(exit.starts_with("Pi RPC process exited with"));
    }

    #[test]
    fn resolves_official_rpc_entry_beside_npm_cli() {
        let temp = tempfile::tempdir().expect("tempdir");
        let dist = temp.path().join("dist");
        std::fs::create_dir(&dist).expect("create dist");
        let cli = dist.join("cli.js");
        let rpc_entry = dist.join("rpc-entry.js");
        std::fs::write(&cli, "#!/usr/bin/env node").expect("write cli");
        std::fs::write(&rpc_entry, "#!/usr/bin/env node").expect("write rpc entry");

        assert_eq!(
            rpc_entry_for_cli(cli.to_str().unwrap(), &[]),
            Some(std::fs::canonicalize(rpc_entry).expect("canonical rpc entry"))
        );
    }

    #[test]
    fn custom_launcher_keeps_cli_rpc_fallback() {
        let temp = tempfile::tempdir().expect("tempdir");
        let cli = temp.path().join("cli.js");
        std::fs::write(&cli, "#!/usr/bin/env node").expect("write cli");
        std::fs::write(temp.path().join("rpc-entry.js"), "#!/usr/bin/env node")
            .expect("write rpc entry");

        assert_eq!(
            rpc_entry_for_cli(cli.to_str().unwrap(), &["loader.js".to_string()]),
            None
        );
    }

    #[test]
    fn parse_pi_rpc_json_large_get_tree_fixture() {
        let path = std::path::Path::new("/tmp/pi-get-tree.json");
        if !path.exists() {
            return;
        }
        let data = std::fs::read_to_string(path).unwrap();
        // Wrap as a full RPC response line like Pi emits.
        let line = format!(
            "{{\"type\":\"response\",\"command\":\"get_tree\",\"success\":true,\"id\":\"x\",\"data\":{data}}}"
        );
        let start = std::time::Instant::now();
        let value = parse_pi_rpc_json(&line).expect("deep parse");
        let elapsed = start.elapsed();
        eprintln!("fixture parse elapsed_ms={}", elapsed.as_millis());
        assert_eq!(value["command"], "get_tree");
        assert!(value["data"]["tree"].as_array().is_some());
        // Flatten projection must also finish quickly on large stack.
        let tree = with_large_stack({
            let value = value;
            move || crate::model::parse_session_tree(&value["data"])
        });
        eprintln!(
            "fixture flatten nodes={} elapsed_ms_total={}",
            tree.rows.len(),
            start.elapsed().as_millis()
        );
        assert!(!tree.rows.is_empty());
        assert!(start.elapsed().as_secs() < 30);
    }

    #[test]
    fn parse_pi_rpc_json_accepts_deeply_nested_trees() {
        // Build a chain deeper than serde_json's default recursion limit.
        let mut node =
            String::from("{\"entry\":{\"id\":\"leaf\",\"type\":\"message\"},\"children\":[]}");
        for i in 0..200 {
            node = format!(
                "{{\"entry\":{{\"id\":\"n{i}\",\"type\":\"message\"}},\"children\":[{node}]}}"
            );
        }
        let line = format!(
            "{{\"type\":\"response\",\"command\":\"get_tree\",\"success\":true,\"data\":{{\"tree\":[{node}],\"leafId\":\"leaf\"}}}}"
        );
        // Default from_str would fail with recursion limit exceeded.
        assert!(serde_json::from_str::<Value>(&line).is_err());
        let value = parse_pi_rpc_json(&line).expect("unbounded parse");
        assert_eq!(value["command"], "get_tree");
        assert!(value["data"]["tree"].as_array().is_some());
    }
}
