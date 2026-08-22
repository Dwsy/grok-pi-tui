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
            let stream = match self.accept_stream().await {
                Ok(stream) => stream,
                Err(error) => {
                    tracing::warn!(%error, "failed to accept Pi subagent local stream");
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    continue;
                }
            };
            let mut lines = BufReader::new(stream).lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) if line.trim().is_empty() => continue,
                    Ok(Some(line)) => match serde_json::from_str::<Value>(&line) {
                        Ok(event) => {
                            if tx.send(event).is_err() {
                                return;
                            }
                        }
                        Err(error) => {
                            tracing::warn!(%error, "invalid Pi subagent transient event")
                        }
                    },
                    Ok(None) => break,
                    Err(error) => {
                        tracing::warn!(%error, "failed to read Pi subagent local stream");
                        break;
                    }
                }
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::io::AsyncWriteExt;

    #[tokio::test(flavor = "current_thread")]
    async fn forwards_ndjson_over_unix_socket() {
        let transport = Arc::new(SubagentEventTransport::bind().expect("bind transport"));
        let endpoint = transport.endpoint().to_owned();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let task = {
            let transport = transport.clone();
            tokio::spawn(async move { transport.forward(tx).await })
        };

        let mut stream = tokio::net::UnixStream::connect(endpoint)
            .await
            .expect("connect transport");
        stream
            .write_all(b"{\"type\":\"custom\",\"data\":{\"sequence\":1}}\n")
            .await
            .expect("write event");

        let event = rx.recv().await.expect("receive event");
        assert_eq!(event["data"]["sequence"], 1);
        task.abort();
    }
}
