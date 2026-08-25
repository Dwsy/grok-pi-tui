use anyhow::{Context, Result};
use serde_json::Value;
use std::time::Duration;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    sync::mpsc,
};

/// Process-private local IPC for high-frequency subagent UI events.
///
/// Unix uses a filesystem Unix-domain socket. Windows uses a named pipe with
/// the same path-style client API exposed by Node's `net.connect({ path })`.
pub struct SubagentEventTransport {
    endpoint: String,
    #[cfg(unix)]
    listener: tokio::net::UnixListener,
    #[cfg(unix)]
    _socket_dir: tempfile::TempDir,
    #[cfg(windows)]
    next_server: tokio::sync::Mutex<Option<tokio::net::windows::named_pipe::NamedPipeServer>>,
}

impl SubagentEventTransport {
    pub fn bind() -> Result<Self> {
        #[cfg(unix)]
        {
            let socket_dir = tempfile::Builder::new()
                .prefix("pi-grok-subagent-")
                .tempdir()
                .context("create Pi subagent socket directory")?;
            let path = socket_dir.path().join("events.sock");
            let listener = tokio::net::UnixListener::bind(&path)
                .with_context(|| format!("bind Pi subagent socket {}", path.display()))?;
            return Ok(Self {
                endpoint: path.to_string_lossy().into_owned(),
                listener,
                _socket_dir: socket_dir,
            });
        }

        #[cfg(windows)]
        {
            use tokio::net::windows::named_pipe::ServerOptions;

            let endpoint = format!(
                r"\\.\pipe\pi-grok-subagent-{}",
                uuid::Uuid::now_v7().simple()
            );
            let first = ServerOptions::new()
                .first_pipe_instance(true)
                .create(&endpoint)
                .context("bind Pi subagent named pipe")?;
            return Ok(Self {
                endpoint,
                next_server: tokio::sync::Mutex::new(Some(first)),
            });
        }

        #[allow(unreachable_code)]
        Err(anyhow::anyhow!("unsupported local IPC platform"))
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    #[cfg(unix)]
    async fn accept_stream(&self) -> std::io::Result<tokio::net::UnixStream> {
        self.listener.accept().await.map(|(stream, _)| stream)
    }

    #[cfg(windows)]
    async fn accept_stream(
        &self,
    ) -> std::io::Result<tokio::net::windows::named_pipe::NamedPipeServer> {
        use tokio::net::windows::named_pipe::ServerOptions;

        let mut slot = self.next_server.lock().await;
        let server = match slot.take() {
            Some(server) => server,
            None => ServerOptions::new().create(&self.endpoint)?,
        };
        match server.connect().await {
            Ok(()) => {
                *slot = Some(ServerOptions::new().create(&self.endpoint)?);
                Ok(server)
            }
            Err(error) => {
                *slot = ServerOptions::new().create(&self.endpoint).ok();
                Err(error)
            }
        }
    }

    pub async fn forward(&self, tx: mpsc::UnboundedSender<Value>) {
        loop {
            if tx.is_closed() {
                return;
            }
            let stream = match self.accept_stream().await {
                Ok(stream) => stream,
                Err(error) => {
                    tracing::warn!(%error, "failed to accept Pi subagent local stream");
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    continue;
                }
            };
            let stream_tx = tx.clone();
            // Reload/recovery can overlap old and new extension runtimes. Both
            // may still emit a final lifecycle event, so drain every accepted
            // connection instead of serialising or choosing a winner.
            tokio::task::spawn_local(async move {
                let mut lines = BufReader::new(stream).lines();
                loop {
                    match lines.next_line().await {
                        Ok(Some(line)) if line.trim().is_empty() => continue,
                        Ok(Some(line)) => match serde_json::from_str::<Value>(&line) {
                            Ok(event) => {
                                if stream_tx.send(event).is_err() {
                                    return;
                                }
                            }
                            Err(error) => {
                                tracing::warn!(%error, "invalid Pi subagent transient event")
                            }
                        },
                        Ok(None) => return,
                        Err(error) => {
                            tracing::warn!(%error, "failed to read Pi subagent local stream");
                            return;
                        }
                    }
                }
            });
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::io::AsyncWriteExt;

    #[tokio::test(flavor = "current_thread")]
    async fn second_connection_is_not_blocked_by_an_idle_first_connection() {
        tokio::task::LocalSet::new()
            .run_until(async {
                let transport = Arc::new(SubagentEventTransport::bind().expect("bind transport"));
                let endpoint = transport.endpoint().to_owned();
                let (tx, mut rx) = mpsc::unbounded_channel();
                let task = {
                    let transport = transport.clone();
                    tokio::task::spawn_local(async move { transport.forward(tx).await })
                };

                let _idle = tokio::net::UnixStream::connect(&endpoint)
                    .await
                    .expect("connect idle stream");
                tokio::task::yield_now().await;
                let mut live = tokio::net::UnixStream::connect(endpoint)
                    .await
                    .expect("connect live stream");
                live.write_all(b"{\"type\":\"custom\",\"data\":{\"sequence\":9}}\n")
                    .await
                    .expect("write live event");

                let event = tokio::time::timeout(Duration::from_secs(1), rx.recv())
                    .await
                    .expect("live stream must not wait for idle EOF")
                    .expect("receive event");
                assert_eq!(event["data"]["sequence"], 9);
                task.abort();
            })
            .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn both_connections_keep_forwarding_after_the_second_connects() {
        tokio::task::LocalSet::new()
            .run_until(async {
                let transport = Arc::new(SubagentEventTransport::bind().expect("bind transport"));
                let endpoint = transport.endpoint().to_owned();
                let (tx, mut rx) = mpsc::unbounded_channel();
                let task = {
                    let transport = transport.clone();
                    tokio::task::spawn_local(async move { transport.forward(tx).await })
                };

                let mut first = tokio::net::UnixStream::connect(&endpoint)
                    .await
                    .expect("connect first stream");
                first
                    .write_all(b"{\"type\":\"custom\",\"data\":{\"sequence\":1}}\n")
                    .await
                    .expect("write first event");
                assert_eq!(rx.recv().await.unwrap()["data"]["sequence"], 1);

                let mut second = tokio::net::UnixStream::connect(endpoint)
                    .await
                    .expect("connect second stream");
                second
                    .write_all(b"{\"type\":\"custom\",\"data\":{\"sequence\":2}}\n")
                    .await
                    .expect("write second event");
                assert_eq!(rx.recv().await.unwrap()["data"]["sequence"], 2);

                first
                    .write_all(b"{\"type\":\"custom\",\"data\":{\"sequence\":3}}\n")
                    .await
                    .expect("write late first event");
                assert_eq!(rx.recv().await.unwrap()["data"]["sequence"], 3);
                task.abort();
            })
            .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn forwards_ndjson_over_unix_socket() {
        tokio::task::LocalSet::new()
            .run_until(async {
                let transport = Arc::new(SubagentEventTransport::bind().expect("bind transport"));
                let endpoint = transport.endpoint().to_owned();
                let (tx, mut rx) = mpsc::unbounded_channel();
                let task = {
                    let transport = transport.clone();
                    tokio::task::spawn_local(async move { transport.forward(tx).await })
                };

                let mut stream = tokio::net::UnixStream::connect(endpoint)
                    .await
                    .expect("connect transport");
                stream
                    .write_all(
                        b"{\"type\":\"custom\",\"data\":{\"sequence\":1}}\n\
                  {\"type\":\"custom\",\"data\":{\"sequence\":2}}\n\
                  {\"type\":\"custom\",\"data\":{\"sequence\":3}}\n",
                    )
                    .await
                    .expect("write events");

                for expected in 1..=3 {
                    let event = rx.recv().await.expect("receive event");
                    assert_eq!(event["data"]["sequence"], expected);
                }
                task.abort();
            })
            .await;
    }
}
